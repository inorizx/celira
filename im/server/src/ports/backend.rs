//! Backend Binding / Credential 窄端口。
//!
//! 本端口只声明 Backend 切片的持久化边界，不依赖具体数据库或 HTTP 框架。
//! SQLite adapter 实现本端口；应用用例只通过端口消费存储能力。

use crate::domain::agent::IdempotencyKey;
use crate::domain::backend::{
    BackendBinding, BackendCredential, BackendLabel, BindingId, CredentialId,
};
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::ports::storage::StorageError;

/// 待持久化的新 Binding（status=active、无撤销时间）。
#[derive(Debug, Clone)]
pub struct NewBinding {
    pub id: BindingId,
    pub agent_id: ParticipantId,
    pub created_by_human_id: ParticipantId,
    pub label: BackendLabel,
    pub idempotency_key: IdempotencyKey,
    pub created_at: UtcMillis,
}

/// 待持久化的新 Credential（status=active、无撤销时间）。Debug 必须遮蔽
/// token hash，避免存储命令在诊断日志中暴露可用于精确认证查询的摘要。
#[derive(Clone)]
pub struct NewCredential {
    pub id: CredentialId,
    pub binding_id: BindingId,
    pub issued_by_human_id: ParticipantId,
    pub token_hash: String,
    pub replaces_credential_id: Option<CredentialId>,
    pub idempotency_key: IdempotencyKey,
    pub created_at: UtcMillis,
}

