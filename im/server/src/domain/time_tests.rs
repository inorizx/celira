//! UTC 时间戳格式化测试：已知 epoch 向量。

use crate::domain::time::UtcMillis;

#[test]
fn epoch_renders_as_1970() {
    assert_eq!(
        UtcMillis::from_millis(0).to_rfc3339(),
        "1970-01-01T00:00:00.000Z"
    );
}

#[test]
fn millisecond_fraction_renders() {
    assert_eq!(
        UtcMillis::from_millis(1234).to_rfc3339(),
        "1970-01-01T00:00:01.234Z"
    );
}

#[test]
fn year_2000_known_vector() {
    assert_eq!(
        UtcMillis::from_millis(946_684_800_000).to_rfc3339(),
        "2000-01-01T00:00:00.000Z"
    );
}

#[test]
fn leap_day_known_vector() {
    assert_eq!(
        UtcMillis::from_millis(1_709_210_096_789).to_rfc3339(),
        "2024-02-29T12:34:56.789Z"
    );
}

#[test]
fn year_2026_known_vector() {
    assert_eq!(
        UtcMillis::from_millis(1_786_406_400_000).to_rfc3339(),
        "2026-08-11T00:00:00.000Z"
    );
}
