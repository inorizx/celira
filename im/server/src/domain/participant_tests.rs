//! 显示名规则测试（CORE_V1_IDENTITY_AUTH.md 4.1）。

use crate::domain::participant::{
    DisplayName, DisplayNameError, ParticipantKind, ParticipantStatus,
};

#[test]
fn ordinary_name_is_accepted() {
    let name = DisplayName::parse("Alice").expect("ordinary name must parse");
    assert_eq!(name.as_str(), "Alice");
}

#[test]
fn empty_and_whitespace_only_names_are_rejected() {
    for raw in ["", "   ", "\t\n ", "\u{00A0}", "\r\n\t"] {
        assert_eq!(
            DisplayName::parse(raw),
            Err(DisplayNameError::NoNonWhitespace),
            "expected rejection for {raw:?}"
        );
    }
}

#[test]
fn nul_character_is_rejected() {
    assert_eq!(
        DisplayName::parse("al\u{0}ice"),
        Err(DisplayNameError::ContainsNul)
    );
}

#[test]
fn eighty_scalars_ok_and_eighty_one_rejected() {
    let ok = "a".repeat(80);
    assert!(
        DisplayName::parse(&ok).is_ok(),
        "80 ASCII scalars must pass"
    );

    let too_long = "a".repeat(81);
    assert_eq!(
        DisplayName::parse(&too_long),
        Err(DisplayNameError::TooLong(81))
    );
}

#[test]
fn length_counts_unicode_scalar_values_not_bytes() {
    let wide = "界".repeat(80);
    assert!(
        DisplayName::parse(&wide).is_ok(),
        "80 scalars of 3-byte chars must pass"
    );
    assert_eq!(
        DisplayName::parse(&"界".repeat(81)),
        Err(DisplayNameError::TooLong(81))
    );

    let astral = "𠜎".repeat(80);
    assert!(
        DisplayName::parse(&astral).is_ok(),
        "80 scalars of 4-byte chars must pass"
    );
}

#[test]
fn stored_verbatim_without_normalization() {
    let composed = "Alice\u{0301}";
    let name = DisplayName::parse(composed).expect("combining accent must pass");
    assert_eq!(name.as_str(), composed, "no normalization may be applied");

    let fullwidth = "Ａｌｉｃｅ";
    let name = DisplayName::parse(fullwidth).unwrap();
    assert_eq!(name.as_str(), fullwidth);
}

#[test]
fn kind_and_status_roundtrip() {
    assert_eq!(
        ParticipantKind::parse(ParticipantKind::Human.as_str()),
        Some(ParticipantKind::Human)
    );
    assert_eq!(
        ParticipantKind::parse(ParticipantKind::Agent.as_str()),
        Some(ParticipantKind::Agent)
    );
    assert_eq!(ParticipantKind::parse("robot"), None);

    assert_eq!(
        ParticipantStatus::parse(ParticipantStatus::Active.as_str()),
        Some(ParticipantStatus::Active)
    );
    assert_eq!(
        ParticipantStatus::parse(ParticipantStatus::Revoked.as_str()),
        Some(ParticipantStatus::Revoked)
    );
    assert_eq!(ParticipantStatus::parse("suspended"), None);
}
