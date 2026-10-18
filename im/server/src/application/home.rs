//! Home 身份、配对与 Session 管理用例。
//!
//! 用例层负责规则编排与事务边界（CORE_V1_IM_CONTRACT.md §3.1）：
//! - Home Identity 幂等初始化（guard 一次成功）、读取；
//! - Pairing Grant 签发（生成 ID/token/摘要，TTL 由配置给定 1..=60 分钟）、
//!   列表（本人未消费）、幂等撤销；
//! - 一次性兑换：只接受 Grant Bearer；对外一切不可兑换形态（格式错、
//!   未知、已用、过期、撤销）统一 `grant_invalid`；成功事务原子成立
//!   「建 Session + 1:1 Metadata + 标记 consumed」，失败回滚 grant 仍可
//!   重试；重放绝不新建/重发 Session；
//! - Session 列表/撤销：仅本 Human 自己的 Session，只返回非秘密元数据，
//!   永不返回 token/hash；独立撤销不影响其他 Session。

use uuid::Uuid;

use crate::application::unexpected_store_error;
use crate::domain::grant_token::GrantToken;
use crate::domain::home::{GrantId, GrantKind, Home, NewGrant};
use crate::domain::human_session::{SessionId, TokenHash};
use crate::domain::pairing_transaction::{TransactionId, TransactionView};
use crate::domain::participant::{DisplayName, DisplayNameError, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::domain::token::TokenGenError as SessionTokenGenError;
use crate::ports::home::{
    AbortTransactionCommand, CommitTransactionCommand, ExchangeOutcome, ExchangeRequest, HomeStore,
    NewHome, PrepareTransactionRequest, SessionListItem, StoreError,
};
use crate::ports::storage::StorageError;

/// Grant TTL 可配置范围（分钟）：1..=60（HOME_ACCESS.md §5.3）。
pub const GRANT_TTL_MINUTES_MIN: u32 = 1;
pub const GRANT_TTL_MINUTES_MAX: u32 = 60;

/// 解析并校验 Grant TTL 配置（分钟）：越界或非法返回 None。
pub fn parse_grant_ttl_minutes(raw: &str) -> Option<u32> {
    let value = raw.parse::<u32>().ok()?;
    (GRANT_TTL_MINUTES_MIN..=GRANT_TTL_MINUTES_MAX)
        .contains(&value)
        .then_some(value)
}

#[derive(Debug)]
pub enum InitHomeError {
    DisplayName(DisplayNameError),
    Store(StorageError),
}

impl std::fmt::Display for InitHomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DisplayName(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for InitHomeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 幂等初始化 Home Identity：guard 冲突视为已初始化成功（返回值只区分
/// 「本次创建」与「已存在」），绝不因重复初始化报错。
pub async fn init_home(
    store: &dyn HomeStore,
    display_name: Option<&str>,
) -> Result<bool, InitHomeError> {
    let display_name = match display_name {
        Some(raw) => Some(DisplayName::parse(raw).map_err(InitHomeError::DisplayName)?),
        None => None,
    };
    let home = NewHome {
        home_id: crate::domain::home::HomeId::new_v4(),
        display_name,
        created_at: UtcMillis::now(),
    };
    match store.init_home(&home).await {
        Ok(()) => Ok(true),
        Err(StoreError::AlreadyInitialized) => Ok(false),
        Err(StoreError::Database(e)) => Err(InitHomeError::Store(e)),
        Err(other) => Err(InitHomeError::Store(unexpected_store_error(
            "home store error",
            other,
        ))),
    }
}

#[derive(Debug)]
pub enum HomeError {
    Store(StorageError),
}

impl std::fmt::Display for HomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for HomeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
        }
    }
}

/// 读取 Home Identity；未初始化返回 None（对外不变量是：启动与 CLI 都
/// 幂等初始化，正常拓扑下恒存在）。
pub async fn get_home(store: &dyn HomeStore) -> Result<Option<Home>, HomeError> {
    store.get_home().await.map_err(|e| match e {
        StoreError::Database(e) => HomeError::Store(e),
        other => HomeError::Store(unexpected_store_error("home store error", other)),
    })
}

