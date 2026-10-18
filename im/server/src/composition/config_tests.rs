//! 配置解析测试：默认值、非法值可观察失败。

use crate::application::home::{
    GRANT_TTL_MINUTES_MAX, GRANT_TTL_MINUTES_MIN, parse_grant_ttl_minutes,
};
use crate::composition::config::{
    Config, ConfigError, DEFAULT_GRANT_TTL_MINUTES, DEFAULT_LISTEN_ADDR,
};

#[test]
fn defaults_apply_when_env_absent() {
    let config = Config::parse(None, None, None, None, None).expect("defaults must parse");
    assert_eq!(config.listen_addr.to_string(), DEFAULT_LISTEN_ADDR);
    assert_eq!(config.db_path.to_string_lossy(), "data/core.db");
    assert_eq!(config.media_root.to_string_lossy(), "data/media");
    assert_eq!(config.grant_ttl_minutes, DEFAULT_GRANT_TTL_MINUTES);
    assert_eq!(config.home_display_name, None);
}

#[test]
fn explicit_values_are_used() {
    let config = Config::parse(
        Some("0.0.0.0:9000"),
        Some("/tmp/other.db"),
        Some("/var/celira/media"),
        Some("30"),
        Some("My Home"),
    )
    .unwrap();
    assert_eq!(config.listen_addr.to_string(), "0.0.0.0:9000");
    assert_eq!(config.db_path.to_string_lossy(), "/tmp/other.db");
    assert_eq!(config.media_root.to_string_lossy(), "/var/celira/media");
    assert_eq!(config.grant_ttl_minutes, 30);
    assert_eq!(config.home_display_name.as_deref(), Some("My Home"));
}

#[test]
fn invalid_listen_addr_fails_observably() {
    let err = Config::parse(Some("not-an-address"), None, None, None, None).unwrap_err();
    assert_eq!(
        err,
        ConfigError::InvalidListenAddr("not-an-address".to_owned())
    );
}

#[test]
fn empty_db_path_fails_observably() {
    let err = Config::parse(None, Some("   "), None, None, None).unwrap_err();
    assert_eq!(err, ConfigError::EmptyDbPath);
}

#[test]
fn empty_media_root_fails_observably() {
    let err = Config::parse(None, None, Some("   "), None, None).unwrap_err();
    assert_eq!(err, ConfigError::EmptyMediaRoot);
}

#[test]
fn grant_ttl_bounds_are_strictly_enforced() {
    assert_eq!(
        parse_grant_ttl_minutes(&GRANT_TTL_MINUTES_MIN.to_string()),
        Some(1)
    );
    assert_eq!(
        parse_grant_ttl_minutes(&GRANT_TTL_MINUTES_MAX.to_string()),
        Some(60)
    );
    assert_eq!(parse_grant_ttl_minutes("0"), None);
    assert_eq!(parse_grant_ttl_minutes("61"), None);
    assert_eq!(parse_grant_ttl_minutes("-1"), None);
    assert_eq!(parse_grant_ttl_minutes("abc"), None);
    assert_eq!(parse_grant_ttl_minutes(""), None);
    assert_eq!(parse_grant_ttl_minutes("1.5"), None);

    for raw in ["0", "61", "-3", "x", "", "1.5", " 5"] {
        let err = Config::parse(None, None, None, Some(raw), None).unwrap_err();
        assert_eq!(
            err,
            ConfigError::InvalidGrantTtl(raw.to_owned()),
            "raw={raw:?}"
        );
    }
    let ok = Config::parse(None, None, None, Some("1"), None).unwrap();
    assert_eq!(ok.grant_ttl_minutes, 1);
    let ok = Config::parse(None, None, None, Some("60"), None).unwrap();
    assert_eq!(ok.grant_ttl_minutes, 60);
}

#[test]
fn invalid_home_display_name_fails_observably() {
    let err = Config::parse(None, None, None, None, Some("\u{0}")).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidHomeDisplayName(_)));
    let err = Config::parse(None, None, None, None, Some("   ")).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidHomeDisplayName(_)));
    let long = "x".repeat(81);
    let err = Config::parse(None, None, None, None, Some(&long)).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidHomeDisplayName(_)));
}
