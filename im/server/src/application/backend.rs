//! Backend 用例：Binding 创建（幂等）、列表、读取、软撤销；Credential
//! 首次签发 / CAS 轮换（幂等）、列表、软撤销；Backend 认证解析。
//!
//! 用例层负责规则编排：校验幂等键、label 与 ID 格式，生成 Binding /
//! Credential ID、token 与摘要，再把已就绪的事实交给
//! `ports::backend::BackendStore` 一次性持久化。unknown / bad-id /
//! not-owned Agent 在应用层合并为统一 NotFound（HTTP 404 agent_not_found），
//! 不泄露其他 Human 的 Agent 是否存在。

use uuid::Uuid;

use crate::application::unexpected_store_error;
use crate::domain::agent::IdempotencyKey;
use crate::domain::backend::{
    BackendBinding, BackendCredential, BackendLabel, BackendLabelError, BackendToken, BindingId,
    CredentialId,
};
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::domain::token::TokenGenError;
use crate::ports::backend::{
    BackendAuthView, BackendStore, BackendStoreError, CreateBindingOutcome as StoreCreateOutcome,
    IssueCredentialOutcome as StoreIssueOutcome, NewBinding, NewCredential,
};
use crate::ports::storage::StorageError;

/// 创建结果：首次创建与幂等重放返回相同的响应模型，以
/// `idempotency_replayed` 区分（HTTP 201 / 200）。
#[derive(Debug)]
pub enum CreateBindingOutcome {
    Created { binding: BackendBinding },
    Replayed { binding: BackendBinding },
}

#[derive(Debug)]
pub enum CreateBindingError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// backend label 不合法。
    Label(BackendLabelError),
    /// Agent 未知、格式错误或当前 Human 从未拥有（统一 404）。
    AgentNotFound,
    /// Agent 或其 Owner 关系已 revoked（409 agent_not_active）。
    AgentNotActive,
    /// Agent 已有 active Binding（409 backend_binding_exists）。
    BindingExists,
    /// 同 Human + 同 key 已被不同请求占用（409 idempotency_conflict）。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for CreateBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::Label(e) => write!(f, "{e}"),
            Self::AgentNotFound => write!(f, "agent not found"),
            Self::AgentNotActive => write!(f, "agent or owner relationship is revoked"),
            Self::BindingExists => write!(f, "agent already has an active backend binding"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "backend store failure: {e}"),
        }
    }
}

impl std::error::Error for CreateBindingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn canonical_uuid_v4(raw: &str) -> Option<String> {
    let uuid = Uuid::parse_str(raw).ok()?;
    (uuid.get_version() == Some(uuid::Version::Random)).then(|| uuid.hyphenated().to_string())
}

/// 创建 Binding：校验 key 与 label → 生成稳定 ID → 原子持久化 Binding +
/// 幂等协调列；同 Human + 同 key + 同规范化 Agent + 同 label 返回既有事实。
pub async fn create_binding(
    store: &dyn BackendStore,
    human: &Participant,
    agent_id_raw: &str,
    label_raw: &str,
    idempotency_key_raw: &str,
) -> Result<CreateBindingOutcome, CreateBindingError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| CreateBindingError::InvalidIdempotencyKey)?;
    let label = BackendLabel::parse(label_raw).map_err(CreateBindingError::Label)?;
    let agent_id =
        ParticipantId::parse_canonical(agent_id_raw).ok_or(CreateBindingError::AgentNotFound)?;
    let created_at = UtcMillis::now();
    let binding = NewBinding {
        id: BindingId(Uuid::new_v4().to_string()),
        agent_id,
        created_by_human_id: human.id.clone(),
        label,
        idempotency_key: key,
        created_at,
    };

    match store.create_binding(&human.id, &binding).await {
        Ok(StoreCreateOutcome::Created { binding }) => {
            Ok(CreateBindingOutcome::Created { binding })
        }
        Ok(StoreCreateOutcome::Replayed { binding }) => {
            Ok(CreateBindingOutcome::Replayed { binding })
        }
        Err(e) => Err(map_create_error(e)),
    }
}