/// 签发成功结果：机器可解析输出所需的全部事实。
#[derive(Debug)]
pub struct IssuedGrant {
    pub grant: crate::domain::home::PairingGrant,
    pub token: GrantToken,
}

#[derive(Debug)]
pub enum IssueGrantError {
    UnknownHuman,
    HumanNotActive,
    DeviceLabel(DisplayNameError),
    Token(crate::domain::grant_token::TokenGenError),
    Store(StorageError),
}

impl std::fmt::Display for IssueGrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownHuman => write!(f, "no such human participant"),
            Self::HumanNotActive => write!(f, "human participant is not active"),
            Self::DeviceLabel(e) => write!(f, "{e}"),
            Self::Token(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for IssueGrantError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 邀请类 Grant 的 TTL（分钟）：72 小时。
///
/// 为什么不用设备配对那个 15 分钟：**设备配对时签发者就在机器旁边**，短窗口
/// 是安全收益；**邀请一个人时朋友不一定马上点开**，短窗口只会逼人反复重发。
/// 长窗口的风险由"一次性 + 可撤销 + `issued_by` 留痕"承担。
pub const INVITE_TTL_MINUTES: u32 = 72 * 60;

/// 签发 Pairing Grant：生成 Grant ID / 一次性 token / 摘要与时间窗
/// （issued_at 现在，expires_at = now + ttl），校验签发者后同事务持久化。
/// 原始 token 只在成功路径返回一次。
pub async fn issue_grant(
    store: &dyn HomeStore,
    human_id: &ParticipantId,
    device_label: Option<&str>,
    kind: GrantKind,
    ttl_minutes: u32,
) -> Result<IssuedGrant, IssueGrantError> {
    // 用途决定 TTL：设备配对是"签发者就在机器旁"（短窗口即可），
    // 邀请一个人是"朋友不一定马上点开"（明显更长，仍可撤销、仍一次性）。
    let ttl_minutes = match kind {
        GrantKind::SelfDevice => ttl_minutes,
        GrantKind::NewHuman => INVITE_TTL_MINUTES,
    };
    let device_label = match device_label {
        Some(raw) => Some(DisplayName::parse(raw).map_err(IssueGrantError::DeviceLabel)?),
        None => None,
    };
    let now = UtcMillis::now();
    let token = GrantToken::generate().map_err(IssueGrantError::Token)?;
    let grant = NewGrant {
        grant_id: GrantId::new_v4(),
        issued_by: human_id.clone(),
        device_label,
        kind,
        token_hash: TokenHash::from_token_str(token.as_str()),
        issued_at: now,
        expires_at: UtcMillis::from_millis(
            now.millis().saturating_add(ttl_minutes as i64 * 60_000),
        ),
    };
    store.issue_grant(&grant).await.map_err(|e| match e {
        StoreError::UnknownHuman => IssueGrantError::UnknownHuman,
        StoreError::HumanNotActive => IssueGrantError::HumanNotActive,
        StoreError::Database(e) => IssueGrantError::Store(e),
        other => IssueGrantError::Store(unexpected_store_error("home store error", other)),
    })?;
    let stored = crate::domain::home::PairingGrant {
        grant_id: grant.grant_id.clone(),
        issued_by: grant.issued_by.clone(),
        device_label: grant.device_label.clone(),
        kind,
        status: crate::domain::home::GrantStatus::Active,
        issued_at: grant.issued_at,
        expires_at: grant.expires_at,
        consumed_at: None,
        revoked_at: None,
    };
    Ok(IssuedGrant {
        grant: stored,
        token,
    })
}

#[derive(Debug)]
pub enum ListGrantsError {
    Store(StorageError),
}

impl std::fmt::Display for ListGrantsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for ListGrantsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
        }
    }
}

/// 列出当前 Human 签发的未消费 Grant（对外状态含派生 expired）。
pub async fn list_grants(
    store: &dyn HomeStore,
    human_id: &ParticipantId,
) -> Result<Vec<crate::domain::home::PairingGrant>, ListGrantsError> {
    store.list_grants(human_id).await.map_err(|e| match e {
        StoreError::Database(e) => ListGrantsError::Store(e),
        other => ListGrantsError::Store(unexpected_store_error("home store error", other)),
    })
}

