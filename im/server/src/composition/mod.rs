//! 配置读取、依赖组装和启动。

pub mod cli;
pub mod config;

#[cfg(test)]
mod config_tests;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tracing::info;

use crate::adapters::blob::LocalBlobStorage;
use crate::adapters::call::InMemoryCallState;
use crate::adapters::http;
use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::attachment_store::SqliteAttachmentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::contact_store::SqliteContactStore;
use crate::adapters::sqlite::event_store::SqliteEventStore;
use crate::adapters::sqlite::home_store::SqliteHomeStore;
use crate::adapters::sqlite::human_event_store::SqliteHumanEventStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::{DbError, MigrationStore};
use crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::ports::attachment::AttachmentStore;
use crate::ports::blob::BlobStorage;
use crate::ports::migration::MigrationProbe;
use crate::ports::participant_presentation::ParticipantPresentationStore;

pub use config::Config;

/// 附件字节子目录（媒体根下的受控命名空间）。
const ATTACHMENTS_SUBDIR: &str = "attachments";
/// 运行期附件维护固定间隔：pending 最长 24h，小时级回收把长期运行时的
/// 最坏额外占用控制在约 1h 内，不增加第二配置事实源。
const ATTACHMENT_MAINTENANCE_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(60 * 60);

/// 启动失败（对应进程以非零退出码终止）。
///
/// `error_kind()` 供 main 记录稳定分类；typed cause 经 `Error::source()`
/// 保留（DbError → sqlx/MigrateError，Bind/Serve → io::Error），绝不把
/// 底层文本当作默认日志输出。
#[derive(Debug)]
pub enum StartupError {
    Database(DbError),
    Media(std::io::Error),
    Bind(std::io::Error),
    Serve(std::io::Error),
    Home(crate::application::home::InitHomeError),
}

impl StartupError {
    /// 稳定内部分类（结构化日志 error_kind 专用）。
    pub fn error_kind(&self) -> &'static str {
        match self {
            StartupError::Database(_) => "database",
            StartupError::Media(_) => "media",
            StartupError::Bind(_) => "bind",
            StartupError::Serve(_) => "serve",
            StartupError::Home(_) => "home",
        }
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupError::Database(e) => write!(f, "database initialization failed: {e}"),
            StartupError::Media(e) => write!(f, "media storage initialization failed: {e}"),
            StartupError::Bind(e) => write!(f, "failed to bind listener: {e}"),
            StartupError::Serve(e) => write!(f, "server error: {e}"),
            StartupError::Home(e) => write!(f, "home identity initialization failed: {e}"),
        }
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StartupError::Database(e) => Some(e),
            StartupError::Media(e) => Some(e),
            StartupError::Bind(e) => Some(e),
            StartupError::Serve(e) => Some(e),
            StartupError::Home(e) => Some(e),
        }
    }
}