fn map_create_error(e: BackendStoreError) -> CreateBindingError {
    match e {
        BackendStoreError::AgentNotFound => CreateBindingError::AgentNotFound,
        BackendStoreError::AgentNotActive => CreateBindingError::AgentNotActive,
        BackendStoreError::BindingExists => CreateBindingError::BindingExists,
        BackendStoreError::IdempotencyConflict => CreateBindingError::IdempotencyConflict,
        BackendStoreError::Database(e) => CreateBindingError::Store(e),
        other => CreateBindingError::Store(unexpected_store_error("backend store error", other)),
    }
}

/// 查询类失败：Agent unknown/bad/not-owned 统一 AgentNotFound（404
/// agent_not_found）；只有确认 Agent 曾属于当前 Human 后，才区分
/// BindingNotFound（404 backend_binding_not_found）。
#[derive(Debug)]
pub enum BackendQueryError {
    AgentNotFound,
    BindingNotFound,
    Store(StorageError),
}

impl std::fmt::Display for BackendQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentNotFound => write!(f, "agent not found"),
            Self::BindingNotFound => write!(f, "backend binding not found"),
            Self::Store(e) => write!(f, "backend store failure: {e}"),
        }
    }
}

impl std::error::Error for BackendQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_query_error(e: BackendStoreError) -> BackendQueryError {
    match e {
        BackendStoreError::AgentNotFound => BackendQueryError::AgentNotFound,
        BackendStoreError::BindingNotFound => BackendQueryError::BindingNotFound,
        BackendStoreError::Database(e) => BackendQueryError::Store(e),
        other => BackendQueryError::Store(unexpected_store_error("backend store error", other)),
    }
}

fn parse_agent_or_agent_not_found(raw: &str) -> Result<ParticipantId, BackendQueryError> {
    ParticipantId::parse_canonical(raw).ok_or(BackendQueryError::AgentNotFound)
}

/// 列出当前 Human 曾经授权给该 Agent 的全部 Binding（active + revoked），
/// 已按创建时间升序、再按 Binding ID 排序。
pub async fn list_bindings(
    store: &dyn BackendStore,
    human_id: &ParticipantId,
    agent_id_raw: &str,
) -> Result<Vec<BackendBinding>, BackendQueryError> {
    let agent_id = parse_agent_or_agent_not_found(agent_id_raw)?;
    store
        .list_bindings(human_id, &agent_id)
        .await
        .map_err(map_query_error)
}

/// 读取当前 Human 曾拥有的 Agent 下的单个 Binding 管理视图。
pub async fn get_binding(
    store: &dyn BackendStore,
    human_id: &ParticipantId,
    agent_id_raw: &str,
    binding_id_raw: &str,
) -> Result<BackendBinding, BackendQueryError> {
    let agent_id = parse_agent_or_agent_not_found(agent_id_raw)?;
    let binding_id = canonical_uuid_v4(binding_id_raw)
        .map(BindingId)
        .ok_or(BackendQueryError::BindingNotFound)?;
    store
        .get_binding(human_id, &agent_id, binding_id.as_str())
        .await
        .map_err(map_query_error)
}

/// 幂等软撤销 Binding：同一事务内先撤销其 active Credential；重复撤销
/// 返回成功且不改原 revoked_at。
pub async fn revoke_binding(
    store: &dyn BackendStore,
    human_id: &ParticipantId,
    agent_id_raw: &str,
    binding_id_raw: &str,
) -> Result<(), BackendQueryError> {
    let agent_id = parse_agent_or_agent_not_found(agent_id_raw)?;
    let binding_id = canonical_uuid_v4(binding_id_raw)
        .map(BindingId)
        .ok_or(BackendQueryError::BindingNotFound)?;
    store
        .revoke_binding(human_id, &agent_id, binding_id.as_str())
        .await
        .map_err(map_query_error)
}

/// 签发结果：首次成功签发才携带原始 token；幂等重放只返回历史元数据。
#[derive(Debug)]
pub enum IssueCredentialOutcome {
    Created {
        credential: BackendCredential,
        token: BackendToken,
    },
    Replayed {
        credential: BackendCredential,
    },
}

