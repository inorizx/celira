//! Agent 生命周期与 Owner 窄端口。
//!
//! 本端口只声明 Agent 切片的持久化边界，不依赖具体数据库或 HTTP 框架。
//! SQLite adapter 实现本端口；应用用例只通过端口消费存储能力。

use crate::domain::agent::{Agent, AgentOwner, IdempotencyKey, OwnedAgent};
use crate::domain::participant::{DisplayName, ParticipantId};
use crate::domain::room::MembershipId;
use crate::domain::time::UtcMillis;
use crate::ports::storage::StorageError;

/// 待持久化的新 Agent（kind=agent、status=active、无撤销时间）。
#[derive(Debug, Clone)]
pub struct NewAgent {
    pub id: ParticipantId,
    pub display_name: DisplayName,
    pub created_at: UtcMillis,
}

/// 待持久化的初始 Owner 关系（status=active、无撤销时间）。
#[derive(Debug, Clone)]
pub struct NewAgentOwnership {
    pub agent_id: ParticipantId,
    pub human_id: ParticipantId,
    pub created_at: UtcMillis,
}

/// 待持久化的幂等记录：同 Human + key 唯一，关联请求摘要与已创建 Agent。
#[derive(Debug, Clone)]
pub struct NewIdempotencyRecord {
    pub human_id: ParticipantId,
    pub key: IdempotencyKey,
    pub request_digest: String,
    pub agent_id: ParticipantId,
    pub created_at: UtcMillis,
}

/// 创建结果。
#[derive(Debug, Clone)]
pub enum CreateAgentOutcome {
    /// 首次创建：Agent、Owner、幂等记录在同一事务提交。
    Created,
    /// 同 Human + 同 key + 同请求摘要的重放：返回既有的 Agent 与 Owner。
    Replayed { agent: Agent, ownership: AgentOwner },
}

/// 存储层错误。
#[derive(Debug)]
pub enum AgentStoreError {
    /// 目标不存在：未知 Agent、格式错误 ID 或当前 Human 从未拥有。
    /// 三种形态对外统一为 404。
    NotFound,
    /// 同 Human + 同 key 对应不同请求摘要：409。
    IdempotencyConflict,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for AgentStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such owned agent"),
            Self::IdempotencyConflict => write!(
                f,
                "idempotency key was already used with a different request"
            ),
            Self::Database(e) => write!(f, "agent store failure: {e}"),
        }
    }
}

impl std::error::Error for AgentStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// Agent 生命周期存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait AgentStore: Send + Sync {
    /// 原子创建 Agent + Owner + 幂等记录：三者同时成功或失败。
    ///
    /// 同 Human + key 已存在时：摘要一致返回 `Replayed`（不再创建任何行），
    /// 摘要不一致返回 `IdempotencyConflict` 且不留下半成品。
    async fn create_agent(
        &self,
        human_id: &ParticipantId,
        agent: &NewAgent,
        ownership: &NewAgentOwnership,
        idempotency: &NewIdempotencyRecord,
    ) -> Result<CreateAgentOutcome, AgentStoreError>;

    /// 列出当前 Human 曾经拥有的全部 Agent（active + revoked），
    /// 按 Agent 创建时间升序、再按稳定 ID 排序。
    async fn list_owned_agents(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<OwnedAgent>, AgentStoreError>;

    /// 读取当前 Human 曾经拥有的单个 Agent 管理视图；从未拥有返回 None。
    async fn get_owned_agent(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
    ) -> Result<Option<OwnedAgent>, AgentStoreError>;

    /// 幂等软撤销：Agent、全部 active Owner 与数据库迁移声明的下游公共事实
    /// 在同一事务标记 revoked；已撤销的重复撤销不改原 revoked_at。无 Owner
    /// 历史返回 NotFound。返回本次撤销实际关闭的 Agent Membership 周期 ID
    /// （post-commit 唤醒目标查询的 source ID；重复撤销返回空列表）。
    async fn revoke_agent(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
    ) -> Result<Vec<MembershipId>, AgentStoreError>;
}