/// 单轮附件维护（契约 CORE_V1_IM_CONTRACT.md 8）：先执行内部受控清理
/// `purge_expired_pending`（BEGIN IMMEDIATE 原子裁决，回收过期 pending
/// 私有暂存租约行）并 best-effort 删除返回 storage key 的字节（失败只
/// 记录脱敏 warning，不阻塞启动；进程在 DB 提交后崩溃的残留由 orphan
/// sweep 兜底），随后再跑 orphan sweep：清理受控命名空间内超过 grace
/// period、且不被任何 SQLite 行引用的孤儿文件。**引用集合 = 附件行 ∪
/// 头像 Profile 版本行**（头像字节与附件字节共用同一媒体目录，头像版本
/// 行不可变、其字节必须被保护，只回收从未被任何行引用的崩溃残留）。任何
/// 失败只记录脱敏 warning；启动与运行期任务都复用这一条唯一维护链。
async fn maintain_attachments_once(
    blob: &LocalBlobStorage,
    attachment_store: &SqliteAttachmentStore,
    presentation_store: &SqliteParticipantPresentationStore,
) {
    // 1. 内部受控清理：过期 pending 元数据（只删过期 + pending + 无绑定
    //    行；bound 与公共事实绝不删除）。
    let purged = match attachment_store
        .purge_expired_pending(crate::domain::time::UtcMillis::now())
        .await
    {
        Ok(keys) => keys,
        Err(_) => {
            tracing::warn!(
                operation = "attachment.maintenance.purge_expired_pending",
                error_kind = "storage_unavailable",
                "expired pending purge skipped; attachment maintenance continues"
            );
            return;
        }
    };
    // 2. best-effort 删除刚回收行的字节；失败只记录脱敏 warning（孤儿由
    //    orphan sweep 兜底），不阻塞启动。
    for key in &purged {
        if blob.delete(key).await.is_err() {
            tracing::warn!(
                operation = "attachment.maintenance.purge_blob",
                error_kind = "connection_or_io",
                storage_key = %key,
                "failed to remove blob bytes of an expired pending attachment; orphan sweep covers it"
            );
        }
    }
    // 3. orphan sweep 的引用集合是 purge 之后的全部行 + 头像版本行引用的
    //    storage key：过期 pending 不再被保护到永远；头像字节（含历史版本）
    //    全程受保护。头像引用集合不可用时跳过整个 sweep（宁可留着崩溃残留，
    //    也绝不能以不完整引用集合误删活字节）。
    let mut referenced = match attachment_store.referenced_storage_keys().await {
        Ok(keys) => keys,
        Err(_) => {
            tracing::warn!(
                operation = "attachment.maintenance.orphan_sweep",
                error_kind = "storage_unavailable",
                "orphan attachment sweep skipped: referenced storage keys unavailable; attachment maintenance continues"
            );
            return;
        }
    };
    match presentation_store.avatar_storage_keys().await {
        Ok(avatar_keys) => referenced.extend(avatar_keys),
        Err(_) => {
            tracing::warn!(
                operation = "attachment.maintenance.avatar_reference_keys",
                error_kind = "storage_unavailable",
                "orphan attachment sweep skipped: avatar reference keys unavailable; attachment maintenance continues"
            );
            return;
        }
    }
    match blob
        .sweep_orphans(&referenced, crate::adapters::blob::ORPHAN_SWEEP_GRACE)
        .await
    {
        Ok(()) => {}
        Err(_) => {
            tracing::warn!(
                operation = "attachment.maintenance.orphan_sweep",
                error_kind = "connection_or_io",
                "orphan attachment sweep failed; attachment maintenance continues"
            );
        }
    }
}