#[derive(Debug)]
pub enum IssueCredentialError {
    /// key 格式不合法。
    InvalidIdempotencyKey,
    /// 非 null replaces_credential_id 不是规范化 UUID v4。
    InvalidReplacesId,
    /// Agent 未知、格式错误或当前 Human 从未拥有（统一 404）。
    AgentNotFound,
    /// Agent 或其 Owner 关系已 revoked（409 agent_not_active）。
    AgentNotActive,
    /// owned Agent 下 unknown/bad/not-under-agent Binding（404）。
    BindingNotFound,
    /// 首次签发/轮换声明与当前 active Credential 不一致（409），无副作用。
    CredentialStateConflict,
    /// 同 Human + 同 key 已被不同请求占用（409 idempotency_conflict）。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for IssueCredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::InvalidReplacesId => write!(f, "replaces_credential_id must be a UUID v4"),
            Self::AgentNotFound => write!(f, "agent not found"),
            Self::AgentNotActive => write!(f, "agent or owner relationship is revoked"),
            Self::BindingNotFound => write!(f, "backend binding not found"),
            Self::CredentialStateConflict => {
                write!(
                    f,
                    "credential issue or rotation does not match current state"
                )
            }
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "backend store failure: {e}"),
        }
    }
}

impl std::error::Error for IssueCredentialError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_issue_error(e: BackendStoreError) -> IssueCredentialError {
    match e {
        BackendStoreError::AgentNotFound => IssueCredentialError::AgentNotFound,
        BackendStoreError::AgentNotActive => IssueCredentialError::AgentNotActive,
        BackendStoreError::BindingNotFound => IssueCredentialError::BindingNotFound,
        BackendStoreError::CredentialStateConflict => IssueCredentialError::CredentialStateConflict,
        BackendStoreError::IdempotencyConflict => IssueCredentialError::IdempotencyConflict,
        BackendStoreError::Database(e) => IssueCredentialError::Store(e),
        other => IssueCredentialError::Store(unexpected_store_error("backend store error", other)),
    }
}

/// 首次签发或 CAS 轮换 Credential：校验 key 与 replaces ID → 生成稳定 ID、
/// 密码学安全 token 与其 SHA-256 摘要 → 交给存储层在同一事务内完成幂等
/// 裁决、状态 CAS 与旧 Credential 交接。原始 token 只在本用例返回一次。
pub async fn issue_credential(
    store: &dyn BackendStore,
    human: &Participant,
    agent_id_raw: &str,
    binding_id_raw: &str,
    replaces_credential_id_raw: Option<&str>,
    idempotency_key_raw: &str,
) -> Result<IssueCredentialOutcome, IssueCredentialError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| IssueCredentialError::InvalidIdempotencyKey)?;
    let agent_id =
        ParticipantId::parse_canonical(agent_id_raw).ok_or(IssueCredentialError::AgentNotFound)?;
    let binding_id = canonical_uuid_v4(binding_id_raw)
        .map(BindingId)
        .ok_or(IssueCredentialError::BindingNotFound)?;
    let replaces = match replaces_credential_id_raw {
        Some(raw) => Some(
            canonical_uuid_v4(raw)
                .map(CredentialId)
                .ok_or(IssueCredentialError::InvalidReplacesId)?,
        ),
        None => None,
    };

    let created_at = UtcMillis::now();
    let token = match BackendToken::generate() {
        Ok(token) => token,
        Err(TokenGenError(e)) => {
            return Err(IssueCredentialError::Store(StorageError::from_message(
                crate::ports::storage::StorageFailureKind::Unknown,
                e,
            )));
        }
    };
    let credential = NewCredential {
        id: CredentialId(Uuid::new_v4().to_string()),
        binding_id: binding_id.clone(),
        issued_by_human_id: human.id.clone(),
        token_hash: token.sha256_hex(),
        replaces_credential_id: replaces,
        idempotency_key: key,
        created_at,
    };

    match store
        .issue_credential(&human.id, &agent_id, binding_id.as_str(), &credential)
        .await
    {
        Ok(StoreIssueOutcome::Created { credential }) => {
            Ok(IssueCredentialOutcome::Created { credential, token })
        }
        Ok(StoreIssueOutcome::Replayed { credential }) => {
            Ok(IssueCredentialOutcome::Replayed { credential })
        }
        Err(e) => Err(map_issue_error(e)),
    }
}

/// Credential 撤销类失败：在查询失败之上增加 CredentialNotFound（404
/// backend_credential_not_found）。
#[derive(Debug)]
pub enum CredentialQueryError {
    AgentNotFound,
    BindingNotFound,
    CredentialNotFound,
    Store(StorageError),
}

