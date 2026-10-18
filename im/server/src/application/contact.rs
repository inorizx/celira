//! Contact 用例：请求创建（幂等/反向收敛）、列表、接受、拒绝、解除与拉黑。
//!
//! 用例层负责规则编排与事务边界：校验幂等键、把目标 Human ID 规范化、
//! 自我请求拒绝，再把已就绪的事实交给 `ports::contact::ContactStore` 裁决。
//! 同一对 Human 至多一个 active 关系段（对称事实）：A→B 与 B→A 并发收敛到
//! 同一段，只有 addressee 可以接受 pending 请求；已 accepted 的关系重复
//! 请求返回既有关系。关系可结束：addressee 拒绝、任一方解除；拉黑是独立
//! 事实，唯一准入拦截点是「发起好友申请时双方存在任一方向 active 拉黑段」。

use crate::application::unexpected_store_error;
use crate::domain::agent::IdempotencyKey;
use crate::domain::participant::{Participant, ParticipantId};
use crate::ports::contact::{
    BlockStoreError, BlockedHuman, ContactListItem, ContactOutcome as StoreOutcome, ContactStore,
    ContactStoreError,
};
use crate::ports::storage::StorageError;

/// 请求创建结果：首次创建与「返回既有关系」返回相同的响应模型，以
/// `idempotency_replayed` 区分（HTTP 201 / 200）。
#[derive(Debug)]
pub enum SendContactRequestOutcome {
    Created {
        contact: crate::domain::contact::Contact,
        peer: crate::ports::room::ParticipantProfile,
    },
    Existing {
        contact: crate::domain::contact::Contact,
        peer: crate::ports::room::ParticipantProfile,
    },
}

#[derive(Debug)]
pub enum SendContactRequestError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// 向自己发请求。
    InvalidSelfRequest,
    /// bad-id、unknown、非 active Human 或已 revoked 的目标：对外统一 404。
    NotFound,
    /// 同 requester + 同 key 已被不同目标使用。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for SendContactRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::InvalidSelfRequest => write!(f, "cannot send a contact request to yourself"),
            Self::NotFound => write!(f, "human not found"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "contact store failure: {e}"),
        }
    }
}

impl std::error::Error for SendContactRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_request_error(e: ContactStoreError) -> SendContactRequestError {
    match e {
        ContactStoreError::NotFound => SendContactRequestError::NotFound,
        ContactStoreError::IdempotencyConflict => SendContactRequestError::IdempotencyConflict,
        ContactStoreError::SelfRequest => SendContactRequestError::InvalidSelfRequest,
        ContactStoreError::Database(e) => SendContactRequestError::Store(e),
    }
}

/// 发起联系人请求：校验 key → 规范化目标 Human ID → 拒绝自我请求 →
/// 存储层在单写事务内裁决幂等、反向收敛与创建。
pub async fn send_contact_request(
    store: &dyn ContactStore,
    requester: &Participant,
    target_human_raw: &str,
    idempotency_key_raw: &str,
) -> Result<SendContactRequestOutcome, SendContactRequestError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| SendContactRequestError::InvalidIdempotencyKey)?;
    let target_id = ParticipantId::parse_canonical(target_human_raw)
        .ok_or(SendContactRequestError::NotFound)?;
    if target_id == requester.id {
        return Err(SendContactRequestError::InvalidSelfRequest);
    }

    match store
        .send_contact_request(&requester.id, &target_id, &key)
        .await
    {
        Ok(StoreOutcome::Created { contact, peer }) => {
            Ok(SendContactRequestOutcome::Created { contact, peer })
        }
        Ok(StoreOutcome::Existing { contact, peer }) => {
            Ok(SendContactRequestOutcome::Existing { contact, peer })
        }
        Err(e) => Err(map_request_error(e)),
    }
}

/// 查询类失败：unknown / bad-id / 关系不存在统一为 NotFound。
#[derive(Debug)]
pub enum ContactQueryError {
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for ContactQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "contact not found"),
            Self::Store(e) => write!(f, "contact store failure: {e}"),
        }
    }
}

impl std::error::Error for ContactQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_query_error(e: ContactStoreError) -> ContactQueryError {
    match e {
        ContactStoreError::Database(e) => ContactQueryError::Store(e),
        other => ContactQueryError::Store(unexpected_store_error("contact store error", other)),
    }
}

/// 列出当前 Human 作为 requester 或 addressee 的全部联系人关系及对端
/// Human 最小资料；pending 与 accepted 状态原样公开。
pub async fn list_contacts(
    store: &dyn ContactStore,
    human_id: &ParticipantId,
) -> Result<Vec<ContactListItem>, ContactQueryError> {
    store.list_contacts(human_id).await.map_err(map_query_error)
}

/// 由 addressee 接受 pending 请求：pending → accepted；已 accepted 幂等
/// 返回既有关系；unknown、非 addressee、bad-id 统一 NotFound。返回关系
/// 事实及对端（发起方）Human 的最小公共资料。
pub async fn accept_contact(
    store: &dyn ContactStore,
    human_id: &ParticipantId,
    requester_human_raw: &str,
) -> Result<
    (
        crate::domain::contact::Contact,
        crate::ports::room::ParticipantProfile,
    ),
    ContactQueryError,
