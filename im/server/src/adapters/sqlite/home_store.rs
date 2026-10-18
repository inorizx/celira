//! Home、Pairing Grant 与 Session 管理的 SQLite adapter。
//!
//! 契约（CORE_V1_IM_CONTRACT.md §3.1）：
//! - Home Identity 初始化：home_guard（id=1 唯一行）+ home 行同一事务
//!   `BEGIN IMMEDIATE` 提交；guard 冲突即已初始化，不留下半个 Home 行；
//! - Grant 签发：签发事务内校验 issued_by 是 active Human；
//! - 兑换：同一 `BEGIN IMMEDIATE` 事务内裁决 active 未过期 Grant →
//!   创建独立 Human Session（human == grant.issued_by）→ 建立 1:1
//!   Session Metadata → 标记 Grant consumed；任何持久化失败整体回滚且
//!   Grant 仍可重试；已消费重放统一 `GrantInvalid`；
//! - 全部写入为单写事务，先读后写同一事务（CORE_V1_IM_CONTRACT.md §11）。

use sqlx::Connection;
use sqlx::sqlite::SqlitePool;

use crate::domain::home::{GrantId, GrantKind, GrantStatus, Home, NewGrant, PairingGrant};
use crate::domain::human_session::{SessionId, TokenHash};
use crate::domain::pairing_transaction::{
    PairingTransaction, TransactionId, TransactionStatus, TransactionView,
};
use crate::domain::participant::{DisplayName, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::ports::home::{
    AbortTransactionCommand, CommitTransactionCommand, ExchangeOutcome, ExchangeRequest, HomeStore,
    NewHome, PrepareTransactionRequest, SessionListItem, StoreError,
};
use crate::ports::storage::{StorageError, StorageFailureKind};

#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use tokio::sync::oneshot;

/// 写入一律 `BEGIN IMMEDIATE` 单写事务（CORE_V1_IM_CONTRACT.md §11）。
const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

/// 基于同一 SqlitePool 的 Home 存储。
#[derive(Clone)]
pub struct SqliteHomeStore {
    pool: SqlitePool,
    #[cfg(test)]
    commit_checkpoint: Arc<CommitCheckpoint>,
}

impl SqliteHomeStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            #[cfg(test)]
            commit_checkpoint: Arc::new(CommitCheckpoint::new()),
        }
    }

    /// 测试专用：arm 一次「事务已写入、commit 前」的暂停点，供回滚回归
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

const SQL_INSERT_GUARD: &str = "INSERT INTO home_guard (id) VALUES (1)";
const SQL_INSERT_HOME: &str = "INSERT INTO home (id, home_id, display_name, created_at) \
                               VALUES (1, ?, ?, ?)";
const SQL_SELECT_HOME: &str = "SELECT home_id, display_name, created_at FROM home WHERE id = 1";
const SQL_INSERT_GRANT: &str = "INSERT INTO pairing_grants \
    (grant_id, token_hash, issued_by, device_label, grants_human, status, issued_at, expires_at) \
    VALUES (?, ?, ?, ?, ?, 'active', ?, ?)";
const SQL_SELECT_GRANT_BY_HASH: &str = "SELECT grant_id, issued_by, device_label, grants_human, \
                                        status, issued_at, expires_at, consumed_at, revoked_at \
                                        FROM pairing_grants WHERE token_hash = ?";
const SQL_SELECT_GRANT_BY_ID: &str = "SELECT grant_id, issued_by, device_label, grants_human, \
                                      status, issued_at, expires_at, consumed_at, revoked_at \
                                      FROM pairing_grants WHERE grant_id = ? AND issued_by = ?";
const SQL_LIST_GRANTS: &str = "SELECT grant_id, issued_by, device_label, grants_human, status, \
                               issued_at, expires_at, consumed_at, revoked_at FROM pairing_grants \
                               WHERE issued_by = ? AND status <> 'consumed' ORDER BY issued_at";
