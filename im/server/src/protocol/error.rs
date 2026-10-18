//! 统一错误响应模型。
//!
//! 所有非 2xx 响应携带稳定 `error.code` 和可关联日志的 `request_id`。

use serde::Serialize;

/// 稳定错误码。新增错误码必须在此登记，作为公共契约的一部分。
pub mod codes {
    /// 服务可正常响应但尚未就绪（如迁移未完成）。
    pub const NOT_READY: &str = "not_ready";
    /// 请求路径不存在。
    pub const NOT_FOUND: &str = "not_found";
    /// 路径存在但请求方法不被允许（如 POST /health）。
    pub const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
    /// 内部错误：未分类基础设施失败、内部不变量破坏或不可恢复故障（500）。
    pub const INTERNAL_ERROR: &str = "internal_error";
    /// 运行时 SQLite/持久化暂时不可用（503）。不得携带任何底层错误文本。
    pub const STORAGE_UNAVAILABLE: &str = "storage_unavailable";
    /// 受保护端点缺少 `Authorization: Bearer` 头。
    pub const AUTHENTICATION_REQUIRED: &str = "authentication_required";
    /// 非 Bearer、空 token、重复 header、未知/已撤销/指向非 active Human 的 token。
    pub const INVALID_SESSION: &str = "invalid_session";
    /// JSON、display name、agent ID 或请求体不合法。
    pub const INVALID_REQUEST: &str = "invalid_request";
    /// 创建 Agent 请求缺少 Idempotency-Key。
    pub const IDEMPOTENCY_KEY_REQUIRED: &str = "idempotency_key_required";
    /// Idempotency-Key 格式不合法或重复 header。
    pub const INVALID_IDEMPOTENCY_KEY: &str = "invalid_idempotency_key";
    /// 未知、格式错误或当前 Human 从未拥有的 Agent。
    pub const AGENT_NOT_FOUND: &str = "agent_not_found";
    /// 相同 Human + key 对应不同请求。
    pub const IDEMPOTENCY_CONFLICT: &str = "idempotency_conflict";
    /// 请求体超过当前 Agent 创建上限。
    pub const PAYLOAD_TOO_LARGE: &str = "payload_too_large";
    /// unknown、bad-id、当前 Human 从未成为成员的 Room。
    pub const ROOM_NOT_FOUND: &str = "room_not_found";
    /// Backend agent 主动发消息端点（CORE_V1_AGENT_SPEAK.md）：Room 存在但
    /// 该 agent 不是其 active member（403，明确 ACL 拒绝——agent 已通过
    /// BackendAuth 证明身份，成员资格是可观察公共事实）。
    pub const NOT_ROOM_MEMBER: &str = "not_room_member";
    /// 目标 Agent 或当前 Human 对它的 Owner 关系已 revoked。
    pub const AGENT_NOT_ACTIVE: &str = "agent_not_active";
    /// Message body 为空、超过 16384 UTF-8 bytes 或含 NUL。
    pub const INVALID_MESSAGE_BODY: &str = "invalid_message_body";
    /// Backend token 格式、存在性或任一级 active 状态不成立（统一 401）。
    pub const INVALID_CREDENTIAL: &str = "invalid_credential";
    /// owned Agent 下 unknown/bad/not-under-agent Binding。
    pub const BACKEND_BINDING_NOT_FOUND: &str = "backend_binding_not_found";
    /// owned Binding 下 unknown/bad/not-under-binding Credential。
    pub const BACKEND_CREDENTIAL_NOT_FOUND: &str = "backend_credential_not_found";
    /// 新 key 创建 Binding，但 Agent 已有 active Binding。
    pub const BACKEND_BINDING_EXISTS: &str = "backend_binding_exists";
    /// 首次签发/轮换声明与当前 active Credential 不一致。
    pub const CREDENTIAL_STATE_CONFLICT: &str = "credential_state_conflict";
    /// Mention 非法：非 UUID v4、重复目标、非 group Room 携带非空
    /// mentions、目标不是该 Room 的 active 成员或目标 participant 非
    /// active（400）。
    pub const INVALID_MENTION: &str = "invalid_mention";
    /// 联系人目标不存在、不是 active Human 或已 revoked（404）。
    /// **只在仍然只接受 Human 的路径上使用**（human_direct 建房、拉黑目标）。
    pub const HUMAN_NOT_FOUND: &str = "human_not_found";
    /// 关系面目标不存在、非 active 或（对申请路径）被对方拉黑（404，与「目标
    /// 不存在」同形、不泄露）。**批次三第二刀新增**：`POST /v1/contacts/requests`
    /// 的目标从此可以是任意 active 主体（Human 或 Agent），沿用 `human_not_found`
    /// 会是一句假话；其余三条 Human-only 路径继续用上面那个码。
    pub const PARTICIPANT_NOT_FOUND: &str = "participant_not_found";
    /// 联系人请求接受路径：关系不存在或当前 Human 不是 addressee（404）。
    pub const CONTACT_NOT_FOUND: &str = "contact_not_found";
    /// 目标 Human 存在但当前 Human 与它没有 accepted 联系人关系（409）。
    pub const CONTACT_NOT_ACCEPTED: &str = "contact_not_accepted";
    /// 同一对 Human 已由不同 idempotency key 创建 human_direct Room（409）。
    pub const HUMAN_DIRECT_ROOM_EXISTS: &str = "human_direct_room_exists";
    /// 邀请目标不是可邀请对象（未知、非 active、非 accepted 联系人、
    /// 非自己拥有的 active Agent）：统一 404。
    pub const MEMBER_NOT_FOUND: &str = "member_not_found";
    /// 尝试移除 group owner 自己（409）。
    pub const OWNER_NOT_REMOVABLE: &str = "owner_not_removable";
    /// group owner 主动退群前必须先转让（409）。
    pub const OWNER_TRANSFER_REQUIRED: &str = "owner_transfer_required";
    /// 群主转让目标不合法：目标是 Agent 或就是转让者自己（400）。
    pub const INVALID_TRANSFER_TARGET: &str = "invalid_transfer_target";
    /// read cursor 推进目标不合法：Message 未知、属于其他 Room 或对当前
    /// Human 的历史 Membership 窗口不可见（统一 400，不泄露具体原因）。
    pub const INVALID_READ_CURSOR: &str = "invalid_read_cursor";
    /// reply target 非法：target 未知、跨 Room、非已提交 Message 或对当前
    /// actor 任一历史 Membership 窗口不可见（统一 400 invalid_reply_target，
    /// 不区分具体原因，不泄露 target 是否存在、所属 Room 或边界）。
    pub const INVALID_REPLY_TARGET: &str = "invalid_reply_target";
    /// 撤回目标 Message 不存在、属于其他 Room、不是当前 actor 的 Message
    /// 或对 actor 不可见（统一 404 message_not_found，不泄露存在性或归属）。
    pub const MESSAGE_NOT_FOUND: &str = "message_not_found";
    /// 未撤回且超过 120 秒窗口的首次撤回（409；同一 statement 裁决，已撤回
    /// 的重复撤回不受窗口限制、幂等成功）。
    pub const MESSAGE_RECALL_WINDOW_EXPIRED: &str = "message_recall_window_expired";
    /// 上传的 file_name / content_type 缺失或形状非法（400）。
    pub const INVALID_ATTACHMENT_METADATA: &str = "invalid_attachment_metadata";
    /// Message 的 attachments 超过 8 个上限或包含重复附件 ID
    /// （400）。
    pub const INVALID_ATTACHMENT: &str = "invalid_attachment";
    /// 下载或绑定的附件 unknown / pending / 跨 Room / 跨 actor / 已过期 /
    /// 已撤回 / 读取者非成员：统一 404，不泄露存在性或原因。
    pub const ATTACHMENT_NOT_FOUND: &str = "attachment_not_found";
    /// Backend Event 唤醒流（窄解冻切片 §8）并发上限：每 Agent ≤2 或进程
    /// 总流数达硬上限（503，不排队、不降级；唤醒是易失提示，REST 轮询
    /// 仍是完整合法路径）。
    pub const BACKEND_STREAM_LIMIT_EXCEEDED: &str = "backend_stream_limit_exceeded";
    /// 匿名兑换 Pairing Grant 失败：格式错、未知、已用、过期、撤销统一
    /// 401（不泄露 Grant 是否存在或曾经有效）。
    pub const GRANT_INVALID: &str = "grant_invalid";
    /// A2 匿名 prepare 对不可用 Grant 仍统一 `grant_invalid`；事务
    /// 恢复/提交/放弃入口（status/commit/abort，以及参数不变量冲突）
    /// 的不可兑换情形统一 `pairing_transaction_invalid`（400），不泄露
    /// 事务是否存在或处于何种状态，也不提供存在性探测。
    pub const PAIRING_TRANSACTION_INVALID: &str = "pairing_transaction_invalid";
    /// 撤销 Pairing Grant：未知 / 非本人签发 / 已消费统一 404（不泄露
    /// 存在性或状态）。
    pub const GRANT_NOT_FOUND: &str = "grant_not_found";
    /// Session 管理（DELETE /v1/sessions/{id}）：未知 / 非本人统一 404。
    pub const SESSION_NOT_FOUND: &str = "session_not_found";
    /// Agent Profile PUT 输入不合法：base_version 非整数或 < 0，或正文含
    /// U+0000 / 超过对应 UTF-8 字节上限（400）。
    pub const PROFILE_INVALID: &str = "profile_invalid";
    /// Agent Profile PUT：新 key 但 base_version 与当前最新不一致（409，
    /// 不追加版本）。
    pub const PROFILE_STALE: &str = "profile_stale";
    /// Room Presentation PUT 输入不合法：base_version 非整数或 < 0，或正文
    /// 含 U+0000 / 超过对应 UTF-8 字节上限（400）。
    pub const ROOM_PRESENTATION_INVALID: &str = "room_presentation_invalid";
    /// Room Presentation PUT：新 key 但 base_version 与当前最新不一致（409，
    /// 不追加版本）。
    pub const ROOM_PRESENTATION_STALE: &str = "room_presentation_stale";
    /// 目标 Room 不是 group（direct / human_direct 等不支持的类型没有 Group
    /// Room Presentation，400）。
    pub const ROOM_PRESENTATION_NOT_SUPPORTED: &str = "room_presentation_not_supported";
    /// 未知 / 非本 Call 参与方 / 角色不匹配（404 call_not_found；不泄露
    /// 存在性或归属）。
    pub const CALL_NOT_FOUND: &str = "call_not_found";
    /// 当前 Call 状态不允许该动作（accept 非 ringing、已 ended 再操作等；
    /// 409 call_state_conflict）。
    pub const CALL_STATE_CONFLICT: &str = "call_state_conflict";
    /// Caller 或 Callee 已在另一 active Call（409 call_party_busy）。
    pub const CALL_PARTY_BUSY: &str = "call_party_busy";
    /// 信令载荷形状/字节/NUL/数量越界（400 call_payload_invalid）。
    pub const CALL_PAYLOAD_INVALID: &str = "call_payload_invalid";
    /// 进程内激活 Call 达硬上限（503 call_state_limit_exceeded）。
    pub const CALL_STATE_LIMIT_EXCEEDED: &str = "call_state_limit_exceeded";
    /// Avatar PUT/DELETE 输入不合法：非白名单 Content-Type（非 image/png |
    /// image/jpeg | image/webp）、空字节、畸形 multipart 或非法 base_version
    /// （400 avatar_invalid）。
    pub const AVATAR_INVALID: &str = "avatar_invalid";
    /// 读字节时目标无头像（最新 Profile 版本行 avatar=null；404 avatar_not_found，
    /// 与存在性隐藏家族一致）。
    pub const AVATAR_NOT_FOUND: &str = "avatar_not_found";
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    pub code: &'static str,
    pub message: String,
    pub request_id: String,
}

impl ErrorBody {
    pub fn new(
        code: &'static str,
        message: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            error: ErrorDetail {
                code,
                message: message.into(),
                request_id: request_id.into(),
            },
        }
    }
}

/// 成功状态响应（health / ready 共用）。
#[derive(Debug, Clone, Serialize)]
pub struct StatusOk {
    pub status: &'static str,
    pub request_id: String,
}

impl StatusOk {
    pub const STATUS: &'static str = "ok";
}
