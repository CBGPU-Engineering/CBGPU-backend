//! Bounded public web search with DNS pinning and per-redirect SSRF checks.
use cbgpu_ai_store::{Result, fail, now};
use futures_util::StreamExt;
use scraper::{Html, Selector};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || a == 169 && b == 254
                || a == 172 && (16..=31).contains(&b)
                || a == 192 && (b == 168 || b == 88 && c == 99)
                || a == 100 && (64..=127).contains(&b)
                || a == 192 && b == 0
                || a == 198 && (b == 18 || b == 19 || b == 51 && c == 100)
                || a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(v) => {
            if let Some(ip) = v.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(ip));
            }
            let s = v.segments();
            s[0] & 0xe000 == 0x2000
                && s[0] != 0x3fff
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] <= 0x01ff || s[1] == 0x0db8))
        }
    }
}
pub fn checked_url(url: &url::Url) -> Result<(String, u16)> {
    if !["https", "http"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 4096
    {
        return Err(fail("Only public HTTP/HTTPS pages are supported", 400));
    }
    let host = match url.host() {
        Some(url::Host::Domain(h)) => h.to_owned(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(fail("Invalid web host", 400)),
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| fail("Invalid web port", 400))?;
    if ![80, 443].contains(&port) {
        return Err(fail("Only public HTTP/HTTPS pages are supported", 400));
    }
    Ok((host, port))
}
pub fn checked_addresses(addresses: &[std::net::SocketAddr]) -> Result<std::net::SocketAddr> {
    if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
        return Err(fail(
            "Private and internal network addresses are not allowed",
            400,
        ));
    }
    Ok(addresses[0])
}
pub async fn public_target(url: &url::Url) -> Result<(String, std::net::SocketAddr)> {
    let (host, port) = checked_url(url)?;
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![std::net::SocketAddr::new(ip, port)]
    } else {
        tokio::time::timeout(
            std::time::Duration::from_secs(8),
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| fail("Web DNS timed out", 502))?
        .map_err(|_| fail("Web DNS unavailable", 502))?
        .collect()
    };
    Ok((host, checked_addresses(&addresses)?))
}
pub async fn fetch_public(input: &str, maximum: usize) -> Result<(String, String)> {
    let mut url = url::Url::parse(input)
        .map_err(|_| fail("Only public HTTP/HTTPS pages are supported", 400))?;
    for _ in 0..3 {
        let (host, address) = public_target(&url).await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(12))
            .resolve(&host, address)
            .build()
            .map_err(|_| fail("Public web reader unavailable", 503))?;
        let response = client
            .get(url.clone())
            .header("user-agent", "Mozilla/5.0 (compatible; CBGPU-Search/1.0)")
            .header(
                "accept",
                "text/html,application/rss+xml,application/xml,text/plain",
            )
            .header("accept-encoding", "identity")
            .send()
            .await
            .map_err(|_| fail("This public page could not be read", 502))?;
        if response.status().is_redirection() {
            let target = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| fail("Invalid web redirect", 502))?;
            url = url
                .join(target)
                .map_err(|_| fail("Invalid web redirect", 502))?;
            continue;
        }
        if !response.status().is_success() {
            return Err(fail("This public page could not be read", 502));
        }
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !["text/", "xml", "html"].iter().any(|s| ct.contains(s)) {
            return Err(fail("Only text web pages are supported", 400));
        }
        let mut bytes = vec![];
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| fail("Web read interrupted", 502))?;
            if bytes.len() + chunk.len() > maximum {
                return Err(fail("Web page is too large", 413));
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((
            String::from_utf8_lossy(&bytes).into_owned(),
            url.to_string(),
        ));
    }
    Err(fail("Too many redirects", 400))
}
pub fn plain_text(html: &str) -> String {
    let mut value = html.to_owned();
    for tag in ["script", "style", "noscript", "svg"] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>")).unwrap();
        value = re.replace_all(&value, " ").into_owned();
    }
    Html::parse_document(&value)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
