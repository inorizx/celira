//! Participant Presentation 领域校验测试（CORE_V1_PARTICIPANT_PRESENTATION.md
//! §2/§5）：字节/NUL/规范化/preview/版本/摘要。

use crate::domain::participant_presentation::presentation_limits::*;
use crate::domain::participant_presentation::{
    self, BaseVersion, ExistingIdempotency, IdempotencyDisposition, PresentationText,
    PresentationTextError, ProfileVersion, RequestDigest, VersionError,
};

// ---------- 文本校验 ----------

#[test]
fn nul_is_rejected_in_every_presentation_field() {
    assert_eq!(
        PresentationText::<PROFILE_SHORT_DESCRIPTION_MAX_BYTES>::parse("a\u{0}b"),
        Err(PresentationTextError::ContainsNul)
    );
    assert_eq!(
        PresentationText::<PROFILE_PUBLIC_DESCRIPTION_MAX_BYTES>::parse("\u{0}"),
        Err(PresentationTextError::ContainsNul)
    );
    assert_eq!(
        PresentationText::<ROOM_DISPLAY_NAME_MAX_BYTES>::parse("x\u{0}"),
        Err(PresentationTextError::ContainsNul)
    );
    assert_eq!(
        PresentationText::<ROOM_ROLE_NAME_MAX_BYTES>::parse("\u{0}x"),
        Err(PresentationTextError::ContainsNul)
    );
    assert_eq!(
        PresentationText::<ROOM_ROLE_DESCRIPTION_MAX_BYTES>::parse("ab\u{0}"),
        Err(PresentationTextError::ContainsNul)
    );
}

#[test]
fn byte_limits_are_enforced_on_utf8_bytes_not_characters() {
    // 512 字节上限：ASCII 512 通过，513 拒绝。
    assert!(PresentationText::<512>::parse(&"a".repeat(512)).is_ok());
    assert_eq!(
        PresentationText::<512>::parse(&"a".repeat(513)),
        Err(PresentationTextError::TooLong {
            bytes: 513,
            max_bytes: 512
        })
    );
    // 171 个 3 字节 CJK 字符 = 513 字节：按字符计数会误放行，按字节必须拒绝。
    assert_eq!(
        PresentationText::<512>::parse(&"界".repeat(171)),
        Err(PresentationTextError::TooLong {
            bytes: 513,
            max_bytes: 512
        })
    );
    assert!(PresentationText::<512>::parse(&"界".repeat(170)).is_ok());

    // 其余字段边界：room_display 80 / role_name 128 / room_role_description 4096
    // / public_description 16384。
    assert!(PresentationText::<80>::parse(&"a".repeat(80)).is_ok());
    assert!(PresentationText::<80>::parse(&"a".repeat(81)).is_err());
    assert!(PresentationText::<128>::parse(&"a".repeat(128)).is_ok());
    assert!(PresentationText::<128>::parse(&"a".repeat(129)).is_err());
    assert!(PresentationText::<4096>::parse(&"a".repeat(4096)).is_ok());
    assert!(PresentationText::<4096>::parse(&"a".repeat(4097)).is_err());
    assert!(PresentationText::<16384>::parse(&"a".repeat(16384)).is_ok());
    assert!(PresentationText::<16384>::parse(&"a".repeat(16385)).is_err());
}

#[test]
fn empty_strings_normalize_to_none_while_present_text_is_verbatim() {
    // 空串 -> None。
    assert_eq!(
        PresentationText::<512>::parse_optional(""),
        Ok(None),
        "empty string must normalize to None (NULL)"
    );
    assert_eq!(
        PresentationText::<512>::parse_opt(None),
        Ok(None),
        "absent value normalizes to None"
    );
    assert_eq!(
        PresentationText::<512>::parse_opt(Some("")),
        Ok(None),
        "Some(empty) must also normalize to None"
    );
    // 非空照常校验并原样保存（不做 Unicode 归一化、不做 trim）。
    let composed = PresentationText::<80>::parse("  Alice\u{0301} ")
        .expect("present text with spaces and combining marks must parse");
    assert_eq!(composed.as_str(), "  Alice\u{0301} ");
    let ws = PresentationText::<80>::parse("  ")
        .expect("whitespace-only text must parse as a provided value");
    assert_eq!(ws.as_str(), "  ", "no trim is applied");
}

// ---------- 版本 ----------

#[test]
fn base_version_must_be_non_negative() {
    assert_eq!(BaseVersion::parse(0), Ok(BaseVersion::ZERO));
    assert_eq!(BaseVersion::parse(3).unwrap().as_u64(), 3);
    assert_eq!(
        BaseVersion::parse(-1),
        Err(participant_presentation::BaseVersionError::Negative(-1))
    );
}

