//! Backend 领域规则测试：label 校验、token 格式/摘要、Debug 遮蔽与状态枚举。

use crate::domain::backend::{BACKEND_TOKEN_PREFIX, BackendLabel, BackendStatus, BackendToken};

#[test]
fn label_accepts_normal_and_unicode_verbatim() {
    let label = BackendLabel::parse("local runtime").expect("plain label");
    assert_eq!(label.as_str(), "local runtime");
    let unicode = BackendLabel::parse("助手 后端 😀").expect("unicode label");
    assert_eq!(unicode.as_str(), "助手 后端 😀");
}

#[test]
fn label_rejects_whitespace_only() {
    for raw in ["", "   ", "\t\n ", " \u{3000}"] {
        assert!(
            BackendLabel::parse(raw).is_err(),
            "whitespace-only label {raw:?} must be rejected"
        );
    }
}

#[test]
fn label_rejects_nul() {
    assert!(BackendLabel::parse("a\u{0}b").is_err());
}

#[test]
fn label_rejects_over_80_scalar_values() {
    let ok = "a".repeat(80);
    assert!(BackendLabel::parse(&ok).is_ok());
    let too_long = "a".repeat(81);
    let err = BackendLabel::parse(&too_long).expect_err("81 scalars must fail");
    assert!(matches!(
        err,
        crate::domain::backend::BackendLabelError::TooLong(81)
    ));
}

#[test]
fn token_generate_matches_fixed_format() {
    let token = BackendToken::generate().expect("generate");
    let raw = token.as_str();
    assert!(raw.starts_with(BACKEND_TOKEN_PREFIX));
    assert_eq!(raw.len(), BACKEND_TOKEN_PREFIX.len() + 43);
    let rest = &raw[BACKEND_TOKEN_PREFIX.len()..];
    assert!(
        rest.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
}

#[test]
fn token_parse_candidate_roundtrip_and_rejects() {
    let token = BackendToken::generate().expect("generate");
    assert_eq!(
        BackendToken::parse_candidate(token.as_str()).expect("roundtrip"),
        token
    );
    // Human token 前缀不是 Backend token。
    assert!(
        BackendToken::parse_candidate("clr_hs_0000000000000000000000000000000000000000000")
            .is_none()
    );
    // 长度错误。
    assert!(BackendToken::parse_candidate("clr_bc_short").is_none());
    // 字符集错误。
    let bad = format!("clr_bc_{}", "a".repeat(43).replace('a', "!"));
    assert!(BackendToken::parse_candidate(&bad).is_none());
    // 前缀缺失。
    assert!(BackendToken::parse_candidate(&"a".repeat(50)).is_none());
}

#[test]
fn token_hash_is_64_lowercase_hex() {
    let token = BackendToken::generate().expect("generate");
    let hash = token.sha256_hex();
    assert_eq!(hash.len(), 64);
    assert!(
        hash.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
}

#[test]
fn token_debug_is_redacted() {
    let token = BackendToken::generate().expect("generate");
    let debug = format!("{token:?}");
    assert!(
        !debug.contains(token.as_str()),
        "Debug must not leak the token"
    );
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn status_roundtrip() {
    assert_eq!(BackendStatus::parse("active"), Some(BackendStatus::Active));
    assert_eq!(
        BackendStatus::parse("revoked"),
        Some(BackendStatus::Revoked)
    );
    assert_eq!(BackendStatus::parse("unknown"), None);
    assert_eq!(BackendStatus::Active.as_str(), "active");
    assert_eq!(BackendStatus::Revoked.as_str(), "revoked");
}