#[derive(Clone)]
pub struct WebSearch {
    gate: Arc<Semaphore>,
    cache: Arc<Mutex<HashMap<String, (f64, Value)>>>,
    usage: Arc<Mutex<HashMap<String, Vec<f64>>>>,
}
impl Default for WebSearch {
    fn default() -> Self {
        Self {
            gate: Arc::new(Semaphore::new(2)),
            cache: Arc::new(Mutex::new(HashMap::new())),
            usage: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
impl WebSearch {
    pub async fn search(&self, owner: &str, query: &str, compare: bool) -> Result<Value> {
        if query.trim().is_empty() {
            return Err(fail("검색할 질문을 입력해 주세요.", 400));
        }
        let query = query.trim().chars().take(500).collect::<String>();
        let scrub =
            regex::Regex::new(r"cbgpu_ai_[A-Za-z0-9_-]+|sk-[A-Za-z0-9_-]+|eyJ[A-Za-z0-9_.-]{30,}")
                .unwrap();
        let query = scrub
            .replace_all(&query, "[credential removed]")
            .into_owned();
        let key = format!("{owner}:{compare}:{query}");
        let at = now();
        if let Some((time, value)) = self
            .cache
            .lock()
            .map_err(|_| fail("Search unavailable", 503))?
            .get(&key)
            && at - time < 120.
        {
            return Ok(value.clone());
        }
        {
            let mut usage = self
                .usage
                .lock()
                .map_err(|_| fail("Search unavailable", 503))?;
            if usage.len() > 1000 {
                usage.retain(|_, v| v.iter().any(|t| at - t < 60.));
            }
            let recent = usage.entry(owner.into()).or_default();
            recent.retain(|t| at - t < 60.);
            if recent.len() >= 6 {
                return Err(fail("웹 검색은 분당 6번까지 이용할 수 있어요.", 429));
            }
            recent.push(at);
        }
        let _permit = self.gate.clone().try_acquire_owned().map_err(|_| {
            fail(
                "다른 웹 검색을 처리 중이에요. 잠시 후 다시 시도해 주세요.",
                429,
            )
        })?;
        let mut hits = vec![];
        let direct = regex::Regex::new(r"https?://[^\s<>]+").unwrap();
        if let Some(link) = direct.find(&query) {
            let link = link.as_str().trim_end_matches(['.', ',', ')']);
            if let Ok((body, url)) = fetch_public(link, 1024 * 1024).await {
                let content = plain_text(&body).chars().take(2200).collect::<String>();
                hits.push(json!({"title":url::Url::parse(&url).ok().and_then(|u|u.host_str().map(str::to_owned)),"url":url,"snippet":content.chars().take(250).collect::<String>(),"content":content}));
            }
        }
        let mut url = url::Url::parse("https://lite.duckduckgo.com/lite/").unwrap();
        url.query_pairs_mut().append_pair("q", &query);
        let (html, _) = fetch_public(url.as_str(), 512 * 1024).await?;
        if html.contains("anomaly.js") || html.contains("anomaly-modal") {
            return Err(fail(
                "검색엔진에서 요청을 제한했어요. 잠시 후 다시 시도해 주세요.",
                503,
            ));
        }
        let parsed = {
            let doc = Html::parse_document(&html);
            let a = Selector::parse("a.result-link").unwrap();
            let snippets = Selector::parse("td.result-snippet").unwrap();
            let snippets = doc
                .select(&snippets)
                .map(|e| e.text().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>();
            doc.select(&a)
                .take(8)
                .enumerate()
                .filter_map(|(i, e)| {
                    let href = e.value().attr("href")?;
                    let link = url::Url::parse("https://lite.duckduckgo.com")
                        .ok()?
                        .join(href)
                        .ok()?;
                    let target = link
                        .query_pairs()
                        .find(|(k, _)| k == "uddg")
                        .map(|(_, v)| v.to_string())
                        .unwrap_or(link.to_string());
                    let title = e
                        .text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(180)
                        .collect::<String>();
                    let snippet = snippets
                        .get(i)
                        .cloned()
                        .unwrap_or_default()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(500)
                        .collect::<String>();
                    Some(json!({"title":title,"url":target,"snippet":snippet,"content":snippet}))
                })
                .collect::<Vec<_>>()
        };
        for hit in parsed {
            if let Some(link) = hit["url"].as_str() {
                let Ok(url) = url::Url::parse(link) else {
                    continue;
                };
                if public_target(&url).await.is_err() {
                    continue;
                }
            } else {
                continue;
            }
            if !hits.iter().any(|h| h["url"] == hit["url"])
                && hit["title"].as_str().is_some_and(|s| !s.is_empty())
            {
                hits.push(hit);
            }
        }
        if hits.is_empty() {
            return Err(fail(
                "웹 검색 결과를 찾지 못했어요. 질문을 바꾸거나 웹 검색을 꺼 주세요.",
                502,
            ));
        }
        if compare {
            let mut seen = std::collections::HashSet::new();
            let mut unique = vec![];
            let mut repeated = vec![];
            for hit in hits {
                let host = hit["url"]
                    .as_str()
                    .and_then(|s| url::Url::parse(s).ok())
                    .and_then(|u| u.host_str().map(str::to_owned))
                    .unwrap_or_default();
                if seen.insert(host) {
                    unique.push(hit);
                } else {
                    repeated.push(hit);
                }
            }
            unique.extend(repeated);
            hits = unique;
        }
        for hit in hits.iter_mut().take(if compare { 4 } else { 2 }) {
            if let Some(link) = hit["url"].as_str()
                && let Ok((body, url)) = fetch_public(link, 1024 * 1024).await
            {
                let text = plain_text(&body).chars().take(1800).collect::<String>();
                if !text.is_empty() {
                    hit["url"] = json!(url);
                    hit["content"] = json!(text);
                }
            }
        }
        hits.truncate(6);
        let result = json!({"query":query,"sources":hits,"searched_at":now(),"provider":"DuckDuckGo web search"});
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| fail("Search unavailable", 503))?;
        if cache.len() > 200 {
            cache.retain(|_, (t, _)| at - *t < 120.);
        }
        cache.insert(key, (at, result.clone()));
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_private_and_documentation_ips() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "100.64.0.1",
            "203.0.113.1",
            "::1",
            "::ffff:127.0.0.1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip(IpAddr::V4(std::net::Ipv4Addr::new(8, 8, 8, 8))));
    }
    #[test]
    fn text_skips_hidden_markup() {
        assert_eq!(
            plain_text(
                "<html><style>secret</style><body>Hello<script>bad()</script> world</body></html>"
            ),
            "Hello world"
        );
    }
}
#[cfg(test)]
mod ssrf_tests {
    use super::*;
    #[test]
    fn bad_schemes_credentials_and_ports_fail_before_dns() {
        for s in [
            "file:///etc/passwd",
            "ftp://example.com/file",
            "https://u:password@example.com/",
            "http://example.com:3550/",
        ] {
            assert!(checked_url(&url::Url::parse(s).unwrap()).is_err(), "{s}");
        }
    }
    #[test]
    fn any_private_dns_answer_rejects_even_when_first_is_public() {
        let a = [
            "8.8.8.8:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        ];
        assert!(checked_addresses(&a).is_err());
        assert_eq!(checked_addresses(&a[..1]).unwrap(), a[0]);
    }
    #[tokio::test]
    async fn redirects_and_numeric_aliases_use_identical_preflight() {
        let start = url::Url::parse("https://example.com/").unwrap();
        for path in [
            "http://127.1/secret",
            "http://2130706433/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            let redirected = start.join(path).unwrap();
            assert!(public_target(&redirected).await.is_err(), "{path}");
        }
    }
}
