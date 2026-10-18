//! **手机的钟**：时间来源与毫秒刻度。
//!
//! ## 它为什么自己一个家
//!
//! 从前 `Clock` / `ClockHandle` 与那几个格式化点住在 `status_bar.rs` 里——那是它们**第一个**被
//! 需要的地方。后果是**依赖方向反了**：铃（`src/wake.rs`，最底层：没有它什么都不开始）为了拿钟
//! 要穿过**显示区**；时钟 App（`timer_app`）与通知栏（`notification_bar`）同样如此；连模型适配层
//! 那条路也要绕。而按手机模型，**下层绝不依赖上层**。
//!
//! 时间**不是显示区的事**：状态行的 `time` / `since`、持有物的 `when` 与年龄、钟源的等待，
//! 都要它。因此它是**手机的零件**，与显示区平级，住在下面。
//!
//! ## 一个刻度，一个换算点
//!
//! 主体看得见的时间有两种形态，各有**唯一**的换算点：
//!
//! - **可读时刻** ⇒ [`format_local`]（RFC 3339 **带本机偏移**，秒精度）——只给状态行的 `time`
//!   那一栏，给人看"现在几点"；本机偏移读不出来时退回 [`format_utc`] 的形状（`Z`，照旧无歧义）。
//! - **unix 毫秒** ⇒ [`unix_millis`]（数值）与 [`anchor_value`]（同值的十进制文本）——
//!   状态行的 `since`、持有物的 `when` 与年龄都走它。**渲染成主体动词吃的那个形状**，
//!   因此主体可以拿那一行的原文直接递给自己的动词（`im_rooms(since=…)` / `timer_set(when=…)`），
//!   不必在脑子里做一次换算。
//!
//! 两者都从**同一个注入的钟**出来：本件不读环境变量，也没有全局时钟——时间来源只从构造参数
//! 进来，因此"现在几点"在任何一次调用里都只有一个答案、一个来源。
//!
//! ## 它不是什么
//!
//! 不落盘、不进事实、不参与任何判断。它是一个**读数**：钟坏了就如实说读不出来
//! （[`UNKNOWN_TIME`] / [`UNKNOWN_SINCE`]），绝不编一个时刻。

use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// `time` 读不出来时的读数：宿主时钟早于 1970-01-01T00:00:00Z 时没有 RFC 3339 表示。
///
/// 这里给的是一个**读数**而不是一个编出来的时间：宁可主体看见"读不出来"，也不要主体看见一个假的
/// 时刻。它与恢复判定里的 `unknown` 同族——都表示"这一项没有被确知"，不表示"等于零"。
pub const UNKNOWN_TIME: &str = "unknown";

/// `since` 读不出来时的读数：宿主不知道「新」该从哪个时刻算起。
///
/// 最常见的来路是**新进程的第一个 Run**——上一次去看是什么时候这件事不在这个进程里，于是它
/// 如实说不知道，而不是拿一个猜的时刻冒充「上一次」。它与 [`UNKNOWN_TIME`] 值相同但**来源不同**，
/// 因此是两个常量：「宿主钟给不出表示」与「上一次去看的时刻不在本进程里」是两件事。
pub const UNKNOWN_SINCE: &str = "unknown";

/// 时间来源：**唯一**的时间取值入口。
///
/// 把它抽出来只为一件事：让"同一时刻同一输入 ⇒ 逐字节相同输出"这条判据**可判错**——生产装配
/// 注入真实钟，离线夹具与用例注入固定钟（或自己的递增钟），而格式化只有一份实现。
///
/// 本件不读环境变量，也没有全局时钟：时间来源**只从构造参数进来**，因此"现在几点"在任何一次
/// 调用里都只有一个答案、一个来源。
///
/// 任何 `Fn() -> SystemTime` 都自动是一个钟（见下面的实现），所以固定钟与递增钟都不需要为它
/// 新写一个类型。
pub trait Clock: Send + Sync {
    /// 现在是什么时刻。
    fn now(&self) -> SystemTime;
}

impl<F: Fn() -> SystemTime + Send + Sync> Clock for F {
    fn now(&self) -> SystemTime {
        self()
    }
}

/// 一个**可共享的时间来源句柄**：装配时注入，之后只读。
///
/// 它只是一个句柄（内部是共享的只读引用），因此可以被多个装配声明各自持有一份——"现在几点"
/// 是**这一次进程**的事实，不是被继承的历史，恢复时也不从盘上读回一个旧时钟。
#[derive(Clone)]
pub struct ClockHandle(Arc<dyn Clock>);