#[test]
fn versions_must_be_positive() {
    assert_eq!(ProfileVersion::parse(0), Err(VersionError::Zero));
    assert_eq!(ProfileVersion::from_u64(0), Err(VersionError::Zero));
    assert_eq!(ProfileVersion::parse(1).unwrap().as_u64(), 1);
    assert_eq!(ProfileVersion::from_u64(7).unwrap().as_u64(), 7);
    assert_eq!(
        participant_presentation::PresentationVersion::parse(0),
        Err(VersionError::Zero)
    );
    assert_eq!(
        participant_presentation::PresentationVersion::parse(2)
            .unwrap()
            .as_u64(),
        2
    );
}

// ---------- 摘要（canonical JSON 表示） ----------

#[test]
fn digest_is_deterministic_and_collision_sensitive() {
    let d1 =
        participant_presentation::profile_put_digest(BaseVersion::ZERO, Some("hi"), Some("world"));
    let d2 =
        participant_presentation::profile_put_digest(BaseVersion::ZERO, Some("hi"), Some("world"));
    assert_eq!(d1, d2, "same canonical input must produce the same digest");

    // 任一字段变化都改变摘要。
    assert_ne!(
        d1,
        participant_presentation::profile_put_digest(BaseVersion::ZERO, Some("hi"), Some("world!"),)
    );
    assert_ne!(
        d1,
        participant_presentation::profile_put_digest(
            BaseVersion::parse(1).unwrap(),
            Some("hi"),
            Some("world"),
        ),
        "base_version participates in the digest (concurrency is not a replay)"
    );
    // Profile 与 Room Presentation 的 canonical 形状不同，互不碰撞。
    assert_ne!(
        participant_presentation::profile_put_digest(BaseVersion::ZERO, None, None),
        participant_presentation::room_presentation_put_digest(BaseVersion::ZERO, None, None, None),
    );
}

#[test]
fn empty_and_null_normalize_to_the_same_effective_request() {
    // 空串与 NULL 是同一有效请求（契约 §5.3），摘要必须一致。
    assert_eq!(
        participant_presentation::profile_put_digest(BaseVersion::ZERO, None, Some("x")),
        participant_presentation::profile_put_digest(BaseVersion::ZERO, Some(""), Some("x"))
    );
    assert_eq!(
        participant_presentation::room_presentation_put_digest(
            BaseVersion::ZERO,
            Some(""),
            Some(""),
            None
        ),
        participant_presentation::room_presentation_put_digest(BaseVersion::ZERO, None, None, None)
    );
}

/// Golden：固定输入必须产生固定摘要（canonical JSON 编码或其顺序/字段变更会
/// 破坏此断言）。值由 `serde_json::to_vec` 的固定字段序 canonical JSON 计算。
#[test]
fn canonical_json_digests_are_stable_golden() {
    // profile_put_digest(BaseVersion::ZERO, None, None)
    //   => {"base_version":0,"short_description":null,"public_description":null}
    assert_eq!(
        participant_presentation::profile_put_digest(BaseVersion::ZERO, None, None).as_str(),
        "591d9a9fab422a3c3159ca9446ad29117fcc054c71bba2413dee5f4aecec3493"
    );
    // profile_put_digest(BaseVersion::ZERO, Some("hi"), Some("world"))
    //   => {"base_version":0,"short_description":"hi","public_description":"world"}
    assert_eq!(
        participant_presentation::profile_put_digest(BaseVersion::ZERO, Some("hi"), Some("world"),)
            .as_str(),
        "c48ba7f9ee40cb1982d9f45ae7c8a90ca83cb67c809f8f37a73e004376020a8d"
    );
    // room_presentation_put_digest(BaseVersion::ZERO, None, None, None)
    //   => {"base_version":0,"room_display_name":null,"room_role_name":null,"room_role_description":null}
    assert_eq!(
        participant_presentation::room_presentation_put_digest(
            BaseVersion::ZERO,
            None,
            None,
            None,
        )
        .as_str(),
        "b1b5071bde355b68c608a33e820ee188310bf483327c58b45dd0f00417a0e59d"
    );
}

#[test]
fn digest_hex_format_is_stable_and_parseable() {
    let d = participant_presentation::profile_put_digest(BaseVersion::ZERO, None, None);
    assert_eq!(d.as_str().len(), 64);
    assert!(
        d.as_str()
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "digest must be lowercase hex"
    );
    assert_eq!(
        d.as_str(),
        RequestDigest::parse_hex(d.as_str())
            .expect("self parse")
            .as_str()
    );

    // parse_hex 拒绝非法形状。
    assert!(RequestDigest::parse_hex(&"a".repeat(63)).is_none());
    assert!(
        RequestDigest::parse_hex(&"A".repeat(64)).is_none(),
        "uppercase rejected"
    );
    assert!(
        RequestDigest::parse_hex(&"z".repeat(64)).is_none(),
        "non-hex rejected"
    );
}

