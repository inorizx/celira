//! opaque token 生成、格式校验与摘要测试（契约 4.3）。

use crate::domain::human_session::TokenHash;
use crate::domain::token::{OpaqueToken, TOKEN_PREFIX};

#[test]
fn generated_token_has_prefix_and_at_least_256_bit_randomness() {
    let token = OpaqueToken::generate().expect("secure random must work");
    assert!(token.as_str().starts_with(TOKEN_PREFIX));
    assert_eq!(token.as_str().len(), TOKEN_PREFIX.len() + 43);
}

#[test]
fn generated_tokens_are_distinct() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..16 {
        let token = OpaqueToken::generate().unwrap();
        assert!(seen.insert(token.clone()), "tokens must not repeat");
    }
}

#[test]
fn hash_is_fixed_length_deterministic_and_discriminating() {
    let token = OpaqueToken::generate().unwrap();
    let hash = TokenHash::from_token(&token);
    assert_eq!(hash.hex().len(), 64, "fixed-length digest");
    assert_eq!(TokenHash::from_token(&token), hash, "deterministic");
    assert_ne!(
        TokenHash::from_token(&OpaqueToken::generate().unwrap()),
        hash
    );

    let parsed = TokenHash::parse_hex(&hash.hex()).expect("hex roundtrip");
    assert_eq!(parsed, hash);
    assert_eq!(TokenHash::parse_hex(&hash.hex()[..63]), None);
    assert_eq!(TokenHash::parse_hex("zz".repeat(32).as_str()), None);
}

#[test]
fn parse_candidate_accepts_generated_token() {
    let token = OpaqueToken::generate().unwrap();
    assert_eq!(OpaqueToken::parse_candidate(token.as_str()), Some(token));
}

#[test]
fn parse_candidate_rejects_malformed_tokens() {
    let valid = OpaqueToken::generate().unwrap();
    let valid_raw = valid.as_str();
    let rand_part = &valid_raw[TOKEN_PREFIX.len()..];

    let cases = [
        valid_raw[1..].to_owned(),                      // wrong prefix
        format!("{TOKEN_PREFIX}{}", &rand_part[..42]),  // short random part
        format!("{TOKEN_PREFIX}{}x", rand_part),        // long random part
        TOKEN_PREFIX.to_owned(),                        // prefix only
        format!("{TOKEN_PREFIX}!!{}", &rand_part[2..]), // invalid charset
        format!("{TOKEN_PREFIX}{} ", rand_part),        // trailing space
        String::new(),
    ];
    for bad in cases {
        assert_eq!(
            OpaqueToken::parse_candidate(&bad),
            None,
            "expected rejection of {bad:?}"
        );
    }
}

#[test]
fn debug_output_never_leaks_token() {
    let token = OpaqueToken::generate().unwrap();
    let debug = format!("{token:?}");
    assert!(
        !debug.contains(token.as_str()),
        "Debug must redact the token, got {debug}"
    );
}