impl ClockHandle {
    /// 生产钟：真实系统时间。
    pub fn system() -> Self {
        Self::of(SystemTime::now)
    }

    /// 固定钟：每一轮读到的都是同一个时刻（离线夹具与用例用它换确定性）。
    pub fn fixed(at: SystemTime) -> Self {
        Self::of(move || at)
    }

    /// 固定在自 1970-01-01T00:00:00Z 起的第 `seconds` 秒。
    pub fn fixed_at_unix_seconds(seconds: u64) -> Self {
        Self::fixed(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    }

    /// 用一个自己实现的钟装配句柄（用例可以借此给出递增钟等形状）。
    pub fn of(clock: impl Clock + 'static) -> Self {
        Self(Arc::new(clock))
    }

    /// 现在是什么时刻。
    pub fn now(&self) -> SystemTime {
        self.0.now()
    }
}

/// **可读时刻**：RFC 3339 UTC（秒精度：`2026-09-16T06:58:40Z`）。
///
/// 这是**给人看"现在几点"**的那一栏（状态行的 `time`）的格式化点：没有第二种写法、
/// 也没有可配置格式。
///
/// 早于 1970-01-01T00:00:00Z 的时刻**没有** RFC 3339 表示（那会是负的 Unix 秒），这里返回
/// [`UNKNOWN_TIME`]——宁可读作"读不出来"，也不编一个时刻。
///
/// **锚（`since`）不走这里**：它的形状由 [`anchor_value`] 决定（那才是主体要递回去的东西）。
pub fn format_utc(at: SystemTime) -> String {
    let Ok(since_epoch) = at.duration_since(SystemTime::UNIX_EPOCH) else {
        return UNKNOWN_TIME.to_string();
    };
    let seconds = since_epoch.as_secs();
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60
    )
}

/// **本机此刻的 UTC 偏移**（分钟，东为正）——**机器事实，装配时现读一次**。
///
/// 它是 [`format_local`] 的输入。**为什么要有它**：`time` 那一格原本只有 UTC，而这一格要回答的
/// 是"现在是什么时段"——UTC 答不了（+08:00 的机器上，半夜那一刻会被读成下午三点）。
///
/// ⚠ **读一次，不是每次格式化都读**：与 `machine` 行同一条纪律——**每次开场从机器现读**。
/// 一段清醒期内机器的时区不会变；真变了，下一段读到新的。
///
/// ⚠ **读不出来是 [`LocalOffset::Unreadable`]，不是 `0`**：`+00:00` 是一个**断言**
/// （"就在 UTC"），与"读不到"是两件事——与 `platform = unknown` 同款。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalOffset {
    /// 本机此刻在 UTC 之东/西多少分钟（`+08:00` ⇒ `480`）。
    Minutes(i32),
    /// 读不出来：没有臂的平台，或系统调用失败。
    Unreadable,
}

impl LocalOffset {
    /// **从机器现读一次**（此刻）：装配时用（与 `machine` 行"每次开场从机器现读"同款）。
    pub fn of_machine() -> Self {
        Self::at_machine(SystemTime::now())
    }

    /// **那一刻**的本机偏移（平台缝：契约唯一、实现多份、无臂平台如实读不出来）。
    ///
    /// ⚠ **为什么必须按时刻问，而不是拿"此刻"的偏移去套所有时刻**：偏移随夏令时变。
    /// 给**过去**的时刻（世界发来的消息时间戳）套今天的偏移，跨夏令时边界会**差一小时**。
    /// 这台机器今天没有夏令时，但代码不许假设。
    pub fn at_machine(at: SystemTime) -> Self {
        arm::offset_at(at)
    }
}

/// **可读时刻（本机）**：RFC 3339 ＋ 本机偏移（`2026-10-09T23:58:23+08:00`）。
///
/// 与 [`format_utc`] 是**同一个瞬间的两种写法**（解析后相等），区别只有一个：写不写偏移。
/// 主体看的是这一种。
///
/// **偏移读不出来时退回 [`format_utc`] 的形状**（`Z`）：宁可给一个"无歧义但不是本地"的时刻，
/// 也不写 `+00:00` 去冒充"就在 UTC"。
///
/// 早于 1970-01-01T00:00:00Z 的时刻照旧读作 [`UNKNOWN_TIME`]（与 [`format_utc`] 同一条纪律）。
pub fn format_local(at: SystemTime, offset: LocalOffset) -> String {
    let Ok(since_epoch) = at.duration_since(SystemTime::UNIX_EPOCH) else {
        return UNKNOWN_TIME.to_string();
    };
    let LocalOffset::Minutes(minutes) = offset else {
        return format_utc(at);
    };
    // 本地时刻 ＝ UTC ＋ 偏移。秒级整数运算，走**同一个** `civil_from_days`——
    // 不引入第二种日期算法。
    let Ok(seconds) = i64::try_from(since_epoch.as_secs()) else {
        // 超出 i64 的秒现实里不存在；但"读不出来"必须与"算得出来"分开说。
        return UNKNOWN_TIME.to_string();
    };
    let shifted = seconds + i64::from(minutes) * 60;
    // ⚠ **欧几里得除法**：偏移为负时（西半球）`shifted` 为负，用 `/` 与 `%` 会把天与余数拆错
    // （`-1 / 86400 == 0`）⇒ 日期差一天。
    let days = shifted.div_euclid(86_400);
    let rest = shifted.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let sign = if minutes < 0 { '-' } else { '+' };
    let magnitude = minutes.abs();
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60,
        magnitude / 60,
        magnitude % 60
    )
}