// ---------- preview（UTF-8 安全投影辅助） ----------

#[test]
fn preview_never_splits_a_multibyte_scalar() {
    // 3 字节 CJK：预算 5 字节只允许 1 个完整字符（3 字节），不会切成 5 字节
    // 的半个字符。
    let s = "界".repeat(3); // 9 字节
    let p = participant_presentation::preview(&s, 5);
    assert_eq!(p, "界".to_string());
    assert!(s.is_char_boundary(p.len()));

    // 4 字节 astral：预算 6 字节 -> 1 个 4 字节字符。
    let emoji = "𠜎".repeat(2); // 8 字节
    let p = participant_presentation::preview(&emoji, 6);
    assert_eq!(p, "𠜎".to_string());
    assert!(emoji.is_char_boundary(p.len()));
}

#[test]
fn preview_returns_the_whole_text_within_budget() {
    let s = "hello world";
    assert_eq!(participant_presentation::preview(s, 100), s);
    assert_eq!(participant_presentation::preview(s, s.len()), s);
}

// ---------- 幂等裁决（同 key 同摘要恒重放，不因 base_version 过期变 stale） ----------

fn existing(digest: &str, result_version: u64, result_is_create: bool) -> ExistingIdempotency {
    ExistingIdempotency {
        request_digest: RequestDigest::parse_hex(digest).expect("valid hex digest"),
        result_version,
        result_is_create,
    }
}

#[test]
fn same_key_same_digest_is_always_replayed_even_if_latest_advanced() {
    let digest = "a".repeat(64);
    let e = existing(&digest, 1, true);
    // 关键规则：即使最新版本已由**另一个写入**推进到 4，同 key 同摘要仍重放
    // 首次成功结果（版本 1），绝不追加版本、绝不因 base_version 变 stale。
    let verdict = participant_presentation::decide_idempotency(
        Some(&e),
        &RequestDigest::parse_hex(&digest).unwrap(),
        /* base_version= */ 0,
        /* latest_version= */ 4,
    );
    assert_eq!(
        verdict,
        IdempotencyDisposition::Replay {
            result_version: 1,
            result_is_create: true,
        }
    );
}

#[test]
fn same_key_same_digest_replay_when_base_version_still_matches() {
    let digest = "b".repeat(64);
    let e = existing(&digest, 3, false);
    assert_eq!(
        participant_presentation::decide_idempotency(
            Some(&e),
            &RequestDigest::parse_hex(&digest).unwrap(),
            3,
            3,
        ),
        IdempotencyDisposition::Replay {
            result_version: 3,
            result_is_create: false,
        }
    );
}

#[test]
fn same_key_different_digest_is_an_idempotency_conflict() {
    let e = existing(&"c".repeat(64), 2, false);
    let verdict = participant_presentation::decide_idempotency(
        Some(&e),
        &RequestDigest::parse_hex(&"d".repeat(64)).unwrap(),
        2,
        2,
    );
    assert_eq!(verdict, IdempotencyDisposition::IdempotencyConflict);
    // 不同摘要即冲突，与 base_version 是否匹配无关。
    let verdict = participant_presentation::decide_idempotency(
        Some(&e),
        &RequestDigest::parse_hex(&"d".repeat(64)).unwrap(),
        5,
        2,
    );
    assert_eq!(verdict, IdempotencyDisposition::IdempotencyConflict);
}

#[test]
fn new_key_matching_base_version_allows_write() {
    assert_eq!(
        participant_presentation::decide_idempotency(
            None,
            &RequestDigest::parse_hex(&"e".repeat(64)).unwrap(),
            0,
            0
        ),
        IdempotencyDisposition::Write
    );
    assert_eq!(
        participant_presentation::decide_idempotency(
            None,
            &RequestDigest::parse_hex(&"f".repeat(64)).unwrap(),
            2,
            2
        ),
        IdempotencyDisposition::Write
    );
}

#[test]
fn new_key_with_stale_base_version_is_stale() {
    assert_eq!(
        participant_presentation::decide_idempotency(
            None,
            &RequestDigest::parse_hex(&"e".repeat(64)).unwrap(),
            0,
            1
        ),
        IdempotencyDisposition::Stale { latest_version: 1 }
    );
}
