//! SQLite Room adapter：实现 `ports::room::RoomStore`。
//!
//! 本模块是 Room、配对、Membership 周期与房间幂等事实唯一允许接触 SQLite
//! 文件的地方。HTTP handler 和用例不得直接操作数据库，只能通过端口调用。
//!
//! 创建与变更使用 `BEGIN IMMEDIATE` 显式事务，由 SQLx Transaction 持有
//! 生命周期：Room、覆盖行、配对（direct / human_direct）、Membership
//! 周期与幂等记录同时提交，任何失败或 Future 取消都由 Drop 回滚，不留
//! 半成品。Agent revoke 对 Agent Membership 周期的关闭由数据库触发器在
//! AgentStore 的同一事务内完成，本 adapter 不参与撤销路径，也不跨 adapter
//! 双写。
//!
//! kind 语义：结构闭合以 room_coverage 为 kind-specific 结构见证——direct
//! 携带 Human–Agent 配对身份、human_direct 携带 Human–Human 配对身份、
//! group 携带 active owner 周期身份（group_owner_membership_id，复合外键
//! 见证 (room_id, membership_id, role, active_marker)）；rooms →
//! room_coverage → 配对/owner membership 的 deferred 复合外键在 COMMIT 时
//! 反向证明每个已提交 Room 至少存在对应配对/owner（direct/human_direct
//! 至多一个配对、group 至多一个 active owner 由 PK/UNIQUE/部分唯一索引/
//! INSERT 守卫保证，成员资格与角色由 INSERT 守卫保证）。
//!
//! Membership 周期边界：join_after_sequence / left_before_sequence 是
//! 内部 sequence 边界，只参与 History 窗口裁决，不对外暴露。
//!
//! 群主转让：每次转让是一条不可变 `group_owner_transitions` 事实，以
//! DEFERRABLE 复合外键精确见证本次关闭/打开的四条同 Room Membership
//! 周期（旧 owner 与目标 member 以 closed_by=transfer、同一
//! occurred_at/boundary 关闭；新 owner 与旧 owner member 以同一
//! occurred_at/boundary 打开）；新 owner 周期 INSERT 守卫只接受「该 Room
//! 尚无任何 Membership」或「同一事务内精确匹配的 transition」两种来路，
//! 不看模糊历史。转让事务必须先插 transition（其 INSERT guard 要求旧
//! owner 与目标 member 当时 active、两端 active Human、old owner 唯一
//! active owner）再关闭两个旧周期（transfer 关闭守卫要求当前事务存在
//! 匹配 transition），转让不可拆成两笔事务。幂等表收敛为 (human_id, key)
//! → transition_id，重放 JOIN 不可变 transition 读取 Room/目标/新 owner
//! 周期。owner 邀请自己在 active lookup 前显式识别并返回 MemberNotFound。
//!
//! 提醒档（`room_attention`，契约 `CORE_V1_IM_CONTRACT.md` §9.1，2026-10-19
//! 窄解冻切片 1a）：本模块持有那张表**唯一**的写路径
//! （`set_room_attention`，`BEGIN IMMEDIATE`）与两处读投影（Human 房间列表 /
//! Agent 通讯录，共用 `ATTENTION_SQL` 一段）。它是**状态写**——不产生任何
//! 事件、不产生幂等记录、不碰消息主链；读面**缺行投影成 `all`**。
//! ⚠ 本层只落事实与读面：「按档决定那条易失通知发不发」不在本层（1b）。

#[cfg(test)]
use std::sync::Arc;

use sqlx::Connection;
use sqlx::sqlite::{SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::adapters::sqlite::{finish_rollback, rollback_after_primary};
use crate::domain::agent::IdempotencyKey;
use crate::domain::contact::canonical_pair;
use crate::domain::message::MessageId;
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};
use crate::domain::room::{
    AttentionLevel, Membership, MembershipClosedBy, MembershipId, MembershipRole, MembershipStatus,
    PeerRead, ReadState, Room, RoomId, RoomKind, RoomStatus,
};
use crate::domain::time::UtcMillis;
use crate::ports::room::{
    AdvanceReadCursorOutcome, BackendDirectoryRoom, CreateDirectRoomOutcome, CreateRoomOutcome,
    InviteMemberOutcome, MemberView, NewDirectRoom, NewGroup, NewHumanDirectRoom,
    ParticipantProfile, RoomListItem, RoomStoreError, TransferOwnerOutcome,
};
use crate::ports::storage::{StorageError, StorageFailureKind};

#[cfg(test)]
use tokio::sync::oneshot;

const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

const SQL_INSERT_ROOM: &str = "INSERT INTO rooms (id, kind, status, created_at, name) \
     VALUES (?, ?, 'active', ?, ?)";

const SQL_INSERT_COVERAGE_DIRECT: &str = "INSERT INTO room_coverage \
     (room_id, kind, created_at, direct_human_id, direct_agent_id) \
     VALUES (?, 'direct', ?, ?, ?)";

const SQL_INSERT_COVERAGE_HUMAN_DIRECT: &str = "INSERT INTO room_coverage \
     (room_id, kind, created_at, human_direct_a_id, human_direct_b_id) \
     VALUES (?, 'human_direct', ?, ?, ?)";

const SQL_INSERT_COVERAGE_GROUP: &str = "INSERT INTO room_coverage \
     (room_id, kind, created_at, group_owner_membership_id, group_owner_role, group_owner_active_marker) \
     VALUES (?, 'group', ?, ?, 'owner', 1)";

const SQL_INSERT_PAIR: &str = "INSERT INTO direct_room_pairs \
     (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
     VALUES (?, ?, ?, ?, ?, ?)";

const SQL_INSERT_HUMAN_DIRECT_PAIR: &str = "INSERT INTO human_direct_pairs \
     (room_id, human_a_id, human_b_id, contact_period_id, human_a_membership_id, human_b_membership_id, role_a, role_b, created_at) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";

const SQL_INSERT_MEMBERSHIP: &str = "INSERT INTO room_memberships \
     (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
     VALUES (?, ?, ?, ?, ?, ?, 1)";

const SQL_INSERT_DIRECT_IDEMPOTENCY: &str = "INSERT INTO direct_room_idempotency \
     (human_id, key, agent_id, room_id, created_at) \
     VALUES (?, ?, ?, ?, ?)";

const SQL_INSERT_HUMAN_DIRECT_IDEMPOTENCY: &str = "INSERT INTO human_direct_room_idempotency \
     (human_id, key, peer_human_id, canonical_a, canonical_b, room_id, created_at) \
     VALUES (?, ?, ?, ?, ?, ?, ?)";

const SQL_INSERT_GROUP_IDEMPOTENCY: &str = "INSERT INTO group_create_idempotency \
     (human_id, key, room_id, name, created_at) VALUES (?, ?, ?, ?, ?)";

const SQL_INSERT_INVITE_IDEMPOTENCY: &str = "INSERT INTO group_member_invite_idempotency \
     (human_id, key, room_id, participant_id, membership_id, created_at) \
     VALUES (?, ?, ?, ?, ?, ?)";

const SQL_INSERT_TRANSITION: &str = "INSERT INTO group_owner_transitions \
     (transition_id, room_id, old_owner_participant_id, target_participant_id, \
      old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
      old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

const SQL_INSERT_TRANSFER_IDEMPOTENCY: &str = "INSERT INTO group_owner_transfer_idempotency \
     (human_id, key, transition_id, created_at) \
     VALUES (?, ?, ?, ?)";

const SQL_INSERT_ADVANCEMENT: &str = "INSERT INTO read_cursor_advancements \
     (advancement_id, participant_id, room_id, membership_id, from_sequence, to_sequence, \
      last_read_message_id, created_at) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

/// unread_count 投影：当前 cursor（`read_sequence`，缺失时回退到周期 join
/// boundary）之后、落在该主体任一真实 Membership 周期窗口内、且 actor
/// 不是该主体的 Message 数量；list/get 与推进响应用同一裁决。Human 与
/// Agent 走同一条（202609270001 起游标归属 `participant_id`）。
const SQL_COUNT_UNREAD: &str = "SELECT COUNT(*) FROM messages m \
     WHERE m.room_id = ? AND m.sequence > ? AND m.actor_id <> ? \
       AND EXISTS (SELECT 1 FROM room_memberships w \
                   WHERE w.room_id = m.room_id AND w.participant_id = ? \
                     AND w.join_after_sequence < m.sequence \
                     AND (w.left_before_sequence IS NULL OR m.sequence <= w.left_before_sequence))";

type RoomRow = (String, String, String, i64, Option<String>);
type MembershipRow = (
    String,         // membership_id
    String,         // room_id
    String,         // participant_id
    String,         // role
    i64,            // joined_at
    i64,            // join_after_sequence
    Option<i64>,    // left_at
    Option<i64>,    // left_before_sequence
    Option<String>, // closed_by
    i64,            // active_marker
);

/// list/get 共用行：Room + 当前 Human 的 active Membership 周期 + 对端
/// Participant（nullable）+ 当前 Human 的 read state（cursor 的
/// last_read_message_id 与 unread_count 投影）。字段顺序必须与 SELECT
/// 一致。
#[derive(Debug, sqlx::FromRow)]
struct ListItemRow {
    r_id: String,
    r_kind: String,
    r_status: String,
    r_created_at: i64,
    r_name: Option<String>,
    m_membership_id: String,
    m_room_id: String,
    m_participant: String,
    m_role: String,
    m_joined_at: i64,
    m_join_after: i64,
    m_left_at: Option<i64>,
    m_left_before: Option<i64>,
    m_closed_by: Option<String>,
    m_active: i64,
    c_last_read_message_id: Option<String>,
    unread_count: i64,
    peer_read: Option<String>,
    attention: String,
    p_id: Option<String>,
    p_kind: Option<String>,
    p_status: Option<String>,
    p_display_name: Option<String>,
}

/// 成员列表行：Membership 周期 + Participant 最小资料。
#[derive(Debug, sqlx::FromRow)]
struct MemberRow {
    m_membership_id: String,
    m_room_id: String,
    m_participant: String,
    m_role: String,
    m_joined_at: i64,
    m_join_after: i64,
    m_left_at: Option<i64>,
    m_left_before: Option<i64>,
    m_closed_by: Option<String>,
    m_active: i64,
    p_id: String,
    p_kind: String,
    p_status: String,
    p_display_name: String,
}