/// 组装并启动 Core：读配置 → 初始化数据库与迁移 → 确保媒体目录存在并
/// 执行启动清理（过期 pending purge + orphan sweep）→ 绑定监听 → 服务。
///
/// 数据库连接、迁移、媒体目录、配置任何一步失败都会返回 `Err`，由 `main`
/// 以非零码退出；启动清理（purge/orphan sweep）失败只记录脱敏 warning，
/// 不阻塞启动。
pub async fn run(config: &Config) -> Result<(), StartupError> {
    let store = MigrationStore::open(&config.db_path)
        .await
        .map_err(StartupError::Database)?;
    // Home Identity：每个 Core 数据库启动后幂等初始化一次（guard 保证
    // 只成功一次；已初始化则跳过）。失败阻塞启动。
    let home_store = SqliteHomeStore::new(store.pool().clone());
    crate::application::home::init_home(&home_store, config.home_display_name.as_deref())
        .await
        .map_err(StartupError::Home)?;
    // 媒体根目录：启动确保 `<root>/attachments` 存在（失败阻塞启动）。
    let media_root = &config.media_root;
    let attachments_dir: PathBuf = media_root.join(ATTACHMENTS_SUBDIR);
    tokio::fs::create_dir_all(&attachments_dir)
        .await
        .map_err(StartupError::Media)?;
    let blob = LocalBlobStorage::new(attachments_dir);
    let attachment_store = SqliteAttachmentStore::new(store.pool().clone());
    let presentation = SqliteParticipantPresentationStore::new(store.pool().clone());
    maintain_attachments_once(&blob, &attachment_store, &presentation).await;
    let identity = SqliteIdentityStore::new(store.pool().clone());
    let agent = SqliteAgentStore::new(store.pool().clone());
    let room = SqliteRoomStore::new(store.pool().clone());
    let message = SqliteMessageStore::new(store.pool().clone());
    let backend = SqliteBackendStore::new(store.pool().clone());
    let event = SqliteEventStore::new(store.pool().clone());
    let contact = SqliteContactStore::new(store.pool().clone());
    let human_events = SqliteHumanEventStore::new(store.pool().clone());
    // 主体进行态（契约 CORE_V1_AGENT_PROTOCOL.md §6，2026-09-28 重做为
    // 主体形状）：进程内易失（last_seen / typing 两张表 + 两个连接事实
    // 来源）。进程重启全部消失——按契约，这是对的。
    let live = crate::adapters::live::LiveWiring::new_volatile();
    // Call（Core v1 窄解冻切片，CORE_V1_CALL.md）：独立进程内
    // Call 状态机 + 信令中继（含 call-stream 定向送达）。媒体由两端客户端
    // WebRTC P2P 直连，Core 不接触媒体字节；进程重启 Call 全部消失
    // （fail closed 404）。
    let call: Arc<dyn crate::ports::call::CallStore> = Arc::new(InMemoryCallState::new());
    let room: Arc<dyn crate::ports::room::RoomStore> = Arc::new(room);
    let event: Arc<dyn crate::ports::event::EventStore> = Arc::new(event);
    let router = build_router(
        store,
        identity,
        agent,
        room.clone(),
        message,
        backend,
        event.clone(),
        contact,
        human_events,
        live.clone(),
        attachment_store.clone(),
        blob.clone(),
        home_store,
        presentation.clone(),
        call.clone(),
        config.grant_ttl_minutes,
    );
    serve(
        router,
        config.listen_addr,
        call,
        attachment_store,
        blob,
        presentation,
    )
    .await
}

/// 组装路由与状态（composition root）：HTTP handler 只拿到窄端口。
#[allow(clippy::too_many_arguments)]
pub fn build_router(
    store: MigrationStore,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: Arc<dyn crate::ports::room::RoomStore>,
    message: SqliteMessageStore,
    backend: SqliteBackendStore,
    event: Arc<dyn crate::ports::event::EventStore>,
    contact: SqliteContactStore,
    human_events: SqliteHumanEventStore,
    live: crate::adapters::live::LiveWiring,
    attachment_store: SqliteAttachmentStore,
    blob: LocalBlobStorage,
    home: SqliteHomeStore,
    presentation: SqliteParticipantPresentationStore,
    call: Arc<dyn crate::ports::call::CallStore>,
    grant_ttl_minutes: u32,
) -> Router {
    let probe: Arc<dyn MigrationProbe> = Arc::new(store);
    let identity: Arc<dyn crate::ports::identity::IdentityStore> = Arc::new(identity);
    let agent: Arc<dyn crate::ports::agent::AgentStore> = Arc::new(agent);
    let message: Arc<dyn crate::ports::message::MessageStore> = Arc::new(message);
    let backend: Arc<dyn crate::ports::backend::BackendStore> = Arc::new(backend);
    let contact: Arc<dyn crate::ports::contact::ContactStore> = Arc::new(contact);
    let human_events: Arc<dyn crate::ports::human_event::HumanEventStore> = Arc::new(human_events);
    let presentation: Arc<
        dyn crate::ports::participant_presentation::ParticipantPresentationStore,
    > = Arc::new(presentation);
    let attachments: Arc<dyn crate::ports::attachment::AttachmentStore> =
        Arc::new(attachment_store);
    let blob: Arc<dyn crate::ports::blob::BlobStorage> = Arc::new(blob);
    let home: Arc<dyn crate::ports::home::HomeStore> = Arc::new(home);
    http::build_router(
        probe,
        identity,
        agent,
        room,
        message,
        backend,
        event,
        contact,
        human_events,
        live,
        attachments,
        blob,
        home,
        presentation,
        call,
        grant_ttl_minutes,
    )
}

