//! A 包公共响应模型（CORE_V1_IM_CONTRACT.md §3.1）。
//!
//! 只包含公开上下文：Home Identity 非秘密字段、Grant 非秘密元数据、
//! Session 非秘密元数据；绝不携带 token/token hash。

use serde::{Deserialize, Serialize};

/// GET /v1/home 响应：Home Identity（非秘密、供绑定核验与展示）。
#[derive(Debug, Clone, Serialize)]
pub struct HomeResponse {
    pub home_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
    pub request_id: String,
}

/// Pairing Grant 非秘密元数据（列表/撤销视图共用；绝不包含 token/hash）。
#[derive(Debug, Clone, Serialize)]
pub struct GrantView {
    pub grant_id: String,
    pub status: String,
    pub device_label: Option<String>,
    /// 用途（`self_device` | `new_human`）：客户端据此把"设备配对"与
    /// "邀请一个人"分成两件事显示。
    pub grants: String,
    pub issued_at: String,
    pub expires_at: String,
    pub consumed_at: Option<String>,
    pub revoked_at: Option<String>,
}

/// POST /v1/home/pairing-grants 响应：grant 元数据 + 一次性 token。
#[derive(Debug, Clone, Serialize)]
pub struct GrantIssueResponse {
    pub grant_id: String,
    pub status: String,
    pub grants: String,
    pub device_label: Option<String>,
    pub issued_at: String,
    pub expires_at: String,
    pub token: String,
    pub request_id: String,
}

/// GET /v1/home/pairing-grants 响应。
#[derive(Debug, Clone, Serialize)]
pub struct GrantListResponse {
    pub grants: Vec<GrantView>,
    pub request_id: String,
}

/// POST /v1/home/pairing-grants/exchange 响应：新 Session 非秘密元数据 +
/// 一次性 raw Session token + Home Identity（供首次核验与落绑定）。
#[derive(Debug, Clone, Serialize)]
pub struct ExchangeSessionView {
    pub id: String,
    pub human_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExchangeHomeView {
    pub home_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExchangeResponse {
    pub session: ExchangeSessionView,
    pub token: String,
    pub home: ExchangeHomeView,
    pub request_id: String,
}

/// GET /v1/sessions 的单个 Session 项（非秘密元数据 + 可选 Session
/// Metadata；绝不包含 token/hash）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub id: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub device_label: Option<String>,
    pub source_grant_id: Option<String>,
}

/// GET /v1/sessions 响应。
#[derive(Debug, Clone, Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionView>,
    pub request_id: String,
}

// ===== A2 Pairing Transaction（HOME_ACCESS.md §4.3 A2）=====

/// POST /v1/home/pairing-grants/prepare 请求体：Host 生成的固定长度摘要
/// 与 nonce；Core 只接收 SHA-256（64 字符小写 hex），绝不接收原始
/// Session / commit token。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareTransactionRequest {
    pub client_nonce: String,
    pub session_token_hash: String,
    pub commit_token_hash: String,
}

/// 事务非秘密视图（status/commit 成功路径共用；含 Home Identity 供
/// 绑定核验、issued_by Human 非秘密身份与 reserved session id 供 Host
/// 识别自己的预留；绝不包含 token 或 token hash）。
#[derive(Debug, Clone, Serialize)]
pub struct TransactionResponse {
    pub transaction_id: String,
    pub status: String,
    pub home: TransactionHomeView,
    pub human: TransactionHumanView,
    pub reserved_session_id: String,
    pub prepared_at: String,
    pub expires_at: String,
    pub committed_at: Option<String>,
    pub aborted_at: Option<String>,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionHomeView {
    pub home_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionHumanView {
    pub human_id: String,
    pub display_name: Option<String>,
}

/// TransactionResponse 的构造 helper：从领域视图 + Home Identity 组装响应。
pub fn transaction_response(
    view: &crate::domain::pairing_transaction::TransactionView,
    home: &crate::domain::home::Home,
    human_display_name: Option<&crate::domain::participant::DisplayName>,
    request_id: String,
) -> TransactionResponse {
    TransactionResponse {
        transaction_id: view.transaction_id.as_str().to_owned(),
        status: view.status.as_str().to_owned(),
        home: TransactionHomeView {
            home_id: home.home_id.as_str().to_owned(),
            display_name: home.display_name.as_ref().map(|n| n.as_str().to_owned()),
            created_at: home.created_at.to_rfc3339(),
        },
        human: TransactionHumanView {
            human_id: view.human_id.as_str().to_owned(),
            display_name: human_display_name.map(|n| n.as_str().to_owned()),
        },
        reserved_session_id: view.reserved_session_id.as_str().to_owned(),
        prepared_at: view.prepared_at.to_rfc3339(),
        expires_at: view.expires_at.to_rfc3339(),
        committed_at: view.committed_at.map(|t| t.to_rfc3339()),
        aborted_at: view.aborted_at.map(|t| t.to_rfc3339()),
        request_id,
    }
}

/// POST /v1/home/pairing-transactions/{id}/commit 请求体：可选 device_label。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitTransactionBody {
    #[serde(default)]
    pub device_label: Option<String>,
}
