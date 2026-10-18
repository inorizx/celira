//! Contact Resource API 请求与响应模型（契约
//! CORE_V1_CONTACT_REVERSIBILITY.md + 批次三第二刀「关系对主体开放」）。
//!
//! 请求创建使用 Idempotency-Key；公开响应不返回 token/hash/幂等键。
//! 联系人列表只返回当前主体作为 requester 或 addressee 的 **active** 段
//! 及对端公共资料（**对端可以是 Human 或 Agent**——关系是两个主体之间的事实，
//! 批次三第二刀）；公开面 status 只会出现 pending / accepted（已拒绝 /
//! 已解除的闭合段不列出）。拉黑列表只返回当前 Human 自己拉黑的 active 段
//! （拉黑仍两端限 Human），不返回内部 block_id。

use serde::{Deserialize, Serialize};

use crate::protocol::room::ParticipantMinView;

/// POST /v1/contacts/requests 请求体。未知字段一律拒绝。
///
/// 字段名 `participant_id`（批次三第二刀由 `human_id` 改名）：目标从此可以是
/// **任意 active 主体**，旧名字会是一句假话。同一刀里 `POST /v1/human-direct-rooms`
/// 与 Service Access 的 `human_id` **保持不变**——那两条路径仍然只接受 Human。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateContactRequest {
    pub participant_id: String,
}

/// 联系人关系公共视图：对端最小资料（自带 kind）+ 关系状态 + 时间。
/// 幂等键与 requester/addressee 的 token 级事实不进入公开视图。
#[derive(Debug, Clone, Serialize)]
pub struct ContactView {
    pub peer: ParticipantMinView,
    pub status: String,
    /// **这条关系是等谁回答的**（相对观察者）：`incoming` = 对方向我发起，只有我能
    /// 接受/拒绝；`outgoing` = 我发起的，等对方。accepted 段同样如实给出方向（历史事实）。
    ///
    /// 为什么必须有它（批次三第三刀 第②步）：接受/拒绝的判据是"我是不是 addressee"，
    /// 而**世界知道方向、投影却把它藏了**——于是主体分不清"哪条在等我回答"，只能靠
    /// 试错（对被拒的那条回 `contact_not_found`）。给主体做决定的信息不该被藏起来；
    /// 人类客户端同样受益（它今天也只能对每一条 pending 都画「接受」）。
    pub direction: String,
    pub created_at: String,
    pub accepted_at: Option<String>,
}

/// POST /v1/contacts/requests 响应：201 首次创建 / 200 返回既有关系或幂等
/// 重放，以 `idempotency_replayed` 区分。
#[derive(Debug, Clone, Serialize)]
pub struct CreateContactResponse {
    pub contact: ContactView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// GET /v1/contacts 响应：当前 Human 作为 requester 或 addressee 的全部
/// **active** 段，pending 与 accepted 原样公开。
#[derive(Debug, Clone, Serialize)]
pub struct ListContactsResponse {
    pub contacts: Vec<ContactView>,
    pub request_id: String,
}

/// POST /v1/contacts/{human_id}/accept 响应：接受成功（pending → accepted）
/// 或已 accepted 幂等返回，均为 200。
#[derive(Debug, Clone, Serialize)]
pub struct AcceptContactResponse {
    pub contact: ContactView,
    pub request_id: String,
}

/// 拉黑列表项：被当前 Human 拉黑的对端 Human 最小资料 + 拉黑时间。
/// 内部 block_id 不进入公开视图。
#[derive(Debug, Clone, Serialize)]
pub struct BlockView {
    pub peer: ParticipantMinView,
    pub created_at: String,
}

/// GET /v1/blocks 响应：当前 Human 作为 blocker 的全部 active 拉黑段，
/// 按创建时间升序、再按对端 ID 排序。
#[derive(Debug, Clone, Serialize)]
pub struct ListBlocksResponse {
    pub blocks: Vec<BlockView>,
    pub request_id: String,
}