/// 绑定监听地址并服务，直到收到 SIGINT/SIGTERM（Unix）或 Ctrl+C（其他平台）。
///
/// ⚠ 2026-09-28：原先这里还起一个后台 Live sweep task（主动清理过期
/// presence/activity 并唤醒 Live WS）。主体进行态没有推送面、读面按时间
/// 过滤，**不需要任何后台任务**——那条 sweep 随旧 Live State 一起删除。
#[allow(clippy::too_many_arguments)]
async fn serve(
    router: Router,
    addr: SocketAddr,
    call: Arc<dyn crate::ports::call::CallStore>,
    attachment_store: SqliteAttachmentStore,
    blob: LocalBlobStorage,
    presentation_store: SqliteParticipantPresentationStore,
) -> Result<(), StartupError> {
    let listener = TcpListener::bind(addr).await.map_err(StartupError::Bind)?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let call_sweeper = tokio::task::spawn(call_sweeper_task(
        call,
        shutdown_rx.clone(),
        crate::domain::call::CALL_SWEEP_INTERVAL,
    ));
    let attachment_maintenance = tokio::task::spawn(attachment_maintenance_task(
        blob,
        attachment_store,
        presentation_store,
        shutdown_rx,
        ATTACHMENT_MAINTENANCE_INTERVAL,
    ));
    info!(addr = %addr, "celira-im listening");
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(StartupError::Serve);
    let _ = shutdown_tx.send(true);
    let _ = call_sweeper.await;
    let _ = attachment_maintenance.await;
    result
}

/// 单一后台 Call sweep task：以固定短间隔调用
/// `CallStore::sweep_expired`，把过 ring TTL 的 Call 原子置
/// `ended(timeout_ring)`（送达由 store 内的 call-stream 订阅完成）。是
/// ring 超时的生命周期契约，不能依赖下一次写请求。收到关闭信号后在下一
/// 次 tick 边界退出；该 task 不 panic、不终止 Core。`interval` 由调用方
/// 注入（生产取 `CALL_SWEEP_INTERVAL`，测试注入短间隔）。
pub async fn call_sweeper_task(
    call: Arc<dyn crate::ports::call::CallStore>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    interval: std::time::Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = ticker.tick() => {
                call.sweep_expired(crate::domain::time::UtcMillis::now());
            }
        }
    }
}

/// 单一运行期附件维护任务。启动清理已由 run() 同步执行，因此第一轮等待
/// 完整 interval 后再运行；维护失败已由单轮函数脱敏记录，下个周期继续。
/// shutdown 后退出并由 serve await，绝不 detached。
pub async fn attachment_maintenance_task(
    blob: LocalBlobStorage,
    attachment_store: SqliteAttachmentStore,
    presentation_store: SqliteParticipantPresentationStore,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    interval: std::time::Duration,
) {
    let first_tick = tokio::time::Instant::now()
        .checked_add(interval)
        .unwrap_or_else(tokio::time::Instant::now);
    let mut ticker = tokio::time::interval_at(first_tick, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = ticker.tick() => {
                maintain_attachments_once(&blob, &attachment_store, &presentation_store).await
            }
        }
    }
}

#[cfg(test)]
mod attachment_maintenance_tests {
    use super::*;
    use crate::domain::attachment::{
        AttachmentId, ContentType, FileName, NewAttachment, StorageKey,
    };
    use crate::domain::time::UtcMillis;
    use sha2::Digest as _;

