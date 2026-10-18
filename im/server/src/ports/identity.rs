//! 身份与认证窄端口。
//!
//! 本端口只声明身份切片的持久化边界，不依赖具体数据库或 HTTP 框架。
//! 存储 adapter 实现本端口；应用用例只通过端口消费存储能力。

use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::ports::storage::StorageError;

/// 待持久化的新 Human。
#[derive(Debug, Clone)]
pub struct NewHuman {
    pub id: ParticipantId,
    pub display_name: DisplayName,
    pub created_at: UtcMillis,
}

/// 待持久化的新 Human Session。
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: SessionId,
    pub human_id: ParticipantId,
    pub token_hash: TokenHash,
    pub created_at: UtcMillis,
}

/// 认证成功上下文：Session 及其指向的 Participant。
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub participant: Participant,
    pub session: HumanSession,
}

/// 存储层错误。
#[derive(Debug)]
pub enum StoreError {
    /// bootstrap guard 已被占用：首位 Human 已经存在。
    AlreadyBootstrapped,
    /// 目标 Participant 不存在。
    UnknownHuman,
    /// 目标 Participant 存在但不是 active Human。
    HumanNotActive,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyBootstrapped => write!(f, "first human already bootstrapped"),
            Self::UnknownHuman => write!(f, "no such human participant"),
            Self::HumanNotActive => write!(f, "human participant is not active"),
            Self::Database(e) => write!(f, "identity store failure: {e}"),
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

/// 身份与认证存储端口。
#[async_trait::async_trait]
pub trait IdentityStore: Send + Sync {
    /// bootstrap guard、首位 Human 与初始 Session 在同一事务内提交；
    /// guard 冲突返回 `AlreadyBootstrapped`，且不得留下半个 Human 或
    /// 半个 Session。
    async fn bootstrap_first_human(
        &self,
        human: &NewHuman,
        session: &NewSession,
    ) -> Result<(), StoreError>;

    /// 只为存在且 active 的 Human 创建 Session；未知或非 active 明确失败。
    async fn issue_human_session(&self, session: &NewSession) -> Result<(), StoreError>;

    /// 创建新的 active Human 并为其签发 Session（create-human CLI 路径；
    /// 与 bootstrap 共用同一 Human 创建与 Session 签发用例，但不占用
    /// bootstrap guard）。失败不得留下半个 Human 或半个 Session。
    async fn create_human(&self, human: &NewHuman, session: &NewSession) -> Result<(), StoreError>;

    /// 精确查找「未撤销且指向 active Human」的 Session；找不到返回 None。
    async fn authenticate(&self, token_hash: &TokenHash)
    -> Result<Option<AuthContext>, StoreError>;

    /// 幂等撤销 Session；不删除 Human，不影响同一 Human 的其他 Session。
    async fn revoke_session(&self, session_id: &SessionId) -> Result<(), StoreError>;

    /// **改自己的名字**（表达权第四刀，2026-10-17 主人批准）。
    ///
    /// `display_name` 是**既有 Participant 身份事实**（不在呈现版本链上 —— 那条链明文
    /// "不复制 `display_name`/`status`/`kind`"）⇒ 这是**状态写**，不是事件写：
    /// 没有版本行、没有幂等记录，**同值重写天然幂等**（照
    /// `PUT /v1/backend/rooms/{id}/read-cursor` 的先例）。
    ///
    /// 约束与兑换时**同一套**（`DisplayName::parse`：禁 NUL、≤80 个 Unicode scalar value、
    /// trim 后非空；表上也有同样三条 CHECK）。目标必须是**存在且 active 的 Human** ——
    /// 别的 kind 或非 active 一律 `HumanNotActive`（人不写别人的名字：没有 Owner 这一支）。
    async fn rename_human(
        &self,
        human_id: &ParticipantId,
        display_name: &DisplayName,
    ) -> Result<(), StoreError>;
}
