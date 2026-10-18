//! Message 用例：提交（幂等）与有界 History 读取。
//!
//! 用例层负责规则编排与事务边界：校验幂等键、把 room_id 规范化为 UUID
//! v4 小写连字符形式、校验正文、生成 Message ID 与创建时间，再把已就绪
//! 的事实交给 `ports::message::MessageStore` 裁决。幂等比较针对规范化
//! Room ID 与解析后的完整 body string，不比较原始 JSON 字节。
//!
//! 裁决顺序（契约 7）：认证 Human → 校验 Idempotency-Key / Room ID / JSON
//! 形状与 body → 查询 actor + key 既有幂等事实 → 已存在时先做精确重放或
//! 冲突裁决 → 只有新 key 才检查 Room 与 active Membership 并创建 Message。

use std::collections::HashSet;

use crate::application::unexpected_store_error;
use crate::domain::agent::IdempotencyKey;
use crate::domain::attachment::AttachmentId;
use crate::domain::message::{Message, MessageBody, MessageBodyError, MessageId};
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::message::{
    HistoryMessage, MAX_ATTACHMENTS_PER_MESSAGE, MAX_MENTIONS_PER_MESSAGE, MessageStore,
    MessageStoreError, NewMessage, RecallOutcome as StoreRecallOutcome, RecallStoreError,
    SendMessageOutcome as StoreOutcome,
};
use crate::ports::storage::StorageError;
use crate::protocol::cursor::{Cursor, CursorPayload, CursorResource};

/// 提交结果：首次创建与幂等重放返回相同的 Message 事实，以
/// `idempotency_replayed` 区分（HTTP 201 / 200）。
#[derive(Debug)]
pub enum SendMessageOutcome {
    Created { message: Message },
    Replayed { message: Message },
}

#[derive(Debug)]
pub enum SendMessageError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// body 为空、超过 16384 UTF-8 bytes 或含 NUL；不分配 Message ID、
    /// sequence，也不留下幂等事实。
    InvalidMessageBody(MessageBodyError),
    /// reply_to_message_id 存在但不是规范 UUID v4：统一 400 invalid_request；
    /// 或 body 与 attachments 都缺省（Message 必须有正文或附件）。
    InvalidRequest,
    /// bad-id、unknown Room、非 active Human：对外统一 404 room_not_found。
    NotFound,
    /// Room 与 actor 均存在且 active，但 actor 不是其 active member。Human
    /// 路径仍映射 404（不泄露存在性）；Backend agent 主动发消息端点
    /// （CORE_V1_AGENT_SPEAK.md）映射 403 not_room_member。
    NotAMember,
    /// Mention 非法：目标超过 `MAX_MENTIONS_PER_MESSAGE` 上限、目标不是
    /// UUID v4、重复目标、非 group Room 携带非空 mentions、目标不是该
    /// Room 的 active 成员（Human 或 Agent 均可）、目标 participant 非
    /// active：统一 400 invalid_mention，不得留下任何 Message、Mention、
    /// Event 或 sequence。
    InvalidMention,
    /// reply target 非法：target 未知、跨 Room 或对当前 actor 任一历史
    /// Membership 窗口不可见：统一 400 invalid_reply_target（不泄露 target
    /// 是否存在、所属 Room 或 Membership 边界），不得留下任何 Message、
    /// Event 或 sequence。
    InvalidReplyTarget,
    /// 附件集合超过 `MAX_ATTACHMENTS_PER_MESSAGE` 上限或包含重复附件 ID：
    /// 统一 400 invalid_attachment（请求形状错误，先于任何事实）。
    InvalidAttachment,
    /// 附件 unknown / 非当前 actor 上传 / 跨 Room / 非 pending / 已过期：
    /// 统一 404 attachment_not_found（不泄露存在性或原因），整个 Message
    /// 提交失败且不留任何事实。
    AttachmentNotFound,
    /// 同 actor + 同 key 已被不同 Room、不同 body、不同 Mention 集合或
    /// 不同 reply target 或不同有序附件集合使用。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for SendMessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::InvalidMessageBody(e) => write!(f, "{e}"),
            Self::InvalidRequest => write!(f, "invalid request"),
            Self::NotFound => write!(f, "room not found"),
            Self::NotAMember => write!(f, "actor is not an active member of the room"),
            Self::InvalidMention => write!(
                f,
                "invalid mention: targets must be distinct active members of a group room"
            ),
            Self::InvalidReplyTarget => {
                write!(
                    f,
                    "reply target must be a same-room historical message visible to the actor"
                )
            }
            Self::InvalidAttachment => write!(f, "invalid attachment set"),
            Self::AttachmentNotFound => write!(f, "attachment not found"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "message store failure: {e}"),
        }
    }
}

