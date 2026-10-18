//! Home、Pairing Grant、Session 管理与 A2 Pairing Transaction 窄端口。
//!
//! 本端口只声明 A 包与 A2 切片的持久化边界（HOME_ACCESS.md §4.3），
//! 不依赖具体数据库或 HTTP 框架。存储 adapter 实现本端口；应用用例只
//! 通过端口消费存储能力。

use crate::domain::home::{GrantId, Home, NewGrant, PairingGrant};
use crate::domain::human_session::{SessionId, TokenHash};
use crate::domain::pairing_transaction::{TransactionId, TransactionView};
use crate::domain::participant::{DisplayName, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::ports::storage::StorageError;

/// 待持久化的新 Home。
#[derive(Debug, Clone)]
pub struct NewHome {
    pub home_id: crate::domain::home::HomeId,
    pub display_name: Option<DisplayName>,
    pub created_at: UtcMillis,
}

/// 兑换请求（领域层已完成 Grant token 格式校验与摘要计算）。
#[derive(Debug, Clone)]
pub struct ExchangeRequest {
    /// 提交的 Pairing Grant token 摘要；存储层只用它查找待兑换 Grant。
    pub grant_token_hash: TokenHash,
    /// 兑换产出的**新 Human Session** 的摘要（由领域层用独立随机 token
    /// 生成）。会话归谁由 **Grant 的用途**决定：`SelfDevice` ⇒ grant 的
    /// `issued_by`（既有语义，调用方不能指定）；`NewHuman` ⇒ 本次兑换
    /// **新建的那个 Human**。
    pub session_token_hash: TokenHash,
    pub session_id: SessionId,
    pub device_label: Option<DisplayName>,
    /// **仅 `NewHuman` 用**：被邀请者自己给的名字（领域层已完成校验）。
    /// `SelfDevice` 必须为 None（HTTP 层与存储层都拒绝带名字的设备配对）。
    pub new_human_display_name: Option<DisplayName>,
    pub created_at: UtcMillis,
}

/// 兑换成功结果：新 Session（无 token/hash）与 Home Identity 同事务成立。
#[derive(Debug, Clone)]
pub struct ExchangeOutcome {
    pub session_id: SessionId,
    pub human_id: ParticipantId,
    /// 签发者（审计事实）：`SelfDevice` 时等于 `human_id`；`NewHuman` 时是
    /// **邀请者**——兑换后据此建那条"邀请者 → 新人"的待答关系申请。
    pub issued_by: ParticipantId,
    /// 本次兑换是否**新建了一个 Human**（`NewHuman` 用途）。
    pub invited_new_human: bool,
    pub session_created_at: UtcMillis,
    /// 消耗的 Grant（会话列表的 source_grant_id 与之对应；公开协议不
    /// 直接暴露，仅供存储层测试与日志关联）。
    #[allow(dead_code)]
    pub grant_id: GrantId,
    pub home: Home,
}

/// 会话列表项：非秘密元数据 + 可选 Session Metadata；绝不包含 token/hash。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListItem {
    pub session_id: SessionId,
    pub created_at: UtcMillis,
    pub revoked_at: Option<UtcMillis>,
    pub device_label: Option<DisplayName>,
    pub source_grant_id: Option<GrantId>,
}