impl std::fmt::Debug for NewCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewCredential")
            .field("id", &self.id)
            .field("binding_id", &self.binding_id)
            .field("issued_by_human_id", &self.issued_by_human_id)
            .field("token_hash", &"[REDACTED]")
            .field("replaces_credential_id", &self.replaces_credential_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// 创建结果。
#[derive(Debug, Clone)]
pub enum CreateBindingOutcome {
    /// 首次创建：Binding 与幂等协调列在同一事务提交。
    Created { binding: BackendBinding },
    /// 同 Human + 同 key + 同 Agent + 同 label 的重放：返回原 Binding。
    Replayed { binding: BackendBinding },
}

/// 签发结果。
#[derive(Debug, Clone)]
pub enum IssueCredentialOutcome {
    /// 首次签发或轮换：旧 Credential 撤销与新 Credential 创建同一事务提交。
    Created { credential: BackendCredential },
    /// 同 Human + 同 key + 同目标 + 同 replaces ID 的重放：返回原
    /// Credential 元数据；原始 token 不可找回，由应用层返回 token=null。
    Replayed { credential: BackendCredential },
}

/// Backend 认证成功视图：Credential → Binding → Agent 解析出的被代表 Agent。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendAuthView {
    pub agent: AgentAuthView,
    pub binding: BindingAuthView,
    pub credential: CredentialAuthView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAuthView {
    pub id: ParticipantId,
    pub display_name: String,
    pub status: String,
    /// agent Participant 创建时间（构造发送端点的完整 Participant actor 用）。
    pub created_at: UtcMillis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingAuthView {
    pub id: BindingId,
    pub label: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialAuthView {
    pub id: CredentialId,
    pub status: String,
    pub created_at: UtcMillis,
}

/// 存储层错误。
#[derive(Debug)]
pub enum BackendStoreError {
    /// 目标 Agent 未知、格式错误或当前 Human 从未拥有：对外统一 404
    /// `agent_not_found`，不泄露其他 Human 的 Agent 是否存在。
    AgentNotFound,
    /// 目标 Agent 或其 Owner 关系已 revoked：对外统一 409 `agent_not_active`。
    AgentNotActive,
    /// 已确认是当前 Human 曾拥有的 Agent，但 Binding unknown/bad/
    /// not-under-agent：对外统一 404 `backend_binding_not_found`。
    BindingNotFound,
    /// 已确认是当前 Human 曾拥有的 Binding，但 Credential unknown/bad/
    /// not-under-binding：对外统一 404 `backend_credential_not_found`。
    CredentialNotFound,
    /// 同 Human + 同 key 对应不同请求：409 `idempotency_conflict`。
    IdempotencyConflict,
    /// 新 key 创建 Binding 但 Agent 已有 active Binding：409
    /// `backend_binding_exists`。
    BindingExists,
    /// 首次签发/轮换声明与当前 active Credential 状态不一致：409
    /// `credential_state_conflict`；不得撤销或创建任何 Credential。
    CredentialStateConflict,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for BackendStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentNotFound => write!(f, "no such owned agent"),
            Self::AgentNotActive => write!(f, "agent or owner relationship is revoked"),
            Self::BindingNotFound => write!(f, "no such backend binding under the owned agent"),
            Self::CredentialNotFound => {
                write!(f, "no such backend credential under the owned binding")
            }
            Self::IdempotencyConflict => write!(
                f,
                "idempotency key was already used with a different request"
            ),
            Self::BindingExists => write!(f, "agent already has an active backend binding"),
            Self::CredentialStateConflict => {
                write!(
                    f,
                    "credential issue or rotation does not match current state"
                )
            }
            Self::Database(e) => write!(f, "backend store failure: {e}"),
        }
    }
}

impl std::error::Error for BackendStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// Backend 存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait BackendStore: Send + Sync {
    /// 原子创建 Binding + 幂等协调列。同 Human + key 已存在时：Agent 与
    /// label 完全一致返回 `Replayed`（不再创建任何行），不一致返回
    /// `IdempotencyConflict` 且不留下半成品。重放裁决先于当前 Agent /
    /// Binding 生命周期检查。
    async fn create_binding(
        &self,
        human_id: &ParticipantId,
        binding: &NewBinding,
    ) -> Result<CreateBindingOutcome, BackendStoreError>;

    /// 列出当前 Human 曾经授权给该 Agent 的全部 Binding（active + revoked），
    /// 按创建时间升序、再按 Binding ID 排序。Agent 未知/未拥有返回
    /// `AgentNotFound`。
    async fn list_bindings(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
    ) -> Result<Vec<BackendBinding>, BackendStoreError>;

    /// 读取当前 Human 曾拥有的 Agent 下的单个 Binding；unknown/bad/
    /// not-under-agent 统一 `BindingNotFound`，不泄露其他 Agent 的 Binding。
    async fn get_binding(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
    ) -> Result<BackendBinding, BackendStoreError>;

    /// 幂等软撤销 Binding：同一事务内先撤销其 active Credential，再撤销
    /// Binding；已撤销的重复撤销不改原 revoked_at。Agent 未知/未拥有返回
    /// `AgentNotFound`，unknown/bad/not-under-agent Binding 返回
    /// `BindingNotFound`。
    async fn revoke_binding(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
    ) -> Result<(), BackendStoreError>;

    /// 首次签发或 CAS 轮换：幂等裁决、当前 active 状态 CAS、旧 Credential
    /// 撤销与新 Credential 创建原子提交。同 Human + key 已存在时：Agent /
    /// Binding / replaces ID 完全一致返回 `Replayed`，不一致返回
    /// `IdempotencyConflict`；重放只返回已存在历史，不恢复、不重新签发。
    async fn issue_credential(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
        credential: &NewCredential,
    ) -> Result<IssueCredentialOutcome, BackendStoreError>;

    /// 列出该 Binding 的 Credential 元数据历史（active + revoked），按创建
    /// 时间升序、再按 Credential ID 排序；不返回 token hash 与幂等键。
    async fn list_credentials(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
    ) -> Result<Vec<BackendCredential>, BackendStoreError>;

    /// 幂等软撤销一个 Credential；不撤销 Binding 或 Agent，不自动签发
    /// 替代 Credential；unknown/bad/not-under-binding 返回
    /// `CredentialNotFound`；已撤销的重复撤销不改原 revoked_at。
    async fn revoke_credential(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
        credential_id: &str,
    ) -> Result<(), BackendStoreError>;

    /// Backend 认证：token hash 精确查找，同时证明 Credential active、
    /// 所属 Binding active、Binding 指向 active Agent（kind=agent）。
    /// 任一不成立返回 None，不泄露存在性。
    async fn authenticate(
        &self,
        token_hash: &str,
    ) -> Result<Option<BackendAuthView>, BackendStoreError>;
}
