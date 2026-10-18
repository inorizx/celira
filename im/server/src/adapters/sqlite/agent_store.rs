//! SQLite Agent 生命周期 adapter：实现 `ports::agent::AgentStore`。
//!
//! 本模块是 Agent 与 Owner 事实唯一允许接触 SQLite 文件的地方。HTTP handler
//! 和用例不得直接操作数据库，只能通过端口调用。
//!
//! 创建与撤销都使用 `BEGIN IMMEDIATE` 显式事务，由 SQLx Transaction 持有
//! 生命周期：创建时 Agent、Owner 与幂等记录同时提交，幂等冲突在事务内
//! 检测后由 Drop 回滚；撤销时 Agent、全部 active Owner 以及 migration
//! 触发器覆盖的下游公共事实同时变为 revoked。任何失败或 Future 取消都在
//! 连接归池前回滚，不能留下部分撤销、部分创建或脏事务。

#[cfg(test)]
use std::sync::Arc;

use sqlx::Connection;
use sqlx::sqlite::{SqliteConnection, SqlitePool};

use crate::adapters::sqlite::rollback_after_primary;
use crate::domain::agent::{Agent, AgentOwner, OwnedAgent};
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};
use crate::domain::room::MembershipId;
use crate::domain::time::UtcMillis;
use crate::ports::agent::{
    AgentStoreError, CreateAgentOutcome, NewAgent, NewAgentOwnership, NewIdempotencyRecord,
};
use crate::ports::storage::{StorageError, StorageFailureKind};

#[cfg(test)]
use tokio::sync::oneshot;

const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

const SQL_INSERT_AGENT: &str = "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
     VALUES (?, 'agent', 'active', ?, ?, NULL)";

const SQL_INSERT_OWNER: &str = "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
     VALUES (?, ?, 'active', ?, NULL)";

const SQL_INSERT_IDEMPOTENCY: &str = "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
     VALUES (?, ?, ?, ?, ?)";

const SQL_OWNED_BY_AGENT: &str = "SELECT p.id, p.kind, p.status, p.display_name, p.created_at, p.revoked_at, \
            o.human_id, o.status, o.created_at, o.revoked_at \
     FROM agent_owners o \
     JOIN participants p ON p.id = o.agent_id \
     WHERE o.human_id = ? AND o.agent_id = ?";

type OwnedRow = (
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
    String,
    String,
    i64,
    Option<i64>,
);

/// 基于同一 SqlitePool 的 Agent 存储。
#[derive(Clone)]
pub struct SqliteAgentStore {
    pool: SqlitePool,
    #[cfg(test)]
    commit_checkpoint: Arc<CommitCheckpoint>,
}

impl SqliteAgentStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            #[cfg(test)]
            commit_checkpoint: Arc::new(CommitCheckpoint::new()),
        }
    }

    /// 测试专用：arm 一次「事务已写入、commit 前」的暂停点，供取消回归
    /// 测试在单连接池上 abort 代表写事务。仅 test 构建存在，无行为影响。
    #[cfg(test)]
    pub(crate) fn arm_commit_checkpoint(
        &self,
        reached: oneshot::Sender<()>,
        released: oneshot::Receiver<()>,
    ) {
        self.commit_checkpoint.arm(reached, released);
    }
}

fn db_error(e: sqlx::Error) -> AgentStoreError {
    AgentStoreError::Database(StorageError::from(e))
}