impl std::fmt::Display for CredentialQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentNotFound => write!(f, "agent not found"),
            Self::BindingNotFound => write!(f, "backend binding not found"),
            Self::CredentialNotFound => write!(f, "backend credential not found"),
            Self::Store(e) => write!(f, "backend store failure: {e}"),
        }
    }
}

impl std::error::Error for CredentialQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_credential_query_error(e: BackendStoreError) -> CredentialQueryError {
    match e {
        BackendStoreError::AgentNotFound => CredentialQueryError::AgentNotFound,
        BackendStoreError::BindingNotFound => CredentialQueryError::BindingNotFound,
        BackendStoreError::CredentialNotFound => CredentialQueryError::CredentialNotFound,
        BackendStoreError::Database(e) => CredentialQueryError::Store(e),
        other => CredentialQueryError::Store(unexpected_store_error("backend store error", other)),
    }
}

/// 列出该 Binding 的 Credential 元数据历史（active + revoked），已按创建
/// 时间升序、再按 Credential ID 排序；不返回 token/hash/幂等键。
pub async fn list_credentials(
    store: &dyn BackendStore,
    human_id: &ParticipantId,
    agent_id_raw: &str,
    binding_id_raw: &str,
) -> Result<Vec<BackendCredential>, CredentialQueryError> {
    let agent_id =
        ParticipantId::parse_canonical(agent_id_raw).ok_or(CredentialQueryError::AgentNotFound)?;
    let binding_id = canonical_uuid_v4(binding_id_raw)
        .map(BindingId)
        .ok_or(CredentialQueryError::BindingNotFound)?;
    store
        .list_credentials(human_id, &agent_id, binding_id.as_str())
        .await
        .map_err(map_credential_query_error)
}

/// 幂等软撤销一个 Credential；不撤销 Binding 或 Agent；重复撤销返回成功
/// 且不改原 revoked_at。
pub async fn revoke_credential(
    store: &dyn BackendStore,
    human_id: &ParticipantId,
    agent_id_raw: &str,
    binding_id_raw: &str,
    credential_id_raw: &str,
) -> Result<(), CredentialQueryError> {
    let agent_id =
        ParticipantId::parse_canonical(agent_id_raw).ok_or(CredentialQueryError::AgentNotFound)?;
    let binding_id = canonical_uuid_v4(binding_id_raw)
        .map(BindingId)
        .ok_or(CredentialQueryError::BindingNotFound)?;
    let credential_id = canonical_uuid_v4(credential_id_raw)
        .map(CredentialId)
        .ok_or(CredentialQueryError::CredentialNotFound)?;
    store
        .revoke_credential(
            human_id,
            &agent_id,
            binding_id.as_str(),
            credential_id.as_str(),
        )
        .await
        .map_err(map_credential_query_error)
}

/// Backend 认证失败：格式错误、Human token、未知 token、revoked Credential /
/// Binding / Agent 统一为同一结果，不泄露存在性。
#[derive(Debug)]
pub enum AuthenticateBackendError {
    /// 候选 token 格式不合法或认证链任一 active 状态不成立。
    InvalidCredential,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for AuthenticateBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCredential => write!(f, "invalid backend credential"),
            Self::Store(e) => write!(f, "backend store failure: {e}"),
        }
    }
}

impl std::error::Error for AuthenticateBackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// Backend 认证：严格校验 token 格式 → SHA-256 摘要精确查找 → 存储层同时
/// 证明 Credential/Binding/Agent 全部 active（kind=agent）。
pub async fn authenticate_backend(
    store: &dyn BackendStore,
    candidate: &str,
) -> Result<BackendAuthView, AuthenticateBackendError> {
    let token = BackendToken::parse_candidate(candidate)
        .ok_or(AuthenticateBackendError::InvalidCredential)?;
    let hash = token.sha256_hex();
    store
        .authenticate(&hash)
        .await
        .map_err(|e| match e {
            BackendStoreError::Database(e) => AuthenticateBackendError::Store(e),
            other => AuthenticateBackendError::Store(unexpected_store_error(
                "backend store error",
                other,
            )),
        })?
        .ok_or(AuthenticateBackendError::InvalidCredential)
}