/// **世界给的时刻，摆给主体时换成本机刻度**（与人那边 `toLocaleString()` 是同一步）。
///
/// 世界的**事实**不变（wire 上仍是 UTC，Core v1 冻结）；换的只是**呈现**——给主体的那一份
/// **带偏移**（比给人看的那一份更精确），而那次换算**不该由主体自己做**：给人看的那一半
/// 客户端已经做了，这一处就是我们的那一半。
///
/// **读不出来就原样返回**——宁可摆一个没换过、但完整的原文，也不吞掉一个时刻
/// （与"读不出来就说读不出来"同一条纪律：只是这里原文本身仍然有用）。
pub fn localize_rfc3339(text: &str) -> String {
    match parse_rfc3339(text) {
        Some(at) => format_local(at, LocalOffset::at_machine(at)),
        None => text.to_string(),
    }
}

/// 把 RFC 3339 时刻读成瞬间。**读不出来就 `None`**（不猜）。
///
/// 世界实际发过的两种形状都在里面：
/// - `2026-10-09T14:39:49.336Z`（真机）；
/// - `2026-09-16T11:00:00+00:00`（夹具与契约里的零偏移写法）。
///
/// 小数秒**读到就丢**（渲染本来就是秒精度，且丢它不会跨秒）。早于纪元的时刻读作读不出来。
pub fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let digits = |from: usize, len: usize| -> Option<i64> {
        let slice = text.get(from..from + len)?;
        if !slice.bytes().all(|each| each.is_ascii_digit()) {
            return None;
        }
        slice.parse::<i64>().ok()
    };
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = digits(0, 4)?;
    let month = digits(5, 2)?;
    let day = digits(8, 2)?;
    let hour = digits(11, 2)?;
    let minute = digits(14, 2)?;
    let second = digits(17, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let taken = fraction
            .bytes()
            .take_while(|each| each.is_ascii_digit())
            .count();
        if taken == 0 {
            return None;
        }
        rest = &fraction[taken..];
    }
    let offset_minutes = if rest == "Z" {
        0
    } else {
        let sign = match rest.as_bytes().first()? {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        if rest.len() != 6 || rest.as_bytes()[3] != b':' {
            return None;
        }
        let hh: i64 = rest.get(1..3)?.parse().ok()?;
        let mm: i64 = rest.get(4..6)?.parse().ok()?;
        if hh > 23 || mm > 59 {
            return None;
        }
        sign * (hh * 60 + mm)
    };
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset_minutes * 60;
    u64::try_from(seconds)
        .ok()
        .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

/// **平台臂**：怎么读"那一刻的本机偏移"（与 `platform.rs` 同款形状——契约唯一、实现多份）。
#[cfg(windows)]
mod arm {
    use super::LocalOffset;

    /// `SYSTEMTIME`（`windows.h`）：八个 `WORD`。字段由系统调用填写，本件一个都不读。
    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    /// `TIME_ZONE_INFORMATION`（`windows.h`）。本件**一个字段都不读**——只把它整个交给
    /// `SystemTimeToTzSpecificLocalTime`（它按规则算夏令时，我们不写规则表）。
    #[repr(C)]
    #[allow(dead_code)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime,
        daylight_bias: i32,
    }

    // `GetTimeZoneInformation` 的返回值里只有 `u32::MAX`（`TIME_ZONE_ID_INVALID`）对我们有意义
    // ——那是"读不出来"。**夏令时不看返回值**（`SystemTimeToTzSpecificLocalTime` 按规则算），
    // 所以这里不列那三个取值常量。

    // 安全：签名取自 `windows.h`：
    // `DWORD GetTimeZoneInformation(LPTIME_ZONE_INFORMATION)`；
    // `BOOL SystemTimeToTzSpecificLocalTime(const TIME_ZONE_INFORMATION *, const SYSTEMTIME *, LPSYSTEMTIME)`。
    unsafe extern "system" {
        fn GetTimeZoneInformation(info: *mut TimeZoneInformation) -> u32;
        fn SystemTimeToTzSpecificLocalTime(
            info: *const TimeZoneInformation,
            utc: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
    }

    /// **那一刻**的偏移：`SystemTimeToTzSpecificLocalTime` 按**那个时刻的日期**套当前时区规则
    /// （夏令时由它判，我们不写规则表）⇒ 偏移 ＝ 本地 − UTC。
    pub fn offset_at(at: std::time::SystemTime) -> LocalOffset {
        let Some(utc) = system_time_of(at) else {
            return LocalOffset::Unreadable;
        };
        let mut info = std::mem::MaybeUninit::<TimeZoneInformation>::uninit();
        // 安全：写的是我们自己声明的结构体；这一次调用只读系统状态，不改任何东西。
        let state = unsafe { GetTimeZoneInformation(info.as_mut_ptr()) };
        if state == u32::MAX {
            // 失败时结构体内容未定义 ⇒ 只有这一种读作"读不出来"。
            return LocalOffset::Unreadable;
        }
        // 安全：非失败即已填好（返回 0/1/2 三种都会填结构体）。
        let info = unsafe { info.assume_init() };
        let mut local = std::mem::MaybeUninit::<SystemTime>::uninit();
        // 安全：三个指针都是我们自己的；只读系统状态。
        let ok = unsafe { SystemTimeToTzSpecificLocalTime(&info, &utc, local.as_mut_ptr()) };
        if ok == 0 {
            return LocalOffset::Unreadable;
        }
        // 安全：非零即已填好。
        let local = unsafe { local.assume_init() };
        match i32::try_from(minutes_of(&local) - minutes_of(&utc)) {
            Ok(minutes) => LocalOffset::Minutes(minutes),
            Err(_) => LocalOffset::Unreadable,
        }
    }

    /// 一个瞬间 → `SYSTEMTIME`（UTC）。**早于纪元 / 超出年份范围 ⇒ 读不出来**
    /// （与 `format_local` 同一条纪律：不编一个时刻）。
    fn system_time_of(at: std::time::SystemTime) -> Option<SystemTime> {
        let since_epoch = at.duration_since(std::time::UNIX_EPOCH).ok()?;
        let seconds = i64::try_from(since_epoch.as_secs()).ok()?;
        let days = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400);
        let (year, month, day) = super::civil_from_days(days);
        Some(SystemTime {
            year: u16::try_from(year).ok()?,
            month: u16::try_from(month).ok()?,
            // 星期几本件不读（`SystemTimeToTzSpecificLocalTime` 也不要求它）。
            day_of_week: 0,
            day: u16::try_from(day).ok()?,
            hour: u16::try_from(rest / 3_600).ok()?,
            minute: u16::try_from((rest % 3_600) / 60).ok()?,
            second: u16::try_from(rest % 60).ok()?,
            milliseconds: 0,
        })
    }

    /// 一个 `SYSTEMTIME` → 自纪元起的分钟。**只用来做差**（偏移），所以基准是哪个无所谓。
    fn minutes_of(at: &SystemTime) -> i64 {
        super::days_from_civil(i64::from(at.year), i64::from(at.month), i64::from(at.day)) * 1_440
            + i64::from(at.hour) * 60
            + i64::from(at.minute)
    }
}

