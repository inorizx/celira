//! HTTP handler 测试：health / ready 契约、request id 关联与认证矩阵。

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::adapters::http::build_router;
use crate::adapters::http::routes::response_request_id;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::identity;
use crate::domain::human_session::{SessionId, TokenHash};
use crate::ports::agent::{AgentStoreError, NewAgent, NewAgentOwnership, NewIdempotencyRecord};
use crate::ports::contact::{ContactOutcome, ContactStoreError};
use crate::ports::identity::{AuthContext, NewHuman, NewSession, StoreError};
use crate::ports::message::{MessageStoreError, NewMessage};
use crate::ports::migration::{MigrationProbe, MigrationStatus};
use crate::ports::room::{
    CreateDirectRoomOutcome, MemberView, NewDirectRoom, RoomListItem, RoomStoreError,
};
use crate::ports::storage::{StorageError, StorageFailureKind};

/// 桩存储错误：`denied` 归 `busy_or_locked`（对外稳定 503
/// `storage_unavailable`，与 P0-A 既有断言一致）；typed cause 链保留
/// 消息，供 Display 泄漏负向检查。
fn denied() -> StorageError {
    StorageError::from_message(StorageFailureKind::BusyOrLocked, "denied")
}

/// 桩端口：模拟「有 pending 迁移」的数据库状态。
struct StubProbe {
    pending: u64,
    failed: u64,
}

#[async_trait::async_trait]
impl MigrationProbe for StubProbe {
    async fn status(&self) -> Result<MigrationStatus, StorageError> {
        Ok(MigrationStatus {
            pending: self.pending,
            failed: self.failed,
        })
    }
}

/// 桩端口：探针失败，错误消息故意包含路径 / SQLite 细节（模拟真实底层
/// 错误），用于证明公开响应绝不回显探针原因。
struct FailingProbe;

#[async_trait::async_trait]
impl MigrationProbe for FailingProbe {
    async fn status(&self) -> Result<MigrationStatus, StorageError> {
        Err(StorageError::from_message(
            StorageFailureKind::ConnectionOrIo,
            "database file missing at /tmp/fake/secret/core.db: SQLite error code 5: database is locked",
        ))
    }
}

/// 桩端口：身份存储返回 `decode_or_corrupt`（模拟持久事实无法解析），用于
/// 证明该类失败公开为 500 `internal_error` 且不泄露 cause 内容。
struct DecodeIdentity;
#[async_trait::async_trait]
impl crate::ports::identity::IdentityStore for DecodeIdentity {
    async fn bootstrap_first_human(
        &self,
        _human: &NewHuman,
        _session: &NewSession,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "injected stored snapshot unparseable at /tmp/secret/core.db",
        )))
    }
    async fn issue_human_session(&self, _session: &NewSession) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "injected stored snapshot unparseable at /tmp/secret/core.db",
        )))
    }
    async fn create_human(
        &self,
        _human: &NewHuman,
        _session: &NewSession,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "injected stored snapshot unparseable at /tmp/secret/core.db",
        )))
    }
    async fn authenticate(&self, _hash: &TokenHash) -> Result<Option<AuthContext>, StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "injected stored snapshot unparseable at /tmp/secret/core.db",
        )))
    }
    async fn revoke_session(&self, _session_id: &SessionId) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "injected stored snapshot unparseable at /tmp/secret/core.db",
        )))
    }

    /// 表达权第四刀（2026-10-17）：本桩**不支持**改名 —— 沿用本桩既有的拒绝态度，
    /// 不新造行为（改名那一面的行为用例在 `human_profile_routes_tests.rs`）。
    async fn rename_human(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _display_name: &crate::domain::participant::DisplayName,
    ) -> Result<(), StoreError> {
        Err(StoreError::HumanNotActive)
    }
}

/// 拒绝一切访问的身份桩：只用于不触达受保护端点的探针测试。
struct DenyIdentity;
#[async_trait::async_trait]
impl crate::ports::identity::IdentityStore for DenyIdentity {
    async fn bootstrap_first_human(
        &self,
        _human: &NewHuman,
        _session: &NewSession,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(denied()))
    }
    async fn issue_human_session(&self, _session: &NewSession) -> Result<(), StoreError> {
        Err(StoreError::Database(denied()))
    }
    async fn create_human(
        &self,
        _human: &NewHuman,
        _session: &NewSession,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(denied()))
    }
    async fn authenticate(&self, _hash: &TokenHash) -> Result<Option<AuthContext>, StoreError> {
        Err(StoreError::Database(denied()))
    }
    async fn revoke_session(&self, _session_id: &SessionId) -> Result<(), StoreError> {
        Err(StoreError::Database(denied()))
    }

    /// 表达权第四刀（2026-10-17）：本桩**不支持**改名 —— 沿用本桩既有的拒绝态度，
    /// 不新造行为（改名那一面的行为用例在 `human_profile_routes_tests.rs`）。
    async fn rename_human(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _display_name: &crate::domain::participant::DisplayName,
    ) -> Result<(), StoreError> {
        Err(StoreError::HumanNotActive)
    }
}

/// 拒绝一切访问的 Agent 存储桩：只用于不触达 Agent 端点的探针测试。
struct DenyAgent;

