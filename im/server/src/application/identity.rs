//! 身份用例：bootstrap、补发 Session、认证、撤销。
//!
//! 用例层负责规则编排与事务边界：生成 ID、token 与摘要，再把已就绪的
//! 事实交给 `ports::identity::IdentityStore` 一次性持久化。

use uuid::Uuid;

use crate::application::unexpected_store_error;
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::{DisplayName, DisplayNameError, Participant, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::domain::token::{OpaqueToken, TokenGenError};
use crate::ports::identity::{AuthContext, IdentityStore, NewHuman, NewSession, StoreError};
use crate::ports::storage::StorageError;

/// bootstrap-human 成功结果：机器可解析输出所需的全部事实。
#[derive(Debug)]
pub struct BootstrapOutcome {
    pub participant: Participant,
    pub session: HumanSession,
    pub token: OpaqueToken,
}

#[derive(Debug)]
pub enum BootstrapError {
    DisplayName(DisplayNameError),
    AlreadyBootstrapped,
    Token(TokenGenError),
    Store(StorageError),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DisplayName(e) => write!(f, "{e}"),
            Self::AlreadyBootstrapped => write!(f, "first human already bootstrapped"),
            Self::Token(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "identity store failure: {e}"),
        }
    }
}

impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// create-human 失败：与 bootstrap 共用同一 Human 创建与 Session 签发
/// 用例（`provision_human`），只是不占用 bootstrap guard。
#[derive(Debug)]
pub enum CreateHumanError {
    DisplayName(DisplayNameError),
    Token(TokenGenError),
    Store(StorageError),
}

impl std::fmt::Display for CreateHumanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DisplayName(e) => write!(f, "{e}"),
            Self::Token(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "identity store failure: {e}"),
        }
    }
}

impl std::error::Error for CreateHumanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 共用用例：校验显示名 → 生成 Human ID / Session ID / 一次性 token /
/// 摘要 / 创建时间。bootstrap-human 与 create-human 都只通过本函数生成
/// 身份事实，保证两者创建与 Session 签发语义完全一致。
fn provision_human(
    display_name: &str,
) -> Result<(Participant, HumanSession, OpaqueToken), ProvisionError> {
    let display_name = DisplayName::parse(display_name).map_err(ProvisionError::DisplayName)?;
    let created_at = UtcMillis::now();
    let participant = Participant::new_human(
        ParticipantId(Uuid::new_v4().to_string()),
        display_name,
        created_at,
    );
    let token = OpaqueToken::generate().map_err(ProvisionError::Token)?;
    let session = HumanSession::new(
        SessionId(Uuid::new_v4().to_string()),
        participant.id.clone(),
        TokenHash::from_token(&token),
        created_at,
    );
    Ok((participant, session, token))
}

/// provision_human 的共享失败类型（两处对外错误类型各自收敛）。
enum ProvisionError {
    DisplayName(DisplayNameError),
    Token(TokenGenError),
}

/// 首次 bootstrap：校验显示名 → 生成 ID/ID token/摘要 → 与 guard 同事务持久化。
pub async fn bootstrap_human(
    store: &dyn IdentityStore,
    display_name: &str,
) -> Result<BootstrapOutcome, BootstrapError> {
    let (participant, session, token) = provision_human(display_name).map_err(|e| match e {
        ProvisionError::DisplayName(e) => BootstrapError::DisplayName(e),
        ProvisionError::Token(e) => BootstrapError::Token(e),
    })?;

    store
        .bootstrap_first_human(
            &NewHuman {
                id: participant.id.clone(),
                display_name: participant.display_name.clone(),
                created_at: participant.created_at,
            },
            &NewSession {
                id: session.id.clone(),
                human_id: session.human_id.clone(),
                token_hash: session.token_hash,
                created_at: session.created_at,
            },
        )
        .await
        .map_err(|e| match e {
            StoreError::AlreadyBootstrapped => BootstrapError::AlreadyBootstrapped,
            StoreError::Database(e) => BootstrapError::Store(e),
            other => BootstrapError::Store(unexpected_store_error("identity store error", other)),
        })?;

    Ok(BootstrapOutcome {
        participant,
        session,
        token,
    })
}