/// **Linux 臂**：`localtime_r(3)` 的 `tm_gmtoff`（"UTC 之东的秒数"）。
#[cfg(target_os = "linux")]
mod arm {
    use super::LocalOffset;

    /// `struct tm`（glibc / musl 的公开布局）。字段由系统调用填写，本件只读最后一个。
    #[repr(C)]
    #[allow(dead_code)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: i64,
        zone: *const i8,
    }

    // 安全：签名取自 `time.h`（`struct tm *localtime_r(const time_t *, struct tm *)`）；
    // `time_t` 在 64 位 Linux 上是 `long`（i64）。与 `platform.rs` 的 `kill(2)` 同一条先例：
    // 为两个固定签名拉一个 crate 不划算，签名由我们自己保证。
    unsafe extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }

    /// **那一刻**的偏移：`localtime_r` 收的就是那个时刻，`tm_gmtoff` 因此是**那一刻**的偏移
    /// （夏令时由系统按规则判）。
    pub fn offset_at(at: std::time::SystemTime) -> LocalOffset {
        let Ok(since_epoch) = at.duration_since(std::time::UNIX_EPOCH) else {
            return LocalOffset::Unreadable;
        };
        let Ok(seconds) = i64::try_from(since_epoch.as_secs()) else {
            return LocalOffset::Unreadable;
        };
        let mut out = std::mem::MaybeUninit::<Tm>::uninit();
        // 安全：入参是我们自己的 `i64`；`out` 是足够大的 `struct tm`；只读系统状态。
        let filled = unsafe { localtime_r(&seconds, out.as_mut_ptr()) };
        if filled.is_null() {
            return LocalOffset::Unreadable;
        }
        // 安全：非空即已填好。
        let tm = unsafe { out.assume_init() };
        // 历史时区有**秒级**偏移 ⇒ 取整会编一个数，宁可读不出来。
        if tm.gmtoff % 60 != 0 {
            return LocalOffset::Unreadable;
        }
        match i32::try_from(tm.gmtoff / 60) {
            Ok(minutes) => LocalOffset::Minutes(minutes),
            Err(_) => LocalOffset::Unreadable,
        }
    }
}