impl std::error::Error for SendMessageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_send_error(e: MessageStoreError) -> SendMessageError {
    match e {
        MessageStoreError::NotFound => SendMessageError::NotFound,
        MessageStoreError::NotAMember => SendMessageError::NotAMember,
        MessageStoreError::IdempotencyConflict => SendMessageError::IdempotencyConflict,
        MessageStoreError::InvalidMention => SendMessageError::InvalidMention,
        MessageStoreError::InvalidReplyTarget => SendMessageError::InvalidReplyTarget,
        MessageStoreError::InvalidAttachment => SendMessageError::InvalidAttachment,
        MessageStoreError::AttachmentNotFound => SendMessageError::AttachmentNotFound,
        MessageStoreError::Database(e) => SendMessageError::Store(e),
    }
}

/// 解析 Mention 目标列表：先拒绝超过 `MAX_MENTIONS_PER_MESSAGE` 的
/// 请求（在 UUID 解析与任何容量分配之前，杜绝 128 KiB 请求被放大为数千
/// 次逐目标校验/INSERT/Event 触发），再解析为规范小写连字符 UUID v4，
/// 重复目标直接返回 InvalidMention（契约：Core 先解析 UUID、去除客户端
/// 顺序影响并按规范 ID 排序；重复目标直接 400 invalid_mention），最后按
/// 规范 ID 排序。目标是否为该 Room 的 active 成员由存储层在提交事务内裁决。
fn parse_mentions(raw: &[String]) -> Result<Vec<ParticipantId>, SendMessageError> {
    if raw.len() > MAX_MENTIONS_PER_MESSAGE {
        return Err(SendMessageError::InvalidMention);
    }
    let mut seen: HashSet<String> = HashSet::with_capacity(raw.len());
    let mut targets = Vec::with_capacity(raw.len());
    for item in raw {
        let id = ParticipantId::parse_canonical(item).ok_or(SendMessageError::InvalidMention)?;
        if !seen.insert(id.as_str().to_owned()) {
            return Err(SendMessageError::InvalidMention);
        }
        targets.push(id);
    }
    targets.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(targets)
}

/// 解析有序附件 ID 集合（契约 CORE_V1_ATTACHMENTS.md 5.4）：先拒绝超过
/// `MAX_ATTACHMENTS_PER_MESSAGE` 的请求（在任何 UUID 解析之前），重复
/// 附件 ID 直接 InvalidAttachment（400 invalid_attachment），再解析为规范
/// 小写连字符 UUID v4（bad UUID 统一 400 invalid_request）。顺序保留：
/// 绑定 position 与幂等重放比较都按请求顺序。
fn parse_attachments(raw: &[String]) -> Result<Vec<AttachmentId>, SendMessageError> {
    if raw.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(SendMessageError::InvalidAttachment);
    }
    let mut seen: HashSet<String> = HashSet::with_capacity(raw.len());
    let mut ids = Vec::with_capacity(raw.len());
    for item in raw {
        if !seen.insert(item.clone()) {
            return Err(SendMessageError::InvalidAttachment);
        }
        let id = AttachmentId::parse_canonical(item).ok_or(SendMessageError::InvalidRequest)?;
        ids.push(id);
    }
    Ok(ids)
}