> {
    let requester_id =
        ParticipantId::parse_canonical(requester_human_raw).ok_or(ContactQueryError::NotFound)?;
    store
        .accept_contact(human_id, &requester_id)
        .await
        .map_err(|e| match e {
            ContactStoreError::NotFound => ContactQueryError::NotFound,
            ContactStoreError::Database(e) => ContactQueryError::Store(e),
            other => ContactQueryError::Store(unexpected_store_error("contact store error", other)),
        })
}

/// 由 addressee 拒绝 pending 请求：段以 closed_by='declined' 关闭；unknown、
/// 非 addressee、段不是 pending、bad-id 统一 NotFound。
pub async fn decline_contact(
    store: &dyn ContactStore,
    human_id: &ParticipantId,
    requester_human_raw: &str,
) -> Result<(), ContactQueryError> {
    let requester_id =
        ParticipantId::parse_canonical(requester_human_raw).ok_or(ContactQueryError::NotFound)?;
    store
        .decline_contact(human_id, &requester_id)
        .await
        .map_err(|e| match e {
            ContactStoreError::NotFound => ContactQueryError::NotFound,
            ContactStoreError::Database(e) => ContactQueryError::Store(e),
            other => ContactQueryError::Store(unexpected_store_error("contact store error", other)),
        })
}

/// 任一方单方面解除关系：active 段以 closed_by='ended' 关闭，同一事务经
/// 触发器关闭双方在该 human_direct 房间的成员周期；没有 active 段是成功
/// no-op（幂等 204）。bad-id 统一 NotFound。
pub async fn end_contact(
    store: &dyn ContactStore,
    human_id: &ParticipantId,
    peer_human_raw: &str,
) -> Result<(), ContactQueryError> {
    let peer_id =
        ParticipantId::parse_canonical(peer_human_raw).ok_or(ContactQueryError::NotFound)?;
    store
        .end_contact(human_id, &peer_id)
        .await
        .map_err(|e| match e {
            ContactStoreError::NotFound => ContactQueryError::NotFound,
            ContactStoreError::Database(e) => ContactQueryError::Store(e),
            other => ContactQueryError::Store(unexpected_store_error("contact store error", other)),
        })
}

/// 拉黑失败：bad-id / 未知 / 非 active Human / Agent 目标统一 NotFound（对外
/// 404）；向自己拉黑 InvalidSelfBlock（对外 400）。
#[derive(Debug)]
pub enum BlockContactError {
    NotFound,
    InvalidSelfBlock,
    Store(StorageError),
}

impl std::fmt::Display for BlockContactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "human not found"),
            Self::InvalidSelfBlock => write!(f, "cannot block yourself"),
            Self::Store(e) => write!(f, "contact store failure: {e}"),
        }
    }
}

impl std::error::Error for BlockContactError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_block_error(e: BlockStoreError) -> BlockContactError {
    match e {
        BlockStoreError::NotFound => BlockContactError::NotFound,
        BlockStoreError::SelfBlock => BlockContactError::InvalidSelfBlock,
        BlockStoreError::Database(e) => BlockContactError::Store(e),
    }
}

/// 拉黑另一个 active Human（同一事务内先结束既有 active 段再写拉黑段）；
/// 目标不是 active Human（含 Agent）统一 NotFound；向自己拉黑
/// InvalidSelfBlock；已有同方向 active 拉黑段是成功 no-op（幂等）。
pub async fn block_participant(
    store: &dyn ContactStore,
    human: &Participant,
    target_raw: &str,
) -> Result<(), BlockContactError> {
    let target_id =
        ParticipantId::parse_canonical(target_raw).ok_or(BlockContactError::NotFound)?;
    if target_id == human.id {
        return Err(BlockContactError::InvalidSelfBlock);
    }
    store
        .block_participant(&human.id, &target_id)
        .await
        .map_err(map_block_error)
}

/// 解除拉黑（释放当前 Human → 目标方向的 active 拉黑段）；没有 active 段是
/// 成功 no-op（幂等）。解除拉黑不影响关系（关系不自动恢复）。
pub async fn unblock_participant(
    store: &dyn ContactStore,
    human: &Participant,
    target_raw: &str,
) -> Result<(), BlockContactError> {
    let target_id =
        ParticipantId::parse_canonical(target_raw).ok_or(BlockContactError::NotFound)?;
    store
        .unblock_participant(&human.id, &target_id)
        .await
        .map_err(map_block_error)
}

/// 列出当前 Human 作为 blocker 的全部 active 拉黑段及对端 Human 最小资料，
/// 按创建时间升序、再按对端 ID 排序。
pub async fn list_blocks(
    store: &dyn ContactStore,
    human_id: &ParticipantId,
) -> Result<Vec<BlockedHuman>, ContactQueryError> {
    store.list_blocks(human_id).await.map_err(map_query_error)
}