    #[tokio::test]
    async fn task_sweeps_expired_pending_and_aged_orphan_then_exits() {
        let dir = tempfile::tempdir().unwrap();
        let store = MigrationStore::open(&dir.path().join("core.db"))
            .await
            .expect("open migrated database");
        let pool = store.pool().clone();
        let attachment_store = SqliteAttachmentStore::new(pool.clone());
        let blob_dir = dir.path().join("media/attachments");
        let blob = LocalBlobStorage::new(blob_dir.clone());

        let identity = SqliteIdentityStore::new(pool.clone());
        let human = crate::application::identity::bootstrap_human(&identity, "Owner")
            .await
            .expect("bootstrap owner")
            .participant;
        let room_store = SqliteRoomStore::new(pool.clone());
        let created = crate::application::room::create_group(
            &room_store,
            &human,
            "maintenance-test",
            "maintenance-room-key",
        )
        .await
        .expect("create group room");
        let room_id = match created {
            crate::application::room::CreateRoomOutcome::Created { room, .. }
            | crate::application::room::CreateRoomOutcome::Replayed { room, .. } => room.id,
        };
        let now = UtcMillis::now();

        let expired = NewAttachment {
            id: AttachmentId::new_v4(),
            uploader_participant_id: human.id,
            room_id,
            file_name: FileName::parse("expired.txt").unwrap(),
            content_type: ContentType::parse("text/plain").unwrap(),
            size_bytes: 7,
            storage_key: StorageKey::new_v4(),
            sha256_hex: hex::encode(sha2::Sha256::digest(b"expired")),
            uploaded_at: UtcMillis::from_millis(now.millis() - 90_000_000),
        };
        attachment_store.create_pending(&expired).await.unwrap();
        blob.write(&expired.storage_key, b"expired").await.unwrap();

        let fresh = NewAttachment {
            id: AttachmentId::new_v4(),
            uploader_participant_id: expired.uploader_participant_id.clone(),
            room_id: expired.room_id.clone(),
            file_name: FileName::parse("fresh.txt").unwrap(),
            content_type: ContentType::parse("text/plain").unwrap(),
            size_bytes: 5,
            storage_key: StorageKey::new_v4(),
            sha256_hex: hex::encode(sha2::Sha256::digest(b"fresh")),
            uploaded_at: now,
        };
        attachment_store.create_pending(&fresh).await.unwrap();
        blob.write(&fresh.storage_key, b"fresh").await.unwrap();

        let orphan = StorageKey::new_v4();
        blob.write(&orphan, b"orphan").await.unwrap();
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(90_000);
        let orphan_path = blob_dir.join(orphan.as_str());
        std::fs::File::options()
            .write(true)
            .open(&orphan_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(past))
            .unwrap();

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let presentation_store = SqliteParticipantPresentationStore::new(store.pool().clone());
        let task = tokio::spawn(attachment_maintenance_task(
            blob,
            attachment_store,
            presentation_store,
            shutdown_rx,
            std::time::Duration::from_millis(10),
        ));

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let expired_row: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE id = ?")
                        .bind(expired.id.as_str())
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                if expired_row == 0
                    && !blob_dir.join(expired.storage_key.as_str()).exists()
                    && !orphan_path.exists()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("maintenance must reclaim expired pending and aged orphan");
        let fresh_row: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE id = ?")
            .bind(fresh.id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fresh_row, 1, "fresh pending metadata must stay referenced");
        assert!(
            blob_dir.join(fresh.storage_key.as_str()).exists(),
            "fresh referenced blob must never be swept"
        );

        let _ = shutdown_tx.send(true);
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("maintenance must exit after shutdown")
            .expect("maintenance task must join cleanly");
    }

