use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cbgpu_portal_auth::{
    core::{Authenticated, eligible},
    crypto::*,
    proxy_contract::bound_bearer,
    *,
};
use serde_json::json;
const KEY: &str = "fixture-session-secret-not-production-123456";
fn id() -> uuid::Uuid {
    "10000000-0000-4000-8000-000000000001".parse().unwrap()
}
fn sid() -> uuid::Uuid {
    "20000000-0000-4000-8000-000000000001".parse().unwrap()
}
fn jwt(user: uuid::Uuid, session: uuid::Uuid, expires: i64) -> String {
    format!(
        "eyJhbGciOiJIUzI1NiJ9.{}.fixture_signature",
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"sub":user,"session_id":session,"exp":expires})).unwrap()
        )
    )
}
fn row(c: &Crypto) -> SessionRow {
    SessionRow {
        token_hash: c.hash(&"a".repeat(64)),
        csrf: "b".repeat(64),
        expires: now_ms() + 3_600_000,
        user_id: id(),
        email: "dev@example.invalid".into(),
        auth_version: c.version(id()),
        auth_session_id: sid(),
        auth_tokens: c
            .seal(&Tokens {
                access_token: jwt(id(), sid(), now_ms() / 1000 + 3600),
                refresh_token: "fixture-refresh".into(),
            })
            .unwrap(),
    }
}
#[test]
fn legacy_node_hmac_and_aes_envelope_are_exact() {
    let c = Crypto::new(KEY).unwrap();
    assert_eq!(
        c.hash(&"a".repeat(64)),
        "5208c1bfa756a950ba7f1d1271f71a38cea4dee2228bcecc69851ae29d6103f9"
    );
    assert_eq!(
        c.version(id()),
        "88edeb0544dc95d367b8c1e8fe4ff0e668e5693137c6784761b1f44df342383b"
    );
    let node = "AAECAwQFBgcICQoLqxBMuoWXCVj3JXBzkVW0ziQZ7mdyiWUVDsTvzadqIRsboDPozXDXRQ1nS6TctT01flBKaGi4nCewIXW/+916H4uohmOMZA5FMpcJ5ER7nLmMIvucH8d5657gfYsAfPeZ0XE89EnzjojSNMiyGKpOMsib1rcZMbNogrGKxQozCQT9UEMNta9+GA4XJFjDJw/X0jLwqnIFNOx5pTpYE1xp30h5aI+nUW0Kf6gtjqhxz6tvnGod+buzTy8PrV3bgG+YfEi8KJDIi54PCjJDM5OQACHU11/7KvZK7oeVI7qjX/7FoB015W0wVdsyXQWaY/Iaj2CscayeZJxGghezBJ5FGu8AzLiKrg==";
    let t = c.open(node).unwrap();
    assert_eq!(t.refresh_token, "fixture-refresh");
    assert_eq!(claims(&t.access_token).unwrap().sub, id());
    let mut r = row(&c);
    r.auth_tokens = node.into();
    assert!(
        c.row_tokens(&r).is_ok(),
        "expiredJWTmayremainin8hportalrow, checkedbyfreshAuthsessionauthority"
    );
    assert!(
        Crypto::new("another-session-secret-not-production-987654")
            .unwrap()
            .open(node)
            .is_err()
    );
    let mut bad = node.as_bytes().to_vec();
    bad[50] = if bad[50] == b'A' { b'B' } else { b'A' };
    assert!(c.open(std::str::from_utf8(&bad).unwrap()).is_err());
}
#[test]
fn swapped_ciphertext_row_subject_session_and_version_are_rejected() {
    let c = Crypto::new(KEY).unwrap();
    for n in 0..3 {
        let mut r = row(&c);
        match n {
            0 => r.user_id = "10000000-0000-4000-8000-000000000002".parse().unwrap(),
            1 => r.auth_session_id = "20000000-0000-4000-8000-000000000002".parse().unwrap(),
            _ => r.auth_version = "0".repeat(64),
        }
        assert!(c.row_tokens(&r).is_err())
    }
}
#[test]
fn strict_flags_never_fall_back_on_malformed_scope_configuration() {
    for value in [None, Some(""), Some("0"), Some("false")] {
        assert!(!flag(value).unwrap())
    }
    for value in ["1", "true"] {
        assert!(flag(Some(value)).unwrap())
    }
    for value in ["yes", "enabled", "TRUE", " false ", "2"] {
        assert!(flag(Some(value)).is_err())
    }
}
#[test]
fn explicit_admin_is_independent_of_school_approval_only_when_scoped() {
    let member = Member {
        id: id(),
        email: "dev@example.invalid".into(),
    };
    let mut p = Principal {
        valid: true,
        id: id(),
        email: member.email.clone(),
        name: "Developer".into(),
        is_admin: true,
        legacy_approved: false,
    };
    assert!(eligible(&p, &member, true));
    assert!(!eligible(&p, &member, false));
    p.is_admin = false;
    assert!(
        !eligible(&p, &member, true),
        "equipmentmanagerorconfirmedteacherisnotdeveloper"
    );
    p.is_admin = true;
    p.email = "changed@example.invalid".into();
    assert!(!eligible(&p, &member, true));
    p.email = member.email.clone();
    p.valid = false;
    assert!(!eligible(&p, &member, true));
}
#[test]
fn cookie_scope_and_csrf_contract_do_not_accept_other_portals_or_duplicate_names() {
    let raw = "a".repeat(64);
    assert_eq!(read_cookie(&format!("{COOKIE}={raw}")), Some(raw.as_str()));
    assert!(read_cookie(&format!("__Host-cbgpu-print-manager={raw}")).is_none());
    assert!(read_cookie(&format!("{COOKIE}={raw}; {COOKIE}={raw}")).is_none());
    assert_eq!(
        cookie(COOKIE, &raw, 28800),
        format!("{COOKIE}={raw}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=28800")
    );
    assert!(csrf(&raw, &raw).is_ok());
    assert!(csrf(&raw, &"b".repeat(64)).is_err());
}
#[test]
fn fresh_caller_bearer_is_preserved_but_other_identities_never_borrow_portal_cookie() {
    let c = Crypto::new(KEY).unwrap();
    let auth = Authenticated {
        row: row(&c),
        user: User {
            id: id(),
            email: "dev@example.invalid".into(),
            name: "Developer".into(),
            verification_method: "cbgpu_account",
        },
    };
    let fresh = format!("Bearer {}", jwt(id(), sid(), now_ms() / 1000 + 3600));
    let cached = Tokens {
        access_token: jwt(id(), sid(), 100),
        refresh_token: "stale-refresh-not-renewed".into(),
    };
    assert_eq!(
        bound_bearer(Some(&fresh), &auth, Some(cached))
            .unwrap()
            .as_deref(),
        Some(fresh.as_str())
    );
    for bearer in [
        "Basic invalid".into(),
        format!(
            "Bearer {}",
            jwt(
                "10000000-0000-4000-8000-000000000002".parse().unwrap(),
                sid(),
                now_ms() / 1000 + 3600
            )
        ),
        format!(
            "Bearer {}",
            jwt(
                id(),
                "20000000-0000-4000-8000-000000000002".parse().unwrap(),
                now_ms() / 1000 + 3600
            )
        ),
        format!("Bearer {}", jwt(id(), sid(), 100)),
    ] {
        assert!(bound_bearer(Some(&bearer), &auth, None).is_err())
    }
}
