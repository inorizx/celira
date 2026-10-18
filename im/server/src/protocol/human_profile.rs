//! 人自己的资料（表达权第四刀，2026-10-17 主人批准）。
//!
//! **归属结论**（三问定的，见 `outputs/HUMAN_PROFILE_RULING_DRAFT_2026-10-17.md`）：
//! 换成微信/QQ 还成立吗 ⇒ 名字/头像**归世界**；别人要不要看见 ⇒ 世界必须持有一份权威；
//! 是不是"谁说的" ⇒ 归**主体**且必须署名。
//!
//! 这一组 wire 形状只说**自己**：路径里没有"写谁的"（身份只来自认证），
//! 从形状上排除"改别人"。
//!
//! ⚠ **名字与头像不是同一种事实**：
//! - 名字 = 既有 Participant 身份事实（`participants.display_name`）⇒ 读出来是**一格字符串**，
//!   写下去是**状态写**（无版本行、无幂等键）；
//! - 头像 = 呈现版本链上的一格（`participant_profile_versions`）⇒ 读写都走版本链那一套。
//!
//! 所以这里**没有一个"合起来的 profile 对象"**：读的时候把两件事**并排**给出来，
//! 写的时候各走各的端点 —— 一个端点说一件事。

use serde::{Deserialize, Serialize};

use crate::protocol::participant_presentation::ProfileView;

/// `GET /v1/me/profile` 的响应：**名字**（身份事实）＋ **呈现**（版本链那一格，可能为空）。
#[derive(Debug, Clone, Serialize)]
pub struct GetHumanProfileResponse {
    /// 我叫什么（`participants.display_name`，身份事实）。
    pub display_name: String,
    /// 我的呈现（头像引用 ＋ 版本 ＋ 署名）；**从未写过 ⇒ `null`**（不是空对象）。
    pub profile: Option<ProfileView>,
    pub request_id: String,
}

/// `PUT /v1/me/profile` 的请求：只改名字。
///
/// **只有这一个字段**：简介/头像是呈现版本链上的事，各走各的端点（`/profile/avatar`）。
/// 未知字段由 serde 拒绝（`deny_unknown_fields`）⇒ 形状错了就是 400，不静默忽略。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutHumanProfileRequest {
    pub display_name: String,
}

/// `PUT /v1/me/profile` 的响应：改完之后**我叫什么**（回读值，不是回声）。
#[derive(Debug, Clone, Serialize)]
pub struct PutHumanProfileResponse {
    pub display_name: String,
    pub request_id: String,
}
