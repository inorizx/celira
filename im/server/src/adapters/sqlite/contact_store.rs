//! SQLite Contact adapter：实现 `ports::contact::ContactStore`。
//!
//! 本模块是联系人事实唯一允许接触 SQLite 文件的地方。HTTP handler 和
//! 用例不得直接操作数据库，只能通过端口调用。
//!
//! 关系是周期表 `contact_periods`（migration 202609250001）：同一对 Human
//! 至多一个 active 段（部分唯一索引），闭合段（拒绝/解除）保留历史。请求
//! 创建使用 `BEGIN IMMEDIATE` 显式单写事务：同 requester + key 的既有幂等
//! 事实裁决 → 双方存在任一方向 active 拉黑段则 404 → 同一对 Human 已有
//! active 段时直接返回既有关系（A→B 与 B→A 并发收敛、不创建第二条、不改
//! 变 requester/addressee）→ 插入 pending 段。接受动作由 addressee 发起：
//! pending → accepted 并写入 accepted_at；已 accepted 幂等返回既有关系；
//! 不存在或当前 Human 不是 addressee 统一 `NotFound`（fail closed）。
//! 拒绝（decline）与解除（end）都是「关闭 active 段」；拉黑在同一事务内先
//! 结束既有 active 段再写拉黑段。
//!
//! 请求幂等键只用于创建段（(requester_id, idempotency_key) 唯一，跨全部
//! 周期含闭合段），不进入公开联系人视图；反向请求（对方已有 active 段）
//! 不创建新行，因此不消费该请求自己的幂等键。

#[cfg(test)]
use std::sync::Arc;

use sqlx::Connection;
use sqlx::sqlite::SqliteConnection;
use uuid::Uuid;

use crate::domain::agent::IdempotencyKey;
use crate::domain::contact::{Contact, ContactStatus, canonical_pair};
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};
use crate::domain::time::UtcMillis;
use crate::ports::contact::{
    BlockStoreError, BlockedHuman, ContactListItem, ContactOutcome, ContactStore, ContactStoreError,
};
use crate::ports::room::ParticipantProfile;
use crate::ports::storage::{StorageError, StorageFailureKind};

#[cfg(test)]
use tokio::sync::oneshot;

const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

type ContactRow = (String, String, String, String, String, i64, Option<i64>);

/// 单段行：period_id + 关系公共字段（接受/拒绝/解除裁决用）。
type PeriodRow = (
    String,      // period_id
    String,      // a_id
    String,      // b_id
    String,      // requester_id
    String,      // addressee_id
    String,      // state
    i64,         // created_at
    Option<i64>, // accepted_at
);

fn period_contact_from_row(
    (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at): PeriodRow,
) -> Result<(String, Contact), ContactStoreError> {
    let status = ContactStatus::parse(&state)
        .ok_or_else(|| corrupt(format!("unknown contact period state {state:?}")))?;
    Ok((
        period_id,
        Contact {
            a_id: ParticipantId(a_id),
            b_id: ParticipantId(b_id),
            requester_id: ParticipantId(requester_id),
            addressee_id: ParticipantId(addressee_id),
            status,
            created_at: UtcMillis::from_millis(created_at),
            accepted_at: accepted_at.map(UtcMillis::from_millis),
        },
    ))
}

/// 基于同一 SqlitePool 的 Contact 存储。
#[derive(Clone)]
pub struct SqliteContactStore {
    pool: sqlx::sqlite::SqlitePool,
    #[cfg(test)]
    commit_checkpoint: Arc<CommitCheckpoint>,
}

