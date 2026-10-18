//! Durable Human Event 窄端口：耐久读取 + post-commit 定向唤醒目标。
//!
//! 本端口只声明 Human Event 切片的持久化边界，不依赖具体数据库、HTTP
//! 框架或 WebSocket。SQLite adapter 实现本端口；应用用例只通过端口消费
//! 存储能力；volatile notifier 由独立 adapter 实现。
//!
//! Human Event 生成不经过本端口：source 事实 → 共享 fact → per-Human
//! delivery 的原子成立属于 SQLite 触发器事实链（migration 009），HTTP/
//! application 不并行维护第二套写入流程。本端口只提供：
//! - 当前 Human 自己 sequence > after 的稳定有界恢复读取（带读取时
//!   Membership 授权过滤，唯一例外是读取者自己的 member.left）；
//! - post-commit 唤醒目标查询：从**已提交**的 per-Human delivery / shared
//!   fact 事实按完整 source 身份（kind + source_id）反查应被唤醒的 Human，
//!   避免在 Rust 层复制 targeting 规则；WebSocket 连接状态与服务端投递
//!   ACK 永不写入 SQLite。

use crate::domain::human_event::{HumanEventFact, HumanEventFactId, HumanEventSource};
use crate::domain::participant::ParticipantId;
use crate::ports::storage::StorageError;

/// 一条可恢复的 Human Event：共享 fact + 该 Human 自己的 delivery
/// sequence（读取时已按契约过滤不可见事件；sequence 只作内部排序/位置
/// 事实，协议层通过 stable fact ID 与 opaque cursor 表达位置）。
#[derive(Debug, Clone)]
pub struct HumanEventWithFact {
    pub fact: HumanEventFact,
    #[cfg_attr(not(test), allow(dead_code))]
    pub sequence: i64,
}

/// 存储层错误。
#[derive(Debug)]
pub enum HumanEventStoreError {
    /// 数据库或其他持久化失败（typed cause + 稳定分类）；协议错误与存储
    /// 错误一律不泄露 SQL、路径、token、hash 或其他 Human 的事实。
    Database(StorageError),
}

impl std::fmt::Display for HumanEventStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "human event store failure: {e}"),
        }
    }
}

impl std::error::Error for HumanEventStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
        }
    }
}

/// Human Event 存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait HumanEventStore: Send + Sync {
    /// 返回当前 Human 下 `sequence > after` 的 Event，严格按该 Human 的
    /// sequence 升序，最多 `limit` 条；不使用 OFFSET。读取时按当前
    /// Membership 裁决可见性：普通事件要求读取者在该 Room 有 active
    /// Membership 周期；唯一例外是读取者自己的 `member.left`（离群后仍可
    /// 读取）。纯读取，无副作用：不建立/改写 fact、delivery、cursor、
    /// attempt、ACK 或任何服务端状态；被过滤的不可见事件只造成该 Human
    /// 序列上的空洞，不泄露任何事实。
    async fn list_events(
        &self,
        human_id: &ParticipantId,
        after: i64,
        limit: u32,
    ) -> Result<Vec<HumanEventWithFact>, HumanEventStoreError>;

    /// 把 Human Event 公共 cursor 解析回内部位置：按当前 Human 的作用域
    /// 裁决（cursor 引用的 fact 必须已被该 Human 投递）。未知、跨 Human
    /// 或从未投递给该 Human 的 cursor 一律返回 None（对外统一 400
    /// invalid_request，不泄露 fact 是否存在或属于谁）。cursor 编码/解码
    /// 只存在于 protocol/application 边界，本端口只接收稳定 fact ID 并
    /// 返回内部 i64 sequence。
    async fn resolve_event_after(
        &self,
        human_id: &ParticipantId,
        cursor_fact_id: &HumanEventFactId,
    ) -> Result<Option<i64>, HumanEventStoreError>;

    /// post-commit 定向唤醒目标：按完整 source 身份（kind + source_id）
    /// 从已提交的 per-Human delivery / shared fact 事实反查恰好接收了这些
    /// fact 的 Human（去重）。`sources` 是刚提交写事务的 source 引用
    /// （messages.id / membership_id / transition_id 各自带 kind）；空集合
    /// 返回空。同一 source_id 在不同 kind 下是不同 fact，反查必须同时
    /// 匹配 kind 与 source_id，不能只按 source_id。纯读取；调用方（HTTP
    /// 成功路径）只在提交成功后调用，失败只记录脱敏 warn，不得把已提交
    /// 事务伪装失败。
    async fn delivery_targets(
        &self,
        sources: &[HumanEventSource],
    ) -> Result<Vec<ParticipantId>, HumanEventStoreError>;
}

/// Volatile Human Event 唤醒端口：进程内 best-effort 定向通知。
///
/// 契约（CORE_V1_HUMAN_EVENTS.md 8.x）：notifier 是纯易失提示，只有事务
/// 真实创建了 shared fact / delivery 并成功 commit 后才由 HTTP 成功路径
/// 调用；唤醒丢失、无 receiver、慢消费、Core 重启均不影响耐久 Event。
/// 不写 SQLite、不持久化连接状态、不表示服务端投递 ACK。
///
/// 每连接唤醒通道**有界容量 1**：同一连接在已有待发送提示时，后续
/// notify 的重复提示被合并/丢弃（try_send Full 是合法 best-effort，不
/// 报业务失败，也不会产生无界积压）；通道已关闭（连接结束）才清理订阅。
/// 禁止改用 unbounded 通道掩盖问题。
#[async_trait::async_trait]
pub trait HumanEventNotifier: Send + Sync {
    /// 订阅某个 Human 的唤醒通道（一个连接一个 capacity-1 channel；
    /// notifier 只持有弱引用，连接的强引用由订阅方保持——连接断开时订阅
    /// 方 drop 强引用，实现侧在下一次 notify 惰性清理）。订阅本身不携带
    /// 任何事实。
    fn subscribe(
        &self,
        human_id: &ParticipantId,
        wake: std::sync::Arc<tokio::sync::mpsc::Sender<()>>,
    );

    /// 定向唤醒一批 Human（去重由实现方保证）。容量满（已有待发送提示）
    /// 时合并/丢弃重复提示，不影响耐久事实；发送失败/无接收者直接忽略。
    fn notify(&self, human_ids: &[ParticipantId]);
}