/// 提交普通文本 Message（当前公开入口只接受 Human Session 认证的 actor；
/// 用例与存储端口接收通用 Participant actor）。`mentions` 缺省为空数组；
/// 解析、去重、规范排序后随 Message 事实进入同一幂等/历史事实链。
/// `reply_to_message_id` 缺省/null 表示普通消息；非 None 时必须解析为
/// 规范小写连字符 UUID v4（格式错误统一 400 invalid_request），目标是否
/// 同 Room 且对 actor 历史 Membership 窗口可见由存储层在提交事务内裁决
/// （400 invalid_reply_target）。`attachments` 缺省为空数组；有序附件 ID
/// 集合参与幂等重放裁决，绑定与 Message 同一事务原子成立（契约
/// CORE_V1_ATTACHMENTS.md 5.4/9.1）；body 缺省/null 与 attachments 为空
/// 互斥（400 invalid_request）。
#[allow(clippy::too_many_arguments)]
pub async fn send_message(
    store: &dyn MessageStore,
    actor: &Participant,
    room_id_raw: &str,
    body_raw: Option<&str>,
    mentions_raw: &[String],
    reply_to_message_id_raw: Option<&str>,
    attachments_raw: &[String],
    idempotency_key_raw: &str,
) -> Result<SendMessageOutcome, SendMessageError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| SendMessageError::InvalidIdempotencyKey)?;
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(SendMessageError::NotFound)?;
    let body = match body_raw {
        None => None,
        Some(raw) => Some(MessageBody::parse(raw).map_err(SendMessageError::InvalidMessageBody)?),
    };
    let mentions = parse_mentions(mentions_raw)?;
    let attachments = parse_attachments(attachments_raw)?;
    if body.is_none() && attachments.is_empty() {
        return Err(SendMessageError::InvalidRequest);
    }
    let reply_to_message_id = match reply_to_message_id_raw {
        None => None,
        Some(raw) => Some(MessageId::parse_canonical(raw).ok_or(SendMessageError::InvalidRequest)?),
    };
    let message = NewMessage {
        id: MessageId::new_v4(),
        room_id,
        actor_id: actor.id.clone(),
        body,
        mentions,
        attachments,
        reply_to_message_id,
        created_at: UtcMillis::now(),
    };

    match store.send_message(&message, &key).await {
        Ok(StoreOutcome::Created { message }) => Ok(SendMessageOutcome::Created { message }),
        Ok(StoreOutcome::Replayed { message }) => Ok(SendMessageOutcome::Replayed { message }),
        Err(e) => Err(map_send_error(e)),
    }
}

/// History 查询失败：unknown / bad-id / 非 active Membership 统一为
/// NotFound；**Room 存在但 reader 非 active member → NotAMember**（切片 B2
/// 后端历史端点据以出 403；Human 路径映射回 404，wire 不变）；分页参数
/// （limit / before cursor 语法）不合法、cursor 跨端点类型/跨 Room/未知/
/// 不可见统一为 InvalidRequest。
#[derive(Debug)]
pub enum MessageQueryError {
    NotFound,
    /// Room 存在且 active，但 reader 不是其 active member（明确 ACL 拒绝）。
    NotAMember,
    InvalidRequest,
    Store(StorageError),
}

impl std::fmt::Display for MessageQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "room not found"),
            Self::NotAMember => write!(f, "reader is not an active member of the room"),
            Self::InvalidRequest => write!(f, "invalid history paging parameters"),
            Self::Store(e) => write!(f, "message store failure: {e}"),
        }
    }
}

impl std::error::Error for MessageQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_query_error(e: MessageStoreError) -> MessageQueryError {
    match e {
        MessageStoreError::Database(e) => MessageQueryError::Store(e),
        MessageStoreError::NotAMember => MessageQueryError::NotAMember,
        other => MessageQueryError::Store(unexpected_store_error("message store error", other)),
    }
}

/// 解析 limit：默认 50、范围 1-100。非法、重复、越界统一 Err（HTTP 400
/// invalid_request）。
fn parse_limit(limit_raw: Option<u32>) -> Result<u32, ()> {
    match limit_raw {
        None => Ok(50),
        Some(0) | Some(101..) => Err(()),
        Some(1..=100) => Ok(limit_raw.unwrap()),
    }
}

/// 解析 History `before` cursor：必须引用一条 Message（资源类型 `m` +
/// 规范 UUID v4）。语法非法、未知版本、跨端点类型、起点哨兵（`m` 不
/// 支持）统一 Err；具体存在性与可见性由存储层按 reader + Room 作用域
/// 裁决。
fn parse_message_cursor(before_raw: &str) -> Result<MessageId, ()> {
    let cursor = Cursor::parse(before_raw).ok_or(())?;
    if cursor.resource != CursorResource::Message {
        return Err(());
    }
    match cursor.payload {
        CursorPayload::Id(id) => Ok(MessageId(id)),
        CursorPayload::Start => Err(()),
    }
}