const SQL_CONSUME_GRANT: &str = "UPDATE pairing_grants SET status = 'consumed', consumed_at = ? \
                                 WHERE grant_id = ? AND status = 'active' AND expires_at > ?";
const SQL_REVOKE_GRANT: &str = "UPDATE pairing_grants SET status = 'revoked', revoked_at = ? \
                                WHERE grant_id = ? AND issued_by = ? AND status = 'active'";
const SQL_NAME_INVITED_HUMAN: &str = "UPDATE pairing_grants SET invited_human_id = ? \
                                      WHERE grant_id = ? AND invited_human_id IS NULL";
const SQL_INSERT_NEW_HUMAN: &str = "INSERT INTO participants \
                                    (id, kind, status, display_name, created_at, revoked_at) \
                                    VALUES (?, 'human', 'active', ?, ?, NULL)";
const SQL_INSERT_SESSION: &str = "INSERT INTO human_sessions (id, human_id, token_hash, \
                                  created_at) VALUES (?, ?, ?, ?)";
const SQL_INSERT_SESSION_METADATA: &str = "INSERT INTO session_metadata (session_id, \
                                           device_label, source_grant_id) VALUES (?, ?, ?)";
const SQL_LIST_SESSIONS: &str = "SELECT s.id, s.created_at, s.revoked_at, m.device_label, \
                                 m.source_grant_id FROM human_sessions s \
                                 LEFT JOIN session_metadata m ON m.session_id = s.id \
                                 WHERE s.human_id = ? ORDER BY s.created_at, s.id";
const SQL_REVOKE_SESSION: &str = "UPDATE human_sessions SET revoked_at = ? \
                                  WHERE id = ? AND human_id = ? AND revoked_at IS NULL";

// ===== A2 pairing_transactions 语句组 =====
const SQL_INSERT_TRANSACTION: &str = "INSERT INTO pairing_transactions \
    (transaction_id, grant_id, client_nonce, session_token_hash, commit_token_hash, \
     issued_by, reserved_session_id, status, prepared_at, expires_at) \
    VALUES (?, ?, ?, ?, ?, ?, ?, 'prepared', ?, ?)";
const SQL_SELECT_TRANSACTION_BY_COMMIT_HASH: &str = "SELECT transaction_id, grant_id, \
    client_nonce, session_token_hash, commit_token_hash, issued_by, reserved_session_id, \
    status, prepared_at, expires_at, committed_at, aborted_at \
    FROM pairing_transactions WHERE commit_token_hash = ?";
const SQL_SELECT_TRANSACTION_BY_ID_AND_COMMIT_HASH: &str = "SELECT transaction_id, grant_id, \
    client_nonce, session_token_hash, commit_token_hash, issued_by, reserved_session_id, \
    status, prepared_at, expires_at, committed_at, aborted_at \
    FROM pairing_transactions WHERE transaction_id = ? AND commit_token_hash = ?";
const SQL_UPDATE_TRANSACTION_COMMITTED: &str = "UPDATE pairing_transactions \
    SET status = 'committed', committed_at = ? \
    WHERE transaction_id = ? AND status = 'prepared' AND expires_at > ?";
const SQL_UPDATE_TRANSACTION_ABORTED: &str = "UPDATE pairing_transactions \
    SET status = 'aborted', aborted_at = ? \
    WHERE transaction_id = ? AND status = 'prepared' AND expires_at > ?";

#[async_trait::async_trait]
impl HomeStore for SqliteHomeStore {
    async fn init_home(&self, home: &NewHome) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            match sqlx::query(SQL_INSERT_GUARD).execute(&mut *tx).await {
                Err(sqlx::Error::Database(dberr)) if dberr.is_unique_violation() => {
                    return Err(StoreError::AlreadyInitialized);
                }
                Err(e) => return Err(db_error(e)),
                Ok(_) => {}
            }
            sqlx::query(SQL_INSERT_HOME)
                .bind(home.home_id.as_str())
                .bind(home.display_name.as_ref().map(|n| n.as_str()))
                .bind(home.created_at.millis())
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

