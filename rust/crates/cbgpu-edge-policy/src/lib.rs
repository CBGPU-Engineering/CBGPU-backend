//! Cookie boundary for untrusted application/notebook HTTP and WS responses.
use http::{HeaderMap, HeaderValue};
pub fn protected_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["sb-cbgpu-", "cbgpu", "__host-cbgpu", "__secure-cbgpu"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}
pub fn strip_request(headers: &mut HeaderMap) {
    let mut kept = vec![];
    for h in headers.get_all("cookie") {
        if let Ok(raw) = h.to_str() {
            for p in raw.split(';') {
                if let Some((name, _)) = p.trim().split_once('=')
                    && !protected_name(name.trim())
                {
                    kept.push(p.trim().to_owned());
                }
            }
        }
    }
    headers.remove("cookie");
    if !kept.is_empty()
        && let Ok(value) = HeaderValue::from_str(&kept.join("; "))
    {
        headers.insert("cookie", value);
    }
    let names = headers
        .keys()
        .filter(|n| n.as_str().starts_with("x-cbgpu-"))
        .cloned()
        .collect::<Vec<_>>();
    for name in names {
        headers.remove(name);
    }
}
fn domain(value: &str) -> Option<String> {
    let d = value
        .trim()
        .trim_matches('"')
        .strip_prefix('.')
        .unwrap_or(value.trim().trim_matches('"'))
        .to_ascii_lowercase();
    if d.len() < 2
        || !d.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        || !d.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        || d.ends_with('.')
        || d.contains("..")
        || !d
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        None
    } else {
        Some(d)
    }
}
pub fn safe_set_cookie(cookie: &str, host: &str) -> bool {
    if cookie.contains(['\r', '\n', '\0']) {
        return false;
    }
    let mut p = cookie.split(';');
    let Some((name, _)) = p.next().unwrap_or("").trim().split_once('=') else {
        return false;
    };
    let name = name.trim();
    if name.is_empty()
        || protected_name(name)
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_\x60|~".contains(&b))
    {
        return false;
    }
    let mut scoped = None;
    for a in p {
        if let Some((key, value)) = a.split_once('=')
            && key.trim().eq_ignore_ascii_case("domain")
        {
            if scoped.is_some() {
                return false;
            }
            let Some(value) = domain(value) else {
                return false;
            };
            scoped = Some(value);
        }
    }
    let Some(domain) = scoped else {
        return true;
    };
    let host = host
        .trim()
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host == "cbgpu.com" || host.ends_with(".cbgpu.com") {
        domain == host && domain != "cbgpu.com"
    } else if domain == "cbgpu.com" || domain.ends_with(".cbgpu.com") {
        false
    } else {
        domain.contains('.') && (host == domain || host.ends_with(&format!(".{domain}")))
    }
}
pub fn filter_response(headers: &mut HeaderMap, host: &str) {
    let safe = headers
        .get_all("set-cookie")
        .iter()
        .filter(|h| h.to_str().is_ok_and(|v| safe_set_cookie(v, host)))
        .cloned()
        .collect::<Vec<_>>();
    headers.remove("set-cookie");
    for value in safe {
        headers.append("set-cookie", value);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_parent_sibling_and_protected_cookie_names() {
        for c in [
            "sb-cbgpu-auth-token.0=x",
            "__Host-cbgpu-latex=x",
            "CBGPU-session=x",
            "__Secure-cbgpu=x",
            "login=x; Domain=.cbgpu.com",
            "login=x; Domain=server.cbgpu.com",
            "login=x; Domain=other.server.cbgpu.com",
            "login=x; Domain=tenant.server.cbgpu.com; Domain=cbgpu.com",
        ] {
            assert!(!safe_set_cookie(c, "tenant.server.cbgpu.com"), "{c}");
        }
        assert!(safe_set_cookie(
            "own=x; HttpOnly",
            "tenant.server.cbgpu.com"
        ));
        assert!(safe_set_cookie(
            "own=x; Domain=tenant.server.cbgpu.com",
            "tenant.server.cbgpu.com"
        ));
        assert!(safe_set_cookie(
            "own=x; Domain=example.org",
            "app.example.org"
        ));
    }
    #[test]
    fn keeps_unrelated_credentials_and_filters_all_set_cookie_headers() {
        let mut h = HeaderMap::new();
        h.insert(
            "cookie",
            "sb-cbgpu-auth-token.0=secret; __Host-cbgpu-latex=secret; own=ok; sb-tenant-auth-token=tenant"
                .parse()
                .unwrap(),
        );
        strip_request(&mut h);
        assert_eq!(h["cookie"], "own=ok; sb-tenant-auth-token=tenant");
        assert!(safe_set_cookie(
            "sb-tenant-auth-token=tenant; HttpOnly",
            "tenant.server.cbgpu.com"
        ));
        h.append(
            "set-cookie",
            "sb-cbgpu-auth-token.0=poison; Domain=.cbgpu.com"
                .parse()
                .unwrap(),
        );
        h.append(
            "set-cookie",
            "own=ok; Expires=Wed, 21 Oct 2030 07:28:00 GMT"
                .parse()
                .unwrap(),
        );
        filter_response(&mut h, "gpu.cbgpu.com");
        assert_eq!(h.get_all("set-cookie").iter().count(), 1);
        assert!(h["set-cookie"].to_str().unwrap().starts_with("own=ok"));
    }
}