/// **没有臂的平台**：如实读不出来（`time` 因此保持 `Z`），**绝不换一个近似行为**
/// （不猜 0、不按 UTC 冒充）。
#[cfg(not(any(windows, target_os = "linux")))]
mod arm {
    /// 本平台没有读本机偏移的臂。
    pub fn offset_at(_at: std::time::SystemTime) -> super::LocalOffset {
        super::LocalOffset::Unreadable
    }
}

/// **毫秒刻度**：十进制 unix 毫秒。**这是它唯一的换算点**。
///
/// 主体看得见的毫秒读数——状态行的 `since`（[`anchor_value`]）与持有物的 `when` / 年龄——
/// 都从这里出来，因此"主体原样递回去的那个数"与"系统算出来的那个数"不可能对不上。
///
/// 早于 1970-01-01T00:00:00Z 的时刻返回 `None`：**不编一个数**（与 [`format_utc`] /
/// [`anchor_value`] 同一条纪律，只是这里把"读不出来"交给调用方按自己的处境处理）。
pub fn unix_millis(at: SystemTime) -> Option<u64> {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_millis() as u64)
}

/// **锚（`since`）在处境里的渲染**：十进制 unix 毫秒——与自己的动词吃的形状**同一个**。
///
/// 锚唯一的用处就是递给 `im_rooms(since=…)`，而那个参数要 unix 毫秒（见 `im_app::PARAM_SINCE`）。
/// 按可读时刻渲染，主体就得先在脑子里做一次换算——实测两种行为都出现过：多数时候主体换算，
/// 有时把那一行**原样**递回去，于是被参数网关以 `invalid-argument` 挡下（拒绝是对的，
/// 代价是一轮）。**渲染成主体动词吃的那个形状**，这一整类来回就没有了
/// （2026-09-17 主人裁决：锚行改 unix 毫秒主形；判据 = 主体拿锚行原文直接作 `since` 通过）。
///
/// 它是锚**唯一**的格式化点：状态行第四行与通知栏首行都走它，因此"两处是同一份事实的两处渲染"
/// 在形状上也是同一处。
///
/// 早于 1970-01-01T00:00:00Z 的时刻读作 [`UNKNOWN_SINCE`]：`since` 不收负数，
/// 与其给一个自己的动词都会拒绝的数，不如如实说读不出来（与 [`format_utc`] 同一条纪律）。
/// `time` 那一行**不变**：它没有第二个消费者，给人看的就是可读时刻。
pub fn anchor_value(at: SystemTime) -> String {
    match unix_millis(at) {
        Some(millis) => millis.to_string(),
        None => UNKNOWN_SINCE.to_string(),
    }
}

/// **民用日期 → 自 1970-01-01 起的天数**（[`civil_from_days`] 的逆；Howard Hinnant 的
/// `days_from_civil`，公有领域算法）。
///
/// 两个消费者：[`parse_rfc3339`]（把世界给的时刻读成瞬间）与 Windows 臂（算"那一刻的偏移"
/// 要做两个 `SYSTEMTIME` 的差）。用例钉住它与 `civil_from_days` **互逆**。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 自 1970-01-01（第 0 天）起的**民用日期**（年、月、日）。
///
/// 这是 Howard Hinnant 的 `civil_from_days`（公有领域算法）：只做整数运算，因此没有闰年表、
/// 没有时区、也没有额外依赖（本 crate 不为时间引入依赖——时间在这一层只是一个读数）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}
