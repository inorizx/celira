//! Contact 窄端口。
//!
//! 本端口只声明联系人切片的持久化边界，不依赖具体数据库或 HTTP 框架。
//! SQLite adapter 实现本端口；应用用例只通过端口消费存储能力。
//!
//! 联系人关系是 Human–Human 对称关系段：同一对 Human 至多一个 active 段
//! （规范 (a_id, b_id) 配对，a_id < b_id），A→B 与 B→A 并发收敛到同一段；
//! 只有 addressee 可以接受 pending 请求；请求幂等键只用于创建段，不进入
//! 公开联系人视图。pending 段不能创建 Human direct Room，也不能邀请进
//! group（由 Room 用例裁决）。关系可结束：addressee 拒绝（pending →
//! declined）、任一方解除（accepted/pending → ended）；拉黑是独立事实，
//! 其唯一准入拦截点是「发起好友申请时双方存在任一方向 active 拉黑段」。

use crate::domain::agent::IdempotencyKey;
use crate::domain::contact::Contact;
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::ports::room::ParticipantProfile;
use crate::ports::storage::StorageError;

/// 列表项：联系人关系 + 对端 Human 的最小公共资料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactListItem {
    pub contact: Contact,
    pub peer: ParticipantProfile,
}

/// 拉黑列表项：被当前 Human 拉黑的对端 Human 最小公共资料 + 拉黑时间。
/// 内部 block_id 不进入公开视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedHuman {
    pub peer: ParticipantProfile,
    pub created_at: UtcMillis,
}

/// 请求创建结果：首次创建（201）与「返回既有关系」（反向请求 / 重复
/// 请求 / 幂等重放，200）共享同一视图，以 `idempotency_replayed` 区分。
#[derive(Debug, Clone)]
pub enum ContactOutcome {
    Created {
        contact: Contact,
        peer: ParticipantProfile,
    },
    Existing {
        contact: Contact,
        peer: ParticipantProfile,
    },
}

/// 存储层错误。
#[derive(Debug)]
pub enum ContactStoreError {
    /// 目标不存在、不是 active Human 或已 revoked：对外统一 404。
    NotFound,
    /// 同 requester + 同 key 已被不同目标使用：409。
    IdempotencyConflict,
    /// 向自己发请求：对外 400。
    SelfRequest,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for ContactStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such active human target"),
            Self::IdempotencyConflict => write!(
                f,
                "idempotency key was already used with a different request"
            ),
            Self::SelfRequest => write!(f, "a human cannot send a contact request to itself"),
            Self::Database(e) => write!(f, "contact store failure: {e}"),
        }
    }
}

impl std::error::Error for ContactStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// 拉黑/解除拉黑存储错误。
#[derive(Debug)]
pub enum BlockStoreError {
    /// 目标不存在、不是 active Human 或已 revoked：对外统一 404。
    NotFound,
    /// 向自己拉黑：对外 400。
    SelfBlock,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for BlockStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such active human target"),
            Self::SelfBlock => write!(f, "a human cannot block itself"),
            Self::Database(e) => write!(f, "block store failure: {e}"),
        }
    }
}

impl std::error::Error for BlockStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// Contact 存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait ContactStore: Send + Sync {
    /// 创建或收敛一条联系人关系（`BEGIN IMMEDIATE` 单写事务）：
    ///
    /// 1. 同 requester + 同 key 已存在：目标一致 → `Existing`（幂等重放），
    ///    目标不同 → `IdempotencyConflict`；
    /// 2. 同一对 Human 已有关系（A→B 与 B→A 使用同一对事实）：直接返回
    ///    既有关系为 `Existing`，不创建第二条，不改变 requester/addressee；
    /// 3. 新关系才插入 pending 行并返回 `Created`。
    ///
    /// 目标必须存在且为 active Human，否则 `NotFound`；`requester_id` 与
    /// 目标相同返回 `SelfRequest`。
    async fn send_contact_request(
        &self,
        requester_id: &ParticipantId,
        target_id: &ParticipantId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<ContactOutcome, ContactStoreError>;

    /// 列出当前 Human 作为 requester 或 addressee 的全部关系及对端 Human
    /// 最小资料；pending 与 accepted 状态原样公开，按创建时间升序、再按
    /// 对端 ID 排序。
    async fn list_contacts(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<ContactListItem>, ContactStoreError>;

    /// 由 addressee 接受 pending 请求（`requester_id` 是请求发起方）：
    /// pending → accepted 并写入 accepted_at；已 accepted 幂等返回既有
    /// 关系；不存在、当前 Human 不是 addressee 统一 `NotFound`（fail
    /// closed，不泄露关系是否存在）。返回关系事实及对端（发起方）Human
    /// 的最小公共资料。
    async fn accept_contact(
        &self,
        human_id: &ParticipantId,
        requester_id: &ParticipantId,
    ) -> Result<(Contact, ParticipantProfile), ContactStoreError>;

    /// 由 addressee 拒绝 pending 请求（`requester_id` 是请求发起方）：
    /// 段以 closed_by='declined' 关闭（追加事实、不改写历史）；不存在、
    /// 当前 Human 不是 addressee、段不是 pending 统一 `NotFound`。
    async fn decline_contact(
        &self,
        human_id: &ParticipantId,
        requester_id: &ParticipantId,
    ) -> Result<(), ContactStoreError>;

    /// 任一方单方面解除关系：active 段（pending 或 accepted）以
    /// closed_by='ended' 关闭，同一事务经触发器关闭双方在该 human_direct
    /// 房间的成员周期。没有 active 段时是成功 no-op（幂等）。bad-id 在
    /// 应用层解析为 `NotFound`。
    async fn end_contact(
        &self,
        human_id: &ParticipantId,
        peer_id: &ParticipantId,
    ) -> Result<(), ContactStoreError>;

    /// 拉黑另一个 active Human：同一事务内先以 closed_by='ended' 关闭
    /// 双方之间既有的 active 段（若有），再写新 active 拉黑段；
    /// 已有同方向 active 拉黑段时是成功 no-op（幂等）。目标不是 active
    /// Human 返回 `NotFound`，向自己拉黑返回 `SelfBlock`。
    async fn block_participant(
        &self,
        blocker_id: &ParticipantId,
        blocked_id: &ParticipantId,
    ) -> Result<(), BlockStoreError>;

    /// 解除拉黑（释放 blocker→blocked 方向 active 段）：没有 active 段时是
    /// 成功 no-op（幂等）。解除拉黑不影响关系（关系不自动恢复）。
    async fn unblock_participant(
        &self,
        blocker_id: &ParticipantId,
        blocked_id: &ParticipantId,
    ) -> Result<(), BlockStoreError>;

    /// 列出当前 Human 作为 blocker 的全部 active 拉黑段及对端 Human 最小
    /// 公共资料，按创建时间升序、再按对端 ID 排序。
    async fn list_blocks(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<BlockedHuman>, ContactStoreError>;
}
