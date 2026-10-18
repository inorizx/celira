//! SQLite 身份与认证 adapter：实现 `ports::identity::IdentityStore`。
//!
//! 本模块是身份事实唯一允许接触 SQLite 文件的地方。HTTP handler 和用例
//! 不得直接操作数据库，只能通过端口调用。
//!
//! bootstrap 使用 `BEGIN IMMEDIATE` 显式事务：guard 以主键约束
//! （`bootstrap_guard.id` 只能为 1）在数据库层面保证并发唯一；事务由
//! SQLx Transaction 持有生命周期，任何失败或 Future 取消都由 Drop 回滚，
//! 不留下半个 Human 或半个 Session，也不把未结束事务带回连接池。

#[cfg(test)]
use std::sync::Arc;

use sqlx::Connection;
use sqlx::sqlite::SqlitePool;

use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::{
    DisplayName, Participant, ParticipantId, ParticipantKind, ParticipantStatus,
};
use crate::domain::time::UtcMillis;
use crate::ports::identity::{AuthContext, NewHuman, NewSession, StoreError};
use crate::ports::storage::{StorageError, StorageFailureKind};

#[cfg(test)]
use tokio::sync::oneshot;

const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

const SQL_INSERT_GUARD: &str = "INSERT INTO bootstrap_guard (id) VALUES (1)";

const SQL_INSERT_PARTICIPANT: &str = "INSERT INTO participants (id, kind, status, display_name, created_at) \
                                      VALUES (?, 'human', 'active', ?, ?)";

const SQL_INSERT_SESSION: &str = "INSERT INTO human_sessions (id, human_id, token_hash, created_at) \
                                  VALUES (?, ?, ?, ?)";

/// 基于同一 SqlitePool 的身份存储。
#[derive(Clone)]
pub struct SqliteIdentityStore {
    pool: SqlitePool,
    #[cfg(test)]
    commit_checkpoint: Arc<CommitCheckpoint>,
}

impl SqliteIdentityStore {
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

fn db_error(e: sqlx::Error) -> StoreError {
    StoreError::Database(StorageError::from(e))
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> StoreError {
    StoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

#[async_trait::async_trait]
impl crate::ports::identity::IdentityStore for SqliteIdentityStore {
    async fn bootstrap_first_human(
        &self,
        human: &NewHuman,
        session: &NewSession,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            match sqlx::query(SQL_INSERT_GUARD).execute(&mut *tx).await {
                Err(sqlx::Error::Database(dberr)) if dberr.is_unique_violation() => {
                    return Err(StoreError::AlreadyBootstrapped);
                }
                Err(e) => return Err(db_error(e)),
                Ok(_) => {}
            }
            sqlx::query(SQL_INSERT_PARTICIPANT)
                .bind(human.id.as_str())
                .bind(human.display_name.as_str())
                .bind(human.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_SESSION)
                .bind(session.id.as_str())
                .bind(session.human_id.as_str())
                .bind(session.token_hash.hex())
                .bind(session.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn issue_human_session(&self, session: &NewSession) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT kind, status FROM participants WHERE id = ?")
                    .bind(session.human_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some((kind, status)) = row else {
                return Err(StoreError::UnknownHuman);
            };
            if kind != ParticipantKind::Human.as_str()
                || status != ParticipantStatus::Active.as_str()
            {
                return Err(StoreError::HumanNotActive);
            }
            sqlx::query(SQL_INSERT_SESSION)
                .bind(session.id.as_str())
                .bind(session.human_id.as_str())
                .bind(session.token_hash.hex())
                .bind(session.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                tx.commit().await.map_err(db_error)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn create_human(&self, human: &NewHuman, session: &NewSession) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            sqlx::query(SQL_INSERT_PARTICIPANT)
                .bind(human.id.as_str())
                .bind(human.display_name.as_str())
                .bind(human.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_SESSION)
                .bind(session.id.as_str())
                .bind(session.human_id.as_str())
                .bind(session.token_hash.hex())
                .bind(session.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                tx.commit().await.map_err(db_error)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn authenticate(
        &self,
        token_hash: &TokenHash,
    ) -> Result<Option<AuthContext>, StoreError> {
        let row: Option<(String, String, String, String, i64, String, i64)> = sqlx::query_as(
            "SELECT p.id, p.kind, p.status, p.display_name, p.created_at, \
                    s.id, s.created_at \
             FROM human_sessions s \
             JOIN participants p ON p.id = s.human_id \
             WHERE s.token_hash = ? AND s.revoked_at IS NULL AND p.status = 'active'",
        )
        .bind(token_hash.hex())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(
            |(
                id,
                kind,
                status,
                display_name,
                participant_created_at,
                session_id,
                session_created_at,
            )| {
                let participant = Participant {
                    id: ParticipantId(id),
                    kind: ParticipantKind::parse(&kind)
                        .ok_or_else(|| corrupt(format!("unknown kind {kind:?}")))?,
                    display_name: DisplayName::parse(&display_name)
                        .map_err(|e| corrupt(format!("stored display name invalid: {e}")))?,
                    status: ParticipantStatus::parse(&status)
                        .ok_or_else(|| corrupt(format!("unknown status {status:?}")))?,
                    created_at: UtcMillis::from_millis(participant_created_at),
                };
                Ok(AuthContext {
                    session: HumanSession {
                        id: SessionId(session_id),
                        human_id: participant.id.clone(),
                        token_hash: *token_hash,
                        created_at: UtcMillis::from_millis(session_created_at),
                        revoked_at: None,
                    },
                    participant,
                })
            },
        )
        .transpose()
    }

    async fn revoke_session(&self, session_id: &SessionId) -> Result<(), StoreError> {
        sqlx::query("UPDATE human_sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(UtcMillis::now().millis())
            .bind(session_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    /// **改自己的名字**（表达权第四刀，2026-10-17）：只改 `display_name` 这一列。
    ///
    /// **状态写**：没有版本行、没有幂等记录 —— 同值重写天然幂等。
    /// 判据放在 `WHERE` 里（`kind = 'human' AND status = 'active'`）：目标不是 active Human
    /// 时**零行受影响** ⇒ 明确报错，绝不静默成功。其余列（`kind` / `status` / `revoked_at` /
    /// `created_at`）一个字都不动 —— 表上那三道不可改写守卫本来也只管前三列。
    async fn rename_human(
        &self,
        human_id: &ParticipantId,
        display_name: &DisplayName,
    ) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "UPDATE participants SET display_name = ? \
             WHERE id = ? AND kind = 'human' AND status = 'active'",
        )
        .bind(display_name.as_str())
        .bind(human_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_error)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::HumanNotActive);
        }
        Ok(())
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