    #[tokio::test]
    async fn task_survives_a_maintenance_io_failure_until_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let store = MigrationStore::open(&dir.path().join("core.db"))
            .await
            .expect("open migrated database");
        let attachment_store = SqliteAttachmentStore::new(store.pool().clone());
        let invalid_blob_dir = dir.path().join("not-a-directory");
        std::fs::write(&invalid_blob_dir, b"file blocks read_dir").unwrap();
        let blob = LocalBlobStorage::new(invalid_blob_dir);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let presentation_store = SqliteParticipantPresentationStore::new(store.pool().clone());
        let task = tokio::spawn(attachment_maintenance_task(
            blob,
            attachment_store,
            presentation_store,
            shutdown_rx,
            std::time::Duration::from_millis(10),
        ));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !task.is_finished(),
            "best-effort maintenance I/O failure must not terminate the task"
        );
        let _ = shutdown_tx.send(true);
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("maintenance must exit after shutdown")
            .expect("maintenance task must join cleanly");
    }

    #[tokio::test]
    async fn task_waits_for_interval_and_exits_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let store = MigrationStore::open(&dir.path().join("core.db"))
            .await
            .expect("open migrated database");
        let attachment_store = SqliteAttachmentStore::new(store.pool().clone());
        let blob = LocalBlobStorage::new(dir.path().join("media/attachments"));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let presentation_store = SqliteParticipantPresentationStore::new(store.pool().clone());
        let task = tokio::spawn(attachment_maintenance_task(
            blob,
            attachment_store,
            presentation_store,
            shutdown_rx,
            std::time::Duration::from_secs(60),
        ));

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            !task.is_finished(),
            "task must wait instead of sweeping immediately"
        );
        let _ = shutdown_tx.send(true);
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("maintenance must exit after shutdown")
            .expect("maintenance task must join cleanly");
    }
}

/// 等待终止信号后触发 graceful shutdown。
///
/// Unix（Linux/macOS）：同时监听 SIGINT（Ctrl+C）与 SIGTERM（进程管理器/
/// 容器停止信号），任一信号到达即开始优雅关停；
/// 其他平台：只监听 Ctrl+C。
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = sigint.recv() => info!("SIGINT received, starting graceful shutdown"),
            _ = sigterm.recv() => info!("SIGTERM received, starting graceful shutdown"),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl_c handler");
        info!("Ctrl+C received, starting graceful shutdown");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::storage::{StorageError, StorageFailureKind};
    use std::error::Error;

    /// StartupError：error_kind 稳定，typed cause 经 source() 保留。
    #[test]
    fn startup_error_keeps_stable_kind_and_typed_source() {
        let io = std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "bind failed on 127.0.0.1:8080",
        );
        let err = StartupError::Bind(io);
        assert_eq!(err.error_kind(), "bind");
        let source = err.source().expect("typed cause must be preserved");
        assert!(
            source.to_string().contains("bind failed"),
            "source chain must keep the typed io::Error: {source}"
        );
    }

    /// StartupError::source 可下钻到 DbError → sqlx::Error 的类型链。
    #[test]
    fn startup_database_error_source_reaches_sqlx_cause() {
        let db = DbError::Connect(sqlx::Error::PoolClosed);
        let err = StartupError::Database(db);
        assert_eq!(err.error_kind(), "database");
        let db_err = err.source().expect("DbError must be the typed cause");
        assert!(
            db_err.to_string().contains("failed to connect"),
            "DbError Display keeps its stable boundary message: {db_err}"
        );
        let sqlx_cause = db_err
            .source()
            .expect("DbError::source must expose sqlx::Error");
        assert!(
            sqlx_cause.to_string().contains("pool"),
            "typed sqlx::Error must be reachable down the chain: {sqlx_cause}"
        );
    }

    /// StorageError 携带 token 文本时：StartupError/Display 链只暴露稳定
    /// 分类（生产日志只用 error_kind，绝不 %e 输出底层）。
    #[test]
    fn storage_cause_never_leaks_token_through_display_chain() {
        let storage = StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "token=clr_hs_ABCDEFGHIJKLMNOPQRSTUVWXYZ234567 hash=deadbeef",
        );
        let display = storage.to_string();
        for forbidden in ["clr_hs_", "ABCDEFGH", "deadbeef", "token"] {
            assert!(
                !display.contains(forbidden),
                "Display chain must not leak {forbidden:?}: {display:?}"
            );
        }
    }
}