/// 存储层错误。
#[derive(Debug)]
pub enum StoreError {
    /// home_guard 已被占用：Home Identity 已初始化（幂等初始化路径可
    /// 视为成功；只有「显式只成功一次」语义需要区分）。
    AlreadyInitialized,
    /// 目标 Participant 不存在。
    UnknownHuman,
    /// 目标 Participant 存在但不是 active Human。
    HumanNotActive,
    /// 目标 Grant 不存在、不属于当前 Human 或已消费：对外统一
    /// `grant_not_found`（不泄露存在性/状态）。
    GrantNotFound,
    /// 邀请类 Grant 被兑换时没给名字（`NewHuman` 必须由被邀请者自己命名）：
    /// 对外 400 `invalid_request`，**不消费 Grant**（补上名字即可重试）。
    ///
    /// ⚠ 这条会区分"码有效但没给名字"与"码不可用"——**只有已经持有有效码的
    /// 人**才可能看到它（码是 ≥256 bit 随机），因此不构成可利用的存在性
    /// oracle；反过来把"码有效"谎报成 `grant_invalid` 会让诚实的被邀请者
    /// 以为码坏了。两害相权，如实报。
    NewHumanNameRequired,
    /// 设备配对 Grant 被带上了名字（`SelfDevice` 不接受 display_name）：
    /// 对外 400 `invalid_request`，**不消费 Grant**。
    UnexpectedNewHumanName,
    /// Grant 存在但已消费（不可撤销、不可再兑换）：对外统一
    /// `grant_not_found`，内部保留供幂等裁决。
    GrantConsumed,
    /// 兑换/准备裁决失败：未知、格式、已消费、已撤销、已过期一律统一为
    /// `grant_invalid`（不形成存在性 oracle）。
    GrantInvalid,
    /// 目标 Session 不存在或不属于当前 Human：对外统一
    /// `session_not_found`。
    SessionNotFound,
    /// A2：目标 Transaction 不存在、commit_token 不匹配、非本事务 Human
    /// 或已过期：对外统一 `pairing_transaction_invalid`（不泄露存在性或
    /// 状态，也不提供存在性探测）。
    TransactionNotFound,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "home identity already initialized"),
            Self::UnknownHuman => write!(f, "no such human participant"),
            Self::HumanNotActive => write!(f, "human participant is not active"),
            Self::GrantNotFound => write!(f, "no such pairing grant for this human"),
            Self::GrantConsumed => write!(f, "pairing grant already consumed"),
            Self::GrantInvalid => write!(f, "invalid pairing grant"),
            Self::NewHumanNameRequired => {
                write!(f, "invite exchange requires the invitee's display name")
            }
            Self::UnexpectedNewHumanName => {
                write!(f, "device pairing grant must not carry a display name")
            }
            Self::SessionNotFound => write!(f, "no such session for this human"),
            Self::TransactionNotFound => write!(f, "no such pairing transaction"),
            Self::Database(e) => write!(f, "home store failure: {e}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// A2 prepare 准备请求（领域层已完成摘要格式解析；Grant token 摘要由
/// 路由从授权头计算，issued_by 由 store 从 grant 解析，调用方不能指定）。
#[derive(Debug, Clone)]
pub struct PrepareTransactionRequest {
    pub grant_token_hash: TokenHash,
    /// Host 生成的幂等 nonce（同一 nonce + 同一组摘要恒重放同一事务）。
    pub client_nonce: String,
    /// prepare 阶段提交的正式 Human Session token 的摘要（commit 时原地
    /// 落为正式 Session token hash）。
    pub session_token_hash: TokenHash,
    /// 授权后续 status/commit/abort 的 commit token 摘要。
    pub commit_token_hash: TokenHash,
    pub transaction_id: TransactionId,
    pub reserved_session_id: SessionId,
    pub prepared_at: UtcMillis,
    pub expires_at: UtcMillis,
}

/// A2 commit 命令：以 commit token 摘要寻址事务并原子激活。
#[derive(Debug, Clone)]
pub struct CommitTransactionCommand {
    pub transaction_id: TransactionId,
    pub commit_token_hash: TokenHash,
    pub device_label: Option<DisplayName>,
    pub committed_at: UtcMillis,
}

/// A2 abort 命令：以 commit token 摘要寻址 prepared 事务。
#[derive(Debug, Clone)]
pub struct AbortTransactionCommand {
    pub transaction_id: TransactionId,
    pub commit_token_hash: TokenHash,
    pub aborted_at: UtcMillis,
}

/// Home 与配对存储端口。
#[async_trait::async_trait]
pub trait HomeStore: Send + Sync {
    /// 幂等初始化 Home Identity：guard + home 行同一事务提交；guard 冲突
    /// 返回 `AlreadyInitialized`，且不得留下半个 Home 行。
    async fn init_home(&self, home: &NewHome) -> Result<(), StoreError>;

    /// 读取 Home Identity；未初始化返回 None。
    async fn get_home(&self) -> Result<Option<Home>, StoreError>;

    /// 签发 Pairing Grant：校验 issued_by 是 active Human 后插入；失败
    /// 不得留下半个 Grant 行。
    async fn issue_grant(&self, grant: &NewGrant) -> Result<(), StoreError>;

    /// 一次性兑换：同一 `BEGIN IMMEDIATE` 事务内裁决 active 未过期 Grant、
    /// 创建独立 Human Session、建立 1:1 Session Metadata、标记 Grant
    /// consumed；任何持久化失败整体回滚且 Grant 仍可重试（**失败零残留**：
    /// 不留下 participant / session / metadata 任何一行）。重放（已消费）
    /// 统一 `GrantInvalid`，绝不新建或重发 Session。
    ///
    /// 会话归谁看 Grant 用途：`SelfDevice` ⇒ `grant.issued_by`；
    /// `NewHuman` ⇒ 同一事务内**新建的 active Human**（名字取自
    /// `exchange.new_human_display_name`；缺失则 `NewHumanNameRequired`，
    /// 且不消费 Grant）。
    async fn exchange_grant(
        &self,
        exchange: &ExchangeRequest,
        now: UtcMillis,
    ) -> Result<ExchangeOutcome, StoreError>;

    /// 列出当前 Human 签发的未消费 Grant（含已撤销与派生过期；不含已
    /// 消费）。对外状态（active/expired/revoked）由调用方按 `status_at`
    /// 派生。
    async fn list_grants(&self, human_id: &ParticipantId) -> Result<Vec<PairingGrant>, StoreError>;

    /// 撤销当前 Human 签发的未使用 Grant：幂等（已撤销重复撤销成功）；
    /// 未知 / 非本人 / 已消费统一 `GrantNotFound`；已消费的重复撤销也
    /// 统一 `GrantNotFound`（已使用的 Grant 不可见、不可撤销）。
    async fn revoke_grant(
        &self,
        human_id: &ParticipantId,
        grant_id: &GrantId,
        now: UtcMillis,
    ) -> Result<(), StoreError>;

    /// 列出当前 Human 的全部 Session（含已撤销）的非秘密元数据与可选
    /// Session Metadata；绝不返回 token/hash。
    async fn list_sessions(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<SessionListItem>, StoreError>;

    /// 撤销当前 Human 自己的 Session（独立撤销，不影响同 Human 其他
    /// Session）：幂等；未知 / 非本人统一 `SessionNotFound`。
    async fn revoke_owned_session(
        &self,
        human_id: &ParticipantId,
        session_id: &SessionId,
        now: UtcMillis,
    ) -> Result<(), StoreError>;

    /// A2 prepare：同一 `BEGIN IMMEDIATE` 事务内裁决 active 未过期 Grant、
    /// 创建或幂等重用 `pairing_transaction`（同一 nonce + 同一组摘要恒
    /// 重放既有事务），**不创建正式 Human Session、不消费 Grant**。任何
    /// 持久化失败整体回滚（grant 保持 active、事务不残留半份）。新增或
    /// 重用都返回同一非秘密视图。
    ///
    /// - 同一 `client_nonce` + 同一 `session_token_hash` + 同一
    ///   `commit_token_hash`（以及应用的 `transaction_id`/
    ///   `reserved_session_id`）重试恒返回同一准备事务；
    /// - 同一 nonce 但摘要不一致 → `PairingTransactionInvalid`（参数不变量
    ///   冲突，由应用层统一到该语义，避免提供存在性探测）。
    async fn prepare_transaction(
        &self,
        request: &PrepareTransactionRequest,
        now: UtcMillis,
    ) -> Result<TransactionView, StoreError>;

    /// A2 status：凭 commit token 摘要读取非秘密事务状态；未知 / token
    /// 不匹配 / 已过期统一 `TransactionNotFound`（不泄露存在性）。
    async fn get_transaction_status(
        &self,
        transaction_id: &TransactionId,
        commit_token_hash: &TokenHash,
        now: UtcMillis,
    ) -> Result<Option<TransactionView>, StoreError>;

    /// A2 commit：同一 `BEGIN IMMEDIATE` 事务内完成「裁决 prepared 未过期
    /// 事务（凭 commit token 摘要）→ 以 prepare 阶段提交的
    /// session_token_hash 创建正式 Human Session → 写入 session_metadata
    /// → 标记事务 committed → 消费 Pairing Grant」。首次成功与重复提交
    /// 返回同一非秘密结果，绝不产生第二 Session；任何持久化失败整体
    /// 回滚。重放（已 committed）→ 返回同一视图（幂等成功）。已 aborted
    /// / 未知 / token 不匹配 → `TransactionNotFound`；prepared 但已过期 →
    /// `TransactionNotFound`。
    async fn commit_transaction(
        &self,
        command: &CommitTransactionCommand,
        now: UtcMillis,
    ) -> Result<TransactionView, StoreError>;

    /// A2 abort：凭 commit token 摘要主动放弃 **prepared** 事务（幂等）；
    /// 只影响 prepared，不影响 Grant 的 active 状态（Grant 仍可重新
    /// prepare）。未知 / token 不匹配 / 已过期 / 已 committed / 已 aborted
    /// 统一 `TransactionNotFound`（重复 abort 对外同 `pairing_transaction_invalid`）。
    async fn abort_transaction(
        &self,
        command: &AbortTransactionCommand,
        now: UtcMillis,
    ) -> Result<(), StoreError>;
}
