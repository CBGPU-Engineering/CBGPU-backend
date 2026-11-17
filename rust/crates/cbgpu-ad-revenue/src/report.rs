use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub const MAX_CSV: usize = 2 * 1024 * 1024;
pub const MAX_ROWS: usize = 5000;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Columns {
    pub date: Option<String>,
    pub unit_id: String,
    pub revenue: String,
    pub impressions: Option<String>,
    pub clicks: Option<String>,
    pub requests: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Format {
    pub delimiter: String,
    pub date_format: String,
    pub decimal_separator: String,
    pub group_separator: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub csv: String,
    pub granularity: String,
    pub period_start: String,
    pub period_end: String,
    pub currency: String,
    pub kind: String,
    pub timezone: String,
    pub columns: Columns,
    pub format: Format,
    #[serde(default)]
    pub preview_hash: Option<String>,
    #[serde(default)]
    pub confirmed: bool,
}
#[derive(Clone, Serialize)]
pub struct Fact {
    pub start_date: String,
    pub end_date: String,
    pub unit_id: String,
    pub revenue_micros: String,
    pub impressions: Option<String>,
    pub clicks: Option<String>,
    pub requests: Option<String>,
}
#[derive(Serialize)]
pub struct Prepared {
    pub granularity: String,
    pub period_start: String,
    pub period_end: String,
    pub currency: String,
    pub kind: String,
    pub timezone: String,
    pub source_hash: String,
    pub preview_hash: String,
    pub rows: Vec<Fact>,
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn date(value: &str, format: &str) -> Result<NaiveDate, String> {
    let pattern = match format {
        "YYYY-MM-DD" => "%Y-%m-%d",
        "YYYY.MM.DD" => "%Y.%m.%d",
        "YYYY/MM/DD" => "%Y/%m/%d",
        _ => return Err("Select an explicit supported date format".into()),
    };
    let parsed = NaiveDate::parse_from_str(value, pattern)
        .map_err(|_| "Date does not match the selected format")?;
    if parsed.format(pattern).to_string() != value {
        return Err("Date must use exact four-digit year and two-digit month/day".into());
    }
    Ok(parsed)
}
pub fn valid_date(value: &str) -> bool {
    date(value, "YYYY-MM-DD").is_ok()
}
fn normalized_number(value: &str, decimal: &str, group: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 80 {
        return Err("Numeric cell is empty or too long".into());
    }
    let negative = value.starts_with('-');
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let parts = unsigned.split(decimal).collect::<Vec<_>>();
    if parts.len() > 2 || parts[0].is_empty() {
        return Err("Invalid numeric format".into());
    }
    let integer = if !group.is_empty() && parts[0].contains(group) {
        let groups = parts[0].split(group).collect::<Vec<_>>();
        if groups[0].is_empty()
            || groups[0].len() > 3
            || groups.iter().skip(1).any(|v| v.len() != 3)
            || groups
                .iter()
                .any(|v| !v.bytes().all(|c| c.is_ascii_digit()))
        {
            return Err("Invalid numeric grouping".into());
        }
        groups.concat()
    } else {
        parts[0].to_owned()
    };
    if !integer.bytes().all(|v| v.is_ascii_digit())
        || parts
            .get(1)
            .is_some_and(|v| v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("Only explicitly mapped decimal/group separators are supported".into());
    }
    Ok(format!(
        "{}{}{}",
        if negative { "-" } else { "" },
        integer,
        parts.get(1).map(|v| format!(".{v}")).unwrap_or_default()
    ))
}
pub fn money(value: &str, decimal: &str, group: &str) -> Result<i64, String> {
    let value = normalized_number(value, decimal, group)?;
    let negative = value.starts_with('-');
    let value = value.strip_prefix('-').unwrap_or(&value);
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 6 {
        return Err("Revenue supports at most six decimal KRW places; no silent rounding".into());
    }
    let whole = whole
        .parse::<i64>()
        .map_err(|_| "Revenue is outside the supported fixed-point range")?;
    let fraction = format!("{fraction:0<6}")
        .parse::<i64>()
        .map_err(|_| "Invalid revenue fraction")?;
    let total = whole
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_add(fraction))
        .ok_or("Revenue overflow")?;
    if negative {
        total.checked_neg().ok_or_else(|| "Revenue overflow".into())
    } else {
        Ok(total)
    }
}
fn count(value: &str, format: &Format) -> Result<String, String> {
    let value = normalized_number(value, &format.decimal_separator, &format.group_separator)?;
    if value.contains('.') || value.starts_with('-') {
        return Err("Counts must be nonnegative integers".into());
    }
    let n = value.parse::<u64>().map_err(|_| "Count overflow")?;
    if n > 1_000_000_000_000 {
        return Err("Count exceeds the supported bound".into());
    }
    Ok(n.to_string())
}
pub fn prepare(input: Input) -> Result<Prepared, String> {
    if input.csv.is_empty() || input.csv.len() > MAX_CSV {
        return Err("CSV must contain at most 2 MiB of UTF-8 text".into());
    }
    if input.currency != "KRW"
        || !["ESTIMATED", "CONFIRMED"].contains(&input.kind.as_str())
        || !["Asia/Seoul", "UTC"].contains(&input.timezone.as_str())
        || !["DAY", "PERIOD"].contains(&input.granularity.as_str())
    {
        return Err("Select explicit revenue kind, KRW, report timezone and granularity".into());
    }
    let start = date(&input.period_start, "YYYY-MM-DD")?;
    let end = date(&input.period_end, "YYYY-MM-DD")?;
    if start > end || (end - start).num_days() > 3660 {
        return Err("Invalid report period".into());
    }
    let format = &input.format;
    if ![",", ";", "\t"].contains(&format.delimiter.as_str())
        || ![".", ","].contains(&format.decimal_separator.as_str())
        || !["", ",", ".", " "].contains(&format.group_separator.as_str())
        || format.decimal_separator == format.group_separator
    {
        return Err("Select valid explicit CSV and number separators".into());
    }
    // Validate even PERIOD configuration; never silently guess a provider format.
    if !["YYYY-MM-DD", "YYYY.MM.DD", "YYYY/MM/DD"].contains(&format.date_format.as_str()) {
        return Err("Unsupported explicit date format".into());
    }
    let csv = input.csv.strip_prefix('\u{feff}').unwrap_or(&input.csv);
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(format.delimiter.as_bytes()[0])
        .flexible(false)
        .from_reader(csv.as_bytes());
    let headers = reader.headers().map_err(|_| "Invalid CSV header")?.clone();
    if headers.len() > 128 || headers.iter().any(|v| v.len() > 1024) {
        return Err("CSV header limit exceeded".into());
    }
    let column = |name: &str| -> Result<usize, String> {
        let matches = headers
            .iter()
            .enumerate()
            .filter(|(_, v)| *v == name)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if name.is_empty() || matches.len() != 1 {
            Err("Every selected column must match exactly one CSV header".into())
        } else {
            Ok(matches[0])
        }
    };
    let unit = column(&input.columns.unit_id)?;
    let revenue = column(&input.columns.revenue)?;
    let day = if input.granularity == "DAY" {
        Some(column(
            input
                .columns
                .date
                .as_deref()
                .ok_or("DAY reports require an explicit date column")?,
        )?)
    } else {
        None
    };
    let impressions = input
        .columns
        .impressions
        .as_deref()
        .map(column)
        .transpose()?;
    let clicks = input.columns.clicks.as_deref().map(column).transpose()?;
    let requests = input.columns.requests.as_deref().map(column).transpose()?;
    let mut selected = BTreeSet::new();
    for index in [
        Some(unit),
        Some(revenue),
        day,
        impressions,
        clicks,
        requests,
    ]
    .into_iter()
    .flatten()
    {
        if !selected.insert(index) {
            return Err("Map each metric to its own explicit column".into());
        }
    }
    let mut rows = vec![];
    for record in reader.records() {
        let record = record.map_err(|_| "Malformed RFC4180 CSV row")?;
        if rows.len() >= MAX_ROWS || record.iter().any(|v| v.len() > 4096) {
            return Err("CSV row/cell limit exceeded".into());
        }
        let unit_id = record.get(unit).unwrap_or("");
        if !unit_id.starts_with("DAN-")
            || !(6..=68).contains(&unit_id.len())
            || !unit_id[4..].bytes().all(|v| v.is_ascii_alphanumeric())
        {
            return Err("Ad-unit cells must contain explicit DAN identifiers; totals are not silently discarded".into());
        }
        let (row_start, row_end) = if let Some(index) = day {
            let d = date(record.get(index).unwrap_or(""), &format.date_format)?;
            (d, d)
        } else {
            (start, end)
        };
        if row_start < start || row_end > end {
            return Err("CSV date is outside the explicitly selected report period".into());
        }
        let parse_count = |index: Option<usize>| {
            index
                .map(|i| count(record.get(i).unwrap_or(""), format))
                .transpose()
        };
        rows.push(Fact {
            start_date: row_start.to_string(),
            end_date: row_end.to_string(),
            unit_id: unit_id.to_owned(),
            revenue_micros: money(
                record.get(revenue).unwrap_or(""),
                &format.decimal_separator,
                &format.group_separator,
            )?
            .to_string(),
            impressions: parse_count(impressions)?,
            clicks: parse_count(clicks)?,
            requests: parse_count(requests)?,
        });
    }
    if rows.is_empty() {
        return Err("CSV has no data rows".into());
    }
    rows.sort_by(|a, b| {
        (&a.start_date, &a.end_date, &a.unit_id).cmp(&(&b.start_date, &b.end_date, &b.unit_id))
    });
    let mut ends = std::collections::BTreeMap::<String, String>::new();
    for row in &rows {
        if ends
            .get(&row.unit_id)
            .is_some_and(|end| &row.start_date <= end)
        {
            return Err(
                "Overlapping date/ad-unit rows are ambiguous; export one aggregate grain".into(),
            );
        }
        ends.insert(row.unit_id.clone(), row.end_date.clone());
    }
    let mut canonical = format!("{}|{}|{}\n", input.kind, input.currency, input.timezone);
    for row in &rows {
        canonical.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{}\n",
            row.start_date,
            row.end_date,
            row.unit_id,
            row.revenue_micros,
            row.impressions.as_deref().unwrap_or("~"),
            row.clicks.as_deref().unwrap_or("~"),
            row.requests.as_deref().unwrap_or("~")
        ));
    }
    Ok(Prepared {
        granularity: input.granularity,
        period_start: input.period_start,
        period_end: input.period_end,
        currency: input.currency,
        kind: input.kind,
        timezone: input.timezone,
        source_hash: hash(input.csv.as_bytes()),
        preview_hash: hash(canonical.as_bytes()),
        rows,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input(csv: &str) -> Input {
        serde_json::from_value(serde_json::json!({"csv":csv,"granularity":"DAY","period_start":"2026-10-01","period_end":"2026-10-31","currency":"KRW","kind":"ESTIMATED","timezone":"Asia/Seoul","columns":{"date":"Date","unit_id":"Unit","revenue":"Revenue"},"format":{"delimiter":",","date_format":"YYYY-MM-DD","decimal_separator":".","group_separator":","}})).unwrap()
    }
    #[test]
    fn fixed_point_is_exact_and_never_guesses_or_rounds() {
        assert_eq!(money("1,234.123456", ".", ",").unwrap(), 1_234_123_456);
        assert_eq!(money("-0.000001", ".", ",").unwrap(), -1);
        assert!(money("1.1234567", ".", ",").is_err());
        for bad in ["NaN", "1e3", "₩10", "12,34", "9,999,999,999,999,999,999"] {
            assert!(money(bad, ".", ",").is_err());
        }
        assert_eq!(money("1.234,5", ",", ".").unwrap(), 1_234_500_000);
    }
    #[test]
    fn explicit_mapping_and_overlap_and_provenance_are_enforced() {
        let csv = "Date,Unit,Revenue\n2026-10-01,DAN-kVpBmlPRbOvGb6oi,1.25\n";
        let p = prepare(input(csv)).unwrap();
        assert_eq!(p.rows[0].revenue_micros, "1250000");
        assert!(p.rows[0].impressions.is_none());
        assert_eq!(p.preview_hash, prepare(input(csv)).unwrap().preview_hash);
        assert!(
            prepare(input(
                &(csv.to_owned() + "2026-10-01,DAN-kVpBmlPRbOvGb6oi,9\n")
            ))
            .is_err()
        );
        assert!(prepare(input("Date,Unit,Revenue\n2026-10-01,total,2\n")).is_err());
        let mut a = input(csv);
        a.timezone = "guessed".into();
        assert!(prepare(a).is_err());
    }
}