/// 读取该 Room 的有界历史：初次不传 before 时返回最新一页；messages
/// 始终按 sequence 升序；读取授权按 reader 当前 active Membership 独立
/// 裁决（unknown / bad-id / 非成员统一 NotFound）；`before` 是 opaque
/// cursor，按 reader + Room 作用域解析回内部位置，未知/跨 Room/不可见
/// 统一 InvalidRequest（400，不泄露存在性）。
pub async fn list_messages(
    store: &dyn MessageStore,
    reader_id: &ParticipantId,
    room_id_raw: &str,
    limit_raw: Option<u32>,
    before_raw: Option<&str>,
) -> Result<MessagePageOutcome, MessageQueryError> {
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(MessageQueryError::NotFound)?;
    let limit = parse_limit(limit_raw).map_err(|()| MessageQueryError::InvalidRequest)?;
    let before = match before_raw {
        None => None,
        Some(raw) => {
            let cursor_id =
                parse_message_cursor(raw).map_err(|()| MessageQueryError::InvalidRequest)?;
            let resolved = store
                .resolve_message_before(reader_id, &room_id, &cursor_id)
                .await
                .map_err(map_query_error)?;
            Some(resolved.ok_or(MessageQueryError::InvalidRequest)?)
        }
    };
    let page = store
        .list_messages(reader_id, &room_id, limit, before)
        .await
        .map_err(map_query_error)?
        .ok_or(MessageQueryError::NotFound)?;
    // 有更旧消息时，next_before 是当前页第一条（最旧一条）Message 的
    // opaque cursor；下一页传回它读取 `sequence < cursor_sequence`，
    // 无重复、无跳过。
    let next_before = if page.next_before.is_some() {
        page.messages
            .first()
            .map(|item| Cursor::message(&item.message.id).to_string())
    } else {
        None
    };
    Ok(MessagePageOutcome {
        messages: page.messages,
        next_before,
    })
}

/// History 有界页（公共边界）：messages 按 sequence 升序；next_before 是
/// opaque cursor 字符串（有更旧消息）或 null。
#[derive(Debug)]
pub struct MessagePageOutcome {
    pub messages: Vec<HistoryMessage>,
    pub next_before: Option<String>,
}

/// 撤回（MessageRecall，契约 CORE_V1_MESSAGE_RECALL.md）结果：首次成立与
/// 幂等重放返回同一 recalled_at（首次成立时点；重放不重算、不刷新）。
#[derive(Debug)]
pub struct RecallOutcome {
    pub recalled_at: UtcMillis,
    pub idempotency_replayed: bool,
}

#[derive(Debug)]
pub enum RecallError {
    /// message_id 不是规范 UUID v4（HTTP 400 invalid_request）。
    InvalidRequest,
    /// bad room、Room 非 active、actor 非 active 或该 Room 无 active
    /// Membership：对外统一 404 room_not_found（先于消息裁决）。
    RoomNotFound,
    /// 消息 unknown、跨 Room、非当前 actor 的 Message 或对 actor 不可见：
    /// 对外统一 404 message_not_found（不泄露存在性或归属）。
    MessageNotFound,
    /// 未撤回且超过 120 秒窗口的首次撤回：409 message_recall_window_expired。
    WindowExpired,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for RecallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest => write!(f, "invalid message id"),
            Self::RoomNotFound => write!(f, "room not found"),
            Self::MessageNotFound => write!(f, "message not found"),
            Self::WindowExpired => write!(f, "message recall window expired"),
            Self::Store(e) => write!(f, "message store failure: {e}"),
        }
    }
}

impl std::error::Error for RecallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_recall_error(e: RecallStoreError) -> RecallError {
    match e {
        RecallStoreError::RoomNotFound => RecallError::RoomNotFound,
        RecallStoreError::MessageNotFound => RecallError::MessageNotFound,
        RecallStoreError::WindowExpired => RecallError::WindowExpired,
        RecallStoreError::Database(e) => RecallError::Store(e),
    }
}

