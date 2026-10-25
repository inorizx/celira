//! Group Room Profile 领域规则测试（CORE_V1_ROOM_PROFILE.md §3）。
//!
//! 只测纯规则：正文字节上限（按 UTF-8 字节）、NUL、`trim` 后非空、多行 /
//! 回车 / Tab / Emoji 原样保留、版本与请求摘要的确定性。

use crate::domain::participant_presentation::BaseVersion;
use crate::domain::room_profile::{
    RoomDescription, RoomDescriptionError, RoomProfileVersion, room_profile_limits,
    room_profile_put_digest,
};

#[test]
fn description_accepts_multiline_emoji_and_exactly_the_byte_limit() {
    let text = "第一行\n第二行\r\n\t缩进 🙂 — café";
    let parsed = RoomDescription::parse(text).expect("multiline unicode description");
    assert_eq!(parsed.as_str(), text, "原文保存，不 trim、不归一化");

    let exact = "é".repeat(room_profile_limits::ROOM_DESCRIPTION_MAX_BYTES / 2);
    assert_eq!(exact.len(), 2_000);
    assert!(
        RoomDescription::parse(&exact).is_ok(),
        "恰好 2000 字节是合法边界"
    );

    let one_more = format!("{exact}a");
    assert_eq!(one_more.len(), 2_001);
    assert!(matches!(
        RoomDescription::parse(&one_more),
        Err(RoomDescriptionError::TooLong {
            bytes: 2_001,
            max_bytes: 2_000
        })
    ));
}

#[test]
fn description_rejects_blank_nul_and_over_limit() {
    assert_eq!(
        RoomDescription::parse(""),
        Err(RoomDescriptionError::Empty),
        "空串不是简介"
    );
    for blank in [" ", "   ", "\n", "\r\n", "\t\t", " \n\t\r "] {
        assert_eq!(
            RoomDescription::parse(blank),
            Err(RoomDescriptionError::Empty),
            "{blank:?} 只含空白"
        );
    }
    assert_eq!(
        RoomDescription::parse("abc\u{0}def"),
        Err(RoomDescriptionError::ContainsNul)
    );
    assert!(matches!(
        RoomDescription::parse(&"a".repeat(2_001)),
        Err(RoomDescriptionError::TooLong { .. })
    ));
    // 首尾空白本身合法：只影响「是否为空」的裁决，不改写存储值。
    let padded = RoomDescription::parse("  有内容  ").expect("padded text is valid");
    assert_eq!(padded.as_str(), "  有内容  ");
}

#[test]
fn versions_start_at_one_and_never_go_back() {
    assert!(RoomProfileVersion::parse(0).is_err());
    assert!(RoomProfileVersion::parse(-1).is_err());
    assert!(RoomProfileVersion::from_u64(0).is_err());
    assert_eq!(RoomProfileVersion::parse(1).unwrap().as_u64(), 1);
    assert_eq!(RoomProfileVersion::from_u64(7).unwrap().as_u64(), 7);
}

#[test]
fn request_digest_is_deterministic_and_covers_version_and_text() {
    let base = BaseVersion::parse(0).unwrap();
    let a = room_profile_put_digest(base, "同一段简介");
    let b = room_profile_put_digest(base, "同一段简介");
    assert_eq!(a, b, "同请求必须逐字节同摘要");
    assert_eq!(a.as_str().len(), 64);
    assert!(
        a.as_str()
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "小写 hex：{}",
        a.as_str()
    );
    assert_ne!(
        a,
        room_profile_put_digest(BaseVersion::parse(1).unwrap(), "同一段简介"),
        "base_version 参与摘要"
    );
    assert_ne!(
        a,
        room_profile_put_digest(base, "同一段简介 "),
        "正文逐字节参与摘要（不 trim）"
    );
}