/// `celira-im create-human --display-name <NAME>`：创建新的 active Human
/// 并签发一次性 Session；与 bootstrap 共用 `provision_human` 创建与 Session
/// 签发用例，但不占用 bootstrap guard，可重复为多个 Human 调用。
pub async fn create_human(
    store: &dyn IdentityStore,
    display_name: &str,
) -> Result<BootstrapOutcome, CreateHumanError> {
    let (participant, session, token) = provision_human(display_name).map_err(|e| match e {
        ProvisionError::DisplayName(e) => CreateHumanError::DisplayName(e),
        ProvisionError::Token(e) => CreateHumanError::Token(e),
    })?;

    store
        .create_human(
            &NewHuman {
                id: participant.id.clone(),
                display_name: participant.display_name.clone(),
                created_at: participant.created_at,
            },
            &NewSession {
                id: session.id.clone(),
                human_id: session.human_id.clone(),
                token_hash: session.token_hash,
                created_at: session.created_at,
            },
        )
        .await
        .map_err(|e| match e {
            StoreError::Database(e) => CreateHumanError::Store(e),
            other => CreateHumanError::Store(unexpected_store_error("identity store error", other)),
        })?;

    Ok(BootstrapOutcome {
        participant,
        session,
        token,
    })
}

/// issue-human-session 成功结果。
#[derive(Debug)]
pub struct IssuedSession {
    pub session: HumanSession,
    pub token: OpaqueToken,
}

#[derive(Debug)]
pub enum IssueSessionError {
    UnknownHuman,
    HumanNotActive,
    Token(TokenGenError),
    Store(StorageError),
}

impl std::fmt::Display for IssueSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownHuman => write!(f, "no such human participant"),
            Self::HumanNotActive => write!(f, "human participant is not active"),
            Self::Token(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "identity store failure: {e}"),
        }
    }
}

impl std::error::Error for IssueSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 补发 Session：只对存在且 active 的 Human 创建新 Session；不撤销其他
/// Session，不改变 Human 公共身份。
pub async fn issue_human_session(
    store: &dyn IdentityStore,
    human_id: &str,
) -> Result<IssuedSession, IssueSessionError> {
    let created_at = UtcMillis::now();
    let token = OpaqueToken::generate().map_err(IssueSessionError::Token)?;
    let session = HumanSession::new(
        SessionId(Uuid::new_v4().to_string()),
        ParticipantId(human_id.to_owned()),
        TokenHash::from_token(&token),
        created_at,
    );

    store
        .issue_human_session(&NewSession {
            id: session.id.clone(),
            human_id: session.human_id.clone(),
            token_hash: session.token_hash,
            created_at,
        })
        .await
        .map_err(|e| match e {
            StoreError::UnknownHuman => IssueSessionError::UnknownHuman,
            StoreError::HumanNotActive => IssueSessionError::HumanNotActive,
            StoreError::Database(e) => IssueSessionError::Store(e),
            other => {
                IssueSessionError::Store(unexpected_store_error("identity store error", other))
            }
        })?;

    Ok(IssuedSession { session, token })
}

#[derive(Debug)]
pub enum AuthenticateError {
    /// 候选 token 格式不合法。
    InvalidToken,
    /// 未知、已撤销或指向非 active Human 的 token。
    UnknownSession,
    Store(StorageError),
}

impl std::fmt::Display for AuthenticateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidToken => write!(f, "invalid token format"),
            Self::UnknownSession => write!(f, "unknown or revoked session"),
            Self::Store(e) => write!(f, "identity store failure: {e}"),
        }
    }
}

