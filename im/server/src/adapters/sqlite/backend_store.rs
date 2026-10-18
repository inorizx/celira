//! SQLite Backend Binding / Credential adapter：实现 `ports::backend::BackendStore`。
//!
//! 本模块是 Binding 与 Credential 事实唯一允许接触 SQLite 文件的地方。
//! HTTP handler 和用例不得直接操作数据库，只能通过端口调用。
//!
//! 全部写入使用 SQLx `Transaction`（`begin_with("BEGIN IMMEDIATE")`）持有
//! 事务生命周期：future 被取消或提前释放时由 RAII 自动回滚，不能把未结束
//! 事务归还连接池。并发由 SQLite 单写者、事务内状态检查与数据库唯一约束
//! 共同裁决，不依赖进程内 mutex。
//!
//! 创建 Binding 与幂等协调列原子提交；创建/轮换 Credential 时幂等裁决、
//! 当前 active 状态 CAS、旧 Credential 撤销与新 Credential 创建原子提交；
//! 撤销 Binding 时先撤销其 active Credential；Agent 撤销传播由 migration
//! 006 的 participants AFTER UPDATE 触发器在同一事务内完成。

use sqlx::Connection;
use sqlx::sqlite::{SqliteConnection, SqlitePool};

use crate::adapters::sqlite::{finish_rollback, rollback_after_primary};
use crate::domain::backend::{
    BackendBinding, BackendCredential, BackendLabel, BackendStatus, BindingId, CredentialId,
};
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};
use crate::domain::time::UtcMillis;
use crate::ports::backend::{
    AgentAuthView, BackendAuthView, BackendStoreError, BindingAuthView, CreateBindingOutcome,
    CredentialAuthView, IssueCredentialOutcome, NewBinding, NewCredential,
};
use crate::ports::storage::{StorageError, StorageFailureKind};

const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

type BindingRow = (String, String, String, String, String, i64, Option<i64>);
type CredentialRow = (String, String, String, String, i64, Option<i64>);
type AuthRow = (
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    i64,
);

/// 基于同一 SqlitePool 的 Backend 存储。
#[derive(Clone)]
pub struct SqliteBackendStore {
    pool: SqlitePool,
}

impl SqliteBackendStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn db_error(e: sqlx::Error) -> BackendStoreError {
    BackendStoreError::Database(StorageError::from(e))
}

