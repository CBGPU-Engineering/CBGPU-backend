//! Download the official NEIS secondary-school directory into a reviewable JSON manifest.
//! This tool never connects to a database or creates permissions.
use encoding_rs::EUC_KR;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, error::Error, path::Path};

const ORIGIN: &str = "https://open.neis.go.kr";
const DATASET: &str = "OPEN17020190531110010104913";
const MAX_BYTES: usize = 20 * 1024 * 1024;

fn official_homepage(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 2000 {
        return None;
    }
    let mut url = url::Url::parse(value).ok()?;
    let host = url.host_str()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.port(), None | Some(80 | 443))
        || !matches!(url.host(), Some(url::Host::Domain(_)))
        || !host.contains('.')
        || ["localhost", ".localhost", ".local", ".internal"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.into())
}

fn source_date(name: &str) -> Option<(u32, u32, u32)> {
    let (_, tail) = name.split_once('_')?;
    let (year, tail) = tail.split_once('년')?;
    let (month, tail) = tail.split_once('월')?;
    let (day, _) = tail.split_once('일')?;
    let date = (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    ((2019..=2100).contains(&date.0) && (1..=12).contains(&date.1) && (1..=31).contains(&date.2))
        .then_some(date)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchoolType {
    High,
    Middle,
}
impl SchoolType {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "HIGH_SCHOOL" => Ok(Self::High),
            "MIDDLE_SCHOOL" => Ok(Self::Middle),
            _ => Err("Unsupported school type".into()),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::High => "HIGH_SCHOOL",
            Self::Middle => "MIDDLE_SCHOOL",
        }
    }
    fn kinds(self) -> [&'static str; 2] {
        match self {
            Self::High => ["고등학교", "각종학교(고)"],
            Self::Middle => ["중학교", "각종학교(중)"],
        }
    }
    fn scope(self) -> &'static str {
        match self {
            Self::High => "KOREAN_HIGH_SCHOOLS",
            Self::Middle => "KOREAN_MIDDLE_SCHOOLS",
        }
    }
}
fn normalize(
    bytes: &[u8],
    school_type: SchoolType,
) -> Result<(Vec<Value>, Vec<Value>), Box<dyn Error>> {
    if bytes.len() > MAX_BYTES {
        return Err("School directory exceeds safety limit".into());
    }
    let (text, _, invalid) = EUC_KR.decode(bytes);
    if invalid {
        return Err("Invalid official CSV encoding".into());
    }
    let mut csv = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(text.trim_start_matches('\u{feff}').as_bytes());
    let headers = csv.headers()?.clone();
    let index = |name: &str| {
        headers
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| format!("Missing official column: {name}"))
    };
    let code = index("행정표준코드")?;
    let name = index("학교명")?;
    let kind = index("학교종류명")?;
    let region = index("시도명")?;
    let address = index("도로명주소")?;
    let phone = index("전화번호")?;
    let office = index("시도교육청코드")?;
    let homepage = headers.iter().position(|h| h == "홈페이지주소");
    let mut ids = BTreeSet::new();
    let mut schools = Vec::new();
    let mut excluded = Vec::new();
    for row in csv.records() {
        let row = row?;
        if !school_type.kinds().contains(&row.get(kind).unwrap_or("")) {
            continue;
        }
        let get = |i| row.get(i).unwrap_or("").trim();
        let official_id = get(code);
        if official_id.is_empty() && !get(name).is_empty() {
            excluded.push(json!({"official_name":get(name),"region":get(region),"reason":"MISSING_OFFICIAL_IDENTIFIER"}));
            continue;
        }
        if official_id.len() != 7
            || !official_id
                .bytes()
                .all(|v| v.is_ascii_digit() || v.is_ascii_uppercase())
            || get(name).is_empty()
        {
            return Err(format!(
                "Invalid official school identity: {} ({official_id})",
                get(name)
            )
            .into());
        }
        if !ids.insert(official_id.to_owned()) {
            return Err(format!("Duplicate official identifier: {official_id}").into());
        }
        schools.push(json!({
            "official_id":official_id,"official_name":get(name),"display_name":get(name),
            "type":school_type.name(),"source_kind":get(kind),"region":get(region),"address":get(address),
            "contact_phone":get(phone),"education_office_code":get(office),
            "homepage_url":homepage.and_then(|i|official_homepage(get(i))),
            "status":"LISTED","features":[],"verified_at":null
        }));
    }
    schools.sort_by(|a, b| a["official_id"].as_str().cmp(&b["official_id"].as_str()));
    Ok((schools, excluded))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !matches!(args.len(), 2 | 4)
        || args[0] != "--output"
        || (args.len() == 4 && args[2] != "--school-type")
    {
        return Err(
            "Usage: school-directory --output NEW_MANIFEST.json [--school-type HIGH_SCHOOL|MIDDLE_SCHOOL] (no database writes)".into(),
        );
    }
    let school_type = SchoolType::parse(args.get(3).map(String::as_str).unwrap_or("HIGH_SCHOOL"))?;
    let output = Path::new(&args[1]);
    if output.exists() {
        return Err("Output already exists; choose a new manifest path".into());
    }
    let client = reqwest::Client::builder()
        .user_agent("CBGPU-school-directory/1.0")
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let list: Value = client
        .post(format!("{ORIGIN}/portal/data/file/searchFileData.do"))
        .form(&[("infId", DATASET), ("infSeq", "3")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let files = list["data"]
        .as_array()
        .ok_or("Invalid official file index")?;
    let file = files
        .iter()
        .filter(|v| {
            v["fileExt"] == "csv" && source_date(v["viewFileNm"].as_str().unwrap_or("")).is_some()
        })
        .max_by_key(|v| source_date(v["viewFileNm"].as_str().unwrap_or("")))
        .ok_or("No dated official CSV available")?;
    let date = source_date(file["viewFileNm"].as_str().ok_or("Missing source name")?)
        .ok_or("Invalid source date")?;
    let seq = file["fileSeq"]
        .as_u64()
        .ok_or("Invalid official file identifier")?;
    let url = format!(
        "{ORIGIN}/portal/data/file/downloadBlobFileData.do?infId={DATASET}&infSeq=3&fileSeq={seq}"
    );
    let mut response = client.get(&url).send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err("School directory exceeds safety limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let (schools, excluded) = normalize(&bytes, school_type)?;
    if schools.len() < 1000 || schools.len() > 10000 {
        return Err("Unexpected nationwide school count; manual review required".into());
    }
    let cbsh = schools
        .iter()
        .filter(|s| s["official_name"] == "충북과학고등학교")
        .collect::<Vec<_>>();
    if school_type == SchoolType::High && cbsh.len() != 1 {
        return Err("Existing Chungbuk Science High School must match exactly once".into());
    }
    let manifest = json!({"format_version":1,"normalization_version":2,"scope":school_type.scope(),"included_source_kinds":school_type.kinds(),"source":url,"source_as_of":format!("{:04}-{:02}-{:02}",date.0,date.1,date.2),"source_sha256":format!("{:x}",Sha256::digest(&bytes)),"license_url":format!("{ORIGIN}/portal/data/service/selectServicePage.do?infId={DATASET}&infSeq=2"),"school_count":schools.len(),"schools":schools,"excluded":excluded});
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    file.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    println!(
        "Validated {} official schools as of {}. Manifest only; no memberships, permissions or database rows changed.",
        manifest["school_count"], manifest["source_as_of"]
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn includes_explicit_high_school_alternatives_but_not_middle_or_unspecified_kinds() {
        let input = "행정표준코드,학교명,학교종류명,시도명,도로명주소,전화번호,시도교육청코드\n8000550,단재고등학교,각종학교(고),충청북도,청주시,043-000,M10\n1234567,일반고등학교,고등학교,충청북도,청주시,043-000,M10\n1234568,대안중학교,각종학교(중),충청북도,청주시,043-000,M10\n1234569,일반중학교,중학교,충청북도,청주시,043-000,M10\n1234570,미분류학교,각종학교,충청북도,청주시,043-000,M10\n";
        let (bytes, _, _) = EUC_KR.encode(input);
        let (schools, excluded) = normalize(&bytes, SchoolType::High).unwrap();
        assert_eq!(schools.len(), 2);
        assert!(excluded.is_empty());
        assert!(schools.iter().any(|s| s["official_id"] == "8000550"
            && s["source_kind"] == "각종학교(고)"
            && s["status"] == "LISTED"
            && s["features"] == json!([])
            && s["verified_at"].is_null()));
    }
    #[test]
    fn middle_school_types_are_explicit_and_preserve_listed_only() {
        let input = "행政";
        assert!(SchoolType::parse(input).is_err());
        let input = "행정표준코드,학교명,학교종류명,시도명,도로명주소,전화번호,시도교육청코드\n1234567,일반중학교,중학교,충청북도,청주시,043-000,M10\n1234568,대안중학교,각종학교(중),충청북도,청주시,043-000,M10\n1234569,고등학교,고등학교,충청북도,청주시,043-000,M10\n1234570,미분류,각종학교,충청북도,청주시,043-000,M10\n";
        let (bytes, _, _) = EUC_KR.encode(input);
        let (schools, excluded) = normalize(&bytes, SchoolType::Middle).unwrap();
        assert_eq!(schools.len(), 2);
        assert!(excluded.is_empty());
        for s in schools {
            assert_eq!(s["type"], "MIDDLE_SCHOOL");
            assert_eq!(s["status"], "LISTED");
            assert_eq!(s["features"], json!([]));
            assert!(s["verified_at"].is_null());
        }
    }
    #[test]
    fn official_homepage_is_metadata_not_an_arbitrary_proxy_destination() {
        assert_eq!(
            official_homepage("https://school.cbe.go.kr/cbs-h#content"),
            Some("https://school.cbe.go.kr/cbs-h".into())
        );
        for bad in [
            "http://127.0.0.1/",
            "http://169.254.169.254/",
            "http://metadata.internal/",
            "https://user:secret@school.example/",
            "javascript:alert(1)",
            "https://school.example:5432/",
        ] {
            assert!(official_homepage(bad).is_none())
        }
    }
    #[test]
    fn nested_blob_origins_are_opaque_without_recursive_stack_growth() {
        let nested = format!("{}https://school.example/", "blob:".repeat(10_000));
        assert!(!url::Url::parse(&nested).unwrap().origin().is_tuple());
        assert!(
            url::Url::parse("blob:https://school.example/id")
                .unwrap()
                .origin()
                .is_tuple()
        );
    }
    #[test]
    fn chronological_dates_are_not_lexicographic() {
        assert!(
            source_date("학교기본정보_2026년10월31일기준")
                > source_date("학교기본정보_2026년8월31일기준")
        );
        assert_eq!(source_date("untrusted"), None);
    }
    #[test]
    fn only_high_schools_without_access_grants() {
        let s = "행정표준코드,학교명,학교종류명,시도명,도로명주소,전화번호,시도교육청코드\n1234567,예시고,고등학교,충북,주소,043-000,Q10\n1234568,예시중,중학교,충북,주소,043-001,Q10\n";
        let (bytes, _, invalid) = EUC_KR.encode(s);
        assert!(!invalid);
        let (schools, excluded) = normalize(&bytes, SchoolType::High).unwrap();
        assert_eq!(schools.len(), 1);
        assert!(excluded.is_empty());
        assert_eq!(schools[0]["status"], "LISTED");
        assert_eq!(schools[0]["features"], json!([]));
        assert!(schools[0]["verified_at"].is_null());
    }
    #[test]
    fn duplicate_school_identity_is_not_silently_accepted() {
        let s = "행정표준코드,학교명,학교종류명,시도명,도로명주소,전화번호,시도교육청코드\n1234567,예시고,고등학교,충북,주소,043-000,Q10\n1234567,다른고,고등학교,충북,주소,043-001,Q10\n";
        let (bytes, _, _) = EUC_KR.encode(s);
        assert!(normalize(&bytes, SchoolType::High).is_err());
    }
    #[test]
    fn national_schools_can_use_alphanumeric_codes_and_same_names() {
        let s = "행정표준코드,학교명,학교종류명,시도명,도로명주소,전화번호,시도교육청코드\nC035903,중앙고등학교,고등학교,충남,주소,041-000,N10\n1234567,중앙고등학교,고등학교,충북,주소,043-000,M10\n,설립예정고,고등학교,충북,주소,,M10\n";
        let (bytes, _, invalid) = EUC_KR.encode(s);
        assert!(!invalid);
        let (schools, excluded) = normalize(&bytes, SchoolType::High).unwrap();
        assert_eq!(schools.len(), 2);
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0]["reason"], "MISSING_OFFICIAL_IDENTIFIER");
        assert!(schools.iter().any(|s| s["official_id"] == "C035903"));
    }
}