impl std::error::Error for AuthenticateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 认证：严格校验格式后按摘要精确查找。所有「不存在的 Session」形态
/// （未知 token、已撤销、指向非 active Human）在应用层合并为一个结果，
/// 不泄露 Session 是否曾经存在。
pub async fn authenticate(
    store: &dyn IdentityStore,
    candidate: &str,
) -> Result<AuthContext, AuthenticateError> {
    let token = OpaqueToken::parse_candidate(candidate).ok_or(AuthenticateError::InvalidToken)?;
    let hash = TokenHash::from_token(&token);
    store
        .authenticate(&hash)
        .await
        .map_err(|e| match e {
            StoreError::Database(e) => AuthenticateError::Store(e),
            other => {
                AuthenticateError::Store(unexpected_store_error("identity store error", other))
            }
        })?
        .ok_or(AuthenticateError::UnknownSession)
}

#[derive(Debug)]
pub enum RevokeError {
    Store(StorageError),
}

impl std::fmt::Display for RevokeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "identity store failure: {e}"),
        }
    }
}

impl std::error::Error for RevokeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
        }
    }
}

/// 撤销当前 Session：持久化后才返回成功；不删除 Human，不影响其他 Session。
pub async fn revoke_session(
    store: &dyn IdentityStore,
    session_id: &SessionId,
) -> Result<(), RevokeError> {
    store.revoke_session(session_id).await.map_err(|e| match e {
        StoreError::Database(e) => RevokeError::Store(e),
        other => RevokeError::Store(unexpected_store_error("identity store error", other)),
    })
}

/// 改自己名字时的失败分类（表达权第四刀，2026-10-17）。
///
/// **名字不合法**与**存储失败**分开：前者是调用方的事（400），后者是系统的事（500）。
#[derive(Debug)]
pub enum RenameError {
    /// 名字不合法：禁 NUL、≤80 个 Unicode scalar value、trim 后非空（与兑换时同一套）。
    InvalidName(DisplayNameError),
    /// 目标不是存在且 active 的 Human（自己写自己时只可能是"我已被撤销"这一种）。
    NotActiveHuman,
    /// 存储失败。
    Store(StorageError),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(e) => write!(f, "invalid display name: {e}"),
            Self::NotActiveHuman => write!(f, "human participant is not active"),
            Self::Store(e) => write!(f, "identity store failure: {e}"),
        }
    }
}

impl std::error::Error for RenameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidName(e) => Some(e),
            Self::NotActiveHuman => None,
            Self::Store(e) => Some(e),
        }
    }
}

/// **改自己的名字**（表达权第四刀，2026-10-17 主人批准）。
///
/// 为什么是**状态写**：`display_name` 是既有 Participant 身份事实（不在呈现版本链上 ——
/// 那条链明文"不复制 `display_name`/`status`/`kind`"）⇒ 没有版本行、没有幂等记录，
/// **同值重写天然幂等**（照 `PUT /v1/backend/rooms/{id}/read-cursor` 的先例）。
///
/// 校验在这里（用例拥有规则）：`DisplayName::parse` 与兑换时**同一套** ——
/// 禁 NUL、≤80 个 Unicode scalar value、trim 后非空。表上也有同样三条 CHECK（第二道闸）。
pub async fn rename_human(
    store: &dyn IdentityStore,
    human_id: &ParticipantId,
    raw_display_name: &str,
) -> Result<DisplayName, RenameError> {
    let display_name = DisplayName::parse(raw_display_name).map_err(RenameError::InvalidName)?;
    store
        .rename_human(human_id, &display_name)
        .await
        .map_err(|e| match e {
            StoreError::HumanNotActive | StoreError::UnknownHuman => RenameError::NotActiveHuman,
            StoreError::Database(e) => RenameError::Store(e),
            other => RenameError::Store(unexpected_store_error("identity store error", other)),
        })?;
    Ok(display_name)
}
