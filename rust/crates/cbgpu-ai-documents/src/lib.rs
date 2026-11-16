//! Offline document evidence. Native orchestration of sandboxed Poppler, ImageMagick and Tesseract.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
pub type Result<T> = std::result::Result<T, String>;
pub const MAX_PAGES: usize = 100;
pub const MAX_TEXT: usize = 200_000;
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Extracted {
    pub chunks: Vec<(Option<u32>, String)>,
    #[serde(default)]
    pub pages: usize,
    #[serde(default)]
    pub ocr_page_numbers: Vec<u32>,
    #[serde(default)]
    pub weak_pages: Vec<u32>,
    #[serde(default)]
    pub empty_pages: Vec<u32>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub charts: Vec<Value>,
}
fn fail<T>(s: &str) -> Result<T> {
    Err(s.into())
}
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
fn command(program: &str, args: &[&str], seconds: u64) -> Result<String> {
    let file = tempfile::tempfile().map_err(|_| "문서 임시 파일을 만들지 못했어요.")?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(
            file.try_clone()
                .map_err(|_| "문서 도구를 시작하지 못했어요.")?,
        )
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "문서 처리 도구를 시작하지 못했어요.")?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return fail("문서를 읽지 못했어요. 올바른 PDF나 이미지로 다시 올려 주세요.");
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return fail("문서 처리 시간이 초과됐어요. 파일을 나누어 다시 올려 주세요.");
            }
        }
    }
    use std::io::{Seek, SeekFrom};
    let mut file = file;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "문서 결과를 읽지 못했어요.")?;
    let mut bytes = vec![];
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "문서 결과를 읽지 못했어요.")?;
    if bytes.len() > 4 * 1024 * 1024 {
        return fail("문서가 너무 커요. 파일을 나누어 올려 주세요.");
    }
    String::from_utf8(bytes).map_err(|_| "문서 텍스트 인코딩을 읽지 못했어요.".into())
}
fn path_text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| "잘못된 문서 경로예요.".into())
}
fn emit(value: Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &value).map_err(|_| "문서 진행 상태를 기록하지 못했어요.")?;
    writeln!(out).map_err(|_| "문서 진행 상태를 기록하지 못했어요.".into())
}
/// Tesseract TSV confidence weighted by Unicode character length, matching previous Korean OCR selection.
pub fn recognition_score(tsv: &str) -> (f64, u32) {
    let mut length = 0usize;
    let mut weighted = 0.;
    let mut score = 0.;
    for line in tsv.lines().skip(1) {
        let columns: Vec<_> = line.splitn(12, '\t').collect();
        if columns.len() != 12 || columns[11].trim().is_empty() {
            continue;
        }
        let Ok(conf) = columns[10].parse::<f64>() else {
            continue;
        };
        if !conf.is_finite() || !(0.0..=100.0).contains(&conf) {
            continue;
        }
        let n = columns[11].chars().count();
        length += n;
        weighted += n as f64 * conf;
        score += n as f64 * (conf - 35.0).max(0.0);
    }
    (
        score,
        if length == 0 {
            0
        } else {
            (weighted / length as f64).round_ties_even() as u32
        },
    )
}
fn recognize(source: &str, folder: &Path) -> Result<(String, u32)> {
    let target = folder.join("page.png");
    let target = path_text(&target)?;
    // Prefix is selected by trusted suffix, never from a caller-supplied command or URL.
    let dimensions = command("identify", &["-ping", "-format", "%w %h", source], 10)?;
    let sizes: Vec<_> = dimensions.split_whitespace().collect();
    if sizes.len() != 2 {
        return fail("이미지 크기를 읽지 못했어요.");
    }
    let width = sizes[0]
        .parse::<u64>()
        .map_err(|_| "잘못된 이미지 크기예요.")?;
    let height = sizes[1]
        .parse::<u64>()
        .map_err(|_| "잘못된 이미지 크기예요.")?;
    if width == 0 || height == 0 || width.checked_mul(height).is_none_or(|n| n > 25_000_000) {
        return fail("이미지 해상도가 너무 커요. 2,500만 픽셀 이하로 올려 주세요.");
    }
    let mut args = vec![
        "-limit",
        "memory",
        "128MiB",
        "-limit",
        "map",
        "128MiB",
        source,
        "-auto-orient",
        "-colorspace",
        "Gray",
        "-auto-level",
        "-thumbnail",
        "2400x2400>",
    ];
    if width < 1200 && height < 1200 {
        args.extend(["-resize", "200%"]);
    }
    args.push(target);
    command("convert", &args, 20)?;
    let mut best: Option<(f64, String, u32)> = None;
    for segmentation in ["3", "6"] {
        let prefix = folder.join(format!("recognized-{segmentation}"));
        command(
            "tesseract",
            &[
                target,
                path_text(&prefix)?,
                "-l",
                "kor+eng",
                "--psm",
                segmentation,
                "txt",
                "tsv",
            ],
            30,
        )?;
        let text = fs::read_to_string(prefix.with_extension("txt"))
            .map_err(|_| "OCR 결과를 읽지 못했어요.")?;
        let tsv = fs::read_to_string(prefix.with_extension("tsv"))
            .map_err(|_| "OCR 결과를 읽지 못했어요.")?;
        let (score, confidence) = recognition_score(&tsv);
        if best.as_ref().is_none_or(|v| score > v.0) {
            best = Some((score, text, confidence));
        }
    }
    let (_, text, confidence) = best.ok_or("OCR 결과가 없어요.")?;
    Ok((text, confidence))
}
fn push_page(result: &mut Extracted, page: u32, text: &str, total: &mut usize, limit: usize) {
    let count = text.chars().count();
    let allowed = 12_000.min(limit.saturating_sub(*total));
    result.truncated |= count > allowed;
    let text = take_chars(text, allowed);
    *total += text.chars().count();
    let chars: Vec<_> = text.chars().collect();
    for chunk in chars.chunks(1000) {
        let value: String = chunk.iter().collect();
        if !value.trim().is_empty() {
            result.chunks.push((Some(page), value));
        }
    }
}
pub fn extract_visual(path: &Path, suffix: &str) -> Result<Extracted> {
    if !path.is_file() {
        return fail("파일을 찾을 수 없어요.");
    }
    let folder = tempfile::tempdir().map_err(|_| "문서 임시 폴더를 만들지 못했어요.")?;
    let canonical = path.canonicalize().map_err(|_| "파일을 찾을 수 없어요.")?;
    let input = path_text(&canonical)?;
    let mut result = Extracted::default();
    let mut total = 0;
    if suffix == ".pdf" {
        let info = command("pdfinfo", &[input], 10)?;
        if info.lines().any(|l| {
            l.strip_prefix("Encrypted:")
                .is_some_and(|s| s.trim_start().starts_with("yes"))
        }) {
            return fail("암호가 걸린 PDF예요. 암호를 해제한 파일을 올려 주세요.");
        }
        let count = info
            .lines()
            .find_map(|l| l.strip_prefix("Pages:"))
            .and_then(|s| s.trim().parse::<usize>().ok())
            .ok_or("PDF 페이지 수를 읽지 못했어요.")?;
        if !(1..=MAX_PAGES).contains(&count) {
            return fail("PDF는 1~100쪽까지 읽을 수 있어요.");
        }
        result.pages = count;
        for index in 1..=count {
            emit(json!({"progress":index-1,"pages":count}))?;
            let page = index.to_string();
            let mut text = command(
                "pdftotext",
                &[
                    "-f", &page, "-l", &page, "-layout", "-enc", "UTF-8", input, "-",
                ],
                20,
            )?;
            let meaningful = text.chars().filter(|c| c.is_alphanumeric()).count();
            let images = meaningful < 200
                && command("pdfimages", &["-f", &page, "-l", &page, "-list", input], 10)?
                    .lines()
                    .count()
                    > 2;
            if meaningful < 40 || images {
                let prefix = folder.path().join("render");
                command(
                    "pdftoppm",
                    &[
                        "-f",
                        &page,
                        "-l",
                        &page,
                        "-singlefile",
                        "-scale-to",
                        "2200",
                        "-png",
                        input,
                        path_text(&prefix)?,
                    ],
                    20,
                )?;
                let (recognized, confidence) =
                    recognize(path_text(&prefix.with_extension("png"))?, folder.path())?;
                if !recognized.trim().is_empty() {
                    text = recognized;
                    result.ocr_page_numbers.push(index as u32);
                    if confidence < 65 {
                        result.weak_pages.push(index as u32);
                    }
                }
            }
            if text.trim().is_empty() {
                result.empty_pages.push(index as u32);
            }
            push_page(&mut result, index as u32, &text, &mut total, MAX_TEXT);
            emit(
                json!({"progress":index,"pages":count,"ocr_pages":result.ocr_page_numbers.len()}),
            )?;
            if total >= MAX_TEXT {
                result.truncated |= index < count;
                break;
            }
        }
    } else {
        let format = match suffix {
            ".png" => "png",
            ".jpg" | ".jpeg" => "jpeg",
            ".webp" => "webp",
            ".tif" | ".tiff" => "tiff",
            _ => return fail("지원하지 않는 이미지 형식이에요."),
        };
        let source = format!("{format}:{input}");
        let frames = command("identify", &["-ping", "-format", "%w %h\n", &source], 10)?;
        let count = frames.lines().filter(|s| !s.trim().is_empty()).count();
        if !(1..=20).contains(&count) {
            return fail("여러 장이 담긴 이미지는 20장까지 읽을 수 있어요.");
        }
        result.pages = count;
        for index in 1..=count {
            emit(json!({"progress":index-1,"pages":count}))?;
            let source = format!("{format}:{input}[{}]", index - 1);
            let (text, confidence) = recognize(&source, folder.path())?;
            result.ocr_page_numbers.push(index as u32);
            if confidence < 65 {
                result.weak_pages.push(index as u32);
            }
            if text.trim().is_empty() {
                result.empty_pages.push(index as u32);
            }
            push_page(&mut result, index as u32, &text, &mut total, 240_000);
            emit(json!({"progress":index,"pages":count,"ocr_pages":index}))?;
            if total >= 240_000 {
                result.truncated |= index < count;
                break;
            }
        }
    }
    if result.chunks.is_empty() {
        return fail(
            "읽을 수 있는 글자를 찾지 못했어요. 글자가 선명하게 보이는 파일을 올려 주세요.",
        );
    }
    Ok(result)
}
fn significant(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let exponent = value.abs().log10().floor() as i32;
    if !(-4..8).contains(&exponent) {
        let raw = format!("{value:.7e}");
        let (mantissa, power) = raw.split_once('e').expect("scientific number");
        format!(
            "{}e{:+03}",
            mantissa.trim_end_matches('0').trim_end_matches('.'),
            power.parse::<i32>().expect("scientific exponent")
        )
    } else {
        let raw = format!(
            "{value:.precision$}",
            precision = (7 - exponent).max(0) as usize
        );
        if raw.contains('.') {
            raw.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            raw
        }
    }
}
pub fn csv_analysis(text: &str) -> Result<Extracted> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|_| "CSV 헤더를 확인해 주세요.")?
        .clone();
    if headers.is_empty() || headers.len() > 64 {
        return fail("CSV needs a header and at most 64 columns.");
    }
    let mut columns = vec![vec![]; headers.len()];
    let mut counts = vec![0; headers.len()];
    let mut rows = 0;
    for row in reader.records() {
        let row = row.map_err(|_| "CSV rows must match the header.")?;
        if rows >= 100_000 {
            return fail("CSV supports at most 100,000 rows.");
        }
        rows += 1;
        for (index, cell) in row.iter().enumerate() {
            if cell.trim().is_empty() {
                continue;
            }
            counts[index] += 1;
            if let Ok(value) = cell.trim().parse::<f64>()
                && value.is_finite()
                && value.abs() <= 1e100
            {
                columns[index].push(value);
            }
        }
    }
    let mut lines = vec![format!("Rows: {rows}")];
    let mut result = Extracted {
        pages: 1,
        ..Default::default()
    };
    for (index, values) in columns.iter().enumerate() {
        if values.is_empty() || values.len() != counts[index] {
            continue;
        }
        let count = values.len();
        let mean = values.iter().sum::<f64>() / count as f64;
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let label = take_chars(&headers[index], 100);
        let mut line = format!(
            "{label}: n={count}, missing={}, mean={}, min={}, max={}",
            rows - counts[index],
            significant(mean),
            significant(min),
            significant(max)
        );
        if count > 1 {
            let variance =
                values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (count - 1) as f64;
            line.push_str(&format!(", sample SD={}", significant(variance.sqrt())));
        }
        lines.push(line);
        if result.charts.len() < 8 {
            let n = count.min(80);
            let points: Vec<_> = (0..n)
                .map(|i| {
                    let k = (i as f64 * (count - 1) as f64 / (n - 1).max(1) as f64)
                        .round_ties_even() as usize;
                    json!([k + 1, values[k]])
                })
                .collect();
            result
                .charts
                .push(json!({"column":label,"count":count,"points":points}));
        }
    }
    result.chunks.push((None, lines.join("\n")));
    Ok(result)
}
pub fn extract_text(path: &Path, suffix: &str) -> Result<Extracted> {
    let bytes = fs::read(path).map_err(|_| "파일을 읽지 못했어요.")?;
    if bytes.len() > 8 * 1024 * 1024 {
        return fail("텍스트 파일은 8MB 이하로 올려 주세요.");
    }
    let text = std::str::from_utf8(bytes.strip_prefix(&[239, 187, 191]).unwrap_or(&bytes))
        .map_err(|_| "UTF-8 텍스트 파일로 올려 주세요.")?;
    if suffix == ".csv" {
        return csv_analysis(text);
    }
    if ![".txt", ".md", ".json", ".tex", ".py", ".log"].contains(&suffix) {
        return fail("지원하지 않는 문서 형식이에요.");
    }
    let mut result = Extracted {
        pages: 1,
        truncated: text.chars().count() > MAX_TEXT,
        ..Default::default()
    };
    let chars: Vec<_> = text.chars().take(MAX_TEXT).collect();
    for chunk in chars.chunks(1000) {
        result.chunks.push((None, chunk.iter().collect()));
    }
    Ok(result)
}
pub fn evidence(name: &str, question: &str, result: &Extracted, single: bool) -> Vec<Value> {
    let re = regex::Regex::new(r"\w{2,}").expect("constant regex");
    let lower = question.to_lowercase();
    let mut words = vec![];
    for m in re.find_iter(&lower) {
        if !words.contains(&m.as_str()) {
            words.push(m.as_str());
        }
        if words.len() == 64 {
            break;
        }
    }
    let mut ranked: Vec<_> = result
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, (_, text))| !text.trim().is_empty())
        .collect();
    ranked.sort_by_key(|(i, (_, text))| {
        let text = text.to_lowercase();
        (
            std::cmp::Reverse(words.iter().filter(|word| text.contains(**word)).count()),
            *i,
        )
    });
    ranked.into_iter().take(if single{1}else{2}).map(|(_, (page,text))|{let mut value=json!({"title":name,"page":page,"excerpt":take_chars(text.trim(),1000),"ocr":page.is_some_and(|n|result.ocr_page_numbers.contains(&n)),"low_confidence":page.is_some_and(|n|result.weak_pages.contains(&n)),"truncated":result.truncated});if !result.charts.is_empty(){value["charts"]=json!(result.charts);}value}).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_missing_numeric_and_quoted_fields() {
        let r = csv_analysis("name,value,category\n\"Lee, Min\",2,A\nKim,4,B\nPark,,C\n").unwrap();
        assert_eq!(r.charts.len(), 1);
        assert_eq!(r.charts[0]["column"], "value");
        assert!(r.chunks[0].1.contains("mean=3,"));
        assert!(r.chunks[0].1.contains("missing=1"));
        assert!(csv_analysis("a,b\n1\n").is_err());
    }
    #[test]
    fn rejects_nonfinite_mixed_columns_and_limits_charts() {
        let r = csv_analysis("x,y\nNaN,1\n2,2\n").unwrap();
        assert_eq!(r.charts.len(), 1);
        assert_eq!(r.charts[0]["column"], "y");
    }
    #[test]
    fn scientific_measurements_never_round_small_values_to_zero() {
        let r = csv_analysis("measurement\n1e-50\n3e-50\n").unwrap();
        assert!(r.chunks[0].1.contains("mean=2e-50"));
        assert_eq!(significant(123456789.0), "1.2345679e+08");
        assert_eq!(significant(0.0001), "0.0001");
    }
    #[test]
    fn tsv_weighted_unicode_confidence_and_invalid_rows() {
        let t = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t90\t한국어\n5\t1\t1\t1\t1\t2\t0\t0\t10\t10\t60\tAI\n";
        assert_eq!(recognition_score(t), (215.0, 78));
        // Keep the existing low-confidence threshold at the half-point boundary.
        assert_eq!(
            recognition_score(
                &t.replace("90\t한국어", "64.5\t한국어")
                    .replace("60\tAI", "64.5\tAI")
            )
            .1,
            64
        );
        assert_eq!(recognition_score("header\ninvalid"), (0.0, 0));
    }
    #[test]
    fn unicode_chunk_caps_and_stable_relevance() {
        let mut r = Extracted::default();
        let mut total = 199_999;
        push_page(&mut r, 1, &"가".repeat(2000), &mut total, MAX_TEXT);
        assert_eq!(total, 200_000);
        assert_eq!(r.chunks[0].1, "가");
        assert!(r.truncated);
        r.chunks.push((Some(2), "GPU 연구 결과".into()));
        r.ocr_page_numbers.push(2);
        let e = evidence("private.pdf", "GPU 연구", &r, false);
        assert_eq!(e[0]["page"], 2);
        assert_eq!(e[0]["ocr"], true);
    }
    #[test]
    fn utf8_bom_and_text_limit() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("text");
        fs::write(&f, "\u{feff}hello").unwrap();
        assert_eq!(extract_text(&f, ".txt").unwrap().chunks[0].1, "hello");
        fs::write(&f, [255]).unwrap();
        assert!(extract_text(&f, ".txt").is_err());
    }
}