#[derive(Debug)]
pub enum RevokeGrantError {
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for RevokeGrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such pairing grant for this human"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for RevokeGrantError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 撤销本人未使用的 Grant：幂等（重复撤销成功）。
pub async fn revoke_grant(
    store: &dyn HomeStore,
    human_id: &ParticipantId,
    grant_id: &GrantId,
) -> Result<(), RevokeGrantError> {
    store
        .revoke_grant(human_id, grant_id, UtcMillis::now())
        .await
        .map_err(|e| match e {
            StoreError::GrantNotFound | StoreError::GrantConsumed => RevokeGrantError::NotFound,
            StoreError::Database(e) => RevokeGrantError::Store(e),
            other => RevokeGrantError::Store(unexpected_store_error("home store error", other)),
        })
}

#[derive(Debug)]
pub enum ExchangeError {
    /// 请求体形状不合法（如 device_label 非法）：400 invalid_request。
    InvalidBody(DisplayNameError),
    /// token 格式不合法或兑换裁决失败：对外统一 `grant_invalid`。
    GrantInvalid,
    Token(SessionTokenGenError),
    Store(StorageError),
}

impl std::fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBody(e) => write!(f, "{e}"),
            Self::GrantInvalid => write!(f, "invalid pairing grant"),
            Self::Token(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for ExchangeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 兑换成功结果：ExchangeOutcome（无 token）+ 一次性输出的新 Session token。
#[derive(Debug)]
pub struct ExchangeResult {
    pub outcome: ExchangeOutcome,
    pub session_token: crate::domain::token::OpaqueToken,
}

/// 一次性兑换（匿名端点，只接受 Grant Bearer）：严格校验 Grant token
/// 格式后按摘要原子兑换。所有不可兑换形态（格式错、未知、已用、过期、
/// 撤销）在应用层合并为 `GrantInvalid`，不泄露 Grant 是否曾经存在。
/// 成功后重放统一 `GrantInvalid`，绝不新建或重发 Session；兑换产出的
/// 是全新独立 Human Session（自己的随机 token），不是 Grant token 复用。
pub async fn exchange_grant(
    store: &dyn HomeStore,
    candidate: &str,
    device_label: Option<&str>,
    new_human_display_name: Option<&str>,
) -> Result<ExchangeResult, ExchangeError> {
    let _grant_token = GrantToken::parse_candidate(candidate).ok_or(ExchangeError::GrantInvalid)?;
    let device_label = match device_label {
        Some(raw) => Some(DisplayName::parse(raw).map_err(ExchangeError::InvalidBody)?),
        None => None,
    };
    // 被邀请者自己命名自己：名字由**兑换者**给，不由签发者代填。
    let new_human_display_name = match new_human_display_name {
        Some(raw) => Some(DisplayName::parse(raw).map_err(ExchangeError::InvalidBody)?),
        None => None,
    };
    let now = UtcMillis::now();
    let session_token =
        crate::domain::token::OpaqueToken::generate().map_err(ExchangeError::Token)?;
    let exchange = ExchangeRequest {
        grant_token_hash: TokenHash::from_token_str(candidate),
        session_token_hash: TokenHash::from_token(&session_token),
        session_id: SessionId(Uuid::new_v4().to_string()),
        device_label,
        new_human_display_name,
        created_at: now,
    };
    let outcome = store
        .exchange_grant(&exchange, now)
        .await
        .map_err(|e| match e {
            StoreError::GrantInvalid => ExchangeError::GrantInvalid,
            // 用途与名字对不上：**码是有效的**，只是这次请求不完整/多余——
            // 如实报 400，且 Grant 未被消费（补对即可重试）。
            // 名字缺失/多余都不是"码不可用"：走同一个 400 体面错误。
            StoreError::NewHumanNameRequired | StoreError::UnexpectedNewHumanName => {
                ExchangeError::InvalidBody(
                    crate::domain::participant::DisplayNameError::NoNonWhitespace,
                )
            }
            StoreError::Database(e) => ExchangeError::Store(e),
            other => ExchangeError::Store(unexpected_store_error("home store error", other)),
        })?;
    Ok(ExchangeResult {
        outcome,
        session_token,
    })
}

#[derive(Debug)]
pub enum ListSessionsError {
    Store(StorageError),
}

impl std::fmt::Display for ListSessionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for ListSessionsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
        }
    }
}

