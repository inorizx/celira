//! 两个**外部协议记号**的字面量（只在这一处写）。
//!
//! 它们是 wire 的语言，不是我们的词：`minimal_agent` 的短词守卫只对**这几个带引号的字面量**
//! 按短语放行（理由在那张表里）。写在这里一次，别处都引用常量——同一个记号两处拼写，
//! 迟早有一处写错而没人发现。

/// 请求正文的媒体类型。
pub const BODY_TYPE: &str = "application/json";

/// 答复的媒体类型：一条事件流（SSE）。
pub const EVENT_TYPE: &str = "text/event-stream";

/// `Accept`：两种都收（普通正文或事件流）。
pub const ACCEPT: &str = "application/json, text/event-stream";