/// 撤回一条 Message（契约 CORE_V1_MESSAGE_RECALL.md）。Human 与 Backend
/// 两个 HTTP 入口共用本用例：actor 是认证上下文解析出的 Participant
/// （Human Session → Human Participant；Backend Credential → Binding →
/// Agent），请求不能携带或覆盖 actor/room/message 之外的任何字段；
/// room_id 非规范 UUID v4 统一 RoomNotFound（404 room_not_found），
/// message_id 非规范 UUID v4 统一 InvalidRequest（400 invalid_request）；
/// 窗口与 recalled_at 的裁决全部在存储层同一 INSERT statement 内完成，
/// 本用例不生成也不接收 recalled_at。
pub async fn recall_message(
    store: &dyn MessageStore,
    actor_id: &ParticipantId,
    room_id_raw: &str,
    message_id_raw: &str,
) -> Result<RecallOutcome, RecallError> {
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(RecallError::RoomNotFound)?;
    let message_id =
        MessageId::parse_canonical(message_id_raw).ok_or(RecallError::InvalidRequest)?;
    match store.recall_message(actor_id, &room_id, &message_id).await {
        Ok(StoreRecallOutcome {
            recalled_at,
            replayed,
        }) => Ok(RecallOutcome {
            recalled_at,
            idempotency_replayed: replayed,
        }),
        Err(e) => Err(map_recall_error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_defaults_and_boundaries() {
        assert_eq!(parse_limit(None).unwrap(), 50);
        assert_eq!(parse_limit(Some(1)).unwrap(), 1);
        assert_eq!(parse_limit(Some(100)).unwrap(), 100);
        for bad_limit in [Some(0u32), Some(101u32), Some(10_000u32)] {
            assert!(parse_limit(bad_limit).is_err());
        }
    }

    #[test]
    fn message_cursor_parsing_accepts_message_uuids_only() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            parse_message_cursor(&format!("c1.m.{id}")).unwrap(),
            MessageId(id.into())
        );
        // 跨端点类型、起点哨兵、格式非法一律拒绝。
        for bad in [
            "c1.he.550e8400-e29b-41d4-a716-446655440001",
            "c1.be.550e8400-e29b-41d4-a716-446655440001",
            "c1.m.start",
            "c1.m.not-a-uuid",
            "c1.m.550E8400-E29B-41D4-A716-446655440000",
            "c2.m.550e8400-e29b-41d4-a716-446655440000",
            "",
        ] {
            assert!(parse_message_cursor(bad).is_err(), "must reject {bad:?}");
        }
    }

    #[test]
    fn mentions_beyond_cap_are_rejected_before_uuid_parsing() {
        let over_cap: Vec<String> = (0..=MAX_MENTIONS_PER_MESSAGE)
            .map(|_| "not-a-uuid".to_owned())
            .collect();
        assert_eq!(
            over_cap.len(),
            MAX_MENTIONS_PER_MESSAGE + 1,
            "fixture must hold one over the cap"
        );
        // 数量守卫先于 UUID 解析：即使第一个元素不是 UUID，也返回
        // InvalidMention（上限拒绝发生在任何逐目标解析与容量分配之前）。
        assert!(matches!(
            parse_mentions(&over_cap),
            Err(SendMessageError::InvalidMention)
        ));
    }

    #[test]
    fn attachments_beyond_cap_are_rejected_before_uuid_parsing() {
        let over_cap: Vec<String> = (0..=MAX_ATTACHMENTS_PER_MESSAGE)
            .map(|_| "not-a-uuid".to_owned())
            .collect();
        assert_eq!(
            over_cap.len(),
            MAX_ATTACHMENTS_PER_MESSAGE + 1,
            "fixture must hold one over the cap"
        );
        assert!(matches!(
            parse_attachments(&over_cap),
            Err(SendMessageError::InvalidAttachment)
        ));
    }

    #[test]
    fn attachments_parse_in_order_and_reject_duplicates_and_bad_uuids() {
        let a = "550e8400-e29b-41d4-a716-446655440001".to_owned();
        let b = "550e8400-e29b-41d4-a716-446655440002".to_owned();
        let parsed = parse_attachments(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(
            parsed.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            vec![a.as_str(), b.as_str()],
            "order must be preserved"
        );
        assert!(matches!(
            parse_attachments(&[a.clone(), a]),
            Err(SendMessageError::InvalidAttachment)
        ));
        // 大写 UUID 被规范化（parse_canonical 输出恒为小写连字符）。
        assert_eq!(
            parse_attachments(&["550E8400-E29B-41D4-A716-446655440001".to_owned()]).unwrap()[0]
                .as_str(),
            "550e8400-e29b-41d4-a716-446655440001"
        );
        for bad in ["not-a-uuid", "550e8400-e29b-41d4-a716-44665544000x"] {
            assert!(matches!(
                parse_attachments(&[bad.to_owned()]),
                Err(SendMessageError::InvalidRequest)
            ));
        }
    }

    #[tokio::test]
    async fn message_requires_body_or_attachments() {
        let (_, id, actor, key, room) = (
            "h",
            "00000000-0000-4000-8000-000000000001",
            "h-1",
            "k-1",
            "00000000-0000-4000-8000-000000000002",
        );
        let store = RecordingStore {
            outcome: Ok(StoreOutcome::Created {
                message: Message::new_attachment_only(
                    MessageId(id.into()),
                    RoomId(room.into()),
                    ParticipantId(actor.into()),
                    UtcMillis::from_millis(1),
                ),
            }),
            received: std::sync::Mutex::new(Vec::new()),
        };
        // body 与 attachments 都缺省 → InvalidRequest，且不触达存储层。
        let result =
            send_message(&store, &participant(actor), room, None, &[], None, &[], key).await;
        assert!(matches!(result, Err(SendMessageError::InvalidRequest)));
        assert!(store.received.lock().unwrap().is_empty());
    }

    fn participant(id: &str) -> Participant {
        use crate::domain::participant::{DisplayName, ParticipantKind, ParticipantStatus};
        Participant {
            id: ParticipantId(id.into()),
            kind: ParticipantKind::Human,
            display_name: DisplayName::parse("Owner").unwrap(),
            status: ParticipantStatus::Active,
            created_at: UtcMillis::from_millis(1),
        }
    }

    struct RecordingStore {
        outcome: Result<StoreOutcome, MessageStoreError>,
        received: std::sync::Mutex<Vec<NewMessage>>,
    }

    #[async_trait::async_trait]
    impl MessageStore for RecordingStore {
        async fn send_message(
            &self,
            message: &NewMessage,
            _idempotency_key: &IdempotencyKey,
        ) -> Result<StoreOutcome, MessageStoreError> {
            self.received.lock().unwrap().push(message.clone());
            match &self.outcome {
                Ok(outcome) => Ok(match outcome {
                    StoreOutcome::Created { message } => StoreOutcome::Created {
                        message: message.clone(),
                    },
                    StoreOutcome::Replayed { message } => StoreOutcome::Replayed {
                        message: message.clone(),
                    },
                }),
                Err(e) => Err(match e {
                    MessageStoreError::NotFound => MessageStoreError::NotFound,
                    MessageStoreError::NotAMember => MessageStoreError::NotAMember,
                    MessageStoreError::IdempotencyConflict => {
                        MessageStoreError::IdempotencyConflict
                    }
                    MessageStoreError::InvalidMention => MessageStoreError::InvalidMention,
                    MessageStoreError::InvalidReplyTarget => MessageStoreError::InvalidReplyTarget,
                    MessageStoreError::InvalidAttachment => MessageStoreError::InvalidAttachment,
                    MessageStoreError::AttachmentNotFound => MessageStoreError::AttachmentNotFound,
                    MessageStoreError::Database(d) => MessageStoreError::Database(d.clone()),
                }),
            }
        }
        async fn list_messages(
            &self,
            _reader_id: &ParticipantId,
            _room_id: &RoomId,
            _limit: u32,
            _before: Option<i64>,
        ) -> Result<Option<crate::ports::message::MessagePage>, MessageStoreError> {
            Err(MessageStoreError::Database(unexpected_store_error(
                "unused",
                MessageStoreError::NotFound,
            )))
        }
        async fn resolve_message_before(
            &self,
            _reader_id: &ParticipantId,
            _room_id: &RoomId,
            _cursor_message_id: &MessageId,
        ) -> Result<Option<i64>, MessageStoreError> {
            Err(MessageStoreError::Database(unexpected_store_error(
                "unused",
                MessageStoreError::NotFound,
            )))
        }
        async fn recall_message(
            &self,
            _actor_id: &ParticipantId,
            _room_id: &RoomId,
            _message_id: &MessageId,
        ) -> Result<crate::ports::message::RecallOutcome, RecallStoreError> {
            Err(RecallStoreError::Database(unexpected_store_error(
                "unused",
                MessageStoreError::NotFound,
            )))
        }
    }

    #[test]
    fn mentions_at_cap_parse_and_canonical_sort() {
        let mut raw: Vec<String> = (0..MAX_MENTIONS_PER_MESSAGE)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        let expected = {
            let mut sorted = raw.clone();
            sorted.sort();
            sorted
        };
        let parsed = parse_mentions(&raw).expect("exactly 64 distinct valid UUIDs must parse");
        assert_eq!(parsed.len(), MAX_MENTIONS_PER_MESSAGE);
        let as_str: Vec<String> = parsed.iter().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(as_str, expected, "canonical sorted");
        raw.push(uuid::Uuid::new_v4().to_string());
        assert!(matches!(
            parse_mentions(&raw),
            Err(SendMessageError::InvalidMention)
        ));
    }
}