/// 列出当前 Human 的 Session（含已撤销）非秘密元数据。
pub async fn list_sessions(
    store: &dyn HomeStore,
    human_id: &ParticipantId,
) -> Result<Vec<SessionListItem>, ListSessionsError> {
    store.list_sessions(human_id).await.map_err(|e| match e {
        StoreError::Database(e) => ListSessionsError::Store(e),
        other => ListSessionsError::Store(unexpected_store_error("home store error", other)),
    })
}

#[derive(Debug)]
pub enum RevokeOwnedSessionError {
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for RevokeOwnedSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such session for this human"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for RevokeOwnedSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 撤销当前 Human 自己的 Session：幂等；未知 / 非本人统一 NotFound。
pub async fn revoke_owned_session(
    store: &dyn HomeStore,
    human_id: &ParticipantId,
    session_id: &SessionId,
) -> Result<(), RevokeOwnedSessionError> {
    store
        .revoke_owned_session(human_id, session_id, UtcMillis::now())
        .await
        .map_err(|e| match e {
            StoreError::SessionNotFound => RevokeOwnedSessionError::NotFound,
            StoreError::Database(e) => RevokeOwnedSessionError::Store(e),
            other => {
                RevokeOwnedSessionError::Store(unexpected_store_error("home store error", other))
            }
        })
}

// ===== A2 Pairing Transaction（HOME_ACCESS.md §4.3 A2）=====

/// A2 事务参数校验失败：对外统一 `pairing_transaction_invalid`，不泄露
/// 具体差异。
#[derive(Debug)]
pub struct TransactionInvalidKind;

impl std::fmt::Display for TransactionInvalidKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid pairing transaction")
    }
}

impl std::error::Error for TransactionInvalidKind {}

/// prepare 参数校验失败（摘要格式 / nonce / transaction id / 预留 session
/// id）。路由把它映射为匿名 `pairing_transaction_invalid`（400），而不是
/// `grant_invalid`（401，保留给 Grant Bearer 本身不可用的情形）。
#[derive(Debug)]
pub enum PrepareBodyError {
    Token,
    InvalidBody,
}

impl std::fmt::Display for PrepareBodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token => write!(f, "invalid digest format"),
            Self::InvalidBody => write!(f, "invalid request body"),
        }
    }
}

impl std::error::Error for PrepareBodyError {}

/// PrepareTransactionError：Grant Bearer 不可用或事务参数非法。
#[derive(Debug)]
pub enum PrepareTransactionError {
    InvalidBody(PrepareBodyError),
    GrantInvalid,
    TransactionInvalid,
    Store(StorageError),
}