/// primary 变体 → composite kind 裁决（CORE_V1_ERROR_HANDLING.md §7.2）：
/// `Database(StorageError)` 沿用嵌套 kind；业务变体（`NotFound` /
/// `IdempotencyConflict`）归 `Unknown`——内部事务完成失败分类，对外 500
/// `internal_error`，绝不声称业务冲突已可靠回滚。
fn primary_storage_kind(e: &AgentStoreError) -> StorageFailureKind {
    match e {
        AgentStoreError::Database(storage) => storage.kind(),
        _ => StorageFailureKind::Unknown,
    }
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> AgentStoreError {
    AgentStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn owned_from_row(
    (
        agent_id,
        kind,
        status,
        display_name,
        created_at,
        revoked_at,
        human_id,
        owner_status,
        owner_created_at,
        owner_revoked_at,
    ): OwnedRow,
) -> Result<OwnedAgent, AgentStoreError> {
    if kind != ParticipantKind::Agent.as_str() {
        return Err(corrupt(format!(
            "owner row points to non-agent participant {kind:?}"
        )));
    }
    let agent_id = ParticipantId(agent_id);
    Ok(OwnedAgent {
        agent: Agent {
            id: agent_id.clone(),
            kind: ParticipantKind::Agent,
            display_name: DisplayName::parse(&display_name)
                .map_err(|e| corrupt(format!("stored display name invalid: {e}")))?,
            status: ParticipantStatus::parse(&status)
                .ok_or_else(|| corrupt(format!("unknown participant status {status:?}")))?,
            created_at: UtcMillis::from_millis(created_at),
            revoked_at: revoked_at.map(UtcMillis::from_millis),
        },
        ownership: AgentOwner {
            agent_id,
            human_id: ParticipantId(human_id),
            status: ParticipantStatus::parse(&owner_status)
                .ok_or_else(|| corrupt(format!("unknown owner status {owner_status:?}")))?,
            created_at: UtcMillis::from_millis(owner_created_at),
            revoked_at: owner_revoked_at.map(UtcMillis::from_millis),
        },
    })
}

async fn fetch_owned(
    conn: &mut SqliteConnection,
    human_id: &str,
    agent_id: &str,
) -> Result<Option<OwnedAgent>, AgentStoreError> {
    let row: Option<OwnedRow> = sqlx::query_as(SQL_OWNED_BY_AGENT)
        .bind(human_id)
        .bind(agent_id)
        .fetch_optional(conn)
        .await
        .map_err(db_error)?;
    row.map(owned_from_row).transpose()
}

#[async_trait::async_trait]
impl crate::ports::agent::AgentStore for SqliteAgentStore {
    async fn create_agent(
        &self,
        human_id: &ParticipantId,
        agent: &NewAgent,
        ownership: &NewAgentOwnership,
        idempotency: &NewIdempotencyRecord,
    ) -> Result<CreateAgentOutcome, AgentStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<CreateAgentOutcome, AgentStoreError> = async {
            sqlx::query(SQL_INSERT_AGENT)
                .bind(agent.id.as_str())
                .bind(agent.display_name.as_str())
                .bind(agent.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_OWNER)
                .bind(ownership.agent_id.as_str())
                .bind(ownership.human_id.as_str())
                .bind(ownership.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            match sqlx::query(SQL_INSERT_IDEMPOTENCY)
                .bind(idempotency.human_id.as_str())
                .bind(idempotency.key.as_str())
                .bind(&idempotency.request_digest)
                .bind(idempotency.agent_id.as_str())
                .bind(idempotency.created_at.millis())
                .execute(&mut *tx)
                .await
            {
                Ok(_) => Ok(CreateAgentOutcome::Created),
                Err(sqlx::Error::Database(dberr)) if dberr.is_unique_violation() => {
                    let existing: (String, String) = sqlx::query_as(
                        "SELECT request_digest, agent_id FROM agent_create_idempotency \
                         WHERE human_id = ? AND key = ?",
                    )
                    .bind(idempotency.human_id.as_str())
                    .bind(idempotency.key.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    if existing.0 != idempotency.request_digest {
                        return Err(AgentStoreError::IdempotencyConflict);
                    }
                    let Some(owned) = fetch_owned(&mut tx, human_id.as_str(), &existing.1).await?
                    else {
                        return Err(corrupt("idempotency record points to missing agent"));
                    };
                    Ok(CreateAgentOutcome::Replayed {
                        agent: owned.agent,
                        ownership: owned.ownership,
                    })
                }
                Err(e) => Err(db_error(e)),
            }
        }
        .await;

        match outcome {
            Ok(CreateAgentOutcome::Created) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(CreateAgentOutcome::Created)
            }
            Ok(replayed @ CreateAgentOutcome::Replayed { .. }) => Ok(replayed),
            Err(e) => Err(e),
        }
    }

    async fn list_owned_agents(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<OwnedAgent>, AgentStoreError> {
        let rows: Vec<OwnedRow> = sqlx::query_as(
            "SELECT p.id, p.kind, p.status, p.display_name, p.created_at, p.revoked_at, \
                    o.human_id, o.status, o.created_at, o.revoked_at \
             FROM agent_owners o \
             JOIN participants p ON p.id = o.agent_id \
             WHERE o.human_id = ? \
             ORDER BY p.created_at ASC, p.id ASC",
        )
        .bind(human_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(owned_from_row).collect()
    }

    async fn get_owned_agent(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
    ) -> Result<Option<OwnedAgent>, AgentStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        fetch_owned(&mut conn, human_id.as_str(), agent_id.as_str()).await
    }

    async fn revoke_agent(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
    ) -> Result<Vec<MembershipId>, AgentStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<Vec<MembershipId>, AgentStoreError> = async {
            let owned: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM agent_owners WHERE human_id = ? AND agent_id = ? LIMIT 1",
            )
            .bind(human_id.as_str())
            .bind(agent_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if owned.is_none() {
                return Err(AgentStoreError::NotFound);
            }
            let revoked_at = UtcMillis::now().millis();
            sqlx::query(
                "UPDATE participants SET status = 'revoked', revoked_at = ? \
                 WHERE id = ? AND status = 'active'",
            )
            .bind(revoked_at)
            .bind(agent_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            sqlx::query(
                "UPDATE agent_owners SET status = 'revoked', revoked_at = ? \
                 WHERE agent_id = ? AND status = 'active'",
            )
            .bind(revoked_at)
            .bind(agent_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            // 本次撤销实际关闭的 Agent Membership 周期（agent_revoke
            // 传播触发器用 NEW.revoked_at 精确关闭；重复撤销不改 revoked_at，
            // 因此按 (participant, left_at) 查询精确对应本次事务）。
            let closed: Vec<String> = sqlx::query_scalar(
                "SELECT membership_id FROM room_memberships \
                 WHERE participant_id = ? AND closed_by = 'agent_revoked' AND left_at = ? \
                 ORDER BY membership_id",
            )
            .bind(agent_id.as_str())
            .bind(revoked_at)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_error)?;
            Ok(closed.into_iter().map(MembershipId).collect())
        }
        .await;

        match outcome {
            Ok(closed) => {
                tx.commit().await.map_err(db_error)?;
                Ok(closed)
            }
            Err(e) => {
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, AgentStoreError::Database).await)
            }
        }
    }
}

/// 测试专用取消 seam（仅 test 构建编译，生产构建零存在）：arm 后，下一次
/// 代表写事务在「已写入、commit 前」通知测试并挂起；取消回归测试借此在
/// 事务仍打开时 abort future，验证 Transaction Drop 回滚与连接复用。
#[cfg(test)]
struct CommitCheckpoint {
    pair: std::sync::Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

#[cfg(test)]
impl CommitCheckpoint {
    fn new() -> Self {
        Self {
            pair: std::sync::Mutex::new(None),
        }
    }

    fn arm(&self, reached: oneshot::Sender<()>, released: oneshot::Receiver<()>) {
        *self.pair.lock().unwrap() = Some((reached, released));
    }

    async fn pause(&self) {
        let Some((reached, released)) = self.pair.lock().unwrap().take() else {
            return;
        };
        let _ = reached.send(());
        let _ = released.await;
    }
}
