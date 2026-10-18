//! UTC 时间戳（Unix 毫秒）与 RFC3339 格式化。
//!
//! 领域层自己持有时间语义，不引入日历库：毫秒整数落库，格式化只在需要
//! 对外展示时进行（civil-from-days 算法，Howard Hinnant）。
//!
//! `UtcMillis::now()` 是**进程内单调**的：系统墙钟回拨（NTP 修正等）时
//! 本进程绝不返回比之前更早的时间戳，而是冻结在已返回过的最大值。这是
//! migration 009 时间戳守卫（delivery target 要求
//! `joined_at <= created_at <= left_at`）对一切合法 API 写入恒成立的前提：
//! 先记录的 joined_at 恒不大于后记录的 created_at/left_at/occurred_at，
//! 合法写入不会因时钟回拨被守卫反向 abort；守卫本身不引入容差，直接 SQL
//! 伪造/回拨时间戳仍被拒绝（授权窗口不被削弱）。单调性按进程生效（单
//! Core 进程对应单 SQLite 文件）；跨进程/跨重启共享数据库不在当前部署
//! 拓扑内。

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 进程内已返回过的最大时间戳。`now()` 用原子 fetch_max 推进它：任何
/// 一次调用返回的值都 >= 此前所有调用返回过的值。
static LAST_NOW_MILLIS: AtomicI64 = AtomicI64::new(0);

/// UTC 毫秒时间戳。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcMillis(i64);

impl UtcMillis {
    pub fn now() -> Self {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        // fetch_max 返回调用前的旧值；与墙钟取 max 即本次实际记录值，
        // 恒 >= 任何先前调用返回过的值（并发下也成立）。
        let observed = LAST_NOW_MILLIS.fetch_max(wall, Ordering::Relaxed);
        Self(observed.max(wall))
    }

    pub fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub fn millis(&self) -> i64 {
        self.0
    }

    /// RFC3339 UTC，毫秒精度，如 `2026-08-11T04:57:00.123Z`。
    pub fn to_rfc3339(self) -> String {
        let secs = self.0.div_euclid(1000);
        let ms = self.0.rem_euclid(1000);
        let days = secs.div_euclid(86_400);
        let seconds_of_day = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let (hour, minute, second) = (
            seconds_of_day / 3600,
            (seconds_of_day % 3600) / 60,
            seconds_of_day % 60,
        );
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
    }
}

/// 天数 → (年, 月, 日)。适用于 0000-03-01 之后的全部日期。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个时间测试都会直接操纵进程级 `LAST_NOW_MILLIS`（冻结测试临时把
    /// 计数器推进 +60s 再恢复），并行运行时互为竞态。串行化锁保证它们
    /// 互斥执行：冻结窗口不会打断单调性测试的 100k 轮询，单调性测试的
    /// 轮询也不会观察到期中的冻结/恢复。
    static TIME_TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn now_never_goes_backward_within_the_process() {
        let _guard = TIME_TEST_SERIAL.lock().unwrap();
        let mut prev = UtcMillis::now().millis();
        for _ in 0..100_000 {
            let cur = UtcMillis::now().millis();
            assert!(cur >= prev, "now() must never go backward");
            prev = cur;
        }
    }

    #[test]
    fn now_freezes_at_the_max_when_the_wall_clock_steps_back() {
        let _guard = TIME_TEST_SERIAL.lock().unwrap();
        // 模拟系统时钟回拨：先把进程内时钟记录推进到未来，再调用 now()。
        // 墙钟已落后于记录值，但返回值必须冻结在最大值（绝不回拨）——
        // 这正是 migration 009 target 守卫（joined_at <= created_at）对
        // 合法写入恒成立的前提。测试结束恢复基线，避免影响其他用例。
        let baseline = UtcMillis::now().millis();
        let future = baseline + 60_000;
        LAST_NOW_MILLIS.store(future, Ordering::Relaxed);
        let frozen = UtcMillis::now().millis();
        assert!(frozen >= future, "clock step-back must freeze at the max");
        let again = UtcMillis::now().millis();
        assert!(again >= frozen, "subsequent calls stay frozen/monotonic");

        LAST_NOW_MILLIS.store(baseline, Ordering::Relaxed);
        let resumed = UtcMillis::now().millis();
        assert!(
            resumed >= baseline,
            "clock resumes from the restored baseline"
        );
    }
}