impl std::fmt::Display for PrepareTransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBody(e) => write!(f, "{e}"),
            Self::GrantInvalid => write!(f, "invalid pairing grant"),
            Self::TransactionInvalid => write!(f, "invalid pairing transaction"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for PrepareTransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// A2 prepare 校验 helper：对 Host 生成的固定长度摘要、（可选）非空
/// transaction id / 预留 session id 与 nonce 做形状校验，随后构造可幂等
/// 重试的准备请求。Core 绝不接收原始 Session / commit token（只接收
/// SHA-256 hex 摘要）。
pub fn build_prepare_request(
    nonce: &str,
    session_token_hash_raw: &str,
    commit_token_hash_raw: &str,
) -> Result<PrepareTransactionRequest, PrepareTransactionError> {
    let session_token_hash = parse_sha256_hex(session_token_hash_raw).ok_or(
        PrepareTransactionError::InvalidBody(PrepareBodyError::Token),
    )?;
    let commit_token_hash = parse_sha256_hex(commit_token_hash_raw).ok_or(
        PrepareTransactionError::InvalidBody(PrepareBodyError::Token),
    )?;
    if nonce.is_empty() || nonce.len() > PREPARE_NONCE_MAX_BYTES || nonce.contains('\u{0}') {
        return Err(PrepareTransactionError::InvalidBody(
            PrepareBodyError::InvalidBody,
        ));
    }
    let now = UtcMillis::now();
    Ok(PrepareTransactionRequest {
        // 幂等键的一部分与 reserved 都由 Core 在事务内生成并固定；重试
        // 使用同一 nonce + 摘要，恒落在同一事务。
        grant_token_hash: TokenHash([0u8; 32]), // 占位，由 prepare 用例按授权头替换
        client_nonce: nonce.to_owned(),
        session_token_hash,
        commit_token_hash,
        transaction_id: TransactionId::new_v4(),
        reserved_session_id: SessionId(uuid::Uuid::new_v4().to_string()),
        prepared_at: now,
        expires_at: UtcMillis::from_millis(now.millis().saturating_add(TRANSACTION_TTL_MS_DEFAULT)),
    })
}

/// 事务 TTL：与 Grant TTL 同源语义，默认 15 分钟（应用层固定值；
/// HOME_ACCESS.md §5.3 Grant 默认，事务沿用同一短窗口，不新增配置）。
const TRANSACTION_TTL_MS_DEFAULT: i64 = 15 * 60_000;

/// nonce 上限（应用层校验；数据库 CHECK 同步兜底 1..=128 字符）。
pub const PREPARE_NONCE_MAX_BYTES: usize = 128;

/// 解析 64 字符小写 SHA-256 hex 摘要；格式不合法返回 None（对外统一
/// `pairing_transaction_invalid`，不泄露解析细节）。
pub fn parse_sha256_hex(raw: &str) -> Option<TokenHash> {
    let bytes = hex::decode(raw).ok()?;
    let array: [u8; 32] = bytes.try_into().ok()?;
    Some(TokenHash(array))
}

/// Host 预生成的前缀（与既有 Human Session / Grant token 同卫生标准）。
/// 仅测试与未来 Host 复用；Core 本身只信任摘要。
#[allow(dead_code)]
pub const SESSION_TOKEN_PREFIX: &str = "clr_pt_";

/// prepare：匿名端点（只接受 Grant Bearer + 摘要）。路由与用例的分工：
/// - 路由先做 Bearer 解析与格式校验（`grant_invalid` / 401）；
/// - 本用例把摘要写入事务并生成 transaction_id / reserved_session_id /
///   expires_at（Core 侧事实），再调用 store 的 `prepare_transaction`
///   （原子幂等）。
///
/// 同一 nonce + 摘要重试恒返回同一准备事务；prepare **不创建正式 Human
/// Session、不消费 Grant**。原始 Session / commit token 从不进入 Core。
pub async fn prepare_transaction(
    store: &dyn HomeStore,
    grant_candidate: &str,
    nonce: &str,
    session_token_hash_raw: &str,
    commit_token_hash_raw: &str,
) -> Result<TransactionView, PrepareTransactionError> {
    let grant_token = GrantToken::parse_candidate(grant_candidate)
        .ok_or(PrepareTransactionError::GrantInvalid)?;
    let mut req = build_prepare_request(nonce, session_token_hash_raw, commit_token_hash_raw)?;
    req.grant_token_hash = TokenHash::from_token_str(grant_token.as_str());
    store
        .prepare_transaction(&req, UtcMillis::now())
        .await
        .map_err(|e| match e {
            StoreError::GrantInvalid => PrepareTransactionError::GrantInvalid,
            StoreError::TransactionNotFound => PrepareTransactionError::TransactionInvalid,
            StoreError::Database(e) => PrepareTransactionError::Store(e),
            other => {
                PrepareTransactionError::Store(unexpected_store_error("home store error", other))
            }
        })
}

#[derive(Debug)]
pub enum TransactionStatusError {
    /// 未知 / token 不匹配 / 过期：对外统一 `pairing_transaction_invalid`。
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for TransactionStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such pairing transaction"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for TransactionStatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// status：凭 commit token 摘要查询非秘密事务状态（Host 恢复路径）。
/// 未知、错误 token、其他 Human 或过期事务统一 `NotFound`
/// （`pairing_transaction_invalid`），不提供存在性探测。
pub async fn get_transaction_status(
    store: &dyn HomeStore,
    transaction_id: &TransactionId,
    commit_token_hash: &TokenHash,
) -> Result<Option<TransactionView>, TransactionStatusError> {
    store
        .get_transaction_status(transaction_id, commit_token_hash, UtcMillis::now())
        .await
        .map_err(|e| match e {
            StoreError::TransactionNotFound => TransactionStatusError::NotFound,
            StoreError::Database(e) => TransactionStatusError::Store(e),
            other => {
                TransactionStatusError::Store(unexpected_store_error("home store error", other))
            }
        })
}

/// status/commit/abort 的 commit token 授权解析：Host 呈现的是**原始 commit
/// token**（高熵、非摘要），Core 在此计算 SHA-256 摘要用于寻址；原始 token
/// 绝不进入日志或响应（格式合法即接受任意非空 token，存在性由 store 裁决）。
pub fn commit_token_hash_of(candidate: &str) -> Result<TokenHash, TransactionInvalidKind> {
    if candidate.is_empty() {
        return Err(TransactionInvalidKind);
    }
    Ok(TokenHash::from_token_str(candidate))
}

#[derive(Debug)]
pub enum CommitTransactionError {
    InvalidBody(DisplayNameError),
    /// 未知 / token 不匹配 / 过期 / aborted：统一 `pairing_transaction_invalid`。
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for CommitTransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBody(e) => write!(f, "{e}"),
            Self::NotFound => write!(f, "no such pairing transaction"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for CommitTransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// commit：凭 commit token 摘要，在一个事务内完成「裁决 prepared 未过期
/// 事务 → 以 prepare 阶段提交的 session_token_hash 创建正式 Human Session
/// → 写入 session_metadata → 标记事务 committed → 消费 Pairing Grant」。
/// 首次成功与重复提交返回同一非秘密结果，绝不产生第二 Session；aborted /
/// 过期 / unknown / token 不匹配统一 `pairing_transaction_invalid`。
pub async fn commit_transaction(
    store: &dyn HomeStore,
    transaction_id: &TransactionId,
    commit_token_candidate: &str,
    device_label: Option<&str>,
) -> Result<TransactionView, CommitTransactionError> {
    let commit_token_hash = commit_token_hash_of(commit_token_candidate)
        .map_err(|_| CommitTransactionError::NotFound)?;
    let device_label = match device_label {
        Some(raw) => Some(DisplayName::parse(raw).map_err(CommitTransactionError::InvalidBody)?),
        None => None,
    };
    store
        .commit_transaction(
            &CommitTransactionCommand {
                transaction_id: transaction_id.clone(),
                commit_token_hash,
                device_label,
                committed_at: UtcMillis::now(),
            },
            UtcMillis::now(),
        )
        .await
        .map_err(|e| match e {
            StoreError::TransactionNotFound | StoreError::GrantInvalid => {
                CommitTransactionError::NotFound
            }
            StoreError::Database(e) => CommitTransactionError::Store(e),
            other => {
                CommitTransactionError::Store(unexpected_store_error("home store error", other))
            }
        })
}

#[derive(Debug)]
pub enum AbortTransactionError {
    /// 未知 / token 不匹配 / 过期 / 已 committed / 已 aborted：统一
    /// `pairing_transaction_invalid`。
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for AbortTransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such pairing transaction"),
            Self::Store(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for AbortTransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// abort：凭 commit token 摘要主动放弃 prepared 事务（幂等）。只影响
/// prepared；不影响 Grant 的 active 状态（Grant 仍可重新 prepare）。
/// unknown / token 不匹配 / 过期 / 已 committed / 已 aborted 统一
/// `pairing_transaction_invalid`。
pub async fn abort_transaction(
    store: &dyn HomeStore,
    transaction_id: &TransactionId,
    commit_token_candidate: &str,
) -> Result<(), AbortTransactionError> {
    let commit_token_hash = commit_token_hash_of(commit_token_candidate)
        .map_err(|_| AbortTransactionError::NotFound)?;
    store
        .abort_transaction(
            &AbortTransactionCommand {
                transaction_id: transaction_id.clone(),
                commit_token_hash,
                aborted_at: UtcMillis::now(),
            },
            UtcMillis::now(),
        )
        .await
        .map_err(|e| match e {
            StoreError::TransactionNotFound => AbortTransactionError::NotFound,
            StoreError::Database(e) => AbortTransactionError::Store(e),
            other => {
                AbortTransactionError::Store(unexpected_store_error("home store error", other))
            }
        })
}