/// primary 变体 → composite kind 裁决（CORE_V1_ERROR_HANDLING.md §7.2）：
/// `Database(StorageError)` 沿用嵌套 kind；业务变体（`AgentNotFound` /
/// `AgentNotActive` / `BindingNotFound` / `CredentialNotFound` /
/// `IdempotencyConflict` / `BindingExists` / `CredentialStateConflict`）归
/// `Unknown`——内部事务完成失败分类，对外 500 `internal_error`，绝不声称
/// 业务冲突已可靠回滚。
fn primary_storage_kind(e: &BackendStoreError) -> StorageFailureKind {
    match e {
        BackendStoreError::Database(storage) => storage.kind(),
        _ => StorageFailureKind::Unknown,
    }
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> BackendStoreError {
    BackendStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn binding_from_row(
    (id, agent_id, created_by_human_id, label, status, created_at, revoked_at): BindingRow,
) -> Result<BackendBinding, BackendStoreError> {
    let status = BackendStatus::parse(&status)
        .ok_or_else(|| corrupt(format!("unknown backend binding status {status:?}")))?;
    let label = BackendLabel::parse(&label)
        .map_err(|e| corrupt(format!("stored backend label invalid: {e}")))?;
    Ok(BackendBinding {
        id: BindingId(id),
        agent_id: ParticipantId(agent_id),
        created_by_human_id: ParticipantId(created_by_human_id),
        label,
        status,
        created_at: UtcMillis::from_millis(created_at),
        revoked_at: revoked_at.map(UtcMillis::from_millis),
    })
}

fn credential_from_row(
    (id, binding_id, issued_by_human_id, status, created_at, revoked_at): CredentialRow,
) -> Result<BackendCredential, BackendStoreError> {
    let status = BackendStatus::parse(&status)
        .ok_or_else(|| corrupt(format!("unknown backend credential status {status:?}")))?;
    Ok(BackendCredential {
        id: CredentialId(id),
        binding_id: BindingId(binding_id),
        issued_by_human_id: ParticipantId(issued_by_human_id),
        status,
        created_at: UtcMillis::from_millis(created_at),
        revoked_at: revoked_at.map(UtcMillis::from_millis),
    })
}

/// 当前 Human 是否曾经拥有该 Agent（Owner 历史存在即可，不要求 active）。
async fn owned_agent_exists(
    conn: &mut SqliteConnection,
    human_id: &str,
    agent_id: &str,
) -> Result<bool, BackendStoreError> {
    let row: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM agent_owners WHERE agent_id = ? AND human_id = ? LIMIT 1",
    )
    .bind(agent_id)
    .bind(human_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    Ok(row.is_some())
}

async fn fetch_binding(
    conn: &mut SqliteConnection,
    binding_id: &str,
    agent_id: &str,
) -> Result<Option<BackendBinding>, BackendStoreError> {
    let row: Option<BindingRow> = sqlx::query_as(
        "SELECT id, agent_id, created_by_human_id, label, status, created_at, revoked_at \
         FROM backend_bindings WHERE id = ? AND agent_id = ?",
    )
    .bind(binding_id)
    .bind(agent_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    row.map(binding_from_row).transpose()
}

async fn fetch_credential(
    conn: &mut SqliteConnection,
    credential_id: &str,
    binding_id: &str,
) -> Result<Option<BackendCredential>, BackendStoreError> {
    let row: Option<CredentialRow> = sqlx::query_as(
        "SELECT id, binding_id, issued_by_human_id, status, created_at, revoked_at \
         FROM backend_credentials WHERE id = ? AND binding_id = ?",
    )
    .bind(credential_id)
    .bind(binding_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    row.map(credential_from_row).transpose()
}

#[async_trait::async_trait]
impl crate::ports::backend::BackendStore for SqliteBackendStore {
    async fn create_binding(
        &self,
        human_id: &ParticipantId,
        binding: &NewBinding,
    ) -> Result<CreateBindingOutcome, BackendStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<CreateBindingOutcome, BackendStoreError> = async {
            let existing: Option<BindingRow> = sqlx::query_as(
                "SELECT id, agent_id, created_by_human_id, label, status, created_at, revoked_at \
                 FROM backend_bindings WHERE created_by_human_id = ? AND idempotency_key = ?",
            )
            .bind(human_id.as_str())
            .bind(binding.idempotency_key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;

            if let Some(row) = existing {
                if row.1 != binding.agent_id.as_str() || row.3 != binding.label.as_str() {
                    return Err(BackendStoreError::IdempotencyConflict);
                }
                return Ok(CreateBindingOutcome::Replayed {
                    binding: binding_from_row(row)?,
                });
            }

            let agent_row: Option<(String, String)> =
                sqlx::query_as("SELECT kind, status FROM participants WHERE id = ?")
                    .bind(binding.agent_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some((kind, status)) = agent_row else {
                return Err(BackendStoreError::AgentNotFound);
            };
            if kind != ParticipantKind::Agent.as_str() {
                return Err(BackendStoreError::AgentNotFound);
            }
            if status != ParticipantStatus::Active.as_str() {
                return Err(BackendStoreError::AgentNotActive);
            }

            let owner_status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM agent_owners WHERE agent_id = ? AND human_id = ?",
            )
            .bind(binding.agent_id.as_str())
            .bind(human_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            match owner_status {
                None => return Err(BackendStoreError::AgentNotFound),
                Some(status) if status != ParticipantStatus::Active.as_str() => {
                    return Err(BackendStoreError::AgentNotActive);
                }
                Some(_) => {}
            }

            let active_exists: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM backend_bindings WHERE agent_id = ? AND status = 'active' LIMIT 1",
            )
            .bind(binding.agent_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if active_exists.is_some() {
                return Err(BackendStoreError::BindingExists);
            }

            sqlx::query(
                "INSERT INTO backend_bindings \
                 (id, agent_id, created_by_human_id, label, status, created_at, revoked_at, idempotency_key) \
                 VALUES (?, ?, ?, ?, 'active', ?, NULL, ?)",
            )
            .bind(binding.id.as_str())
            .bind(binding.agent_id.as_str())
            .bind(binding.created_by_human_id.as_str())
            .bind(binding.label.as_str())
            .bind(binding.created_at.millis())
            .bind(binding.idempotency_key.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;

            Ok(CreateBindingOutcome::Created {
                binding: BackendBinding {
                    id: binding.id.clone(),
                    agent_id: binding.agent_id.clone(),
                    created_by_human_id: human_id.clone(),
                    label: binding.label.clone(),
                    status: BackendStatus::Active,
                    created_at: binding.created_at,
                    revoked_at: None,
                },
            })
        }
        .await;

        match outcome {
            Ok(CreateBindingOutcome::Created { .. }) => {
                tx.commit().await.map_err(db_error)?;
                outcome
            }
            Ok(replayed @ CreateBindingOutcome::Replayed { .. }) => {
                // 重放成功分支无 primary：rollback 失败按普通 rollback
                // StorageError 返回。
                finish_rollback(tx)
                    .await
                    .map_err(BackendStoreError::Database)?;
                Ok(replayed)
            }
            Err(e) => {
                // primary + 显式 rollback 双因果：rollback 成功原样返回
                // primary（类型/变体不变，含业务变体）；rollback 失败返回
                // 携带两条 cause 的复合 Database 错误，HTTP 映射只看
                // composite kind。
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, BackendStoreError::Database).await)
            }
        }
    }

    async fn list_bindings(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
    ) -> Result<Vec<BackendBinding>, BackendStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        if !owned_agent_exists(&mut conn, human_id.as_str(), agent_id.as_str()).await? {
            return Err(BackendStoreError::AgentNotFound);
        }
        let rows: Vec<BindingRow> = sqlx::query_as(
            "SELECT id, agent_id, created_by_human_id, label, status, created_at, revoked_at \
             FROM backend_bindings WHERE agent_id = ? \
             ORDER BY created_at ASC, id ASC",
        )
        .bind(agent_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(binding_from_row).collect()
    }

    async fn get_binding(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
    ) -> Result<BackendBinding, BackendStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        if !owned_agent_exists(&mut conn, human_id.as_str(), agent_id.as_str()).await? {
            return Err(BackendStoreError::AgentNotFound);
        }
        fetch_binding(&mut conn, binding_id, agent_id.as_str())
            .await?
            .ok_or(BackendStoreError::BindingNotFound)
    }

    async fn revoke_binding(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
    ) -> Result<(), BackendStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), BackendStoreError> = async {
            if !owned_agent_exists(&mut tx, human_id.as_str(), agent_id.as_str()).await? {
                return Err(BackendStoreError::AgentNotFound);
            }
            let Some(binding) = fetch_binding(&mut tx, binding_id, agent_id.as_str()).await? else {
                return Err(BackendStoreError::BindingNotFound);
            };
            if binding.status == BackendStatus::Active {
                let revoked_at = UtcMillis::now().millis();
                sqlx::query(
                    "UPDATE backend_credentials SET status = 'revoked', revoked_at = ? \
                     WHERE binding_id = ? AND status = 'active'",
                )
                .bind(revoked_at)
                .bind(binding_id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                sqlx::query(
                    "UPDATE backend_bindings SET status = 'revoked', revoked_at = ? \
                     WHERE id = ? AND status = 'active'",
                )
                .bind(revoked_at)
                .bind(binding_id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            }
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                tx.commit().await.map_err(db_error)?;
                Ok(())
            }
            Err(e) => {
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, BackendStoreError::Database).await)
            }
        }
    }

    async fn issue_credential(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
        credential: &NewCredential,
    ) -> Result<IssueCredentialOutcome, BackendStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<IssueCredentialOutcome, BackendStoreError> = async {
            let existing: Option<(String, String, Option<String>, i64, Option<i64>)> =
                sqlx::query_as(
                    "SELECT id, binding_id, replaces_credential_id, created_at, revoked_at \
                     FROM backend_credentials WHERE issued_by_human_id = ? AND idempotency_key = ?",
                )
                .bind(human_id.as_str())
                .bind(credential.idempotency_key.as_str())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;

            if let Some((id, existing_binding, existing_replaces, created_at, revoked_at)) =
                existing
            {
                let binding_agent: Option<String> = sqlx::query_scalar(
                    "SELECT agent_id FROM backend_bindings WHERE id = ?",
                )
                .bind(&existing_binding)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
                let Some(existing_agent) = binding_agent else {
                    return Err(corrupt("idempotency record points to missing binding"));
                };
                let same_target = existing_binding == binding_id
                    && existing_agent == agent_id.as_str()
                    && existing_replaces.as_deref()
                        == credential
                            .replaces_credential_id
                            .as_ref()
                            .map(|id| id.as_str());
                if !same_target {
                    return Err(BackendStoreError::IdempotencyConflict);
                }
                let status = if revoked_at.is_some() {
                    "revoked"
                } else {
                    "active"
                };
                return Ok(IssueCredentialOutcome::Replayed {
                    credential: BackendCredential {
                        id: CredentialId(id),
                        binding_id: BindingId(existing_binding),
                        issued_by_human_id: human_id.clone(),
                        status: BackendStatus::parse(status).expect("stored status is valid"),
                        created_at: UtcMillis::from_millis(created_at),
                        revoked_at: revoked_at.map(UtcMillis::from_millis),
                    },
                });
            }

            if !owned_agent_exists(&mut tx, human_id.as_str(), agent_id.as_str()).await? {
                return Err(BackendStoreError::AgentNotFound);
            }
            let binding: Option<(String, String)> = sqlx::query_as(
                "SELECT status, agent_id FROM backend_bindings WHERE id = ? AND agent_id = ?",
            )
            .bind(binding_id)
            .bind(agent_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some((binding_status, _binding_agent)) = binding else {
                return Err(BackendStoreError::BindingNotFound);
            };
            if binding_status != BackendStatus::Active.as_str() {
                return Err(BackendStoreError::CredentialStateConflict);
            }

            let agent_status: Option<String> =
                sqlx::query_scalar("SELECT status FROM participants WHERE id = ?")
                    .bind(agent_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            match agent_status {
                Some(status) if status == ParticipantStatus::Active.as_str() => {}
                _ => return Err(BackendStoreError::AgentNotActive),
            }
            let owner_status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM agent_owners WHERE agent_id = ? AND human_id = ?",
            )
            .bind(agent_id.as_str())
            .bind(human_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            match owner_status {
                Some(status) if status == ParticipantStatus::Active.as_str() => {}
                _ => return Err(BackendStoreError::AgentNotActive),
            }

            match &credential.replaces_credential_id {
                None => {
                    let active_exists: Option<i64> = sqlx::query_scalar(
                        "SELECT 1 FROM backend_credentials \
                         WHERE binding_id = ? AND status = 'active' LIMIT 1",
                    )
                    .bind(binding_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    if active_exists.is_some() {
                        return Err(BackendStoreError::CredentialStateConflict);
                    }
                }
                Some(replaces_id) => {
                    // 先做同事务前置查询以稳定映射为 409；真正的 CAS 与旧
                    // Credential 撤销由 migration 006 的 BEFORE INSERT 触发器
                    // 原子完成，使数据库本身也无法从 stale 历史分叉。
                    let current: Option<i64> = sqlx::query_scalar(
                        "SELECT 1 FROM backend_credentials \
                         WHERE id = ? AND binding_id = ? AND status = 'active' LIMIT 1",
                    )
                    .bind(replaces_id.as_str())
                    .bind(binding_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    if current.is_none() {
                        return Err(BackendStoreError::CredentialStateConflict);
                    }
                }
            }

            sqlx::query(
                "INSERT INTO backend_credentials \
                 (id, binding_id, issued_by_human_id, token_hash, replaces_credential_id, status, created_at, revoked_at, idempotency_key) \
                 VALUES (?, ?, ?, ?, ?, 'active', ?, NULL, ?)",
            )
            .bind(credential.id.as_str())
            .bind(binding_id)
            .bind(credential.issued_by_human_id.as_str())
            .bind(&credential.token_hash)
            .bind(
                credential
                    .replaces_credential_id
                    .as_ref()
                    .map(|id| id.as_str()),
            )
            .bind(credential.created_at.millis())
            .bind(credential.idempotency_key.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;

            Ok(IssueCredentialOutcome::Created {
                credential: BackendCredential {
                    id: credential.id.clone(),
                    binding_id: credential.binding_id.clone(),
                    issued_by_human_id: human_id.clone(),
                    status: BackendStatus::Active,
                    created_at: credential.created_at,
                    revoked_at: None,
                },
            })
        }
        .await;

        match outcome {
            Ok(IssueCredentialOutcome::Created { .. }) => {
                tx.commit().await.map_err(db_error)?;
                outcome
            }
            Ok(replayed @ IssueCredentialOutcome::Replayed { .. }) => {
                finish_rollback(tx)
                    .await
                    .map_err(BackendStoreError::Database)?;
                Ok(replayed)
            }
            Err(e) => {
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, BackendStoreError::Database).await)
            }
        }
    }

    async fn list_credentials(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
    ) -> Result<Vec<BackendCredential>, BackendStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        if !owned_agent_exists(&mut conn, human_id.as_str(), agent_id.as_str()).await? {
            return Err(BackendStoreError::AgentNotFound);
        }
        let binding_exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM backend_bindings WHERE id = ? AND agent_id = ? LIMIT 1",
        )
        .bind(binding_id)
        .bind(agent_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        if binding_exists.is_none() {
            return Err(BackendStoreError::BindingNotFound);
        }
        let rows: Vec<CredentialRow> = sqlx::query_as(
            "SELECT id, binding_id, issued_by_human_id, status, created_at, revoked_at \
             FROM backend_credentials WHERE binding_id = ? \
             ORDER BY created_at ASC, id ASC",
        )
        .bind(binding_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(credential_from_row).collect()
    }

    async fn revoke_credential(
        &self,
        human_id: &ParticipantId,
        agent_id: &ParticipantId,
        binding_id: &str,
        credential_id: &str,
    ) -> Result<(), BackendStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), BackendStoreError> = async {
            if !owned_agent_exists(&mut tx, human_id.as_str(), agent_id.as_str()).await? {
                return Err(BackendStoreError::AgentNotFound);
            }
            let binding_exists: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM backend_bindings WHERE id = ? AND agent_id = ? LIMIT 1",
            )
            .bind(binding_id)
            .bind(agent_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if binding_exists.is_none() {
                return Err(BackendStoreError::BindingNotFound);
            }
            if fetch_credential(&mut tx, credential_id, binding_id)
                .await?
                .is_none()
            {
                return Err(BackendStoreError::CredentialNotFound);
            }
            sqlx::query(
                "UPDATE backend_credentials SET status = 'revoked', revoked_at = ? \
                 WHERE id = ? AND binding_id = ? AND status = 'active'",
            )
            .bind(UtcMillis::now().millis())
            .bind(credential_id)
            .bind(binding_id)
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
            Err(e) => {
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, BackendStoreError::Database).await)
            }
        }
    }

    async fn authenticate(
        &self,
        token_hash: &str,
    ) -> Result<Option<BackendAuthView>, BackendStoreError> {
        let row: Option<AuthRow> = sqlx::query_as(
            "SELECT a.id, a.status, a.display_name, a.created_at, \
                    b.id, b.label, b.status, \
                    c.id, c.status, c.created_at \
             FROM backend_credentials c \
             JOIN backend_bindings b ON b.id = c.binding_id \
             JOIN participants a ON a.id = b.agent_id \
             WHERE c.token_hash = ? \
               AND c.status = 'active' AND c.revoked_at IS NULL \
               AND b.status = 'active' AND b.revoked_at IS NULL \
               AND a.kind = 'agent' AND a.status = 'active' \
             LIMIT 1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(
            |(
                agent_id,
                agent_status,
                display_name,
                agent_created_at,
                binding_id,
                label,
                binding_status,
                credential_id,
                credential_status,
                created_at,
            )| {
                DisplayName::parse(&display_name)
                    .map_err(|e| corrupt(format!("stored display name invalid: {e}")))?;
                Ok(BackendAuthView {
                    agent: AgentAuthView {
                        id: ParticipantId(agent_id),
                        display_name,
                        status: agent_status,
                        created_at: UtcMillis::from_millis(agent_created_at),
                    },
                    binding: BindingAuthView {
                        id: BindingId(binding_id),
                        label,
                        status: binding_status,
                    },
                    credential: CredentialAuthView {
                        id: CredentialId(credential_id),
                        status: credential_status,
                        created_at: UtcMillis::from_millis(created_at),
                    },
                })
            },
        )
        .transpose()
    }
}
