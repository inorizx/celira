//! Agent 用例：创建（幂等）、列表、读取、软撤销。
//!
//! 用例层负责规则编排与事务边界：校验幂等键与显示名、生成 ID 与摘要，
//! 再把已就绪的事实交给 `ports::agent::AgentStore` 一次性持久化。
//! unknown / bad-id / not-owned 在应用层合并为统一的 NotFound，不泄露
//! 其他 Human 的 Agent 是否存在。

use uuid::Uuid;

use crate::application::unexpected_store_error;
use crate::domain::agent::{Agent, AgentOwner, IdempotencyKey, OwnedAgent, create_request_digest};
use crate::domain::participant::{DisplayName, DisplayNameError, Participant, ParticipantId};
use crate::domain::room::MembershipId;
use crate::domain::time::UtcMillis;
use crate::ports::agent::{
    AgentStore, AgentStoreError, CreateAgentOutcome as StoreCreateOutcome, NewAgent,
    NewAgentOwnership, NewIdempotencyRecord,
};
use crate::ports::storage::StorageError;

/// 创建结果：首次创建与幂等重放返回相同的响应模型，以
/// `idempotency_replayed` 区分（HTTP 201 / 200）。
#[derive(Debug)]
pub enum CreateAgentOutcome {
    Created { agent: Agent, ownership: AgentOwner },
    Replayed { agent: Agent, ownership: AgentOwner },
}

#[derive(Debug)]
pub enum CreateAgentError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// display name 不合法。
    DisplayName(DisplayNameError),
    /// 同 Human + 同 key 已被不同请求占用。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for CreateAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::DisplayName(e) => write!(f, "{e}"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "agent store failure: {e}"),
        }
    }
}

impl std::error::Error for CreateAgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 创建 Agent：校验 key 与 display name → 生成稳定 ID → 原子持久化
/// Agent + Owner + 幂等记录；同 Human + 同 key + 同请求摘要返回既有事实。
pub async fn create_agent(
    store: &dyn AgentStore,
    human: &Participant,
    display_name_raw: &str,
    idempotency_key_raw: &str,
) -> Result<CreateAgentOutcome, CreateAgentError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| CreateAgentError::InvalidIdempotencyKey)?;
    let display_name =
        DisplayName::parse(display_name_raw).map_err(CreateAgentError::DisplayName)?;
    let created_at = UtcMillis::now();
    let agent = Agent::new(
        ParticipantId(Uuid::new_v4().to_string()),
        display_name,
        created_at,
    );
    let ownership = AgentOwner::new(agent.id.clone(), human.id.clone(), created_at);
    let idempotency = NewIdempotencyRecord {
        human_id: human.id.clone(),
        key,
        request_digest: create_request_digest(agent.display_name.as_str()),
        agent_id: agent.id.clone(),
        created_at,
    };

    match store
        .create_agent(
            &human.id,
            &NewAgent {
                id: agent.id.clone(),
                display_name: agent.display_name.clone(),
                created_at,
            },
            &NewAgentOwnership {
                agent_id: ownership.agent_id.clone(),
                human_id: ownership.human_id.clone(),
                created_at,
            },
            &idempotency,
        )
        .await
    {
        Ok(StoreCreateOutcome::Created) => Ok(CreateAgentOutcome::Created { agent, ownership }),
        Ok(StoreCreateOutcome::Replayed { agent, ownership }) => {
            Ok(CreateAgentOutcome::Replayed { agent, ownership })
        }
        Err(AgentStoreError::IdempotencyConflict) => Err(CreateAgentError::IdempotencyConflict),
        Err(AgentStoreError::Database(e)) => Err(CreateAgentError::Store(e)),
        Err(AgentStoreError::NotFound) => Err(CreateAgentError::Store(unexpected_store_error(
            "agent store error",
            "creating human not found",
        ))),
    }
}

/// 查询类失败：未知 / 格式错误 / 从未拥有统一为 NotFound。
#[derive(Debug)]
pub enum AgentQueryError {
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for AgentQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "agent not found"),
            Self::Store(e) => write!(f, "agent store failure: {e}"),
        }
    }
}

impl std::error::Error for AgentQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_query_error(e: AgentStoreError) -> AgentQueryError {
    match e {
        AgentStoreError::NotFound => AgentQueryError::NotFound,
        AgentStoreError::Database(e) => AgentQueryError::Store(e),
        AgentStoreError::IdempotencyConflict => AgentQueryError::Store(unexpected_store_error(
            "agent store error",
            "unexpected idempotency conflict",
        )),
    }
}

/// 列出当前 Human 曾经拥有的 Agent（active + revoked），已按
/// 创建时间升序、再按稳定 ID 排序。
pub async fn list_agents(
    store: &dyn AgentStore,
    human_id: &ParticipantId,
) -> Result<Vec<OwnedAgent>, AgentQueryError> {
    store
        .list_owned_agents(human_id)
        .await
        .map_err(map_query_error)
}

/// 读取当前 Human 曾经拥有的单个 Agent；unknown、bad-id、not-owned 统一
/// NotFound（HTTP 404 agent_not_found）。
pub async fn get_agent(
    store: &dyn AgentStore,
    human_id: &ParticipantId,
    agent_id: &str,
) -> Result<OwnedAgent, AgentQueryError> {
    let id = ParticipantId::parse_stable(agent_id).ok_or(AgentQueryError::NotFound)?;
    store
        .get_owned_agent(human_id, &id)
        .await
        .map_err(map_query_error)?
        .ok_or(AgentQueryError::NotFound)
}

/// 幂等软撤销：Agent、全部 active Owner 与数据库迁移声明的下游公共事实
/// 同时 revoked；重复撤销返回成功且不改原 revoked_at；unknown、bad-id、
/// not-owned 统一 NotFound。返回本次撤销实际关闭的 Agent Membership 周期
/// ID（post-commit 唤醒目标查询的 source ID）。
pub async fn revoke_agent(
    store: &dyn AgentStore,
    human_id: &ParticipantId,
    agent_id: &str,
) -> Result<Vec<MembershipId>, AgentQueryError> {
    let id = ParticipantId::parse_stable(agent_id).ok_or(AgentQueryError::NotFound)?;
    store
        .revoke_agent(human_id, &id)
        .await
        .map_err(map_query_error)
}