/// Agent 通讯录（切片 B1）单房目录行：Room + 机械显示名 + 最近可见消息
/// 投影 + since 增量计数 + read state（该 agent 的 cursor 位置与未读）+
/// 房间创建时间（排序回退基准）。字段顺序必须与 SELECT 一致。
#[derive(Debug, sqlx::FromRow)]
struct BackendDirectoryRow {
    r_id: String,
    r_kind: String,
    r_display_name: Option<String>,
    last_activity_ms: Option<i64>,
    preview: Option<String>,
    since_count: i64,
    c_last_read_message_id: Option<String>,
    unread_count: i64,
    peer_read: Option<String>,
    attention: String,
    r_created_at: i64,
}

/// 基于同一 SqlitePool 的 Room 存储。
#[derive(Clone)]
pub struct SqliteRoomStore {
    pool: SqlitePool,
    #[cfg(test)]
    commit_checkpoint: Arc<CommitCheckpoint>,
}

impl SqliteRoomStore {
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

fn db_error(e: sqlx::Error) -> RoomStoreError {
    RoomStoreError::Database(StorageError::from(e))
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> RoomStoreError {
    RoomStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn room_from_row((id, kind, status, created_at, name): RoomRow) -> Result<Room, RoomStoreError> {
    Ok(Room {
        id: RoomId(id),
        kind: RoomKind::parse(&kind)
            .ok_or_else(|| corrupt(format!("unknown room kind {kind:?}")))?,
        status: RoomStatus::parse(&status)
            .ok_or_else(|| corrupt(format!("unknown room status {status:?}")))?,
        name,
        created_at: UtcMillis::from_millis(created_at),
    })
}

fn membership_from_row(
    (
        membership_id,
        room_id,
        participant_id,
        role,
        joined_at,
        join_after_sequence,
        left_at,
        left_before_sequence,
        closed_by,
        active_marker,
    ): MembershipRow,
) -> Result<Membership, RoomStoreError> {
    let status = if active_marker == 1 {
        MembershipStatus::Active
    } else if active_marker == 0 {
        MembershipStatus::Closed
    } else {
        return Err(corrupt(format!(
            "unknown membership active_marker {active_marker:?}"
        )));
    };
    let closed_by = match closed_by.as_deref() {
        None => None,
        Some(value) => Some(
            MembershipClosedBy::parse(value)
                .ok_or_else(|| corrupt(format!("unknown membership closed_by {value:?}")))?,
        ),
    };
    if (status == MembershipStatus::Active && closed_by.is_some())
        || (status == MembershipStatus::Closed && closed_by.is_none())
    {
        return Err(corrupt("inconsistent membership period row"));
    }
    Ok(Membership {
        membership_id: MembershipId(membership_id),
        room_id: RoomId(room_id),
        participant_id: ParticipantId(participant_id),
        role: MembershipRole::parse(&role)
            .ok_or_else(|| corrupt(format!("unknown membership role {role:?}")))?,
        status,
        joined_at: UtcMillis::from_millis(joined_at),
        join_after_sequence,
        left_at: left_at.map(UtcMillis::from_millis),
        left_before_sequence,
        closed_by,
    })
}

fn profile_from_row(
    (id, kind, status, display_name): (String, String, String, String),
) -> Result<ParticipantProfile, RoomStoreError> {
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

/// 解析「对方读没读我最后那条」那一列（契约 `CORE_V1_READ_RECEIPT.md` §2）。
///
/// `NULL` ＝ **问题不成立**（不是 `direct` 房 / 对方不是 active 成员 / 我还没说过话）
/// ⇒ `None`；认不出的取值**不猜**（内部不变量破坏 ⇒ corrupt）。
fn peer_read_from_column(column: Option<String>) -> Result<Option<PeerRead>, RoomStoreError> {
    match column {
        None => Ok(None),
        Some(value) => PeerRead::parse(&value)
            .map(Some)
            .ok_or_else(|| corrupt(format!("unknown peer_read value {value:?}"))),
    }
}

/// 解析提醒档那一列（契约 `CORE_V1_IM_CONTRACT.md` §9.1）。
///
/// SQL 已经把「缺行」投影成 `'all'`（`ATTENTION_SQL`），所以这里恒有值；
/// 认不出的取值**不猜**（内部不变量破坏 ⇒ corrupt）。
fn attention_from_column(column: String) -> Result<AttentionLevel, RoomStoreError> {
    AttentionLevel::parse(&column)
        .ok_or_else(|| corrupt(format!("unknown room attention level {column:?}")))
}

fn list_item_from_row(row: ListItemRow) -> Result<RoomListItem, RoomStoreError> {
    let ListItemRow {
        r_id,
        r_kind,
        r_status,
        r_created_at,
        r_name,
        m_membership_id,
        m_room_id,
        m_participant,
        m_role,
        m_joined_at,
        m_join_after,
        m_left_at,
        m_left_before,
        m_closed_by,
        m_active,
        c_last_read_message_id,
        unread_count,
        peer_read,
        attention,
        p_id,
        p_kind,
        p_status,
        p_display_name,
    } = row;
    Ok(RoomListItem {
        room: room_from_row((r_id, r_kind, r_status, r_created_at, r_name))?,
        membership: membership_from_row((
            m_membership_id,
            m_room_id,
            m_participant,
            m_role,
            m_joined_at,
            m_join_after,
            m_left_at,
            m_left_before,
            m_closed_by,
            m_active,
        ))?,
        read_state: ReadState {
            last_read_message_id: c_last_read_message_id.map(MessageId),
            unread_count,
        },
        peer_read: peer_read_from_column(peer_read)?,
        attention: attention_from_column(attention)?,
        peer: match (p_id, p_kind, p_status, p_display_name) {
            (Some(id), Some(kind), Some(status), Some(display_name)) => {
                Some(profile_from_row((id, kind, status, display_name))?)
            }
            (None, None, None, None) => None,
            _ => return Err(corrupt("inconsistent peer projection row")),
        },
    })
}

/// Agent 通讯录目录行解析（切片 B1）：机械显示名（`COALESCE(标题, 对端名)`
/// 恒非空——group 恒有群名、direct/human_direct 恒有配对对端；理论不可空，
/// 空即内部不变量破坏 → corrupt）。`since_count` 与 `read_state.unread_count`
/// 恒非负（COUNT 投影）；`read_state` 与 Human 侧同一投影形状
/// （CORE_V1_AGENT_READ_CURSOR.md §3）。
fn backend_directory_from_row(
    row: BackendDirectoryRow,
) -> Result<BackendDirectoryRoom, RoomStoreError> {
    let BackendDirectoryRow {
        r_id,
        r_kind,
        r_display_name,
        last_activity_ms,
        preview,
        since_count,
        c_last_read_message_id,
        unread_count,
        peer_read,
        attention,
        r_created_at,
    } = row;
    Ok(BackendDirectoryRoom {
        room_id: RoomId(r_id),
        kind: RoomKind::parse(&r_kind)
            .ok_or_else(|| corrupt(format!("unknown room kind {r_kind:?}")))?,
        display_name: r_display_name
            .ok_or_else(|| corrupt("directory display name cannot be null"))?,
        room_created_at: UtcMillis::from_millis(r_created_at),
        last_activity_at: last_activity_ms.map(UtcMillis::from_millis),
        preview,
        since_count,
        read_state: ReadState {
            last_read_message_id: c_last_read_message_id.map(MessageId),
            unread_count,
        },
        peer_read: peer_read_from_column(peer_read)?,
        attention: attention_from_column(attention)?,
    })
}

/// **「对方读没读我最后那条」的投影表达式**（契约 `CORE_V1_READ_RECEIPT.md` §2）
/// ——**一处权威**：Human 侧房间列表（`fetch_list_items`）与 Agent 侧通讯录
/// （`list_backend_rooms`）**共用这一段**，不各写一遍判据。
///
/// 语义（三态里只有两态，第三种是"问题不成立"）：
///
/// - **不是 `direct` 房**（`dp.room_id IS NULL`）⇒ `NULL`（这一格不出现）；
/// - **我在这间房还没说过话**（没有"我最后那条"）⇒ `NULL`；
/// - **对方当前不是这间房的 active 成员**（没有 active 周期）⇒ `NULL`
///   ——她不在里面了，读位置无从谈起；
/// - 否则：对方的读位置（cursor 的 `read_sequence`；cursor 缺失时按既有防御性
///   回退到周期 join boundary，与 `read_state` 那条同律）**≥** 我最后那条的
///   `sequence` ⇒ `'read'`，否则 `'unread'`。
///
/// ⚠ **参数**：`?` 出现**两次**，都绑**问这一格的那个人自己的 id**（"我最后那条"
/// 里的"我"）；`p.id` 是对端（`direct` 配对里的另一位）。⚠ **不改任何写路径**：
/// 它只读 `messages` / `room_memberships` / `read_cursors` 三张既有表。
const PEER_READ_SQL: &str = "\
CASE \
  WHEN dp.room_id IS NULL THEN NULL \
  WHEN (SELECT MAX(mine.sequence) FROM messages mine \
         WHERE mine.room_id = r.id AND mine.actor_id = ?) IS NULL THEN NULL \
  WHEN (SELECT COALESCE(prc.read_sequence, pm.join_after_sequence) \
          FROM room_memberships pm \
          LEFT JOIN read_cursors prc ON prc.membership_id = pm.membership_id \
         WHERE pm.room_id = r.id AND pm.participant_id = p.id \
           AND pm.active_marker = 1) IS NULL THEN NULL \
  WHEN (SELECT COALESCE(prc.read_sequence, pm.join_after_sequence) \
          FROM room_memberships pm \
          LEFT JOIN read_cursors prc ON prc.membership_id = pm.membership_id \
         WHERE pm.room_id = r.id AND pm.participant_id = p.id \
           AND pm.active_marker = 1) \
       >= (SELECT MAX(mine.sequence) FROM messages mine \
             WHERE mine.room_id = r.id AND mine.actor_id = ?) \
    THEN 'read' \
  ELSE 'unread' \
END";

/// **提醒档的投影表达式**（契约 `CORE_V1_IM_CONTRACT.md` §9.1）——**一处权威**：
/// Human 侧房间列表（`fetch_list_items`）与 Agent 侧通讯录（`list_backend_rooms`）
/// **共用这一段**，不各写一遍。
///
/// **缺行 = `'all'`**：这一档不预建行（与读游标的「缺行＝按 join boundary 兜底」
/// 同族），所以投影必须把"没设过"读成**默认档**——不是 NULL、也不是"未知"。
/// 两人房（`direct` / `human_direct`）里守卫根本不让落行，于是它们恒投影成
/// `all`，**不需要按 kind 特判**。
///
/// ⚠ **无占位符**：JOIN 谓词用的是本次查询那个主体自己的 active 周期
/// （`m.participant_id`），因此它不改变任何一条既有查询的绑定顺序。
const ATTENTION_SQL: &str = "COALESCE(ra.level, 'all')";

/// 提醒档的 JOIN 前缀（与 `ATTENTION_SQL` 是一对；两张表的主键就是身份）。
const ATTENTION_JOIN: &str =
    "LEFT JOIN room_attention ra ON ra.room_id = r.id AND ra.participant_id = m.participant_id";

/// 当前 Human 可见的 Room 管理视图查询（list 与 get 共用同一裁决：
/// 当前 Human 必须存在该 Room 的 **active** Membership 周期；对端是
/// direct/human_direct 配对中的另一位 Participant，group 无对端）。
/// read state 投影：LEFT JOIN 当前周期的 read cursor（成员周期与 cursor
/// 一对一，migration 010 触发器保证 Human 周期必有 cursor；cursor 缺失时
/// 防御性回退到周期 join boundary，last_read_message_id 为 null）。
/// `room_id` 为 None 时列出全部并按创建时间升序、再按 ID 排序。离群后
/// 无 active 周期，fail closed：list 不包含、get 返回 None。
async fn fetch_list_items(
    conn: &mut SqliteConnection,
    human_id: &str,
    room_id: Option<&str>,
) -> Result<Vec<RoomListItem>, RoomStoreError> {
    // `{PEER_READ_SQL}` 是一段**共享的判据**（一处权威，Human 侧与 Agent 侧共用）。
    let mut query = format!(
        "SELECT r.id AS r_id, r.kind AS r_kind, r.status AS r_status, r.created_at AS r_created_at, \
                r.name AS r_name, \
                m.membership_id AS m_membership_id, m.room_id AS m_room_id, \
                m.participant_id AS m_participant, m.role AS m_role, \
                m.joined_at AS m_joined_at, m.join_after_sequence AS m_join_after, \
                m.left_at AS m_left_at, m.left_before_sequence AS m_left_before, \
                m.closed_by AS m_closed_by, m.active_marker AS m_active, \
                c.last_read_message_id AS c_last_read_message_id, \
                (SELECT COUNT(*) FROM messages un \
                 WHERE un.room_id = r.id AND un.sequence > COALESCE(c.read_sequence, m.join_after_sequence) \
                   AND un.actor_id <> ? \
                   AND EXISTS (SELECT 1 FROM room_memberships w \
                               WHERE w.room_id = un.room_id AND w.participant_id = ? \
                                 AND w.join_after_sequence < un.sequence \
                                 AND (w.left_before_sequence IS NULL OR un.sequence <= w.left_before_sequence))) \
                 AS unread_count, \
                {PEER_READ_SQL} AS peer_read, \
                {ATTENTION_SQL} AS attention, \
                p.id AS p_id, p.kind AS p_kind, p.status AS p_status, \
                p.display_name AS p_display_name \
         FROM rooms r \
         JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = ? AND m.active_marker = 1 \
         LEFT JOIN read_cursors c ON c.membership_id = m.membership_id \
         {ATTENTION_JOIN} \
         LEFT JOIN direct_room_pairs dp ON dp.room_id = r.id \
         LEFT JOIN human_direct_pairs hp ON hp.room_id = r.id \
         LEFT JOIN participants p ON p.id = CASE \
             WHEN dp.room_id IS NOT NULL THEN CASE WHEN dp.human_id = ? THEN dp.agent_id ELSE dp.human_id END \
             WHEN hp.room_id IS NOT NULL THEN CASE WHEN hp.human_a_id = ? THEN hp.human_b_id ELSE hp.human_a_id END \
             ELSE NULL END",
    );
    if room_id.is_some() {
        query.push_str(" WHERE r.id = ?");
    }
    query.push_str(" ORDER BY r.created_at ASC, r.id ASC");

    // 绑定顺序 = 占位符在 SQL 文本里的出现顺序（`peer_read` 那一段在
    // `unread_count` 之后、JOIN 之前 ⇒ 它的两次"我"紧跟在前两次之后）。
    let mut q = sqlx::query_as::<_, ListItemRow>(&query)
        .bind(human_id)
        .bind(human_id)
        .bind(human_id)
        .bind(human_id)
        .bind(human_id)
        .bind(human_id)
        .bind(human_id);
    if let Some(id) = room_id {
        q = q.bind(id);
    }
    let rows = q.fetch_all(&mut *conn).await.map_err(db_error)?;
    rows.into_iter().map(list_item_from_row).collect()
}

/// 读取单个 Room 的全部 Membership 周期（owner 在前，其余按 joined_at
/// 升序、再按 Participant ID 排序；含 closed 周期，供 direct/human_direct
/// 配对成员展示与幂等重放使用）。
async fn fetch_memberships(
    conn: &mut SqliteConnection,
    room_id: &str,
) -> Result<Vec<Membership>, RoomStoreError> {
    let rows: Vec<MembershipRow> = sqlx::query_as(
        "SELECT membership_id, room_id, participant_id, role, joined_at, \
                join_after_sequence, left_at, left_before_sequence, closed_by, active_marker \
         FROM room_memberships WHERE room_id = ? \
         ORDER BY CASE WHEN role = 'owner' THEN 0 ELSE 1 END, joined_at, participant_id",
    )
    .bind(room_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_error)?;
    rows.into_iter().map(membership_from_row).collect()
}

/// 按 membership_id 读取单个 Membership 周期（幂等重放用）。
async fn fetch_membership(
    conn: &mut SqliteConnection,
    membership_id: &str,
) -> Result<Membership, RoomStoreError> {
    let row: Option<MembershipRow> = sqlx::query_as(
        "SELECT membership_id, room_id, participant_id, role, joined_at, \
                join_after_sequence, left_at, left_before_sequence, closed_by, active_marker \
         FROM room_memberships WHERE membership_id = ?",
    )
    .bind(membership_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    row.map(membership_from_row)
        .transpose()?
        .ok_or_else(|| corrupt("idempotency record points to missing membership period"))
}

/// 读取 Room 公共行（幂等重放用）。
async fn fetch_room(conn: &mut SqliteConnection, room_id: &str) -> Result<Room, RoomStoreError> {
    let row: Option<RoomRow> =
        sqlx::query_as("SELECT id, kind, status, created_at, name FROM rooms WHERE id = ?")
            .bind(room_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_error)?;
    row.map(room_from_row)
        .transpose()?
        .ok_or_else(|| corrupt("idempotency record points to missing room"))
}

/// 读取重放所需的 Room + 全部 Membership 周期。
async fn fetch_replay(
    conn: &mut SqliteConnection,
    room_id: &str,
) -> Result<(Room, Vec<Membership>), RoomStoreError> {
    let room = fetch_room(conn, room_id).await?;
    let memberships = fetch_memberships(conn, room_id).await?;
    if (room.kind == RoomKind::Direct || room.kind == RoomKind::HumanDirect)
        && memberships.len() != 2
    {
        return Err(corrupt(format!(
            "room {room_id} has {} memberships, expected 2",
            memberships.len()
        )));
    }
    Ok((room, memberships))
}

/// 事务内读取该 Room 当前最大 message sequence（周期边界分配用，必须
/// 在 BEGIN IMMEDIATE 内调用）。
async fn max_message_sequence(
    conn: &mut SqliteConnection,
    room_id: &str,
) -> Result<i64, RoomStoreError> {
    let max: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM messages WHERE room_id = ?")
            .bind(room_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_error)?;
    Ok(max)
}

/// 当前 Human 在该 Room 的未读投影（契约 CORE_V1_READ_STATE.md 3.3）：
/// cursor（`read_sequence`）之后、落在该 Human 任一真实 Membership 周期
/// 窗口内、且 actor 不是该 Human 的 Message 数量；与 list/get 共用同一
/// 裁决，不建立第二张计数器表。
async fn count_unread(
    conn: &mut SqliteConnection,
    room_id: &str,
    human_id: &str,
    read_sequence: i64,
) -> Result<i64, RoomStoreError> {
    let count: i64 = sqlx::query_scalar(SQL_COUNT_UNREAD)
        .bind(room_id)
        .bind(read_sequence)
        .bind(human_id)
        .bind(human_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_error)?;
    Ok(count)
}

/// 业务失败分支的 primary 稳定分类（CORE_V1_ERROR_HANDLING.md §7.2）：
/// Database 沿用嵌套 kind，业务变体归 Unknown（内部事务完成失败分类）。
fn primary_storage_kind(e: &RoomStoreError) -> StorageFailureKind {
    match e {
        RoomStoreError::Database(inner) => inner.kind(),
        _ => StorageFailureKind::Unknown,
    }
}

/// 校验调用者是否是目标 Room 的 active group owner（invite/remove 共用）。
async fn require_active_group_owner(
    conn: &mut SqliteConnection,
    owner_id: &str,
    room_id: &str,
) -> Result<(), RoomStoreError> {
    let ok: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM rooms r \
         JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = ? \
         WHERE r.id = ? AND r.kind = 'group' AND m.role = 'owner' AND m.active_marker = 1 \
         LIMIT 1",
    )
    .bind(owner_id)
    .bind(room_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    if ok.is_none() {
        return Err(RoomStoreError::NotFound);
    }
    Ok(())
}

/// 校验邀请目标资格：**一条判据，不分邀请者与目标的 kind**（批次三第二刀）——
/// 目标 active，且（与邀请者有 accepted 联系人关系 **或** 被邀请者以 active
/// Agent Owner 拥有）；否则统一 MemberNotFound。
///
/// 为什么是"并集"而不是"按 kind 二选一"：所有权守卫本来就只可能 human→agent，
/// 所以第二条分支天然自限；而第一条分支在关系放开之后对**任意**主体成立。
/// 这条判据与数据库守卫 `room_memberships_group_member_qualification` 同形——
/// 同一不变量不允许两处各写一套。
async fn require_invitable_target(
    conn: &mut SqliteConnection,
    owner_id: &str,
    participant_id: &str,
) -> Result<(), RoomStoreError> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM participants WHERE id = ?")
        .bind(participant_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
    let Some(status) = status else {
        return Err(RoomStoreError::MemberNotFound);
    };
    if status != ParticipantStatus::Active.as_str() {
        return Err(RoomStoreError::MemberNotFound);
    }
    let (a, b) = canonical_pair(
        &ParticipantId(owner_id.to_owned()),
        &ParticipantId(participant_id.to_owned()),
    );
    // 资格 = **归属 或 关系**（批次三第二刀收成的并集；第三刀第③步确认它是对的形状：
    // Agent 是被人类用户拉进来的，ownership 是资格之一）。与数据库守卫
    // `room_memberships_group_member_qualification` **同形**——"一条判据，两处同形"。
    let related: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM contact_periods \
         WHERE a_id = ? AND b_id = ? AND state = 'accepted' AND active_marker = 1 \
         UNION ALL \
         SELECT 1 FROM agent_owners \
         WHERE agent_id = ? AND human_id = ? AND status = 'active' \
         LIMIT 1",
    )
    .bind(a.as_str())
    .bind(b.as_str())
    .bind(participant_id)
    .bind(owner_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    if related.is_none() {
        return Err(RoomStoreError::MemberNotFound);
    }
    Ok(())
}

/// 事务内关闭一个 active Membership 周期（closed 是终态，关闭后整行不可
/// 变）。closed_by 决定合法性，由数据库 close guard 兜底。
async fn close_membership_period(
    conn: &mut SqliteConnection,
    membership_id: &str,
    left_at_millis: i64,
    left_before_sequence: i64,
    closed_by: &str,
) -> Result<(), RoomStoreError> {
    sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 0, left_at = ?, left_before_sequence = ?, closed_by = ? \
         WHERE membership_id = ? AND active_marker = 1",
    )
    .bind(left_at_millis)
    .bind(left_before_sequence)
    .bind(closed_by)
    .bind(membership_id)
    .execute(&mut *conn)
    .await
    .map_err(db_error)?;
    Ok(())
}

/// 事务内打开一个 active Membership 周期。
async fn insert_membership_period(
    conn: &mut SqliteConnection,
    membership: &Membership,
) -> Result<(), RoomStoreError> {
    sqlx::query(SQL_INSERT_MEMBERSHIP)
        .bind(membership.membership_id.as_str())
        .bind(membership.room_id.as_str())
        .bind(membership.participant_id.as_str())
        .bind(membership.role.as_str())
        .bind(membership.joined_at.millis())
        .bind(membership.join_after_sequence)
        .execute(&mut *conn)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// 事务内插入一条群主转让事实（四条周期以 deferred 复合外键精确见证，
/// 新周期 FK 延迟到 COMMIT 裁决，因此必须先于新周期插入）。参数即
/// group_owner_transitions 的固定列集，不另行造类型。
#[allow(clippy::too_many_arguments)]
async fn insert_owner_transition(
    conn: &mut SqliteConnection,
    transition_id: &str,
    room_id: &str,
    old_owner_participant_id: &str,
    target_participant_id: &str,
    old_owner_membership_id: &str,
    target_member_membership_id: &str,
    new_owner_membership_id: &str,
    old_owner_new_member_membership_id: &str,
    occurred_at_millis: i64,
    boundary_sequence: i64,
) -> Result<(), RoomStoreError> {
    sqlx::query(SQL_INSERT_TRANSITION)
        .bind(transition_id)
        .bind(room_id)
        .bind(old_owner_participant_id)
        .bind(target_participant_id)
        .bind(old_owner_membership_id)
        .bind(target_member_membership_id)
        .bind(new_owner_membership_id)
        .bind(old_owner_new_member_membership_id)
        .bind(occurred_at_millis)
        .bind(boundary_sequence)
        .execute(&mut *conn)
        .await
        .map_err(db_error)?;
    Ok(())
}

#[async_trait::async_trait]
impl crate::ports::room::RoomStore for SqliteRoomStore {
    async fn create_direct_room(
        &self,
        human_id: &ParticipantId,
        room: &NewDirectRoom,
        idempotency_key: &IdempotencyKey,
    ) -> Result<CreateDirectRoomOutcome, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<CreateDirectRoomOutcome, RoomStoreError> = async {
            let existing: Option<(String, String)> = sqlx::query_as(
                "SELECT agent_id, room_id FROM direct_room_idempotency \
                 WHERE human_id = ? AND key = ?",
            )
            .bind(human_id.as_str())
            .bind(idempotency_key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;

            if let Some((existing_agent, existing_room)) = existing {
                if existing_agent != room.agent_id.as_str() {
                    return Err(RoomStoreError::IdempotencyConflict);
                }
                let (replay_room, memberships) = fetch_replay(&mut tx, &existing_room).await?;
                return Ok(CreateDirectRoomOutcome::Replayed {
                    room: replay_room,
                    memberships,
                });
            }

            let agent_row: Option<(String, String)> =
                sqlx::query_as("SELECT kind, status FROM participants WHERE id = ?")
                    .bind(room.agent_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some((kind, status)) = agent_row else {
                return Err(RoomStoreError::NotFound);
            };
            if kind != ParticipantKind::Agent.as_str() {
                return Err(RoomStoreError::NotFound);
            }
            if status != ParticipantStatus::Active.as_str() {
                return Err(RoomStoreError::AgentNotActive);
            }

            // 一间房 = 一段关系（批次二）：这对 Human–Agent 的**私人房至多一间**
            //（每段关系一间）。已经有了就不再开第二间——把这条幂等
            // 记录也指向那间房，并原样回放：IM 里"再点一次同一个联系人"就是回到
            // 你们那间房，不是再开一间。（2026-09-28 服务层退役前还有"服务房是
            // 另一段关系、另一条路径"这一句——那条路径已不存在。）
            //
            // ⚠ **重放排在准入之前**（批次三第三刀 第③步）：准入从此要求"双方确认过的
            // 关系"，而既有房是**已经发生过的事实**——把它排在准入后面，就会让
            // "关系结束之后，你们那间房再也回不去"成为一条新的静默卡死。重放不写、
            // 不改任何事实（本函数对 `Replayed` 不 commit）。
            let existing_private: Option<String> = sqlx::query_scalar(
                "SELECT room_id FROM direct_room_pairs \
                 WHERE human_id = ? AND agent_id = ?",
            )
            .bind(human_id.as_str())
            .bind(room.agent_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if let Some(existing_room) = existing_private {
                // **重放不写**：本函数对 `Replayed` 不 commit（事务直接丢弃），所以
                // 这里**不能**留幂等记录——那会是一条被回滚的白写。关系唯一性本身
                // 就是幂等依据：同一对任何时候再要一次，都回到这间房。
                let (replay_room, memberships) = fetch_replay(&mut tx, &existing_room).await?;
                return Ok(CreateDirectRoomOutcome::Replayed {
                    room: replay_room,
                    memberships,
                });
            }

            // 准入 = **归属 或 关系**（批次三第三刀 第③步，主人 2026-09-27 裁决：
            //「像飞书 QQ 那种，Agent 要进来，肯定是人类用户拉他进来得」）——ownership
            // 本来就是准入资格之一，不是"代它答应"。补上"关系"这一支，消掉了第二刀
            // 登记的缺口：Human 与"自己并不拥有的 Agent"建立 accepted 关系之后，
            // 终于有了开房路径。判据与群成员资格**同形**（一条判据，两处同形）。
            let (a_id, b_id) = canonical_pair(human_id, &room.agent_id);
            let admitted: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM agent_owners \
                 WHERE agent_id = ? AND human_id = ? AND status = 'active' \
                 UNION ALL \
                 SELECT 1 FROM contact_periods \
                 WHERE a_id = ? AND b_id = ? AND state = 'accepted' AND active_marker = 1 \
                 LIMIT 1",
            )
            .bind(room.agent_id.as_str())
            .bind(human_id.as_str())
            .bind(a_id.as_str())
            .bind(b_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if admitted.is_none() {
                // 与既有判据同口径：不归属、也没有关系 ⇒ 统一 404（不泄露 Agent 是否存在）。
                // 本刀**不**改这个码——"归属或关系"是**放宽**准入，错误面一个字不动。
                return Err(RoomStoreError::NotFound);
            }

            // 房的标题恒为空（direct 房没有标题；`rooms.name` 只供 group）。
            sqlx::query(SQL_INSERT_ROOM)
                .bind(room.room_id.as_str())
                .bind(RoomKind::Direct.as_str())
                .bind(room.created_at.millis())
                .bind(Option::<String>::None)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_COVERAGE_DIRECT)
                .bind(room.room_id.as_str())
                .bind(room.created_at.millis())
                .bind(room.human_id.as_str())
                .bind(room.agent_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_PAIR)
                .bind(room.room_id.as_str())
                .bind(room.human_id.as_str())
                .bind(room.agent_id.as_str())
                .bind(room.human_membership_id.as_str())
                .bind(room.agent_membership_id.as_str())
                .bind(room.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_MEMBERSHIP)
                .bind(room.human_membership_id.as_str())
                .bind(room.room_id.as_str())
                .bind(room.human_id.as_str())
                .bind(MembershipRole::Owner.as_str())
                .bind(room.created_at.millis())
                .bind(0i64)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_MEMBERSHIP)
                .bind(room.agent_membership_id.as_str())
                .bind(room.room_id.as_str())
                .bind(room.agent_id.as_str())
                .bind(MembershipRole::Member.as_str())
                .bind(room.created_at.millis())
                .bind(0i64)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_DIRECT_IDEMPOTENCY)
                .bind(human_id.as_str())
                .bind(idempotency_key.as_str())
                .bind(room.agent_id.as_str())
                .bind(room.room_id.as_str())
                .bind(room.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            // direct 房没有标题（批次二）：`Room` 仍有 name 字段（group 在用它），
            // 这里恒为空。
            let created_room = Room::new_direct(room.room_id.clone(), None, room.created_at);
            let memberships = vec![
                Membership::new(
                    room.human_membership_id.clone(),
                    room.room_id.clone(),
                    room.human_id.clone(),
                    MembershipRole::Owner,
                    room.created_at,
                    0,
                ),
                Membership::new(
                    room.agent_membership_id.clone(),
                    room.room_id.clone(),
                    room.agent_id.clone(),
                    MembershipRole::Member,
                    room.created_at,
                    0,
                ),
            ];
            Ok(CreateDirectRoomOutcome::Created {
                room: created_room,
                memberships,
            })
        }
        .await;

        match outcome {
            Ok(created @ CreateDirectRoomOutcome::Created { .. }) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(created)
            }
            Ok(replayed @ CreateDirectRoomOutcome::Replayed { .. }) => Ok(replayed),
            Err(e) => Err(e),
        }
    }

    async fn create_human_direct_room(
        &self,
        human_id: &ParticipantId,
        room: &NewHumanDirectRoom,
        idempotency_key: &IdempotencyKey,
    ) -> Result<CreateRoomOutcome, RoomStoreError> {
        let (canonical_a, canonical_b) = canonical_pair(&room.creator_id, &room.other_id);
        // 创建者恒为 owner；canonical 顺序只保证同一对唯一，role 跟随
        // 创建者位置（schema 的 role_a/role_b 允许 owner/member 互换）。
        let (role_a, role_b) = if room.creator_id == canonical_a {
            (MembershipRole::Owner, MembershipRole::Member)
        } else {
            (MembershipRole::Member, MembershipRole::Owner)
        };
        // 奠基周期身份必须按 canonical 位置落位（配对行的
        // human_a_membership_id / human_b_membership_id 与 human_a_id /
        // human_b_id 一一对应）。
        let (a_membership_id, b_membership_id) = if room.creator_id == canonical_a {
            (
                room.creator_membership_id.clone(),
                room.other_membership_id.clone(),
            )
        } else {
            (
                room.other_membership_id.clone(),
                room.creator_membership_id.clone(),
            )
        };
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<CreateRoomOutcome, RoomStoreError> = async {
            let existing: Option<(String, String)> = sqlx::query_as(
                "SELECT peer_human_id, room_id FROM human_direct_room_idempotency \
                 WHERE human_id = ? AND key = ?",
            )
            .bind(human_id.as_str())
            .bind(idempotency_key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;

            if let Some((existing_peer, existing_room)) = existing {
                if existing_peer != room.other_id.as_str() {
                    return Err(RoomStoreError::IdempotencyConflict);
                }
                let (replay_room, memberships) = fetch_replay(&mut tx, &existing_room).await?;
                return Ok(CreateRoomOutcome::Replayed {
                    room: replay_room,
                    memberships,
                });
            }

            // 目标必须是存在且 active 的 Human。
            let target_row: Option<(String, String)> =
                sqlx::query_as("SELECT kind, status FROM participants WHERE id = ?")
                    .bind(room.other_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some((kind, status)) = target_row else {
                return Err(RoomStoreError::HumanNotFound);
            };
            if kind != ParticipantKind::Human.as_str()
                || status != ParticipantStatus::Active.as_str()
            {
                return Err(RoomStoreError::HumanNotFound);
            }

            // 只允许「当前 active 且 accepted 的关系段」创建（pending / 已拒绝 /
            // 已解除的段都不能建 Human direct Room）。
            let active_period: Option<String> = sqlx::query_scalar(
                "SELECT period_id FROM contact_periods \
                 WHERE a_id = ? AND b_id = ? AND state = 'accepted' AND active_marker = 1 \
                 LIMIT 1",
            )
            .bind(canonical_a.as_str())
            .bind(canonical_b.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some(contact_period_id) = active_period else {
                return Err(RoomStoreError::ContactNotAccepted);
            };

            // 同一段关系至多一个 Room（下一段关系可另建新房）。
            let pair_exists: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM human_direct_pairs WHERE contact_period_id = ? LIMIT 1",
            )
            .bind(&contact_period_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if pair_exists.is_some() {
                return Err(RoomStoreError::HumanDirectRoomExists);
            }

            sqlx::query(SQL_INSERT_ROOM)
                .bind(room.room_id.as_str())
                .bind(RoomKind::HumanDirect.as_str())
                .bind(room.created_at.millis())
                .bind(Option::<String>::None)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_COVERAGE_HUMAN_DIRECT)
                .bind(room.room_id.as_str())
                .bind(room.created_at.millis())
                .bind(canonical_a.as_str())
                .bind(canonical_b.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_HUMAN_DIRECT_PAIR)
                .bind(room.room_id.as_str())
                .bind(canonical_a.as_str())
                .bind(canonical_b.as_str())
                .bind(&contact_period_id)
                .bind(a_membership_id.as_str())
                .bind(b_membership_id.as_str())
                .bind(role_a.as_str())
                .bind(role_b.as_str())
                .bind(room.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_MEMBERSHIP)
                .bind(room.creator_membership_id.as_str())
                .bind(room.room_id.as_str())
                .bind(room.creator_id.as_str())
                .bind(MembershipRole::Owner.as_str())
                .bind(room.created_at.millis())
                .bind(0i64)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_MEMBERSHIP)
                .bind(room.other_membership_id.as_str())
                .bind(room.room_id.as_str())
                .bind(room.other_id.as_str())
                .bind(MembershipRole::Member.as_str())
                .bind(room.created_at.millis())
                .bind(0i64)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_HUMAN_DIRECT_IDEMPOTENCY)
                .bind(human_id.as_str())
                .bind(idempotency_key.as_str())
                .bind(room.other_id.as_str())
                .bind(canonical_a.as_str())
                .bind(canonical_b.as_str())
                .bind(room.room_id.as_str())
                .bind(room.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            let created_room = Room::new_human_direct(room.room_id.clone(), room.created_at);
            let memberships = vec![
                Membership::new(
                    room.creator_membership_id.clone(),
                    room.room_id.clone(),
                    room.creator_id.clone(),
                    MembershipRole::Owner,
                    room.created_at,
                    0,
                ),
                Membership::new(
                    room.other_membership_id.clone(),
                    room.room_id.clone(),
                    room.other_id.clone(),
                    MembershipRole::Member,
                    room.created_at,
                    0,
                ),
            ];
            Ok(CreateRoomOutcome::Created {
                room: created_room,
                memberships,
            })
        }
        .await;

        match outcome {
            Ok(created @ CreateRoomOutcome::Created { .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(created)
            }
            Ok(replayed @ CreateRoomOutcome::Replayed { .. }) => Ok(replayed),
            Err(e) => Err(e),
        }
    }

    async fn create_group(
        &self,
        human_id: &ParticipantId,
        room: &NewGroup,
        idempotency_key: &IdempotencyKey,
    ) -> Result<CreateRoomOutcome, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<CreateRoomOutcome, RoomStoreError> = async {
            let existing: Option<(String, String)> = sqlx::query_as(
                "SELECT name, room_id FROM group_create_idempotency \
                 WHERE human_id = ? AND key = ?",
            )
            .bind(human_id.as_str())
            .bind(idempotency_key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;

            if let Some((existing_name, existing_room)) = existing {
                if existing_name != room.name {
                    return Err(RoomStoreError::IdempotencyConflict);
                }
                let (replay_room, memberships) = fetch_replay(&mut tx, &existing_room).await?;
                return Ok(CreateRoomOutcome::Replayed {
                    room: replay_room,
                    memberships,
                });
            }

            sqlx::query(SQL_INSERT_ROOM)
                .bind(room.room_id.as_str())
                .bind(RoomKind::Group.as_str())
                .bind(room.created_at.millis())
                .bind(Some(room.name.as_str()))
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_COVERAGE_GROUP)
                .bind(room.room_id.as_str())
                .bind(room.created_at.millis())
                .bind(room.owner_membership_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_MEMBERSHIP)
                .bind(room.owner_membership_id.as_str())
                .bind(room.room_id.as_str())
                .bind(room.owner_id.as_str())
                .bind(MembershipRole::Owner.as_str())
                .bind(room.created_at.millis())
                .bind(0i64)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            sqlx::query(SQL_INSERT_GROUP_IDEMPOTENCY)
                .bind(human_id.as_str())
                .bind(idempotency_key.as_str())
                .bind(room.room_id.as_str())
                .bind(room.name.as_str())
                .bind(room.created_at.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            let created_room =
                Room::new_group(room.room_id.clone(), room.name.clone(), room.created_at);
            let memberships = vec![Membership::new(
                room.owner_membership_id.clone(),
                room.room_id.clone(),
                room.owner_id.clone(),
                MembershipRole::Owner,
                room.created_at,
                0,
            )];
            Ok(CreateRoomOutcome::Created {
                room: created_room,
                memberships,
            })
        }
        .await;

        match outcome {
            Ok(created @ CreateRoomOutcome::Created { .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(created)
            }
            Ok(replayed @ CreateRoomOutcome::Replayed { .. }) => Ok(replayed),
            Err(e) => Err(e),
        }
    }

    async fn invite_group_member(
        &self,
        owner_id: &ParticipantId,
        room_id: &RoomId,
        participant_id: &ParticipantId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InviteMemberOutcome, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<InviteMemberOutcome, RoomStoreError> = async {
            // 1. 先查 key 再查权限：同 key 重试只重放该 key 记录的周期
            //    （即使该周期后来关闭也只重放历史结果，绝不误开新周期）。
            let existing: Option<(String, String, String)> = sqlx::query_as(
                "SELECT room_id, participant_id, membership_id \
                 FROM group_member_invite_idempotency WHERE human_id = ? AND key = ?",
            )
            .bind(owner_id.as_str())
            .bind(idempotency_key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if let Some((key_room, key_participant, membership_id)) = existing {
                if key_room != room_id.as_str() || key_participant != participant_id.as_str() {
                    return Err(RoomStoreError::IdempotencyConflict);
                }
                let membership = fetch_membership(&mut tx, &membership_id).await?;
                return Ok(InviteMemberOutcome::Replayed { membership });
            }

            // 2. 调用者必须是该 group 的 active owner；unknown/bad-id/非
            //    owner/非 group 统一 NotFound。
            require_active_group_owner(&mut tx, owner_id.as_str(), room_id.as_str()).await?;

            // 3. owner 不能邀请自己：在 active lookup 前显式识别，返回既有
            //    MemberNotFound；不写幂等记录（「目标已有 active 周期返回
            //    200」不包括 owner 自己）。
            if participant_id.as_str() == owner_id.as_str() {
                return Err(RoomStoreError::MemberNotFound);
            }

            // 4. 已有 active 周期：新 key 记录到该 active 周期并返回 200
            //    （不同 key 并发同目标最终只有一个 active 周期），不重复
            //    打开周期。
            let active: Option<MembershipRow> = sqlx::query_as(
                "SELECT membership_id, room_id, participant_id, role, joined_at, \
                        join_after_sequence, left_at, left_before_sequence, closed_by, active_marker \
                 FROM room_memberships \
                 WHERE room_id = ? AND participant_id = ? AND active_marker = 1 \
                 LIMIT 1",
            )
            .bind(room_id.as_str())
            .bind(participant_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if let Some(row) = active {
                let membership = membership_from_row(row)?;
                let created_at = UtcMillis::now().millis();
                sqlx::query(SQL_INSERT_INVITE_IDEMPOTENCY)
                    .bind(owner_id.as_str())
                    .bind(idempotency_key.as_str())
                    .bind(room_id.as_str())
                    .bind(participant_id.as_str())
                    .bind(membership.membership_id.as_str())
                    .bind(created_at)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                return Ok(InviteMemberOutcome::AlreadyActive { membership });
            }

            // 5. 目标资格：accepted Human 或自己拥有且 active 的 Agent；
            //    历史 closed 周期不阻塞重入（只要资格仍满足即可用新 key
            //    重入）。
            require_invitable_target(&mut tx, owner_id.as_str(), participant_id.as_str())
                .await?;

            // 6. 打开新 active 周期（join_after_sequence = 当前最大
            //    sequence，只对新周期期间的消息可见）；数据库 INSERT 守卫
            //    在同一事务内再次证明资格。
            let created_at = UtcMillis::now().millis();
            let join_after = max_message_sequence(&mut tx, room_id.as_str()).await?;
            let membership = Membership::new(
                MembershipId::new_v4(),
                room_id.clone(),
                participant_id.clone(),
                MembershipRole::Member,
                UtcMillis::from_millis(created_at),
                join_after,
            );
            insert_membership_period(&mut tx, &membership).await?;
            sqlx::query(SQL_INSERT_INVITE_IDEMPOTENCY)
                .bind(owner_id.as_str())
                .bind(idempotency_key.as_str())
                .bind(room_id.as_str())
                .bind(participant_id.as_str())
                .bind(membership.membership_id.as_str())
                .bind(created_at)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            Ok(InviteMemberOutcome::Created { membership })
        }
        .await;

        match outcome {
            Ok(created @ InviteMemberOutcome::Created { .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(created)
            }
            Ok(replayed @ InviteMemberOutcome::Replayed { .. }) => Ok(replayed),
            Ok(already @ InviteMemberOutcome::AlreadyActive { .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(already)
            }
            Err(e) => Err(e),
        }
    }

    async fn remove_group_member(
        &self,
        owner_id: &ParticipantId,
        room_id: &RoomId,
        participant_id: &ParticipantId,
    ) -> Result<MembershipId, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<MembershipId, RoomStoreError> = async {
            // 1. 调用者必须是该 group 的 active owner。
            require_active_group_owner(&mut tx, owner_id.as_str(), room_id.as_str()).await?;

            // 2. 目标必须是该 Room 的 active 成员周期。
            let target: Option<(String, String)> = sqlx::query_as(
                "SELECT membership_id, role FROM room_memberships \
                 WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
            )
            .bind(room_id.as_str())
            .bind(participant_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some((membership_id, role)) = target else {
                return Err(RoomStoreError::MemberNotFound);
            };
            if role == MembershipRole::Owner.as_str() {
                return Err(RoomStoreError::OwnerNotRemovable);
            }

            // 3. 移除 = 周期进入 closed 终态（closed_by=removed，固化
            //    left_before_sequence）；数据库守卫证明这是 group
            //    non-owner 成员的合法移除路径。
            let left_at = UtcMillis::now().millis();
            let left_before = max_message_sequence(&mut tx, room_id.as_str()).await?;
            close_membership_period(&mut tx, &membership_id, left_at, left_before, "removed")
                .await?;
            Ok(MembershipId(membership_id))
        }
        .await;

        match outcome {
            Ok(closed_membership_id) => {
                tx.commit().await.map_err(db_error)?;
                Ok(closed_membership_id)
            }
            Err(e) => Err(e),
        }
    }

    async fn leave_membership(
        &self,
        participant_id: &ParticipantId,
        room_id: &RoomId,
    ) -> Result<MembershipId, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<MembershipId, RoomStoreError> = async {
            // 1. 当前参与者在目标房必须有 active 周期；unknown/bad-id/无周期
            //    统一 NotFound。**房 kind 只排除 human_direct**（2026-09-27 M2
            //    切片）：那间房的出口归关系模型（contact_ended + 精确见证），
            //    不是「我在一个房里」。group / direct 都可 leave——不分参与者
            //    kind（Human 在 direct 房恒为 owner，由下面第 2 步的既有规则挡住）。
            let target: Option<(String, String)> = sqlx::query_as(
                "SELECT r.kind, m.membership_id FROM rooms r \
                 JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = ? \
                 WHERE r.id = ? AND m.active_marker = 1",
            )
            .bind(participant_id.as_str())
            .bind(room_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some((kind, membership_id)) = target else {
                return Err(RoomStoreError::NotFound);
            };
            if kind == RoomKind::HumanDirect.as_str() {
                return Err(RoomStoreError::NotFound);
            }

            // 2. owner 必须先转让才能退出（任何房 kind 同一规则）。
            let is_owner: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM room_memberships \
                 WHERE membership_id = ? AND role = 'owner' LIMIT 1",
            )
            .bind(&membership_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if is_owner.is_some() {
                return Err(RoomStoreError::OwnerTransferRequired);
            }

            // 3. 关闭周期（closed_by=leave）。
            let left_at = UtcMillis::now().millis();
            let left_before = max_message_sequence(&mut tx, room_id.as_str()).await?;
            close_membership_period(&mut tx, &membership_id, left_at, left_before, "leave").await?;
            Ok(MembershipId(membership_id))
        }
        .await;

        match outcome {
            Ok(closed_membership_id) => {
                tx.commit().await.map_err(db_error)?;
                Ok(closed_membership_id)
            }
            Err(e) => Err(e),
        }
    }

    async fn transfer_group_owner(
        &self,
        owner_id: &ParticipantId,
        room_id: &RoomId,
        target_id: &ParticipantId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<TransferOwnerOutcome, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<TransferOwnerOutcome, RoomStoreError> = async {
            // 1. 先查 key 再查权限：同 key 重试 JOIN 不可变 transition 读取
            //    Room/目标/新 owner 周期，只重放历史结果。
            let existing: Option<(String, String, String)> = sqlx::query_as(
                "SELECT t.room_id, t.target_participant_id, t.new_owner_membership_id \
                 FROM group_owner_transfer_idempotency i \
                 JOIN group_owner_transitions t ON t.transition_id = i.transition_id \
                 WHERE i.human_id = ? AND i.key = ?",
            )
            .bind(owner_id.as_str())
            .bind(idempotency_key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if let Some((key_room, key_target, new_owner_membership_id)) = existing {
                if key_room != room_id.as_str() || key_target != target_id.as_str() {
                    return Err(RoomStoreError::IdempotencyConflict);
                }
                let membership = fetch_membership(&mut tx, &new_owner_membership_id).await?;
                return Ok(TransferOwnerOutcome::Replayed { membership });
            }

            // 2. 调用者必须是该 group 的 active owner。
            require_active_group_owner(&mut tx, owner_id.as_str(), room_id.as_str()).await?;

            // 3. 目标不能是自己。目标可以是 Human 或 Agent：owner 是房内的
            //    治理角色（`role`），不是参与者种类（`kind`）——2026-09-27 起
            //    「owner = 创建者」，转让判据与创建者同形。
            if target_id.as_str() == owner_id.as_str() {
                return Err(RoomStoreError::InvalidTransferTarget);
            }

            // 4. 目标必须是该 Room 当前 active 的 **member**：owner 是房内的治理角色
            //    （`role`），不是参与者种类（`kind`）——2026-09-27 批次三第一刀
            //    「owner = 创建者」。所以这里**不再按 kind 分叉**：Agent 与 Human
            //    同一条判据（"不是本房 active member" ⇒ MemberNotFound 404）。
            let target: Option<(String,)> = sqlx::query_as(
                "SELECT membership_id FROM room_memberships \
                 WHERE room_id = ? AND participant_id = ? AND active_marker = 1 AND role = 'member'",
            )
            .bind(room_id.as_str())
            .bind(target_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some((target_membership_id,)) = target else {
                return Err(RoomStoreError::MemberNotFound);
            };

            // 5. 读取一次 X=当前 Room 最大 message sequence 与时间 T；
            //    生成 transition_id 与两条新周期 ID。
            let occurred_at = UtcMillis::now().millis();
            let x = max_message_sequence(&mut tx, room_id.as_str()).await?;

            let old_owner_membership_id: String = sqlx::query_scalar(
                "SELECT membership_id FROM room_memberships \
                 WHERE room_id = ? AND participant_id = ? AND role = 'owner' AND active_marker = 1",
            )
            .bind(room_id.as_str())
            .bind(owner_id.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?;

            let transition_id = MembershipId::new_v4().to_string();
            let new_owner_membership = Membership::new(
                MembershipId::new_v4(),
                room_id.clone(),
                target_id.clone(),
                MembershipRole::Owner,
                UtcMillis::from_millis(occurred_at),
                x,
            );
            let old_owner_member_membership = Membership::new(
                MembershipId::new_v4(),
                room_id.clone(),
                owner_id.clone(),
                MembershipRole::Member,
                UtcMillis::from_millis(occurred_at),
                x,
            );

            // 6. 插入不可变 transition 事实：必须在关闭之前插入——此时旧
            //    owner 与目标 member 周期仍 active，transition INSERT guard
            //    （group、同 room、角色/participant 匹配、两端 active Human、
            //    old owner 唯一 active owner）当场通过；新周期 FK deferred，
            //    先于新周期插入。
            insert_owner_transition(
                &mut tx,
                &transition_id,
                room_id.as_str(),
                owner_id.as_str(),
                target_id.as_str(),
                &old_owner_membership_id,
                &target_membership_id,
                new_owner_membership.membership_id.as_str(),
                old_owner_member_membership.membership_id.as_str(),
                occurred_at,
                x,
            )
            .await?;

            // 7. 关闭旧 owner 与目标 member 周期（closed_by=transfer，
            //    left_before_sequence=X）——transfer 关闭守卫在当前事务中
            //    找到刚插入的匹配 transition（room/时间/边界全部一致），
            //    transfer 因此不可拆成两笔事务。
            close_membership_period(&mut tx, &old_owner_membership_id, occurred_at, x, "transfer")
                .await?;
            close_membership_period(&mut tx, &target_membership_id, occurred_at, x, "transfer")
                .await?;

            // 8. 打开目标 owner 与旧 owner member 周期
            //    （join_after_sequence=X）——新 owner INSERT 守卫只接受
            //    同一事务内精确匹配的 transition。
            insert_membership_period(&mut tx, &new_owner_membership).await?;
            insert_membership_period(&mut tx, &old_owner_member_membership).await?;

            // 9. 更新 coverage 见证到新 owner 周期。
            sqlx::query(
                "UPDATE room_coverage SET group_owner_membership_id = ? WHERE room_id = ?",
            )
            .bind(new_owner_membership.membership_id.as_str())
            .bind(room_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;

            // 10. 写幂等记录 (human_id, key) → transition_id。
            sqlx::query(SQL_INSERT_TRANSFER_IDEMPOTENCY)
                .bind(owner_id.as_str())
                .bind(idempotency_key.as_str())
                .bind(&transition_id)
                .bind(occurred_at)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            Ok(TransferOwnerOutcome::Transferred {
                membership: new_owner_membership,
                transition_id,
            })
        }
        .await;

        match outcome {
            Ok(transferred @ TransferOwnerOutcome::Transferred { .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(transferred)
            }
            Ok(replayed @ TransferOwnerOutcome::Replayed { .. }) => Ok(replayed),
            Err(e) => Err(e),
        }
    }

    async fn list_rooms(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<RoomListItem>, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        fetch_list_items(&mut conn, human_id.as_str(), None).await
    }

    async fn list_backend_rooms(
        &self,
        agent_id: &ParticipantId,
        since: Option<i64>,
    ) -> Result<Vec<BackendDirectoryRoom>, RoomStoreError> {
        // 纯读投影（切片 B1）：该 agent 为 active member 的全部 Room。
        // 预览/最近活动 = 该房对该 agent **任一 Membership 周期窗口内**
        // 可见的最近一条公共 Message（与 History/read state 同一可见性口径，
        // 撤回 tombstone → 预览 null）；增量计数 = 自 since 以来可见且
        // author ≠ 该 agent 的 Message 数（与 unread_count 同口径）；
        // since 缺省 = i64::MAX 边界 → 恒 0（无基线无增量）。
        // read state（切片 M1，CORE_V1_AGENT_READ_CURSOR.md §3）：LEFT JOIN
        // 该 agent 当前周期的 read cursor，未读数与 Human 列表**同一投影**
        // （cursor 缺失时防御性回退周期 join boundary、last_read 为 null）。
        let since_bound = since.unwrap_or(i64::MAX);
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // `{PEER_READ_SQL}` 与 Human 侧**同一段判据**（一处权威，人机同形）。
        let rows: Vec<BackendDirectoryRow> = sqlx::query_as::<_, BackendDirectoryRow>(&format!(
            "SELECT r.id AS r_id, r.kind AS r_kind, \
                    COALESCE(NULLIF(r.name, ''), p.display_name) AS r_display_name, \
                    (SELECT ms.created_at FROM messages ms \
                       WHERE ms.room_id = r.id \
                         AND EXISTS (SELECT 1 FROM room_memberships w \
                                     WHERE w.room_id = ms.room_id AND w.participant_id = ? \
                                       AND w.join_after_sequence < ms.sequence \
                                       AND (w.left_before_sequence IS NULL \
                                            OR ms.sequence <= w.left_before_sequence)) \
                       ORDER BY ms.sequence DESC LIMIT 1) AS last_activity_ms, \
                    (SELECT CASE WHEN EXISTS (SELECT 1 FROM message_recalls rc \
                                              WHERE rc.message_id = ms.id) \
                             THEN NULL ELSE ms.body END \
                       FROM messages ms \
                       WHERE ms.room_id = r.id \
                         AND EXISTS (SELECT 1 FROM room_memberships w \
                                     WHERE w.room_id = ms.room_id AND w.participant_id = ? \
                                       AND w.join_after_sequence < ms.sequence \
                                       AND (w.left_before_sequence IS NULL \
                                            OR ms.sequence <= w.left_before_sequence)) \
                       ORDER BY ms.sequence DESC LIMIT 1) AS preview, \
                    (SELECT COUNT(*) FROM messages ms \
                       WHERE ms.room_id = r.id AND ms.actor_id <> ? AND ms.created_at > ? \
                         AND EXISTS (SELECT 1 FROM room_memberships w \
                                     WHERE w.room_id = ms.room_id AND w.participant_id = ? \
                                       AND w.join_after_sequence < ms.sequence \
                                       AND (w.left_before_sequence IS NULL \
                                            OR ms.sequence <= w.left_before_sequence))) \
                       AS since_count, \
                    c.last_read_message_id AS c_last_read_message_id, \
                    (SELECT COUNT(*) FROM messages un \
                       WHERE un.room_id = r.id \
                         AND un.sequence > COALESCE(c.read_sequence, m.join_after_sequence) \
                         AND un.actor_id <> ? \
                         AND EXISTS (SELECT 1 FROM room_memberships w \
                                     WHERE w.room_id = un.room_id AND w.participant_id = ? \
                                       AND w.join_after_sequence < un.sequence \
                                       AND (w.left_before_sequence IS NULL \
                                            OR un.sequence <= w.left_before_sequence))) \
                       AS unread_count, \
                    {PEER_READ_SQL} AS peer_read, \
                    {ATTENTION_SQL} AS attention, \
                    r.created_at AS r_created_at \
             FROM rooms r \
             JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = ? AND m.active_marker = 1 \
             LEFT JOIN read_cursors c ON c.membership_id = m.membership_id \
             {ATTENTION_JOIN} \
             LEFT JOIN direct_room_pairs dp ON dp.room_id = r.id \
             LEFT JOIN human_direct_pairs hp ON hp.room_id = r.id \
             LEFT JOIN participants p ON p.id = CASE \
                 WHEN dp.room_id IS NOT NULL THEN CASE WHEN dp.human_id = ? THEN dp.agent_id ELSE dp.human_id END \
                 WHEN hp.room_id IS NOT NULL THEN CASE WHEN hp.human_a_id = ? THEN hp.human_b_id ELSE hp.human_a_id END \
                 ELSE NULL END",
        ))
        // 绑定顺序 = 占位符出现顺序（`peer_read` 那一段在 `unread_count` 之后、
        // JOIN 之前 ⇒ 它的两次"我"紧跟在前七次之后）。
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(since_bound)
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .bind(agent_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        let mut rooms: Vec<BackendDirectoryRoom> = rows
            .into_iter()
            .map(backend_directory_from_row)
            .collect::<Result<_, RoomStoreError>>()?;
        // 机械排序：最近活动时间倒序（无消息房回退房间创建时间），再按房间 ID 升序。
        rooms.sort_by(|a, b| {
            let a_key = a.last_activity_at.unwrap_or(a.room_created_at);
            let b_key = b.last_activity_at.unwrap_or(b.room_created_at);
            b_key
                .cmp(&a_key)
                .then_with(|| a.room_id.as_str().cmp(b.room_id.as_str()))
        });
        Ok(rooms)
    }

    async fn get_room(
        &self,
        human_id: &ParticipantId,
        room_id: &RoomId,
    ) -> Result<Option<RoomListItem>, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut items =
            fetch_list_items(&mut conn, human_id.as_str(), Some(room_id.as_str())).await?;
        Ok(items.pop())
    }

    async fn get_room_members(
        &self,
        human_id: &ParticipantId,
        room_id: &RoomId,
    ) -> Result<Option<Vec<MemberView>>, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 裁决必须显式要求当前 Human 的 Membership 周期为 active：即使
        // 异常库中存在 closed Human 周期，也 fail closed 返回 None（离群
        // 后无房间访问）。
        let visible: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM room_memberships \
             WHERE room_id = ? AND participant_id = ? AND active_marker = 1 LIMIT 1",
        )
        .bind(room_id.as_str())
        .bind(human_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        if visible.is_none() {
            return Ok(None);
        }

        // direct / human_direct：保持配对成员展示（含 Agent revoke 后
        // closed 的 Agent 周期）；group：只列 active 周期。
        let rows: Vec<MemberRow> = sqlx::query_as(
            "SELECT m.membership_id AS m_membership_id, m.room_id AS m_room_id, \
                    m.participant_id AS m_participant, m.role AS m_role, \
                    m.joined_at AS m_joined_at, m.join_after_sequence AS m_join_after, \
                    m.left_at AS m_left_at, m.left_before_sequence AS m_left_before, \
                    m.closed_by AS m_closed_by, m.active_marker AS m_active, \
                    p.id AS p_id, p.kind AS p_kind, p.status AS p_status, \
                    p.display_name AS p_display_name \
             FROM room_memberships m \
             JOIN participants p ON p.id = m.participant_id \
             WHERE m.room_id = ? \
               AND (((SELECT kind FROM rooms WHERE id = m.room_id) = 'group' \
                     AND m.active_marker = 1) \
                    OR m.membership_id IN ( \
                        SELECT human_membership_id FROM direct_room_pairs WHERE room_id = m.room_id \
                        UNION \
                        SELECT agent_membership_id FROM direct_room_pairs WHERE room_id = m.room_id \
                        UNION \
                        SELECT human_a_membership_id FROM human_direct_pairs WHERE room_id = m.room_id \
                        UNION \
                        SELECT human_b_membership_id FROM human_direct_pairs WHERE room_id = m.room_id \
                    )) \
             ORDER BY CASE WHEN m.role = 'owner' THEN 0 ELSE 1 END, m.joined_at, m.participant_id",
        )
        .bind(room_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row: MemberRow| {
                let MemberRow {
                    m_membership_id: m_membership,
                    m_room_id: m_room,
                    m_participant,
                    m_role,
                    m_joined_at: m_joined,
                    m_join_after,
                    m_left_at,
                    m_left_before,
                    m_closed_by,
                    m_active,
                    p_id,
                    p_kind,
                    p_status,
                    p_display_name,
                } = row;
                Ok(MemberView {
                    membership: membership_from_row((
                        m_membership,
                        m_room,
                        m_participant,
                        m_role,
                        m_joined,
                        m_join_after,
                        m_left_at,
                        m_left_before,
                        m_closed_by,
                        m_active,
                    ))?,
                    participant: profile_from_row((p_id, p_kind, p_status, p_display_name))?,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn advance_read_cursor(
        &self,
        participant_id: &ParticipantId,
        room_id: &RoomId,
        message_id: &MessageId,
    ) -> Result<AdvanceReadCursorOutcome, RoomStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<AdvanceReadCursorOutcome, RoomStoreError> = async {
            // 1. 该主体必须存在该 Room 的 active Membership 周期；
            //    unknown / bad-id / 无 active 周期统一 NotFound（离群后
            //    cursor 冻结，不再可推进）。Human 与 Agent 同一条。
            let membership: Option<(String, i64)> = sqlx::query_as(
                "SELECT m.membership_id, m.join_after_sequence FROM rooms r \
                 JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = ? \
                 WHERE r.id = ? AND m.active_marker = 1 LIMIT 1",
            )
            .bind(participant_id.as_str())
            .bind(room_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some((membership_id, join_after)) = membership else {
                return Err(RoomStoreError::NotFound);
            };

            // 2. 目标 Message 必须属于该 Room 且对该主体的历史
            //    Membership 窗口可见；未知 / 他 Room / 不可见统一
            //    InvalidReadCursor，不区分具体原因。
            let target_sequence: Option<i64> = sqlx::query_scalar(
                "SELECT m.sequence FROM messages m \
                 WHERE m.id = ? AND m.room_id = ? \
                   AND EXISTS (SELECT 1 FROM room_memberships w \
                               WHERE w.room_id = m.room_id AND w.participant_id = ? \
                                 AND w.join_after_sequence < m.sequence \
                                 AND (w.left_before_sequence IS NULL OR m.sequence <= w.left_before_sequence))",
            )
            .bind(message_id.as_str())
            .bind(room_id.as_str())
            .bind(participant_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some(target_sequence) = target_sequence else {
                return Err(RoomStoreError::InvalidReadCursor);
            };

            // 3. 当前 cursor 位置（migration 202609270001 起 Human 与 Agent
            //    周期都必有 cursor 行；异常缺失时防御性回退 join boundary）。
            let cursor: Option<(i64, Option<String>)> = sqlx::query_as(
                "SELECT read_sequence, last_read_message_id FROM read_cursors \
                 WHERE membership_id = ?",
            )
            .bind(&membership_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let (current_sequence, current_message_id) = match cursor {
                Some(row) => row,
                None => (join_after, None),
            };

            // 4. 目标不高于当前：成功 no-op（200 advanced=false），不产生
            //    advancement / Human Event / 唤醒，也不倒退。
            if target_sequence <= current_sequence {
                let unread =
                    count_unread(&mut tx, room_id.as_str(), participant_id.as_str(), current_sequence)
                        .await?;
                return Ok(AdvanceReadCursorOutcome::NoOp {
                    read_state: ReadState {
                        last_read_message_id: current_message_id.map(MessageId),
                        unread_count: unread,
                    },
                });
            }

            // 5. 真实推进：唯一写路径 = 插入不可变 advancement 事实
            //    （migration 010 的 INSERT 守卫在同一事务内证明 from =
            //    当前 cursor、Message 同 Room 且窗口可见）。其 AFTER
            //    INSERT 触发器链在同一语句内自动完成剩余全部步骤：
            //    read_cursors_advancement_applies 用精确见证把 cursor
            //    单调推进到 to_sequence / last_read_message_id（UPDATE
            //    守卫要求与 advancement 全等，无见证的直接 SQL 无法绕过）；
            //    human_event_facts_read_cursor_advanced 原子生成
            //    read.cursor.advanced fact（**只对 Human**，见 migration
            //    202609270001）；定向扇出只投推进者本人。
            //    任一步失败，整个语句与外层事务整体回滚；Rust 不再执行
            //    独立 UPDATE。
            let advancement_id = Uuid::new_v4().to_string();
            let created_at = UtcMillis::now().millis();
            sqlx::query(SQL_INSERT_ADVANCEMENT)
                .bind(&advancement_id)
                .bind(participant_id.as_str())
                .bind(room_id.as_str())
                .bind(&membership_id)
                .bind(current_sequence)
                .bind(target_sequence)
                .bind(message_id.as_str())
                .bind(created_at)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;

            // 6. 推进后的权威 read state（同一事务内投影，响应与已提交
            //    事实一致）。
            let unread =
                count_unread(&mut tx, room_id.as_str(), participant_id.as_str(), target_sequence)
                    .await?;
            Ok(AdvanceReadCursorOutcome::Advanced {
                read_state: ReadState {
                    last_read_message_id: Some(message_id.clone()),
                    unread_count: unread,
                },
                advancement_id,
            })
        }
        .await;

        match outcome {
            Ok(advanced @ AdvanceReadCursorOutcome::Advanced { .. }) => {
                #[cfg(test)]
                self.commit_checkpoint.pause().await;
                tx.commit().await.map_err(db_error)?;
                Ok(advanced)
            }
            Ok(noop @ AdvanceReadCursorOutcome::NoOp { .. }) => {
                finish_rollback(tx)
                    .await
                    .map_err(RoomStoreError::Database)?;
                Ok(noop)
            }
            Err(e) => {
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, RoomStoreError::Database).await)
            }
        }
    }

    async fn set_room_attention(
        &self,
        participant_id: &ParticipantId,
        room_id: &RoomId,
        level: AttentionLevel,
    ) -> Result<AttentionLevel, RoomStoreError> {
        // 提醒档（契约 §9.1）：一条**状态写**，不是事实链。BEGIN IMMEDIATE
        // 让"先判准入、再写"在同一事务里成立，并发两笔只会有最后一笔生效。
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<AttentionLevel, RoomStoreError> = async {
            // 1. 准入一条判据、三个失败同形：本人必须是该 Room 的 active
            //    成员，且该 Room 必须是 group。unknown / bad-id / 不是群 /
            //    非成员 / 已离群 一律 NotFound（对外 404 room_not_found）——
            //    与非成员同形，**不泄露房间是否存在**（这是"只有群房有这一档"
            //    与"只有本人能写"两条裁决的共同出口）。
            let allowed: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM rooms r \
                 JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = ? \
                 WHERE r.id = ? AND r.kind = 'group' AND m.active_marker = 1 LIMIT 1",
            )
            .bind(participant_id.as_str())
            .bind(room_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if allowed.is_none() {
                return Err(RoomStoreError::NotFound);
            }

            // 2. 缺行则建行、有行则改值（同值重写是 no-op，天然幂等）。
            //    ON CONFLICT 走的是 UPDATE 分支 ⇒ 守卫 2 生效（身份不可改写、
            //    仍须 group + active 成员）；INSERT 分支由守卫 1 把关。
            sqlx::query(
                "INSERT INTO room_attention (room_id, participant_id, level, updated_at) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT(room_id, participant_id) DO UPDATE SET \
                     level = excluded.level, updated_at = excluded.updated_at",
            )
            .bind(room_id.as_str())
            .bind(participant_id.as_str())
            .bind(level.as_str())
            .bind(UtcMillis::now().millis())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;

            Ok(level)
        }
        .await;

        match outcome {
            Ok(level) => {
                tx.commit().await.map_err(db_error)?;
                Ok(level)
            }
            Err(e) => {
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, RoomStoreError::Database).await)
            }
        }
    }

    async fn attention_wake_targets(
        &self,
        room_id: &RoomId,
    ) -> Result<Vec<ParticipantId>, RoomStoreError> {
        // 提醒档的唤醒目标（契约 §9.1 的「通知条件」）：该房 **active Agent 成员**
        // 里档位为 `all` 的那些。**缺行 = all** ⇒ LEFT JOIN + COALESCE，与两条
        // 列表的投影**同一段判据**（`ATTENTION_SQL`）。
        //
        // 纯读、不开事务：唤醒是可丢的提示，读到的就是"此刻已提交的档"。
        // **不排除被 @ 的人**——它本来就该在结果里；mention 穿透由调用方把
        // `agent_events` 反查出来的目标并进来（两处各自一个权威，不复制判据）。
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT m.participant_id FROM room_memberships m \
             JOIN participants p ON p.id = m.participant_id \
             LEFT JOIN room_attention ra ON ra.room_id = m.room_id AND ra.participant_id = m.participant_id \
             WHERE m.room_id = ? AND m.active_marker = 1 AND p.kind = 'agent' \
               AND COALESCE(ra.level, 'all') = 'all' \
             ORDER BY m.participant_id",
        )
        .bind(room_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(ParticipantId).collect())
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
