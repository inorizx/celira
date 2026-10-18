//! Agent 领域规则测试（CORE_V1_AGENT_OWNERSHIP.md 4.x）。

use sha2::{Digest, Sha256};

use crate::domain::agent::{
    Agent, AgentOwner, IdempotencyKey, IdempotencyKeyError, create_request_digest,
};
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};

#[test]
fn idempotency_key_accepts_visible_ascii_boundaries() {
    assert_eq!(
        IdempotencyKey::parse("a"),
        Ok(IdempotencyKey("a".to_owned()))
    );
    assert_eq!(
        IdempotencyKey::parse("!"),
        Ok(IdempotencyKey("!".to_owned())),
        "0x21 is the lowest visible ASCII"
    );
    assert_eq!(
        IdempotencyKey::parse("~"),
        Ok(IdempotencyKey("~".to_owned())),
        "0x7E is the highest visible ASCII"
    );
    let boundary = "a".repeat(128);
    assert!(
        IdempotencyKey::parse(&boundary).is_ok(),
        "128 characters must pass"
    );
    assert_eq!(
        IdempotencyKey::parse(&"a".repeat(129)),
        Err(IdempotencyKeyError::TooLong(129))
    );
}

#[test]
fn idempotency_key_rejects_empty() {
    assert_eq!(IdempotencyKey::parse(""), Err(IdempotencyKeyError::Empty));
}

#[test]
fn idempotency_key_rejects_invalid_characters() {
    for raw in [
        "a,b",
        ",",
        "a\u{0}b",
        "\u{1}ab",
        "a b",
        "\u{00A0}a",
        "a\u{00A0}",
        "é",
        "ａ",
        "ab\u{00A0}",
    ] {
        assert_eq!(
            IdempotencyKey::parse(raw),
            Err(IdempotencyKeyError::InvalidCharacter),
            "expected rejection for {raw:?}"
        );
    }
}

#[test]
fn idempotency_key_rejects_leading_and_trailing_whitespace() {
    for raw in [" a", "a ", " a ", "\ta", "a\t"] {
        assert_eq!(
            IdempotencyKey::parse(raw),
            Err(IdempotencyKeyError::LeadingOrTrailingWhitespace),
            "expected rejection for {raw:?}"
        );
    }
}

#[test]
fn idempotency_key_preserves_bytes_verbatim() {
    let key = IdempotencyKey::parse("key-1_2.3").expect("printable key");
    assert_eq!(key.as_str(), "key-1_2.3");
    assert_eq!(
        IdempotencyKey::parse("key-1_2.3 "),
        Err(IdempotencyKeyError::LeadingOrTrailingWhitespace),
        "trailing space must be rejected, not normalized away"
    );
    assert_eq!(
        IdempotencyKey::parse("key-1_2.3\u{0}"),
        Err(IdempotencyKeyError::InvalidCharacter),
        "NUL must be rejected, not trimmed"
    );
}

#[test]
fn create_request_digest_is_deterministic_and_covers_raw_display_name() {
    let digest = create_request_digest("助手");
    let mut hasher = Sha256::new();
    hasher.update(b"display_name=");
    hasher.update("助手".as_bytes());
    assert_eq!(digest, hex::encode(hasher.finalize()));
    assert_eq!(digest, create_request_digest("助手"));
    assert_ne!(
        create_request_digest("助手"),
        create_request_digest("助手 ")
    );
    assert_ne!(create_request_digest("a"), create_request_digest("b"));
    assert_eq!(digest.len(), 64);
}

#[test]
fn new_agent_and_owner_defaults_follow_contract() {
    let id = ParticipantId("00000000-0000-0000-0000-000000000001".into());
    let name = DisplayName::parse("助手").unwrap();
    let agent = Agent::new(
        id.clone(),
        name.clone(),
        crate::domain::time::UtcMillis::now(),
    );
    assert_eq!(agent.kind, ParticipantKind::Agent);
    assert_eq!(agent.status, ParticipantStatus::Active);
    assert_eq!(agent.revoked_at, None);
    assert_eq!(agent.display_name, name);

    let human = ParticipantId("00000000-0000-0000-0000-000000000002".into());
    let owner = AgentOwner::new(
        id.clone(),
        human.clone(),
        crate::domain::time::UtcMillis::now(),
    );
    assert_eq!(owner.agent_id, id);
    assert_eq!(owner.human_id, human);
    assert_eq!(owner.status, ParticipantStatus::Active);
    assert_eq!(owner.revoked_at, None);
}