impl SqliteContactStore {
    pub fn new(pool: sqlx::sqlite::SqlitePool) -> Self {
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

fn db_error(e: sqlx::Error) -> ContactStoreError {
    ContactStoreError::Database(StorageError::from(e))
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> ContactStoreError {
    ContactStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn contact_from_row(
    (a_id, b_id, requester_id, addressee_id, status, created_at, accepted_at): ContactRow,
) -> Result<Contact, ContactStoreError> {
    let status = ContactStatus::parse(&status)
        .ok_or_else(|| corrupt(format!("unknown contact status {status:?}")))?;
    Ok(Contact {
        a_id: ParticipantId(a_id),
        b_id: ParticipantId(b_id),
        requester_id: ParticipantId(requester_id),
        addressee_id: ParticipantId(addressee_id),
        status,
        created_at: UtcMillis::from_millis(created_at),
        accepted_at: accepted_at.map(UtcMillis::from_millis),
    })
}

fn profile_from_row(
    (id, kind, status, display_name): (String, String, String, String),
) -> Result<ParticipantProfile, ContactStoreError> {
    Ok(ParticipantProfile {
        id: ParticipantId(id),
        kind: ParticipantKind::parse(&kind)
            .ok_or_else(|| corrupt(format!("unknown participant kind {kind:?}")))?,
        status: ParticipantStatus::parse(&status)
            .ok_or_else(|| corrupt(format!("unknown participant status {status:?}")))?,
        display_name: DisplayName::parse(&display_name)
            .map_err(|e| corrupt(format!("stored display name invalid: {e}")))?,
    })
}

/// 读取对端 Human 的最小公共资料（peer 是相对 `viewer_id` 的另一端）。
async fn fetch_peer(
    conn: &mut SqliteConnection,
    contact: &Contact,
    viewer_id: &ParticipantId,
) -> Result<ParticipantProfile, ContactStoreError> {
    let peer_id = if contact.requester_id == *viewer_id {
        &contact.addressee_id
    } else {
        &contact.requester_id
    };
    fetch_participant_profile(conn, peer_id)
        .await?
        .ok_or_else(|| corrupt("contact references a missing peer participant"))
}

/// 按 id 读取 Participant 最小公共资料（不存在返回 None）。
async fn fetch_participant_profile(
    conn: &mut SqliteConnection,
    id: &ParticipantId,
) -> Result<Option<ParticipantProfile>, ContactStoreError> {
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT id, kind, status, display_name FROM participants WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_error)?;
    row.map(profile_from_row).transpose()
}

/// 读取该对 Human 当前的 active 关系段（至多一段）。
async fn fetch_active_period(
    conn: &mut SqliteConnection,
    a_id: &str,
    b_id: &str,
) -> Result<Option<PeriodRow>, ContactStoreError> {
    sqlx::query_as(
        "SELECT period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at \
         FROM contact_periods WHERE a_id = ? AND b_id = ? AND active_marker = 1 LIMIT 1",
    )
    .bind(a_id)
    .bind(b_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)
}

/// 事务内关闭一个 active 关系段（闭合是终态）。
async fn close_active_period(
    conn: &mut SqliteConnection,
    period_id: &str,
    closed_by: &str,
) -> Result<(), ContactStoreError> {
    let closed_at = UtcMillis::now().millis();
    sqlx::query(
        "UPDATE contact_periods \
         SET active_marker = 0, closed_at = ?, closed_by = ? \
         WHERE period_id = ? AND active_marker = 1",
    )
    .bind(closed_at)
    .bind(closed_by)
    .bind(period_id)
    .execute(&mut *conn)
    .await
    .map_err(db_error)?;
    Ok(())
}

#[async_trait::async_trait]
impl ContactStore for SqliteContactStore {
    async fn send_contact_request(
        &self,
        requester_id: &ParticipantId,
        target_id: &ParticipantId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<ContactOutcome, ContactStoreError> {
        if requester_id == target_id {
            return Err(ContactStoreError::SelfRequest);
        }
        let (a_id, b_id) = canonical_pair(requester_id, target_id);
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<ContactOutcome, ContactStoreError> = async {
            // 1. 同 requester + key 的既有幂等事实：目标一致且该段仍 active
            //    → 幂等重放；目标不同或该段已闭合（key 已消费，重新申请必须
            //    新 key）→ 冲突。
            let existing_key: Option<(String, String, i64)> = sqlx::query_as(
                "SELECT a_id, b_id, active_marker FROM contact_periods \
                 WHERE requester_id = ? AND idempotency_key = ?",
            )
            .bind(requester_id.as_str())
            .bind(idempotency_key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if let Some((stored_a, stored_b, active_marker)) = existing_key {
                let expected_other = if requester_id.as_str() == stored_a {
                    &stored_b
                } else {
                    &stored_a
                };
                if expected_other != target_id.as_str() || active_marker != 1 {
                    return Err(ContactStoreError::IdempotencyConflict);
                }
                let row = fetch_active_period(&mut tx, a_id.as_str(), b_id.as_str())
                    .await?
                    .ok_or_else(|| corrupt("active idempotency period vanished"))?;
                let (_, contact) = period_contact_from_row(row)?;
                let peer = fetch_peer(&mut tx, &contact, requester_id).await?;
                return Ok(ContactOutcome::Existing { contact, peer });
            }

            // 2. 双方存在任一方向 active 拉黑段 → 与「目标不存在」同形
            //    404（不泄露被拉黑）。
            let blocked: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM participant_blocks \
                 WHERE active_marker = 1 \
                   AND ((blocker_id = ? AND blocked_id = ?) \
                        OR (blocker_id = ? AND blocked_id = ?)) \
                 LIMIT 1",
            )
            .bind(requester_id.as_str())
            .bind(target_id.as_str())
            .bind(target_id.as_str())
            .bind(requester_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if blocked.is_some() {
                return Err(ContactStoreError::NotFound);
            }

            // 3. 同一对 Human 已有 active 段：返回既有段（幂等收敛），不创建
            //    第二条，不改变 requester/addressee。
            if let Some(row) = fetch_active_period(&mut tx, a_id.as_str(), b_id.as_str()).await? {
                let (_, contact) = period_contact_from_row(row)?;
                let peer = fetch_peer(&mut tx, &contact, requester_id).await?;
                return Ok(ContactOutcome::Existing { contact, peer });
            }

            // 4. 目标必须是存在且 active 的 participant——**不分 kind**（批次三
            //    第二刀：关系是两个主体之间的事实）；未知、非 active（含 revoked）
            //    统一 NotFound（fail closed）。
            let target_status: Option<String> =
                sqlx::query_scalar("SELECT status FROM participants WHERE id = ?")
                    .bind(target_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some(status) = target_status else {
                return Err(ContactStoreError::NotFound);
            };
            if status != ParticipantStatus::Active.as_str() {
                return Err(ContactStoreError::NotFound);
            }

            // 5. 插入 pending 段（数据库 CHECK/守卫再次兜底）。
            let created_at = UtcMillis::now().millis();
            sqlx::query(
                "INSERT INTO contact_periods \
                 (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
                  accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
                 VALUES (?, ?, ?, ?, ?, 'pending', ?, NULL, NULL, NULL, 1, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(a_id.as_str())
            .bind(b_id.as_str())
            .bind(requester_id.as_str())
            .bind(target_id.as_str())
            .bind(created_at)
            .bind(idempotency_key.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;

            let contact = Contact {
                a_id,
                b_id,
                requester_id: requester_id.clone(),
                addressee_id: target_id.clone(),
                status: ContactStatus::Pending,
                created_at: UtcMillis::from_millis(created_at),
                accepted_at: None,
            };
            let peer = fetch_peer(&mut tx, &contact, requester_id).await?;
            Ok(ContactOutcome::Created { contact, peer })
        }
        .await;

        match outcome {
            Ok(created @ ContactOutcome::Created { .. }) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(created)
            }
            Ok(existing @ ContactOutcome::Existing { .. }) => Ok(existing),
            Err(e) => Err(e),
        }
    }

    async fn list_contacts(
        &self,
        participant_id: &ParticipantId,
    ) -> Result<Vec<ContactListItem>, ContactStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 只列 active 段（闭合段保留历史但不进入公开视图）；观察者必须是
        // active participant——**不分 kind**（批次三第二刀）。
        let rows: Vec<ContactRow> = sqlx::query_as(
            "SELECT c.a_id, c.b_id, c.requester_id, c.addressee_id, c.state, \
                    c.created_at, c.accepted_at \
             FROM contact_periods c \
             JOIN participants p ON p.id = ? AND p.status = 'active' \
             WHERE (c.a_id = ? OR c.b_id = ?) AND c.active_marker = 1 \
             ORDER BY c.created_at ASC, c.a_id ASC",
        )
        .bind(participant_id.as_str())
        .bind(participant_id.as_str())
        .bind(participant_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let contact = contact_from_row(row)?;
            let peer = fetch_peer(&mut conn, &contact, participant_id).await?;
            items.push(ContactListItem { contact, peer });
        }
        Ok(items)
    }

    async fn accept_contact(
        &self,
        human_id: &ParticipantId,
        requester_id: &ParticipantId,
    ) -> Result<(Contact, ParticipantProfile), ContactStoreError> {
        let (a_id, b_id) = canonical_pair(human_id, requester_id);
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(Contact, ParticipantProfile), ContactStoreError> = async {
            // 不存在 active 段、当前 Human 不是 addressee（只能 addressee 接受）
            // 统一 NotFound，不泄露关系是否存在。
            let Some(row) = fetch_active_period(&mut tx, a_id.as_str(), b_id.as_str()).await?
            else {
                return Err(ContactStoreError::NotFound);
            };
            let (period_id, contact) = period_contact_from_row(row)?;
            if contact.addressee_id != *human_id {
                return Err(ContactStoreError::NotFound);
            }
            let contact = if contact.status == ContactStatus::Accepted {
                contact
            } else {
                let accepted_at = UtcMillis::now().millis();
                sqlx::query(
                    "UPDATE contact_periods SET state = 'accepted', accepted_at = ? \
                     WHERE period_id = ? AND state = 'pending' AND active_marker = 1",
                )
                .bind(accepted_at)
                .bind(&period_id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                Contact {
                    status: ContactStatus::Accepted,
                    accepted_at: Some(UtcMillis::from_millis(accepted_at)),
                    ..contact
                }
            };
            let peer = fetch_peer(&mut tx, &contact, human_id).await?;
            Ok((contact, peer))
        }
        .await;

        match outcome {
            Ok(accepted) => {
                tx.commit().await.map_err(db_error)?;
                Ok(accepted)
            }
            Err(e) => Err(e),
        }
    }

    async fn decline_contact(
        &self,
        human_id: &ParticipantId,
        requester_id: &ParticipantId,
    ) -> Result<(), ContactStoreError> {
        let (a_id, b_id) = canonical_pair(human_id, requester_id);
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), ContactStoreError> = async {
            let Some(row) = fetch_active_period(&mut tx, a_id.as_str(), b_id.as_str()).await?
            else {
                return Err(ContactStoreError::NotFound);
            };
            let (period_id, contact) = period_contact_from_row(row)?;
            // 只有 addressee 可以拒绝；只有 pending 段可以被拒绝。
            if contact.addressee_id != *human_id || contact.status != ContactStatus::Pending {
                return Err(ContactStoreError::NotFound);
            }
            close_active_period(&mut tx, &period_id, "declined").await?;
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

    async fn end_contact(
        &self,
        human_id: &ParticipantId,
        peer_id: &ParticipantId,
    ) -> Result<(), ContactStoreError> {
        let (a_id, b_id) = canonical_pair(human_id, peer_id);
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<(), ContactStoreError> = async {
            // 没有 active 段（含对方从未是联系人、已是陌生人）→ 成功 no-op
            // （幂等 204），不泄露关系是否存在。
            if let Some(row) = fetch_active_period(&mut tx, a_id.as_str(), b_id.as_str()).await? {
                let (period_id, _) = period_contact_from_row(row)?;
                close_active_period(&mut tx, &period_id, "ended").await?;
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

    async fn block_participant(
        &self,
        blocker_id: &ParticipantId,
        blocked_id: &ParticipantId,
    ) -> Result<(), BlockStoreError> {
        if blocker_id == blocked_id {
            return Err(BlockStoreError::SelfBlock);
        }
        let (a_id, b_id) = canonical_pair(blocker_id, blocked_id);
        let mut conn = self.pool.acquire().await.map_err(map_block_db)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(map_block_db)?;

        let outcome: Result<(), BlockStoreError> = async {
            // 目标必须是存在且 active 的 participant——**不分 kind**（批次三第三刀
            // 第①步：拉黑是关系事实的对称面，与"关系"一样属于两个主体之间）；
            // 未知 / 非 active（含 revoked）统一 NotFound。
            let target_status: Option<String> =
                sqlx::query_scalar("SELECT status FROM participants WHERE id = ?")
                    .bind(blocked_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(map_block_db)?;
            match target_status {
                Some(status) if status == ParticipantStatus::Active.as_str() => {}
                _ => return Err(BlockStoreError::NotFound),
            }

            // 已有同方向 active 拉黑段 → 成功 no-op（幂等）。
            let already: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM participant_blocks \
                 WHERE blocker_id = ? AND blocked_id = ? AND active_marker = 1 LIMIT 1",
            )
            .bind(blocker_id.as_str())
            .bind(blocked_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_block_db)?;
            if already.is_some() {
                return Ok(());
            }

            // 拉黑 ⊇ 解除：同一事务内先结束双方之间既有的 active 段（若有）。
            if let Some(row) = fetch_active_period(&mut tx, a_id.as_str(), b_id.as_str())
                .await
                .map_err(map_block_contact)?
            {
                let (period_id, _) = period_contact_from_row(row).map_err(map_block_contact)?;
                close_active_period(&mut tx, &period_id, "ended")
                    .await
                    .map_err(map_block_contact)?;
            }

            let created_at = UtcMillis::now().millis();
            sqlx::query(
                "INSERT INTO participant_blocks \
                 (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
                 VALUES (?, ?, ?, ?, NULL, 1)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(blocker_id.as_str())
            .bind(blocked_id.as_str())
            .bind(created_at)
            .execute(&mut *tx)
            .await
            .map_err(map_block_db)?;
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                tx.commit().await.map_err(map_block_db)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn unblock_participant(
        &self,
        blocker_id: &ParticipantId,
        blocked_id: &ParticipantId,
    ) -> Result<(), BlockStoreError> {
        let mut conn = self.pool.acquire().await.map_err(map_block_db)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(map_block_db)?;

        let outcome: Result<(), BlockStoreError> = async {
            // 没有 active 拉黑段 → 成功 no-op（幂等）；解除不影响关系。
            let unblocked_at = UtcMillis::now().millis();
            sqlx::query(
                "UPDATE participant_blocks SET active_marker = 0, unblocked_at = ? \
                 WHERE blocker_id = ? AND blocked_id = ? AND active_marker = 1",
            )
            .bind(unblocked_at)
            .bind(blocker_id.as_str())
            .bind(blocked_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(map_block_db)?;
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                tx.commit().await.map_err(map_block_db)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn list_blocks(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<BlockedHuman>, ContactStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT p.id, p.kind, p.status, p.display_name, b.created_at \
             FROM participant_blocks b \
             JOIN participants p ON p.id = b.blocked_id \
             WHERE b.blocker_id = ? AND b.active_marker = 1 \
             ORDER BY b.created_at ASC, b.blocked_id ASC",
        )
        .bind(human_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        let mut items = Vec::with_capacity(rows.len());
        for (id, kind, status, display_name, created_at) in rows {
            items.push(BlockedHuman {
                peer: profile_from_row((id, kind, status, display_name))?,
                created_at: UtcMillis::from_millis(created_at),
            });
        }
        Ok(items)
    }
}

/// 拉黑路径的存储错误映射：ContactStoreError::Database ↔ BlockStoreError::Database。
fn map_block_contact(e: ContactStoreError) -> BlockStoreError {
    match e {
        ContactStoreError::Database(e) => BlockStoreError::Database(e),
        other => BlockStoreError::Database(StorageError::from_message(
            StorageFailureKind::Unknown,
            format!("unexpected contact store error on block path: {other}"),
        )),
    }
}

fn map_block_db(e: sqlx::Error) -> BlockStoreError {
    BlockStoreError::Database(StorageError::from(e))
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