#[async_trait::async_trait]
impl crate::ports::agent::AgentStore for DenyAgent {
    async fn create_agent(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent: &NewAgent,
        _ownership: &NewAgentOwnership,
        _idempotency: &NewIdempotencyRecord,
    ) -> Result<crate::ports::agent::CreateAgentOutcome, AgentStoreError> {
        Err(AgentStoreError::Database(denied()))
    }
    async fn list_owned_agents(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<crate::domain::agent::OwnedAgent>, AgentStoreError> {
        Err(AgentStoreError::Database(denied()))
    }
    async fn get_owned_agent(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Option<crate::domain::agent::OwnedAgent>, AgentStoreError> {
        Err(AgentStoreError::Database(denied()))
    }
    async fn revoke_agent(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<crate::domain::room::MembershipId>, AgentStoreError> {
        Err(AgentStoreError::Database(denied()))
    }
}

/// 拒绝一切访问的 Room 存储桩：只用于不触达 Room 端点的探针测试。
struct DenyRoom;

#[async_trait::async_trait]
impl crate::ports::room::RoomStore for DenyRoom {
    async fn create_direct_room(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _room: &NewDirectRoom,
        _idempotency_key: &crate::domain::agent::IdempotencyKey,
    ) -> Result<CreateDirectRoomOutcome, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn create_human_direct_room(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _room: &crate::ports::room::NewHumanDirectRoom,
        _idempotency_key: &crate::domain::agent::IdempotencyKey,
    ) -> Result<crate::ports::room::CreateRoomOutcome, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn create_group(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _room: &crate::ports::room::NewGroup,
        _idempotency_key: &crate::domain::agent::IdempotencyKey,
    ) -> Result<crate::ports::room::CreateRoomOutcome, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn invite_group_member(
        &self,
        _owner_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _participant_id: &crate::domain::participant::ParticipantId,
        _idempotency_key: &crate::domain::agent::IdempotencyKey,
    ) -> Result<crate::ports::room::InviteMemberOutcome, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn remove_group_member(
        &self,
        _owner_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _participant_id: &crate::domain::participant::ParticipantId,
    ) -> Result<crate::domain::room::MembershipId, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn leave_membership(
        &self,
        _participant_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
    ) -> Result<crate::domain::room::MembershipId, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn transfer_group_owner(
        &self,
        _owner_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _target_id: &crate::domain::participant::ParticipantId,
        _idempotency_key: &crate::domain::agent::IdempotencyKey,
    ) -> Result<crate::ports::room::TransferOwnerOutcome, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn list_rooms(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<RoomListItem>, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn get_room(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
    ) -> Result<Option<RoomListItem>, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn get_room_members(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
    ) -> Result<Option<Vec<MemberView>>, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn advance_read_cursor(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _message_id: &crate::domain::message::MessageId,
    ) -> Result<crate::ports::room::AdvanceReadCursorOutcome, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn set_room_attention(
        &self,
        _participant_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _level: crate::domain::room::AttentionLevel,
    ) -> Result<crate::domain::room::AttentionLevel, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn attention_wake_targets(
        &self,
        _room_id: &crate::domain::room::RoomId,
    ) -> Result<Vec<crate::domain::participant::ParticipantId>, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
    async fn list_backend_rooms(
        &self,
        _agent_id: &crate::domain::participant::ParticipantId,
        _since: Option<i64>,
    ) -> Result<Vec<crate::ports::room::BackendDirectoryRoom>, RoomStoreError> {
        Err(RoomStoreError::Database(denied()))
    }
}

/// 拒绝一切访问的 Message 存储桩：只用于不触达 Message 端点的探针测试。
struct DenyMessage;

#[async_trait::async_trait]
impl crate::ports::message::MessageStore for DenyMessage {
    async fn send_message(
        &self,
        _message: &NewMessage,
        _idempotency_key: &crate::domain::agent::IdempotencyKey,
    ) -> Result<crate::ports::message::SendMessageOutcome, MessageStoreError> {
        Err(MessageStoreError::Database(denied()))
    }
    async fn list_messages(
        &self,
        _reader_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _limit: u32,
        _before: Option<i64>,
    ) -> Result<Option<crate::ports::message::MessagePage>, MessageStoreError> {
        Err(MessageStoreError::Database(denied()))
    }
    async fn resolve_message_before(
        &self,
        _reader_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _cursor_message_id: &crate::domain::message::MessageId,
    ) -> Result<Option<i64>, MessageStoreError> {
        Err(MessageStoreError::Database(denied()))
    }
    async fn recall_message(
        &self,
        _actor_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _message_id: &crate::domain::message::MessageId,
    ) -> Result<crate::ports::message::RecallOutcome, crate::ports::message::RecallStoreError> {
        Err(crate::ports::message::RecallStoreError::Database(denied()))
    }
}

/// 拒绝一切访问的附件元数据桩：只用于不触达附件端点的探针测试。
struct DenyAttachment;

#[async_trait::async_trait]
impl crate::ports::attachment::AttachmentStore for DenyAttachment {
    async fn create_pending(
        &self,
        _attachment: &crate::domain::attachment::NewAttachment,
    ) -> Result<(), crate::ports::attachment::AttachmentStoreError> {
        Err(crate::ports::attachment::AttachmentStoreError::Database(
            denied(),
        ))
    }
    async fn authorized_attachment(
        &self,
        _reader_id: &crate::domain::participant::ParticipantId,
        _room_id: &crate::domain::room::RoomId,
        _attachment_id: &crate::domain::attachment::AttachmentId,
    ) -> Result<
        Option<crate::domain::attachment::Attachment>,
        crate::ports::attachment::AttachmentStoreError,
    > {
        Err(crate::ports::attachment::AttachmentStoreError::Database(
            denied(),
        ))
    }
    async fn purge_expired_pending(
        &self,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<
        Vec<crate::domain::attachment::StorageKey>,
        crate::ports::attachment::AttachmentStoreError,
    > {
        Err(crate::ports::attachment::AttachmentStoreError::Database(
            denied(),
        ))
    }
    async fn referenced_storage_keys(
        &self,
    ) -> Result<std::collections::HashSet<String>, crate::ports::attachment::AttachmentStoreError>
    {
        Err(crate::ports::attachment::AttachmentStoreError::Database(
            denied(),
        ))
    }
}

/// 拒绝一切访问的 Participant Presentation 存储桩：只用于不触达
/// Profile/Presentation 端点的探针测试。
struct DenyPresentation;

#[async_trait::async_trait]
impl crate::ports::participant_presentation::ParticipantPresentationStore for DenyPresentation {
    async fn put_profile_version(
        &self,
        _key: &crate::domain::agent::IdempotencyKey,
        _digest: &crate::domain::participant_presentation::RequestDigest,
        _base_version: &crate::domain::participant_presentation::BaseVersion,
        _record: &crate::domain::participant_presentation::ParticipantProfileVersion,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<
        crate::ports::participant_presentation::ProfilePutOutcome,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn put_room_presentation_version(
        &self,
        _key: &crate::domain::agent::IdempotencyKey,
        _digest: &crate::domain::participant_presentation::RequestDigest,
        _base_version: &crate::domain::participant_presentation::BaseVersion,
        _record: &crate::domain::participant_presentation::RoomPresentationVersion,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<
        crate::ports::participant_presentation::RoomPresentationPutOutcome,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn put_room_profile_version(
        &self,
        _key: &crate::domain::agent::IdempotencyKey,
        _digest: &crate::domain::participant_presentation::RequestDigest,
        _base_version: &crate::domain::participant_presentation::BaseVersion,
        _record: &crate::domain::room_profile::RoomProfileVersionRow,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<
        crate::ports::participant_presentation::RoomProfilePutOutcome,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn latest_room_profile(
        &self,
        _room_id: &crate::domain::room::RoomId,
    ) -> Result<
        Option<crate::ports::participant_presentation::RoomProfileRead>,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn room_profile_header(
        &self,
        _room_id: &crate::domain::room::RoomId,
    ) -> Result<
        Option<crate::ports::participant_presentation::RoomProfileHeader>,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn latest_profile(
        &self,
        _participant_id: &crate::domain::participant::ParticipantId,
    ) -> Result<
        Option<crate::ports::participant_presentation::ProfileRead>,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn latest_room_presentation(
        &self,
        _room_id: &crate::domain::room::RoomId,
        _participant_id: &crate::domain::participant::ParticipantId,
    ) -> Result<
        Option<crate::ports::participant_presentation::RoomPresentationRead>,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn member_presentation_summaries(
        &self,
        _room_id: &crate::domain::room::RoomId,
    ) -> Result<
        Vec<crate::ports::participant_presentation::MemberPresentationSummary>,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn latest_avatar(
        &self,
        _participant_id: &crate::domain::participant::ParticipantId,
    ) -> Result<
        Option<crate::ports::participant_presentation::LatestAvatar>,
        crate::ports::participant_presentation::StoreError,
    > {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
    async fn avatar_storage_keys(
        &self,
    ) -> Result<std::collections::HashSet<String>, crate::ports::participant_presentation::StoreError>
    {
        Err(crate::ports::participant_presentation::StoreError::Database(denied()))
    }
}

/// 拒绝一切访问的 Blob 存储桩：只用于不触达附件端点的探针测试。
struct DenyBlob;
#[async_trait::async_trait]
impl crate::ports::blob::BlobStorage for DenyBlob {
    async fn write(
        &self,
        _key: &crate::domain::attachment::StorageKey,
        _bytes: &[u8],
    ) -> Result<(), StorageError> {
        Err(denied())
    }
    async fn read(
        &self,
        _key: &crate::domain::attachment::StorageKey,
    ) -> Result<Vec<u8>, StorageError> {
        Err(denied())
    }
    async fn delete(
        &self,
        _key: &crate::domain::attachment::StorageKey,
    ) -> Result<(), StorageError> {
        Err(denied())
    }
    async fn sweep_orphans(
        &self,
        _referenced: &std::collections::HashSet<String>,
        _grace: std::time::Duration,
    ) -> Result<(), StorageError> {
        Err(denied())
    }
}

/// 拒绝一切访问的 Home 存储桩：只用于不触达 Home 端点的探针测试。
struct DenyHome;

#[async_trait::async_trait]
impl crate::ports::home::HomeStore for DenyHome {
    async fn init_home(
        &self,
        _home: &crate::ports::home::NewHome,
    ) -> Result<(), crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn get_home(
        &self,
    ) -> Result<Option<crate::domain::home::Home>, crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn issue_grant(
        &self,
        _grant: &crate::domain::home::NewGrant,
    ) -> Result<(), crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn exchange_grant(
        &self,
        _exchange: &crate::ports::home::ExchangeRequest,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<crate::ports::home::ExchangeOutcome, crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn list_grants(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<crate::domain::home::PairingGrant>, crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn revoke_grant(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _grant_id: &crate::domain::home::GrantId,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<(), crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn list_sessions(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<crate::ports::home::SessionListItem>, crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn revoke_owned_session(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _session_id: &crate::domain::human_session::SessionId,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<(), crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn prepare_transaction(
        &self,
        _request: &crate::ports::home::PrepareTransactionRequest,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<crate::domain::pairing_transaction::TransactionView, crate::ports::home::StoreError>
    {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn get_transaction_status(
        &self,
        _transaction_id: &crate::domain::pairing_transaction::TransactionId,
        _commit_token_hash: &crate::domain::human_session::TokenHash,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<
        Option<crate::domain::pairing_transaction::TransactionView>,
        crate::ports::home::StoreError,
    > {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn commit_transaction(
        &self,
        _command: &crate::ports::home::CommitTransactionCommand,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<crate::domain::pairing_transaction::TransactionView, crate::ports::home::StoreError>
    {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
    async fn abort_transaction(
        &self,
        _command: &crate::ports::home::AbortTransactionCommand,
        _now: crate::domain::time::UtcMillis,
    ) -> Result<(), crate::ports::home::StoreError> {
        Err(crate::ports::home::StoreError::Database(denied()))
    }
}

fn probe_router() -> axum::Router {
    build_router(
        Arc::new(StubProbe {
            pending: 0,
            failed: 0,
        }),
        Arc::new(DenyIdentity),
        Arc::new(DenyAgent),
        Arc::new(DenyRoom),
        Arc::new(DenyMessage),
        Arc::new(DenyBackend),
        Arc::new(DenyEvent),
        Arc::new(DenyContact),
        Arc::new(DenyHumanEvents),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(DenyAttachment),
        Arc::new(DenyBlob),
        Arc::new(DenyHome),
        Arc::new(DenyPresentation),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    )
}

/// 拒绝一切访问的 Event 存储桩：只用于不触达 Event 端点的探针测试。
struct DenyEvent;

#[async_trait::async_trait]
impl crate::ports::event::EventStore for DenyEvent {
    async fn list_events(
        &self,
        _target_agent_id: &crate::domain::participant::ParticipantId,
        _after: i64,
        _limit: u32,
    ) -> Result<crate::ports::event::EventPage, crate::ports::event::EventStoreError> {
        Err(crate::ports::event::EventStoreError::Database(denied()))
    }
    async fn resolve_event_after(
        &self,
        _target_agent_id: &crate::domain::participant::ParticipantId,
        _cursor_event_id: &crate::domain::agent_event::AgentEventId,
    ) -> Result<Option<i64>, crate::ports::event::EventStoreError> {
        Err(crate::ports::event::EventStoreError::Database(denied()))
    }
    async fn event_targets(
        &self,
        _kind: &crate::domain::agent_event::AgentEventKind,
        _message_id: &crate::domain::message::MessageId,
    ) -> Result<Vec<crate::domain::participant::ParticipantId>, crate::ports::event::EventStoreError>
    {
        Err(crate::ports::event::EventStoreError::Database(denied()))
    }
}

/// 拒绝一切访问的 Human Event 存储桩：只用于不触达 Human Event 端点的
/// 探针测试。
struct DenyHumanEvents;

#[async_trait::async_trait]
impl crate::ports::human_event::HumanEventStore for DenyHumanEvents {
    async fn list_events(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _after: i64,
        _limit: u32,
    ) -> Result<
        Vec<crate::ports::human_event::HumanEventWithFact>,
        crate::ports::human_event::HumanEventStoreError,
    > {
        Err(crate::ports::human_event::HumanEventStoreError::Database(
            denied(),
        ))
    }
    async fn delivery_targets(
        &self,
        _sources: &[crate::domain::human_event::HumanEventSource],
    ) -> Result<
        Vec<crate::domain::participant::ParticipantId>,
        crate::ports::human_event::HumanEventStoreError,
    > {
        Err(crate::ports::human_event::HumanEventStoreError::Database(
            denied(),
        ))
    }
    async fn resolve_event_after(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _cursor_fact_id: &crate::domain::human_event::HumanEventFactId,
    ) -> Result<Option<i64>, crate::ports::human_event::HumanEventStoreError> {
        Err(crate::ports::human_event::HumanEventStoreError::Database(
            denied(),
        ))
    }
}

/// 拒绝一切访问的 Completion 存储桩：只用于不触达 Completion 端点的
/// 探针测试。
struct DenyContact;

#[async_trait::async_trait]
impl crate::ports::contact::ContactStore for DenyContact {
    async fn send_contact_request(
        &self,
        _requester_id: &crate::domain::participant::ParticipantId,
        _target_id: &crate::domain::participant::ParticipantId,
        _idempotency_key: &crate::domain::agent::IdempotencyKey,
    ) -> Result<ContactOutcome, ContactStoreError> {
        Err(ContactStoreError::Database(denied()))
    }
    async fn list_contacts(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<crate::ports::contact::ContactListItem>, ContactStoreError> {
        Err(ContactStoreError::Database(denied()))
    }
    async fn accept_contact(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _requester_id: &crate::domain::participant::ParticipantId,
    ) -> Result<
        (
            crate::domain::contact::Contact,
            crate::ports::room::ParticipantProfile,
        ),
        ContactStoreError,
    > {
        Err(ContactStoreError::Database(denied()))
    }
    async fn decline_contact(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _requester_id: &crate::domain::participant::ParticipantId,
    ) -> Result<(), ContactStoreError> {
        Err(ContactStoreError::Database(denied()))
    }
    async fn end_contact(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _peer_id: &crate::domain::participant::ParticipantId,
    ) -> Result<(), ContactStoreError> {
        Err(ContactStoreError::Database(denied()))
    }
    async fn block_participant(
        &self,
        _blocker_id: &crate::domain::participant::ParticipantId,
        _blocked_id: &crate::domain::participant::ParticipantId,
    ) -> Result<(), crate::ports::contact::BlockStoreError> {
        Err(crate::ports::contact::BlockStoreError::Database(denied()))
    }
    async fn unblock_participant(
        &self,
        _blocker_id: &crate::domain::participant::ParticipantId,
        _blocked_id: &crate::domain::participant::ParticipantId,
    ) -> Result<(), crate::ports::contact::BlockStoreError> {
        Err(crate::ports::contact::BlockStoreError::Database(denied()))
    }
    async fn list_blocks(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<crate::ports::contact::BlockedHuman>, ContactStoreError> {
        Err(ContactStoreError::Database(denied()))
    }
}

/// 拒绝一切访问的 Backend 存储桩：只用于不触达 Backend 端点的探针测试。
struct DenyBackend;

#[async_trait::async_trait]
impl crate::ports::backend::BackendStore for DenyBackend {
    async fn create_binding(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _binding: &crate::ports::backend::NewBinding,
    ) -> Result<crate::ports::backend::CreateBindingOutcome, crate::ports::backend::BackendStoreError>
    {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
    async fn list_bindings(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
    ) -> Result<Vec<crate::domain::backend::BackendBinding>, crate::ports::backend::BackendStoreError>
    {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
    async fn get_binding(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
        _binding_id: &str,
    ) -> Result<crate::domain::backend::BackendBinding, crate::ports::backend::BackendStoreError>
    {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
    async fn revoke_binding(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
        _binding_id: &str,
    ) -> Result<(), crate::ports::backend::BackendStoreError> {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
    async fn issue_credential(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
        _binding_id: &str,
        _credential: &crate::ports::backend::NewCredential,
    ) -> Result<
        crate::ports::backend::IssueCredentialOutcome,
        crate::ports::backend::BackendStoreError,
    > {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
    async fn list_credentials(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
        _binding_id: &str,
    ) -> Result<
        Vec<crate::domain::backend::BackendCredential>,
        crate::ports::backend::BackendStoreError,
    > {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
    async fn revoke_credential(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _agent_id: &crate::domain::participant::ParticipantId,
        _binding_id: &str,
        _credential_id: &str,
    ) -> Result<(), crate::ports::backend::BackendStoreError> {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
    async fn authenticate(
        &self,
        _token_hash: &str,
    ) -> Result<
        Option<crate::ports::backend::BackendAuthView>,
        crate::ports::backend::BackendStoreError,
    > {
        Err(crate::ports::backend::BackendStoreError::Database(denied()))
    }
}

/// 真实 SQLite + 已 bootstrap Human 的完整路由。
struct IdentityFixture {
    _dir: tempfile::TempDir,
    identity: SqliteIdentityStore,
    token: String,
    participant_id: String,
    session_id: String,
}

async fn identity_router() -> (axum::Router, IdentityFixture) {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let agent = SqliteAgentStore::new(pool.clone());
    let room = SqliteRoomStore::new(pool.clone());
    let outcome = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap");
    let router = build_router(
        Arc::new(store),
        Arc::new(identity.clone()),
        Arc::new(agent),
        Arc::new(room),
        Arc::new(crate::adapters::sqlite::message_store::SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::event_store::SqliteEventStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::contact_store::SqliteContactStore::new(pool.clone())),
        Arc::new(
            crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(pool.clone()),
        ),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(
            crate::adapters::sqlite::attachment_store::SqliteAttachmentStore::new(pool.clone()),
        ),
        Arc::new(crate::adapters::blob::LocalBlobStorage::new(
            dir.path().join("media"),
        )),
        Arc::new(crate::adapters::sqlite::home_store::SqliteHomeStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    (
        router,
        IdentityFixture {
            _dir: dir,
            identity,
            token: outcome.token.as_str().to_owned(),
            participant_id: outcome.participant.id.as_str().to_owned(),
            session_id: outcome.session.id.as_str().to_owned(),
        },
    )
}

async fn send(
    router: &axum::Router,
    method: axum::http::Method,
    path: &str,
    authorization: Option<&str>,
    duplicate: bool,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
        if duplicate {
            builder = builder.header(header::AUTHORIZATION, auth);
        }
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let request_id = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json, request_id)
}

async fn get(router: &axum::Router, path: &str) -> (StatusCode, serde_json::Value, Option<String>) {
    send(router, axum::http::Method::GET, path, None, false).await
}

async fn post(
    router: &axum::Router,
    path: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    send(router, axum::http::Method::POST, path, None, false).await
}

#[tokio::test]
async fn health_returns_200_with_request_id() {
    let router = probe_router();
    let (status, json, request_id) = get(&router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(request_id.is_some(), "x-request-id header must be present");
    assert_eq!(
        json["request_id"].as_str().unwrap(),
        request_id.as_deref().unwrap()
    );
}

#[tokio::test]
async fn ready_returns_200_on_real_temp_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let router = build_router(
        Arc::new(store),
        Arc::new(DenyIdentity),
        Arc::new(DenyAgent),
        Arc::new(DenyRoom),
        Arc::new(DenyMessage),
        Arc::new(DenyBackend),
        Arc::new(DenyEvent),
        Arc::new(DenyContact),
        Arc::new(DenyHumanEvents),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(DenyAttachment),
        Arc::new(DenyBlob),
        Arc::new(DenyHome),
        Arc::new(DenyPresentation),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );

    let (status, json, request_id) = get(&router, "/ready").await;
    assert_eq!(status, StatusCode::OK, "ready must pass on a migrated db");
    assert_eq!(json["status"], "ok");
    assert!(request_id.is_some());
}

#[tokio::test]
async fn ready_returns_503_with_stable_code_when_not_ready() {
    let router = build_router(
        Arc::new(StubProbe {
            pending: 1,
            failed: 0,
        }),
        Arc::new(DenyIdentity),
        Arc::new(DenyAgent),
        Arc::new(DenyRoom),
        Arc::new(DenyMessage),
        Arc::new(DenyBackend),
        Arc::new(DenyEvent),
        Arc::new(DenyContact),
        Arc::new(DenyHumanEvents),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(DenyAttachment),
        Arc::new(DenyBlob),
        Arc::new(DenyHome),
        Arc::new(DenyPresentation),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    let (status, json, request_id) = get(&router, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["error"]["code"], "not_ready");
    assert!(request_id.is_some());
    assert_eq!(
        json["error"]["request_id"].as_str().unwrap(),
        request_id.as_deref().unwrap()
    );
}

#[tokio::test]
async fn unknown_route_returns_stable_not_found() {
    let (status, json, _) = get(&probe_router(), "/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "not_found");
}

#[tokio::test]
async fn post_health_returns_405_with_stable_code_and_request_id() {
    let (status, json, request_id) = post(&probe_router(), "/health").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(json["error"]["code"], "method_not_allowed");
    assert!(request_id.is_some(), "x-request-id header must be present");
    assert_eq!(
        json["error"]["request_id"].as_str().unwrap(),
        request_id.as_deref().unwrap()
    );
}

#[tokio::test]
async fn post_ready_returns_405_with_stable_code_and_request_id() {
    let (status, json, request_id) = post(&probe_router(), "/ready").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(json["error"]["code"], "method_not_allowed");
    assert!(request_id.is_some(), "x-request-id header must be present");
}

#[tokio::test]
async fn post_unknown_route_still_returns_404() {
    let (status, json, _) = post(&probe_router(), "/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "not_found");
}

#[tokio::test]
async fn me_returns_participant_and_session_without_secrets() {
    let (router, fixture) = identity_router().await;
    let (status, json, request_id) = send(
        &router,
        axum::http::Method::GET,
        "/v1/me",
        Some(&format!("Bearer {}", fixture.token)),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["participant"]["id"], fixture.participant_id);
    assert_eq!(json["participant"]["kind"], "human");
    assert_eq!(json["participant"]["display_name"], "Alice");
    assert_eq!(json["participant"]["status"], "active");
    assert!(
        json["participant"]["created_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    assert_eq!(json["session"]["id"], fixture.session_id);
    assert!(
        json["session"]["created_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );

    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["request_id"], request_id);

    let body = serde_json::to_string(&json).unwrap();
    assert!(
        !body.contains(&fixture.token),
        "token must not leak into /v1/me"
    );
    assert!(
        !body.contains("token_hash"),
        "token hash must not leak into /v1/me"
    );
    assert!(json.get("token").is_none());
}

#[tokio::test]
async fn me_missing_authorization_is_authentication_required() {
    let (router, _fixture) = identity_router().await;
    let (status, json, request_id) = get(&router, "/v1/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "authentication_required");
    assert!(request_id.is_some());
    assert_eq!(json["error"]["request_id"], request_id.unwrap());
}

#[tokio::test]
async fn me_wrong_scheme_is_invalid_session() {
    let (router, _fixture) = identity_router().await;
    for auth in ["Basic dXNlcjpwYXNz", "digest xyz", "Token abc"] {
        let (status, json, _) = send(
            &router,
            axum::http::Method::GET,
            "/v1/me",
            Some(auth),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "scheme {auth:?}");
        assert_eq!(json["error"]["code"], "invalid_session", "scheme {auth:?}");
    }
}

#[tokio::test]
async fn me_empty_or_malformed_token_is_invalid_session() {
    let (router, _fixture) = identity_router().await;
    for auth in ["Bearer ", "Bearer", "bearer ", "Bearer  abc"] {
        let (status, json, _) = send(
            &router,
            axum::http::Method::GET,
            "/v1/me",
            Some(auth),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "auth {auth:?}");
        assert_eq!(json["error"]["code"], "invalid_session", "auth {auth:?}");
    }
}

#[tokio::test]
async fn me_duplicate_authorization_headers_is_invalid_session() {
    let (router, fixture) = identity_router().await;
    let (status, json, _) = send(
        &router,
        axum::http::Method::GET,
        "/v1/me",
        Some(&format!("Bearer {}", fixture.token)),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["code"], "invalid_session");
}

#[tokio::test]
async fn me_random_or_garbage_token_is_invalid_session() {
    let (router, _fixture) = identity_router().await;
    for auth in [
        "Bearer clr_hs_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "Bearer clr_hs_AAAA",
        "clr_hs_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "Bearer not-a-valid-token-format-at-all",
    ] {
        let (status, json, _) = send(
            &router,
            axum::http::Method::GET,
            "/v1/me",
            Some(auth),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "auth {auth:?}");
        assert_eq!(json["error"]["code"], "invalid_session", "auth {auth:?}");
    }
}

#[tokio::test]
async fn delete_current_session_returns_204_with_request_id() {
    let (router, fixture) = identity_router().await;
    let (status, json, request_id) = send(
        &router,
        axum::http::Method::DELETE,
        "/v1/sessions/current",
        Some(&format!("Bearer {}", fixture.token)),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(request_id.is_some(), "204 must still carry x-request-id");
    assert!(json.get("error").is_none());
}

#[tokio::test]
async fn revoked_token_fails_closed_and_repeat_delete_returns_401() {
    let (router, fixture) = identity_router().await;

    let (status, _, _) = send(
        &router,
        axum::http::Method::DELETE,
        "/v1/sessions/current",
        Some(&format!("Bearer {}", fixture.token)),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    for (method, path) in [
        (axum::http::Method::GET, "/v1/me"),
        (axum::http::Method::DELETE, "/v1/sessions/current"),
    ] {
        let (status, json, _) = send(
            &router,
            method.clone(),
            path,
            Some(&format!("Bearer {}", fixture.token)),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_eq!(json["error"]["code"], "invalid_session", "{method} {path}");
    }
}

#[tokio::test]
async fn revoke_affects_only_current_session_and_keeps_human() {
    let (router, fixture) = identity_router().await;
    let second = identity::issue_human_session(&fixture.identity, &fixture.participant_id)
        .await
        .expect("issue second session");
    let token2 = second.token.as_str().to_owned();

    let (status, _, _) = send(
        &router,
        axum::http::Method::DELETE,
        "/v1/sessions/current",
        Some(&format!("Bearer {}", fixture.token)),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _, _) = send(
        &router,
        axum::http::Method::GET,
        "/v1/me",
        Some(&format!("Bearer {}", fixture.token)),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked token must fail");

    let (status, json, _) = send(
        &router,
        axum::http::Method::GET,
        "/v1/me",
        Some(&format!("Bearer {token2}")),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "other session must stay valid");
    assert_eq!(
        json["participant"]["id"], fixture.participant_id,
        "human survives revoke"
    );
}

#[tokio::test]
async fn health_and_ready_stay_anonymous_after_bootstrap() {
    let (router, _fixture) = identity_router().await;
    let (status, json, _) = get(&router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");

    let (status, json, _) = get(&router, "/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn post_v1_me_returns_405() {
    let (router, _fixture) = identity_router().await;
    let (status, json, _) = post(&router, "/v1/me").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(json["error"]["code"], "method_not_allowed");
}

/// 带请求体的 HTTP 请求（认证可选）。
async fn send_with_body(
    router: &axum::Router,
    method: axum::http::Method,
    path: &str,
    authorization: Option<&str>,
    body: &[u8],
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let request_id = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json, request_id)
}

#[tokio::test]
async fn client_x_request_id_does_not_override_canonical_id() {
    let router = probe_router();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(axum::http::Method::GET)
                .uri("/health")
                .header("x-request-id", "client-trace-abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let header = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .expect("x-request-id must be present")
        .to_owned();
    assert_ne!(
        header, "client-trace-abc",
        "canonical request id must be server-generated, client value must not override"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["request_id"], header,
        "success body request_id must equal response header"
    );
}

#[tokio::test]
async fn malformed_json_returns_uniform_error_body() {
    let (router, fixture) = identity_router().await;
    let (status, json, request_id) = send_with_body(
        &router,
        axum::http::Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {}", fixture.token)),
        b"{not json",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);
}

#[tokio::test]
async fn body_too_large_returns_uniform_413() {
    let (router, fixture) = identity_router().await;
    let big = vec![b'a'; 5000];
    let (status, json, request_id) = send_with_body(
        &router,
        axum::http::Method::POST,
        "/v1/agents",
        Some(&format!("Bearer {}", fixture.token)),
        &big,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json["error"]["code"], "payload_too_large");
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);
}

#[tokio::test]
async fn unknown_route_and_method_mismatch_have_matching_request_id() {
    let router = probe_router();
    let (status, json, request_id) = get(&router, "/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], "not_found");
    assert_eq!(json["error"]["request_id"], request_id.unwrap());

    let (status, json, request_id) = post(&router, "/health").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(json["error"]["code"], "method_not_allowed");
    assert_eq!(json["error"]["request_id"], request_id.unwrap());
}

#[tokio::test]
async fn ready_probe_failure_returns_fixed_copy_without_leaking_reason() {
    let router = build_router(
        Arc::new(FailingProbe),
        Arc::new(DenyIdentity),
        Arc::new(DenyAgent),
        Arc::new(DenyRoom),
        Arc::new(DenyMessage),
        Arc::new(DenyBackend),
        Arc::new(DenyEvent),
        Arc::new(DenyContact),
        Arc::new(DenyHumanEvents),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(DenyAttachment),
        Arc::new(DenyBlob),
        Arc::new(DenyHome),
        Arc::new(DenyPresentation),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    let (status, json, request_id) = get(&router, "/ready").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json["error"]["code"], "internal_error");
    let message = json["error"]["message"].as_str().unwrap();
    assert_eq!(message, "storage probe failed");
    let body = serde_json::to_string(&json).unwrap().to_lowercase();
    for forbidden in ["/tmp", "fake", "sqlite", "sql", "locked", "missing"] {
        assert!(
            !body.contains(forbidden),
            "probe reason must not leak into public body: {forbidden:?}"
        );
    }
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);
}

#[tokio::test]
async fn not_ready_returns_fixed_copy_without_migration_counts() {
    let router = build_router(
        Arc::new(StubProbe {
            pending: 3,
            failed: 1,
        }),
        Arc::new(DenyIdentity),
        Arc::new(DenyAgent),
        Arc::new(DenyRoom),
        Arc::new(DenyMessage),
        Arc::new(DenyBackend),
        Arc::new(DenyEvent),
        Arc::new(DenyContact),
        Arc::new(DenyHumanEvents),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(DenyAttachment),
        Arc::new(DenyBlob),
        Arc::new(DenyHome),
        Arc::new(DenyPresentation),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    let (status, json, request_id) = get(&router, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["error"]["code"], "not_ready");
    assert_eq!(json["error"]["message"], "service not ready");
    let body = serde_json::to_string(&json).unwrap();
    assert!(
        !body.contains("pending") && !body.contains("failed"),
        "migration counts must not leak"
    );
    assert_eq!(json["error"]["request_id"], request_id.unwrap());
}

#[tokio::test]
async fn path_extractor_rejection_is_uniform_json() {
    let (router, fixture) = identity_router().await;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(axum::http::Method::POST)
                .uri("/v1/rooms/%FF/messages")
                .header(header::AUTHORIZATION, format!("Bearer {}", fixture.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(br#"{"body":"x"}"#.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .expect("x-request-id must be present")
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(json["error"]["request_id"], request_id);
}

#[tokio::test]
async fn ws_upgrade_rejection_is_uniform_json() {
    let (status, json, request_id) = get(&probe_router(), "/v1/human/events/stream").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "invalid_request");
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);
}

#[tokio::test]
async fn me_store_failure_returns_storage_unavailable() {
    let (status, json, request_id) = send(
        &probe_router(),
        axum::http::Method::GET,
        "/v1/me",
        Some("Bearer clr_hs_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["error"]["code"], "storage_unavailable");
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);
}

#[tokio::test]
async fn decode_or_corrupt_store_failure_returns_internal_error_without_leaking_cause() {
    let router = build_router(
        Arc::new(StubProbe {
            pending: 0,
            failed: 0,
        }),
        Arc::new(DecodeIdentity),
        Arc::new(DenyAgent),
        Arc::new(DenyRoom),
        Arc::new(DenyMessage),
        Arc::new(DenyBackend),
        Arc::new(DenyEvent),
        Arc::new(DenyContact),
        Arc::new(DenyHumanEvents),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(DenyAttachment),
        Arc::new(DenyBlob),
        Arc::new(DenyHome),
        Arc::new(DenyPresentation),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    let (status, json, request_id) = send(
        &router,
        axum::http::Method::GET,
        "/v1/me",
        Some("Bearer clr_hs_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json["error"]["code"], "internal_error");
    let body = serde_json::to_string(&json).unwrap().to_lowercase();
    for forbidden in ["/tmp", "secret", "snapshot", "unparseable", "injected"] {
        assert!(
            !body.contains(forbidden),
            "decode_or_corrupt response must not leak cause {forbidden:?}: {body:?}"
        );
    }
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);
}

/// 桩端口：身份存储返回 `constraint`（模拟未被应用层吸收的约束失败逃逸到
/// 接入层）。约束逃逸是确定性内部不变量破坏：必须 500 `internal_error`，
/// 禁止可重试 503。
struct ConstraintIdentity;
#[async_trait::async_trait]
impl crate::ports::identity::IdentityStore for ConstraintIdentity {
    async fn bootstrap_first_human(
        &self,
        _human: &NewHuman,
        _session: &NewSession,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::Constraint,
            "injected UNIQUE constraint failed on secret_table",
        )))
    }
    async fn issue_human_session(&self, _session: &NewSession) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::Constraint,
            "injected UNIQUE constraint failed on secret_table",
        )))
    }
    async fn create_human(
        &self,
        _human: &NewHuman,
        _session: &NewSession,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::Constraint,
            "injected UNIQUE constraint failed on secret_table",
        )))
    }
    async fn authenticate(&self, _hash: &TokenHash) -> Result<Option<AuthContext>, StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::Constraint,
            "injected UNIQUE constraint failed on secret_table",
        )))
    }
    async fn revoke_session(&self, _session_id: &SessionId) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            StorageFailureKind::Constraint,
            "injected UNIQUE constraint failed on secret_table",
        )))
    }

    /// 表达权第四刀（2026-10-17）：本桩**不支持**改名 —— 沿用本桩既有的拒绝态度，
    /// 不新造行为（改名那一面的行为用例在 `human_profile_routes_tests.rs`）。
    async fn rename_human(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _display_name: &crate::domain::participant::DisplayName,
    ) -> Result<(), StoreError> {
        Err(StoreError::HumanNotActive)
    }
}

/// 未被应用层吸收的 `constraint` 逃逸到接入层 → 500 `internal_error`
/// （确定性内部不变量，禁止可重试 503），且 cause 文本不泄漏。
#[tokio::test]
async fn escaped_constraint_store_failure_returns_500_internal_error_without_leaking_cause() {
    let router = build_router(
        Arc::new(StubProbe {
            pending: 0,
            failed: 0,
        }),
        Arc::new(ConstraintIdentity),
        Arc::new(DenyAgent),
        Arc::new(DenyRoom),
        Arc::new(DenyMessage),
        Arc::new(DenyBackend),
        Arc::new(DenyEvent),
        Arc::new(DenyContact),
        Arc::new(DenyHumanEvents),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(DenyAttachment),
        Arc::new(DenyBlob),
        Arc::new(DenyHome),
        Arc::new(DenyPresentation),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    let (status, json, request_id) = send(
        &router,
        axum::http::Method::GET,
        "/v1/me",
        Some("Bearer clr_hs_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json["error"]["code"], "internal_error");
    let body = serde_json::to_string(&json).unwrap().to_lowercase();
    for forbidden in ["constraint", "unique", "secret_table", "injected"] {
        assert!(
            !body.contains(forbidden),
            "constraint response must not leak cause {forbidden:?}: {body:?}"
        );
    }
    let request_id = request_id.expect("x-request-id must be present");
    assert_eq!(json["error"]["request_id"], request_id);
}

/// 真实 SQLite Human Event store 的故障注入包装：`list_events` 透传
/// （REST 恢复来源保持真实），`delivery_targets` 注入存储失败（post-commit
/// 唤醒目标查询故障）。
struct FailDeliveryHumanEvents {
    inner: crate::adapters::sqlite::human_event_store::SqliteHumanEventStore,
}

#[async_trait::async_trait]
impl crate::ports::human_event::HumanEventStore for FailDeliveryHumanEvents {
    async fn list_events(
        &self,
        human_id: &crate::domain::participant::ParticipantId,
        after: i64,
        limit: u32,
    ) -> Result<
        Vec<crate::ports::human_event::HumanEventWithFact>,
        crate::ports::human_event::HumanEventStoreError,
    > {
        self.inner.list_events(human_id, after, limit).await
    }
    async fn resolve_event_after(
        &self,
        human_id: &crate::domain::participant::ParticipantId,
        cursor_fact_id: &crate::domain::human_event::HumanEventFactId,
    ) -> Result<Option<i64>, crate::ports::human_event::HumanEventStoreError> {
        self.inner
            .resolve_event_after(human_id, cursor_fact_id)
            .await
    }
    async fn delivery_targets(
        &self,
        _sources: &[crate::domain::human_event::HumanEventSource],
    ) -> Result<
        Vec<crate::domain::participant::ParticipantId>,
        crate::ports::human_event::HumanEventStoreError,
    > {
        Err(crate::ports::human_event::HumanEventStoreError::Database(
            StorageError::from_message(
                StorageFailureKind::BusyOrLocked,
                "injected delivery targets lookup failure",
            ),
        ))
    }
}

/// notify 必 panic 的 notifier 桩：证明 post-commit 通知 panic 不翻转已
/// 提交成功（wake_humans 内最小隔离）。
struct PanicNotifier;

#[async_trait::async_trait]
impl crate::ports::human_event::HumanEventNotifier for PanicNotifier {
    fn subscribe(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _wake: std::sync::Arc<tokio::sync::mpsc::Sender<()>>,
    ) {
    }
    fn notify(&self, _human_ids: &[crate::domain::participant::ParticipantId]) {
        panic!("injected notifier panic");
    }
}

/// 真实 SQLite 全栈 fixture（与 identity_router 同一组装），但 human_events
/// 与 notifier 可注入故障桩。调用方先打开 store（migrations 已应用），传入
/// dir 与 pool 保证所有 Store 指向同一数据库文件。
async fn full_router_with(
    dir: tempfile::TempDir,
    pool: sqlx::SqlitePool,
    human_events: Arc<dyn crate::ports::human_event::HumanEventStore>,
    live: crate::adapters::live::LiveWiring,
) -> (axum::Router, IdentityFixture) {
    let identity = SqliteIdentityStore::new(pool.clone());
    let agent = SqliteAgentStore::new(pool.clone());
    let room = SqliteRoomStore::new(pool.clone());
    let outcome = identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap");
    let router = build_router(
        Arc::new(
            crate::adapters::sqlite::migration_store::MigrationStore::open(
                &dir.path().join("core.db"),
            )
            .await
            .expect("real sqlite with applied migrations"),
        ),
        Arc::new(identity.clone()),
        Arc::new(agent),
        Arc::new(room),
        Arc::new(crate::adapters::sqlite::message_store::SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::event_store::SqliteEventStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::contact_store::SqliteContactStore::new(pool.clone())),
        human_events,
        // 接线束由调用方给出：要观察唤醒的用例注入自己的 notifier 桩，
        // 其余用默认组合（唤醒端口与连接事实同一个对象）。
        live,
        Arc::new(
            crate::adapters::sqlite::attachment_store::SqliteAttachmentStore::new(pool.clone()),
        ),
        Arc::new(crate::adapters::blob::LocalBlobStorage::new(
            dir.path().join("media"),
        )),
        Arc::new(crate::adapters::sqlite::home_store::SqliteHomeStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    );
    (
        router,
        IdentityFixture {
            _dir: dir,
            identity,
            token: outcome.token.as_str().to_owned(),
            participant_id: outcome.participant.id.as_str().to_owned(),
            session_id: outcome.session.id.as_str().to_owned(),
        },
    )
}

/// 带 Idempotency-Key 的 JSON 请求（故障注入流程专用）。
async fn send_with_key(
    router: &axum::Router,
    method: axum::http::Method,
    path: &str,
    authorization: Option<&str>,
    body: &[u8],
    key: &str,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", key);
    if let Some(auth) = authorization {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let request_id = response_request_id(&response);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    (status, json, request_id)
}

/// 捕获 tracing 输出的内存缓冲 writer（断言日志字段用）。
#[derive(Clone)]
struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 消息已 commit 后 `delivery_targets` 故障：POST 仍返回 201（成功不翻转），
/// 唤醒 warn 带 canonical request_id，随后 REST list_events 恢复出耐久事实。
#[tokio::test]
async fn post_commit_delivery_targets_failure_keeps_success_and_events_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let inner = crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(
        store.pool().clone(),
    );
    let (router, fixture) = full_router_with(
        dir,
        store.pool().clone(),
        Arc::new(FailDeliveryHumanEvents { inner }),
        crate::adapters::live::LiveWiring::new_volatile(),
    )
    .await;
    let bearer = format!("Bearer {}", fixture.token);

    let (status, json, _) = send_with_key(
        &router,
        axum::http::Method::POST,
        "/v1/agents",
        Some(&bearer),
        br#"{"display_name":"Bot"}"#,
        "k1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "agent create must succeed");
    let agent_id = json["agent"]["id"].as_str().expect("agent id").to_owned();

    let (status, json, _) = send_with_key(
        &router,
        axum::http::Method::POST,
        "/v1/direct-rooms",
        Some(&bearer),
        &serde_json::json!({"agent_id": agent_id})
            .to_string()
            .into_bytes(),
        "k2",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "direct room must succeed");
    let room_id = json["room"]["id"].as_str().expect("room id").to_owned();

    // 捕获本请求日志：wake_humans 的 warn 必须带 canonical request_id。
    let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(move || LogBuffer(writer.clone()))
        .finish();
    let dispatch = tracing::subscriber::set_default(subscriber);

    let (status, json, request_id) = send_with_key(
        &router,
        axum::http::Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&bearer),
        br#"{"body":"hello"}"#,
        "k3",
    )
    .await;
    drop(dispatch);

    assert_eq!(
        status,
        StatusCode::CREATED,
        "delivery_targets failure must not flip committed success"
    );
    assert_eq!(json["message"]["body"], "hello");
    let request_id = request_id.expect("x-request-id must be present");
    let logs = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("human_event.wake.delivery_targets") && logs.contains("storage_unavailable"),
        "wake target lookup failure must be recorded as safe warn: {logs}"
    );
    assert!(
        logs.contains(&request_id),
        "wake warn must carry the canonical request_id: {logs}"
    );

    // 已提交消息事实仍可由 REST 恢复：list_events 返回 message.created。
    let (status, json, _) = send(
        &router,
        axum::http::Method::GET,
        "/v1/human/events",
        Some(&bearer),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "REST recovery must still work");
    let events = json["events"].as_array().expect("events array");
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "message.created" && e["snapshot"]["body"] == "hello"),
        "durable message.created event must be recoverable: {json}"
    );
}

/// notifier panic（最小隔离捕获）不翻转已提交成功：POST 消息仍 201，REST
/// 恢复不受影响。
#[tokio::test]
async fn post_commit_notifier_panic_does_not_flip_committed_success() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let human_events: Arc<dyn crate::ports::human_event::HumanEventStore> = Arc::new(
        crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(
            store.pool().clone(),
        ),
    );
    let (router, fixture) = full_router_with(
        dir,
        store.pool().clone(),
        human_events,
        // 注入一个会 panic 的唤醒端口：post-commit 唤醒炸了不许翻转已成功
        // 的请求。连接事实另给一份干净实现（本用例不验在线）。
        crate::adapters::live::LiveWiring::from_parts(
            Arc::new(PanicNotifier),
            Arc::new(crate::adapters::notifier::VolatileHumanEventNotifier::new()),
        ),
    )
    .await;
    let bearer = format!("Bearer {}", fixture.token);

    let (status, json, _) = send_with_key(
        &router,
        axum::http::Method::POST,
        "/v1/agents",
        Some(&bearer),
        br#"{"display_name":"Bot"}"#,
        "k1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let agent_id = json["agent"]["id"].as_str().expect("agent id").to_owned();

    let (status, json, _) = send_with_key(
        &router,
        axum::http::Method::POST,
        "/v1/direct-rooms",
        Some(&bearer),
        &serde_json::json!({"agent_id": agent_id})
            .to_string()
            .into_bytes(),
        "k2",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let room_id = json["room"]["id"].as_str().expect("room id").to_owned();

    let (status, json, _) = send_with_key(
        &router,
        axum::http::Method::POST,
        &format!("/v1/rooms/{room_id}/messages"),
        Some(&bearer),
        br#"{"body":"still committed"}"#,
        "k3",
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "notifier panic must not flip committed success"
    );
    assert_eq!(json["message"]["body"], "still committed");

    let (status, json, _) = send(
        &router,
        axum::http::Method::GET,
        "/v1/human/events",
        Some(&bearer),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = json["events"].as_array().expect("events array");
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "message.created" && e["snapshot"]["body"] == "still committed"),
        "durable event must be recoverable after notifier panic: {json}"
    );
}

/// 身份存储包装：认证透传真实 SQLite，`revoke_session` 注入指定分类的
/// 存储失败（用于验证 DELETE /v1/sessions/current 的 StorageFailureKind
/// 分类映射）。
struct RevokeFailingIdentity {
    inner: SqliteIdentityStore,
    kind: StorageFailureKind,
}

#[async_trait::async_trait]
impl crate::ports::identity::IdentityStore for RevokeFailingIdentity {
    async fn bootstrap_first_human(
        &self,
        human: &NewHuman,
        session: &NewSession,
    ) -> Result<(), StoreError> {
        self.inner.bootstrap_first_human(human, session).await
    }
    async fn issue_human_session(&self, session: &NewSession) -> Result<(), StoreError> {
        self.inner.issue_human_session(session).await
    }
    async fn create_human(&self, human: &NewHuman, session: &NewSession) -> Result<(), StoreError> {
        self.inner.create_human(human, session).await
    }
    async fn authenticate(&self, hash: &TokenHash) -> Result<Option<AuthContext>, StoreError> {
        self.inner.authenticate(hash).await
    }
    async fn revoke_session(&self, _session_id: &SessionId) -> Result<(), StoreError> {
        Err(StoreError::Database(StorageError::from_message(
            self.kind,
            "injected revoke failure at /tmp/secret/core.db",
        )))
    }

    /// 表达权第四刀（2026-10-17）：本桩**不支持**改名 —— 沿用本桩既有的拒绝态度，
    /// 不新造行为（改名那一面的行为用例在 `human_profile_routes_tests.rs`）。
    async fn rename_human(
        &self,
        _human_id: &crate::domain::participant::ParticipantId,
        _display_name: &crate::domain::participant::DisplayName,
    ) -> Result<(), StoreError> {
        Err(StoreError::HumanNotActive)
    }
}

async fn revoke_router_with(
    _dir: tempfile::TempDir,
    store: MigrationStore,
    identity: Arc<RevokeFailingIdentity>,
) -> axum::Router {
    let pool = store.pool().clone();
    build_router(
        Arc::new(store),
        identity,
        Arc::new(SqliteAgentStore::new(pool.clone())),
        Arc::new(SqliteRoomStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::message_store::SqliteMessageStore::new(pool.clone())),
        Arc::new(SqliteBackendStore::new(pool.clone())),
        Arc::new(crate::adapters::sqlite::event_store::SqliteEventStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::contact_store::SqliteContactStore::new(pool.clone())),
        Arc::new(
            crate::adapters::sqlite::human_event_store::SqliteHumanEventStore::new(pool.clone()),
        ),
        crate::adapters::live::LiveWiring::new_volatile(),
        Arc::new(
            crate::adapters::sqlite::attachment_store::SqliteAttachmentStore::new(pool.clone()),
        ),
        Arc::new(crate::adapters::blob::LocalBlobStorage::new(
            _dir.path().join("media"),
        )),
        Arc::new(crate::adapters::sqlite::home_store::SqliteHomeStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore::new(
            pool.clone(),
        )),
        Arc::new(crate::adapters::call::InMemoryCallState::new()),
        15,
    )
}

/// DELETE /v1/sessions/current 的 Store 失败必须复用统一
/// StorageFailureKind 分类：busy/io → 503 storage_unavailable，其余
/// （decode_or_corrupt / constraint / unknown）→ 500 internal_error；
/// 错误日志带 canonical request_id。
#[tokio::test]
async fn revoke_store_failure_classifies_by_storage_failure_kind() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let inner = SqliteIdentityStore::new(store.pool().clone());
    let outcome = identity::bootstrap_human(&inner, "Alice")
        .await
        .expect("bootstrap");
    let bearer = format!("Bearer {}", outcome.token.as_str());

    for (kind, expected_status, expected_code) in [
        (
            StorageFailureKind::BusyOrLocked,
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
        ),
        (
            StorageFailureKind::ConnectionOrIo,
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
        ),
        (
            StorageFailureKind::DecodeOrCorrupt,
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
        ),
        (
            StorageFailureKind::Constraint,
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
        ),
        (
            StorageFailureKind::Unknown,
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
        ),
    ] {
        let router = revoke_router_with(
            tempfile::tempdir().unwrap(),
            MigrationStore::open(&dir.path().join("core.db"))
                .await
                .expect("reopen"),
            Arc::new(RevokeFailingIdentity {
                inner: inner.clone(),
                kind,
            }),
        )
        .await;
        let (status, json, request_id) = send(
            &router,
            axum::http::Method::DELETE,
            "/v1/sessions/current",
            Some(&bearer),
            false,
        )
        .await;
        assert_eq!(status, expected_status, "kind {kind:?}");
        assert_eq!(json["error"]["code"], expected_code, "kind {kind:?}");
        let request_id = request_id.expect("x-request-id must be present");
        assert_eq!(json["error"]["request_id"], request_id);
        let body = serde_json::to_string(&json).unwrap().to_lowercase();
        assert!(
            !body.contains("secret") && !body.contains("/tmp"),
            "revoke error must not leak the cause: {body}"
        );
    }
}

/// revoke 失败日志必须携带 canonical request_id（可观察性）。
#[tokio::test(flavor = "current_thread")]
async fn revoke_store_failure_logs_carry_canonical_request_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("real sqlite with applied migrations");
    let inner = SqliteIdentityStore::new(store.pool().clone());
    let outcome = identity::bootstrap_human(&inner, "Alice")
        .await
        .expect("bootstrap");
    let bearer = format!("Bearer {}", outcome.token.as_str());
    let router = revoke_router_with(
        tempfile::tempdir().unwrap(),
        MigrationStore::open(&dir.path().join("core.db"))
            .await
            .expect("reopen"),
        Arc::new(RevokeFailingIdentity {
            inner: inner.clone(),
            kind: StorageFailureKind::BusyOrLocked,
        }),
    )
    .await;

    let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(move || LogBuffer(writer.clone()))
        .finish();
    let dispatch = tracing::subscriber::set_default(subscriber);

    let (status, _, request_id) = send(
        &router,
        axum::http::Method::DELETE,
        "/v1/sessions/current",
        Some(&bearer),
        false,
    )
    .await;
    drop(dispatch);

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let request_id = request_id.expect("x-request-id must be present");
    let logs = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("identity.session.revoke") && logs.contains("storage_unavailable"),
        "revoke failure must be logged with safe classification: {logs}"
    );
    assert!(
        logs.contains(&request_id),
        "revoke failure log must carry the canonical request_id: {logs}"
    );
}