    async fn get_home(&self) -> Result<Option<Home>, StoreError> {
        let row: Option<(String, Option<String>, i64)> = sqlx::query_as(SQL_SELECT_HOME)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map(|(home_id, display_name, created_at)| {
            let display_name = display_name
                .map(|raw| {
                    DisplayName::parse(&raw)
                        .map_err(|e| corrupt(format!("home display name invalid: {e}")))
                })
                .transpose()?;
            Ok(Home {
                home_id: crate::domain::home::HomeId(home_id),
                display_name,
                created_at: UtcMillis::from_millis(created_at),
            })
        })
        .transpose()
    }

    async fn issue_grant(&self, grant: &NewGrant) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT kind, status FROM participants WHERE id = ?")
                    .bind(grant.issued_by.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some((kind, status)) = row else {
                return Err(StoreError::UnknownHuman);
            };
            if kind != "human" || status != "active" {
                return Err(StoreError::HumanNotActive);
            }
            sqlx::query(SQL_INSERT_GRANT)
                .bind(grant.grant_id.as_str())
                .bind(grant.token_hash.hex())
                .bind(grant.issued_by.as_str())
                .bind(grant.device_label.as_ref().map(|n| n.as_str()))
                .bind(i64::from(grant.kind.grants_human()))
                .bind(grant.issued_at.millis())
                .bind(grant.expires_at.millis())
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

    async fn exchange_grant(
        &self,
        exchange: &ExchangeRequest,
        now: UtcMillis,
    ) -> Result<ExchangeOutcome, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<ExchangeOutcome, StoreError> = async {
            let row: Option<GrantRow> = sqlx::query_as(SQL_SELECT_GRANT_BY_HASH)
                .bind(exchange.grant_token_hash.hex())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
            let Some(grant) = row else {
                return Err(StoreError::GrantInvalid);
            };
            let grant = grant.into_domain()?;
            if !grant.is_exchangeable_at(now) {
                return Err(StoreError::GrantInvalid);
            }
            // 会话归谁，由 **Grant 的用途**决定（调用方指定不了）：
            // - self_device：签发者本人（既有语义，一字不动）；
            // - new_human：本次兑换**新建的那个 Human**——名字由被邀请者
            //   自己给（缺失/多余都在这里 fail closed，且**不消费** Grant）。
            let human_id = match grant.kind {
                GrantKind::SelfDevice => {
                    if exchange.new_human_display_name.is_some() {
                        return Err(StoreError::UnexpectedNewHumanName);
                    }
                    grant.issued_by.clone()
                }
                GrantKind::NewHuman => {
                    let Some(new_human_display_name) = exchange.new_human_display_name.as_ref()
                    else {
                        return Err(StoreError::NewHumanNameRequired);
                    };
                    let new_human_id =
                        crate::domain::participant::ParticipantId(uuid::Uuid::new_v4().to_string());
                    sqlx::query(SQL_INSERT_NEW_HUMAN)
                        .bind(new_human_id.as_str())
                        .bind(new_human_display_name.as_str())
                        .bind(exchange.created_at.millis())
                        .execute(&mut *tx)
                        .await
                        .map_err(db_error)?;
                    // 把"这张邀请码点名了谁"写进 Grant 行：`session_metadata`
                    // 的来源守卫只认 `issued_by` 或**这里点名的那个**——守卫
                    // 因此仍然可证明（一张码最多给一个人铸会话，且必须是它
                    // 在同一事务里新建的那个人）。
                    let named = sqlx::query(SQL_NAME_INVITED_HUMAN)
                        .bind(new_human_id.as_str())
                        .bind(grant.grant_id.as_str())
                        .execute(&mut *tx)
                        .await
                        .map_err(db_error)?;
                    if named.rows_affected() != 1 {
                        // 已点过名（不可能：同一 Grant 只能被消费一次）或
                        // 竞态 ⇒ 整体回滚，绝不留下半个新人。
                        return Err(StoreError::GrantInvalid);
                    }
                    new_human_id
                }
            };
            let home: Option<(String, Option<String>, i64)> = sqlx::query_as(SQL_SELECT_HOME)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
            let Some((home_id, display_name, home_created_at)) = home else {
                return Err(corrupt(
                    "home identity missing while exchange transaction is in flight",
                ));
            };
            let display_name = display_name
                .map(|raw| {
                    DisplayName::parse(&raw)
                        .map_err(|e| corrupt(format!("home display name invalid: {e}")))
                })
                .transpose()?;
            let session_created_at = exchange.created_at;
            sqlx::query(SQL_INSERT_SESSION)
                .bind(exchange.session_id.as_str())
                .bind(human_id.as_str())
                .bind(exchange.session_token_hash.hex())
                .bind(session_created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_SESSION_METADATA)
                .bind(exchange.session_id.as_str())
                .bind(exchange.device_label.as_ref().map(|n| n.as_str()))
                .bind(grant.grant_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            let consumed = sqlx::query(SQL_CONSUME_GRANT)
                .bind(now.millis())
                .bind(grant.grant_id.as_str())
                .bind(now.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            if consumed.rows_affected() != 1 {
                // 前置读取与最终写入必须共同证明一次性消费；任何竞态或
                // 过期边界变化都回滚本次已写入的 Session/Metadata。
                return Err(StoreError::GrantInvalid);
            }
            Ok(ExchangeOutcome {
                session_id: exchange.session_id.clone(),
                issued_by: grant.issued_by.clone(),
                invited_new_human: matches!(grant.kind, GrantKind::NewHuman),
                human_id,
                session_created_at,
                grant_id: grant.grant_id.clone(),
                home: Home {
                    home_id: crate::domain::home::HomeId(home_id),
                    display_name,
                    created_at: UtcMillis::from_millis(home_created_at),
                },
            })
        }
        .await;

        match outcome {
            Ok(inner) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(inner)
            }
            Err(e) => Err(e),
        }
    }

    async fn list_grants(&self, human_id: &ParticipantId) -> Result<Vec<PairingGrant>, StoreError> {
        let rows: Vec<GrantRow> = sqlx::query_as(SQL_LIST_GRANTS)
            .bind(human_id.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;
        rows.into_iter().map(|row| row.into_domain()).collect()
    }

    async fn revoke_grant(
        &self,
        human_id: &ParticipantId,
        grant_id: &GrantId,
        now: UtcMillis,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            let row: Option<GrantRow> = sqlx::query_as(SQL_SELECT_GRANT_BY_ID)
                .bind(grant_id.as_str())
                .bind(human_id.as_str())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
            let Some(grant) = row else {
                return Err(StoreError::GrantNotFound);
            };
            let grant = grant.into_domain()?;
            match grant.status {
                GrantStatus::Consumed => return Err(StoreError::GrantConsumed),
                GrantStatus::Revoked => return Ok(()),
                GrantStatus::Active | GrantStatus::Expired => {}
            }
            sqlx::query(SQL_REVOKE_GRANT)
                .bind(now.millis())
                .bind(grant_id.as_str())
                .bind(human_id.as_str())
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

    async fn list_sessions(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<SessionListItem>, StoreError> {
        let rows: Vec<(String, i64, Option<i64>, Option<String>, Option<String>)> =
            sqlx::query_as(SQL_LIST_SESSIONS)
                .bind(human_id.as_str())
                .fetch_all(&self.pool)
                .await
                .map_err(db_error)?;
        rows.into_iter()
            .map(
                |(session_id, created_at, revoked_at, device_label, source_grant_id)| {
                    let device_label = device_label
                        .map(|raw| {
                            DisplayName::parse(&raw).map_err(|e| {
                                corrupt(format!("session metadata device label invalid: {e}"))
                            })
                        })
                        .transpose()?;
                    let source_grant_id = source_grant_id
                        .map(|raw| {
                            GrantId::parse_stable(&raw).ok_or_else(|| {
                                corrupt(format!(
                                    "session metadata source grant id invalid: {raw:?}"
                                ))
                            })
                        })
                        .transpose()?;
                    Ok(SessionListItem {
                        session_id: SessionId(session_id),
                        created_at: UtcMillis::from_millis(created_at),
                        revoked_at: revoked_at.map(UtcMillis::from_millis),
                        device_label,
                        source_grant_id,
                    })
                },
            )
            .collect()
    }

    async fn revoke_owned_session(
        &self,
        human_id: &ParticipantId,
        session_id: &SessionId,
        now: UtcMillis,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            let owned: Option<(i64,)> =
                sqlx::query_as("SELECT COUNT(*) FROM human_sessions WHERE id = ? AND human_id = ?")
                    .bind(session_id.as_str())
                    .bind(human_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            // SELECT 恒返回一行（COUNT 聚合）。
            if owned.map(|(count,)| count).unwrap_or(0) == 0 {
                return Err(StoreError::SessionNotFound);
            }
            sqlx::query(SQL_REVOKE_SESSION)
                .bind(now.millis())
                .bind(session_id.as_str())
                .bind(human_id.as_str())
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

    async fn prepare_transaction(
        &self,
        request: &PrepareTransactionRequest,
        now: UtcMillis,
    ) -> Result<TransactionView, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<TransactionView, StoreError> = async {
            // 1. 裁决 Grant（token 摘要直接命中；未知/已用/撤销/过期 →
            //    `GrantInvalid`，不泄露存在性）。
            let grant: Option<GrantRow> = sqlx::query_as(SQL_SELECT_GRANT_BY_HASH)
                .bind(request.grant_token_hash.hex())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
            let Some(grant) = grant else {
                return Err(StoreError::GrantInvalid);
            };
            let grant = grant.into_domain()?;
            if !grant.is_exchangeable_at(now) {
                return Err(StoreError::GrantInvalid);
            }
            // **邀请类 Grant 不走两阶段路径**（本切片明确不做）：那条路径是
            // 设备配对的事务化形态，会话归 `issued_by`；邀请要新建一个人，
            // 而事务表里没有"被邀请者自己给的名字"这一格。这里 fail-closed
            // 地拒绝，**绝不静默按设备配对铸一支别人的会话**。
            if grant.kind == GrantKind::NewHuman {
                return Err(StoreError::GrantInvalid);
            }
            let home = load_home_for_view(&mut tx).await?;

            // 2. 以 grant + nonce 定位既有事务（幂等重试）：同一 nonce +
            //    同一组摘要落在同一事务（返回既有）；同一 nonce 但摘要
            //    不一致 → `TransactionNotFound`（参数不变量冲突，统一
            //    `pairing_transaction_invalid`，不泄露存在性）。
            let existing: Option<(String, String, String, i64, String)> = sqlx::query_as(
                "SELECT session_token_hash, commit_token_hash, status, expires_at, \
                        transaction_id \
                 FROM pairing_transactions WHERE grant_id = ? AND client_nonce = ?",
            )
            .bind(grant.grant_id.as_str())
            .bind(&request.client_nonce)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;

            // 3. 幂等重用或创建新事务。
            let db_row: TransactionRow = match existing {
                Some((stored_session, stored_commit, _status, _expires, transaction_id)) => {
                    if stored_session != request.session_token_hash.hex()
                        || stored_commit != request.commit_token_hash.hex()
                    {
                        return Err(StoreError::TransactionNotFound);
                    }
                    let row: Option<TransactionRow> =
                        sqlx::query_as(SQL_SELECT_TRANSACTION_BY_COMMIT_HASH)
                            .bind(&stored_commit)
                            .fetch_optional(&mut *tx)
                            .await
                            .map_err(db_error)?;
                    let _ = transaction_id;
                    row.ok_or_else(|| corrupt("pairing transaction row vanished mid-flight"))?
                }
                None => {
                    // 创建新事务（prepared / active 起止，未消费 Grant）。
                    sqlx::query(SQL_INSERT_TRANSACTION)
                        .bind(request.transaction_id.as_str())
                        .bind(grant.grant_id.as_str())
                        .bind(&request.client_nonce)
                        .bind(request.session_token_hash.hex())
                        .bind(request.commit_token_hash.hex())
                        .bind(grant.issued_by.as_str())
                        .bind(request.reserved_session_id.as_str())
                        .bind(request.prepared_at.millis())
                        .bind(request.expires_at.millis())
                        .execute(&mut *tx)
                        .await
                        .map_err(db_error)?;
                    TransactionRow {
                        transaction_id: request.transaction_id.as_str().to_owned(),
                        grant_id: grant.grant_id.as_str().to_owned(),
                        session_token_hash: request.session_token_hash.hex(),
                        issued_by: grant.issued_by.as_str().to_owned(),
                        reserved_session_id: request.reserved_session_id.as_str().to_owned(),
                        status: "prepared".to_owned(),
                        prepared_at: request.prepared_at.millis(),
                        expires_at: request.expires_at.millis(),
                        committed_at: None,
                        aborted_at: None,
                    }
                }
            };

            // 4. 组装非秘密视图（status 按现在派生 expired）。
            db_row.into_view(home)
        }
        .await;

        match outcome {
            Ok(view) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(view)
            }
            Err(e) => Err(e),
        }
    }

    async fn get_transaction_status(
        &self,
        transaction_id: &TransactionId,
        commit_token_hash: &TokenHash,
        now: UtcMillis,
    ) -> Result<Option<TransactionView>, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let row: Option<TransactionRow> =
            sqlx::query_as(SQL_SELECT_TRANSACTION_BY_ID_AND_COMMIT_HASH)
                .bind(transaction_id.as_str())
                .bind(commit_token_hash.hex())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
        let Some(txn) = row else {
            return Err(StoreError::TransactionNotFound);
        };
        let domain = txn.into_domain()?;
        // 已过期事务对 status 同样 `TransactionNotFound`（不提供恢复，
        // 与 commit/abort 同一 redaction 语义）。committed/aborted 是
        // Host 已收敛的终态，仍可查看。
        if domain.status == TransactionStatus::Prepared && !domain.is_committable_at(now) {
            return Err(StoreError::TransactionNotFound);
        }
        let view = view_from_transaction(&mut tx, domain).await?;
        let _ = tx.rollback().await;
        Ok(Some(view))
    }

    async fn commit_transaction(
        &self,
        command: &CommitTransactionCommand,
        now: UtcMillis,
    ) -> Result<TransactionView, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<TransactionView, StoreError> = async {
            // 1. 凭 commit token 摘要裁决事务（未知/token 不匹配/过期/
            //    aborted 统一 NotFound）。
            let row: Option<TransactionRow> =
                sqlx::query_as(SQL_SELECT_TRANSACTION_BY_ID_AND_COMMIT_HASH)
                    .bind(command.transaction_id.as_str())
                    .bind(command.commit_token_hash.hex())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some(txn) = row else {
                return Err(StoreError::TransactionNotFound);
            };
            let domain = txn.clone().into_domain()?;
            // 已 committed 重放：返回既有视图（幂等成功），绝不二次消费。
            if domain.status == TransactionStatus::Committed {
                return view_from_transaction(&mut tx, domain).await;
            }
            if !domain.is_committable_at(now) {
                return Err(StoreError::TransactionNotFound);
            }

            // 2. 以 prepare 阶段提交的 session_token_hash 创建正式
            //    Human Session（token 原地激活）。
            sqlx::query(SQL_INSERT_SESSION)
                .bind(domain.reserved_session_id.as_str())
                .bind(domain.issued_by.as_str())
                .bind(txn.session_token_hash.clone())
                .bind(command.committed_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            // 3. Session Metadata：来源 Grant 必须与事务 issued_by 同一
            //    Human（数据库触发器兜底）。
            sqlx::query(SQL_INSERT_SESSION_METADATA)
                .bind(domain.reserved_session_id.as_str())
                .bind(command.device_label.as_ref().map(|n| n.as_str()))
                .bind(domain.grant_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            // 4. 标记事务 committed；幂等边界（并发 commit 线性化后
            //    已变终态）整体回滚。
            let updated = sqlx::query(SQL_UPDATE_TRANSACTION_COMMITTED)
                .bind(command.committed_at.millis())
                .bind(domain.transaction_id.as_str())
                .bind(now.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            if updated.rows_affected() != 1 {
                return Err(StoreError::TransactionNotFound);
            }

            // 5. 消费 Pairing Grant（与 Exchange 同一一次性语义）。
            let consumed = sqlx::query(SQL_CONSUME_GRANT)
                .bind(now.millis())
                .bind(domain.grant_id.as_str())
                .bind(now.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            if consumed.rows_affected() != 1 {
                return Err(StoreError::TransactionNotFound);
            }

            // 6. 组装已提交视图（committed_at = now）。
            let mut view = view_from_transaction(&mut tx, domain).await?;
            view.status = TransactionStatus::Committed;
            view.committed_at = Some(command.committed_at);
            Ok(view)
        }
        .await;

        match outcome {
            Ok(view) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(view)
            }
            Err(e) => Err(e),
        }
    }

    async fn abort_transaction(
        &self,
        command: &AbortTransactionCommand,
        now: UtcMillis,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), StoreError> = async {
            let row: Option<TransactionRow> =
                sqlx::query_as(SQL_SELECT_TRANSACTION_BY_ID_AND_COMMIT_HASH)
                    .bind(command.transaction_id.as_str())
                    .bind(command.commit_token_hash.hex())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some(txn) = row else {
                return Err(StoreError::TransactionNotFound);
            };
            let domain = txn.into_domain()?;
            // 已 aborted + 同一 transaction_id/commit token 是 Host 超时
            // 重试的正常路径：幂等成功，不再次写入或改变审计时间。
            if domain.status == TransactionStatus::Aborted {
                return Ok(());
            }
            // 未知 / token 不匹配 / 已过期 / 已 committed 统一
            // `TransactionNotFound`（不泄露状态）。
            if domain.status != TransactionStatus::Prepared || !domain.is_committable_at(now) {
                return Err(StoreError::TransactionNotFound);
            }
            let updated = sqlx::query(SQL_UPDATE_TRANSACTION_ABORTED)
                .bind(command.aborted_at.millis())
                .bind(domain.transaction_id.as_str())
                .bind(now.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            if updated.rows_affected() != 1 {
                // 幂等边界竞态：重复 abort / 并发 commit 线性化后状态已变。
                return Err(StoreError::TransactionNotFound);
            }
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
}

/// pairing_transactions 行原始投影。
#[derive(Debug, Clone, sqlx::FromRow)]
struct TransactionRow {
    transaction_id: String,
    grant_id: String,
    session_token_hash: String,
    issued_by: String,
    reserved_session_id: String,
    status: String,
    prepared_at: i64,
    expires_at: i64,
    committed_at: Option<i64>,
    aborted_at: Option<i64>,
}

impl TransactionRow {
    fn into_domain(self) -> Result<PairingTransaction, StoreError> {
        let status = TransactionStatus::parse(&self.status).ok_or_else(|| {
            corrupt(format!(
                "unknown pairing transaction status {:?}",
                self.status
            ))
        })?;
        Ok(PairingTransaction {
            transaction_id: TransactionId(self.transaction_id),
            grant_id: GrantId(self.grant_id),
            reserved_session_id: SessionId(self.reserved_session_id),
            issued_by: ParticipantId(self.issued_by),
            status,
            prepared_at: UtcMillis::from_millis(self.prepared_at),
            expires_at: UtcMillis::from_millis(self.expires_at),
            committed_at: self.committed_at.map(UtcMillis::from_millis),
            aborted_at: self.aborted_at.map(UtcMillis::from_millis),
        })
    }

    /// 组装非秘密视图（含派生过期状态）。
    fn into_view(self, home: Home) -> Result<TransactionView, StoreError> {
        let domain = self.into_domain()?;
        Ok(TransactionView {
            status: domain.status,
            home_id: home.home_id,
            transaction_id: domain.transaction_id,
            reserved_session_id: domain.reserved_session_id,
            human_id: domain.issued_by,
            prepared_at: domain.prepared_at,
            expires_at: domain.expires_at,
            committed_at: domain.committed_at,
            aborted_at: domain.aborted_at,
        })
    }
}

/// 读取并解析 home 行（prepared 事务存在但 home 缺失即内部不变量破坏）。
async fn load_home_for_view(tx: &mut sqlx::SqliteConnection) -> Result<Home, StoreError> {
    let row: Option<(String, Option<String>, i64)> = sqlx::query_as(SQL_SELECT_HOME)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
    let Some((home_id, display_name, created_at)) = row else {
        return Err(corrupt(
            "home identity missing while pairing transaction is in flight",
        ));
    };
    let display_name = display_name
        .map(|raw| {
            DisplayName::parse(&raw).map_err(|e| corrupt(format!("home display name invalid: {e}")))
        })
        .transpose()?;
    Ok(Home {
        home_id: crate::domain::home::HomeId(home_id),
        display_name,
        created_at: UtcMillis::from_millis(created_at),
    })
}

/// 组装 commit/status 的事务视图：按 id 读取领域事务 → 派生过期状态 →
/// 组装非秘密视图。
async fn view_from_transaction(
    tx: &mut sqlx::SqliteConnection,
    domain: PairingTransaction,
) -> Result<TransactionView, StoreError> {
    let home = match load_home_for_view(tx).await {
        Ok(home) => home,
        Err(_) => {
            return Err(corrupt(
                "home identity missing while reading pairing transaction",
            ));
        }
    };
    let status = domain.status_at(UtcMillis::now());
    Ok(TransactionView {
        status,
        home_id: home.home_id,
        transaction_id: domain.transaction_id,
        reserved_session_id: domain.reserved_session_id,
        human_id: domain.issued_by,
        prepared_at: domain.prepared_at,
        expires_at: domain.expires_at,
        committed_at: domain.committed_at,
        aborted_at: domain.aborted_at,
    })
}

/// pairing_grants 行原始投影。
#[derive(Debug, Clone, sqlx::FromRow)]
struct GrantRow {
    grant_id: String,
    issued_by: String,
    device_label: Option<String>,
    grants_human: i64,
    status: String,
    issued_at: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
    revoked_at: Option<i64>,
}

impl GrantRow {
    fn into_domain(self) -> Result<PairingGrant, StoreError> {
        let device_label = self
            .device_label
            .map(|raw| {
                DisplayName::parse(&raw).map_err(|e| corrupt(format!("device label invalid: {e}")))
            })
            .transpose()?;
        let status = GrantStatus::parse(&self.status)
            .ok_or_else(|| corrupt(format!("unknown pairing grant status {:?}", self.status)))?;
        Ok(PairingGrant {
            grant_id: GrantId(self.grant_id),
            issued_by: ParticipantId(self.issued_by),
            device_label,
            // 列由 CHECK 保证是 0/1；读到别的值就是损坏，不当成 false 混过去。
            kind: match self.grants_human {
                0 => GrantKind::from_grants_human(false),
                1 => GrantKind::from_grants_human(true),
                other => {
                    return Err(corrupt(format!(
                        "pairing grant grants_human out of vocabulary: {other}"
                    )));
                }
            },
            status,
            issued_at: UtcMillis::from_millis(self.issued_at),
            expires_at: UtcMillis::from_millis(self.expires_at),
            consumed_at: self.consumed_at.map(UtcMillis::from_millis),
            revoked_at: self.revoked_at.map(UtcMillis::from_millis),
        })
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
