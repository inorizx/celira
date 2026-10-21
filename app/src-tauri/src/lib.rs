//! Celira 桌面的入口 —— **完整 app 的桌面形态**（`PROJECT_SHAPE.md` §9.1：
//! Windows ＝"完整 app：壳 + 本机世界 + 本机执行端"）。
//!
//! ## 它是什么（别按"壳"读它）
//!
//! 桌面 app 的**默认形态是成品**：打开就在**自己的世界**里，而那个世界与这台机器上的执行端
//! **是它的**（§8.1：本地进程是本地 app 的孩子）。它同时**能当壳子用**（§9.3 那条纪律：
//! app 里家条目统一为"地址 + 钥匙"，本机那条只是地址为 loopback 的一条）—— 连别的世界、
//! 接入别的机器上的家，是**高级设置里的加法**，不是另一份构建、更不是另一个模式。
//!
//! 进程里的三层职责：
//!
//! 1. **把 `client/dist` 端出来**（呈现面）；
//! 2. **当请求的发出者**（下一条说明为什么） —— 出网走 Rust 侧；
//! 3. **机器侧执行**：**本机世界**（[`local_world`]：起世界、首次布置、交一支本机会话，
//!    2026-10-03 落）与**安家**（[`anjia`]：把世界里一支已存在的 Agent 接到这台机器上）。
//!
//! ## 为什么 HTTP 从 Rust 侧发（而不是让 webview 自己 fetch）
//!
//! 浏览器有一条规矩：网页不许拿着身份去跟"别的源"说话，除非服务端明确允许（CORS）。
//! Celira 的 im 服务端**不发任何跨源头**，所以网页版的页面只能跟它同源。
//!
//! 而 app 里的请求由 **Rust 侧**发出 —— 那条规矩是加在*网页*这个身份上的，对 Rust 进程
//! 不成立。于是：**「app 连任意地址的世界」不需要 im 服务端新增任何东西**。
//!
//! ⚠ 这条只在**请求真的经由本进程**时成立。前端必须用 `tauri-plugin-http` 的那份 `fetch`
//! （走 Rust），**不是** webview 自带的 `window.fetch` —— 后者仍然受同源策略管。
//! 这条缝的接线在前端：`client/src/api/` 的传输面（`shell.ts`，2026-09-29 已接线）。
//!
//! ## 这一层做的事与不做的事（护栏）
//!
//! - **本机世界**：探活优先（已经在跑就不动它）、不在才起（无参数、三个 `CELIRA_IM_*` 环境、
//!   `GET /health` 就绪判据 —— 与 `up.js` 同一份）；首次布置（铸首位 Human + 签会话）
//!   **判据逐条对齐 `provision.js`**（同一个家同一时刻只有一只布置的手）。⚠ **不建 Agent**
//!   （§8.5："不存在默认 Agent"）；
//! - **安家**：起**本机执行端的进程**（安家六步里的第 5 步）。⚠ **它不是进程管理器**：
//!   崩了**不自动重启**（§8.6-2）、不做 **PID 认领**（§8.7 整块不要）；
//! - **本机服务**（[`local_services`] ＋ [`local_residents`]）：**起**（打开 app ⇒ 世界与住户都在跑，
//!   §8.1 本地进程是本地 app 的孩子）、**看**（托盘图标看得出"还有本地服务在跑"，§8.2）、
//!   **停**（托盘菜单「退出（停止本机服务）」；停之前**先验身份** —— [`process_facts`]）。
//! - **不存凭据**：Human token 与家钥匙**不落盘**（本机会话只活在内存里、交给页面一次）；
//!   Agent 的后端凭据落**按身份分条**的本地存储（与 `scripts/home/credentials.js` 同一处
//!   —— 一份事实一个权威位置）；
//! - **不代理**：壳不做同源代转，也不做中继。前端要连什么地址，就是什么地址 ——
//!   本机那条地址**由壳解析出来交给它**（§8.4 推论 2）。

pub mod agent_hands;
pub mod anjia;
pub mod app_version;
pub mod console_window;
pub mod external_links;
pub mod home_doors;
pub mod home_facts;
pub mod home_shape;
pub mod local_residents;
pub mod local_services;
pub mod local_store;
pub mod local_world;
pub mod machine_settings;
pub mod model_sources;
pub mod model_witness;
pub mod native_notify;
pub mod process_facts;
pub mod registration;
pub mod unread;
pub mod world_channel;

use tauri::Manager;

/// **这台机器上的 app 配置目录**（壳自己解析，页面不需要知道它在哪）。
///
/// 归它的是机器级表态（`settings.json` 的 `runLocalWorld` / `quitStopsServices`，
/// 见 [`machine_settings`]）—— 不是家的配置（那在 `home.conf`），也不是家条目表
/// （那在页面那一侧）。
pub struct MachineConfigDir(pub std::path::PathBuf);

/// 托盘图标的两个身份（`id` 用同一份常量，避免两处写字面量）。
const TRAY_ID: &str = "celira-tray";
const TRAY_MENU_STATUS: &str = "services-status";
const TRAY_MENU_OPEN: &str = "open-window";
const TRAY_MENU_QUIT: &str = "quit";

/// 「还有本地服务在跑」那一个图标变体（§8.2 点名"必须一起做"的那条）。
///
/// 编译期内嵌（`include_bytes!`）：**不依赖运行时路径**（打包后 resource 目录的形状与
/// 开发时不同，走路径就会在打包版里静默丢图标）。生成脚本与理由见
/// `icons/make-tray-running.py`。
const TRAY_RUNNING_ICON: &[u8] = include_bytes!("../icons/tray-running.png");

/// 读一次本机服务快照（托盘与读数共用一处）。
fn services_snapshot() -> (local_world::LocalFacts, local_services::ServicesSnapshot) {
    let facts = local_world::LocalFacts::load(None, None);
    let snapshot = local_services::snapshot(&facts);
    (facts, snapshot)
}

/// 托盘那两个**要改字的**菜单项（Tauri 2 没有"按 id 取回菜单项"的接口 ⇒ 自己拿着句柄）。
///
/// 为什么状态行与退出项要改字：§8.2 要用户看得出"还有本地服务在跑"；§8.3 要把**后果**
/// 写在菜单项里（`退出（停止本机服务）` / `退出（保持运行）`）—— 两者都不是常量。
pub struct TrayHandles {
    pub status: tauri::menu::MenuItem<tauri::Wry>,
    pub quit: tauri::menu::MenuItem<tauri::Wry>,
}

/// 托盘悬停那句话的**唯一出处**（服务快照 ＋ 未读）。
///
/// ⚠ 未读那一段的拼法在 `unread::tray_tooltip`，但"这句话是什么"只有这里说了算：
/// `refresh_tray`、`set_unread_total` 与真机探针读的都必须是**同一个字符串**。
pub(crate) fn tray_tooltip_now(app: &tauri::AppHandle) -> String {
    let (_facts, snapshot) = services_snapshot();
    unread::tray_tooltip(&snapshot.summary(), unread::current(app))
}

/// 把托盘刷新成"现在的样子"：图标（有服务在跑 / 都停了）、悬停那句话、菜单里的状态行。
///
/// §8.2 的原话：**托盘图标要能看出"还有本地服务在跑"** —— 否则用户会以为已经全关了，
/// 而东西还在后台写磁盘、占端口。所以这里是三处一起说：图标 + tooltip + 菜单状态行。
fn refresh_tray(app: &tauri::AppHandle) {
    let (_facts, snapshot) = services_snapshot();
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let _ = tray.set_tooltip(Some(tray_tooltip_now(app)));
    if snapshot.any_running() {
        if let Ok(image) = tauri::image::Image::from_bytes(TRAY_RUNNING_ICON) {
            let _ = tray.set_icon(Some(image));
        }
    } else if let Some(icon) = app.default_window_icon().cloned() {
        let _ = tray.set_icon(Some(icon));
    }
    let handles = app.state::<TrayHandles>();
    let _ = handles.status.set_text(snapshot.summary());
    let config_dir = MachineConfigDir(
        app.path()
            .app_config_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from(".celira-app")),
    )
    .0;
    let _ = handles
        .quit
        .set_text(quit_item_label(machine_settings::read_quit_stops_services(
            &config_dir,
        )));
}

/// 托盘菜单那一项「退出」的**名字**（§8.3：把后果写在菜单项里，而不是退出后弹一句解释）。
fn quit_item_label(stops_services: bool) -> &'static str {
    if stops_services {
        "退出（停止本机服务）"
    } else {
        "退出（保持运行）"
    }
}

/// 停掉本机服务并把结果写进日志（托盘退出与面板「全部停止」都走这里）。
fn stop_local_services_and_report() -> Vec<String> {
    local_services::stop_local_services_and_report()
}

/// 装托盘（§8.2-8.3）。失败只记一条日志：**没有托盘不该让 app 起不来**。
fn setup_tray(app: &tauri::AppHandle) {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

    let build = || -> tauri::Result<()> {
        let config_dir = MachineConfigDir(
            app.path()
                .app_config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".celira-app")),
        )
        .0;
        let stops = machine_settings::read_quit_stops_services(&config_dir);
        let status = MenuItem::with_id(
            app,
            TRAY_MENU_STATUS,
            "本机服务：还不知道",
            false,
            None::<&str>,
        )?;
        let open = MenuItem::with_id(app, TRAY_MENU_OPEN, "打开窗口", true, None::<&str>)?;
        let quit = MenuItem::with_id(
            app,
            TRAY_MENU_QUIT,
            quit_item_label(stops),
            true,
            None::<&str>,
        )?;
        let menu = Menu::with_items(
            app,
            &[&status, &PredefinedMenuItem::separator(app)?, &open, &quit],
        )?;
        // 句柄进状态（刷新时要改字；Tauri 2 没有"按 id 取回菜单项"）。
        app.manage(TrayHandles {
            status: status.clone(),
            quit: quit.clone(),
        });
        let mut builder = tauri::tray::TrayIconBuilder::with_id(TRAY_ID)
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                TRAY_MENU_OPEN => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                TRAY_MENU_QUIT => {
                    // §8.2：托盘菜单「退出（停止本机服务）」⇒ 本地进程跟着停（它们的出身就是这 app）。
                    // 停这一下可能等一会儿（逐支核对身份），所以放到别的线程里，别冻住托盘。
                    let app = app.clone();
                    std::thread::spawn(move || {
                        let config_dir = MachineConfigDir(
                            app.path()
                                .app_config_dir()
                                .unwrap_or_else(|_| std::path::PathBuf::from(".celira-app")),
                        )
                        .0;
                        if machine_settings::read_quit_stops_services(&config_dir) {
                            let lines = stop_local_services_and_report();
                            log::info!(
                                "[本机服务] 退出前收场：{}",
                                local_services::summary_line(&lines)
                            );
                        } else {
                            log::info!("[本机服务] 退出：按设置**保持运行**（本机服务照旧跑）");
                        }
                        app.exit(0);
                    });
                }
                _ => {}
            })
            .on_tray_icon_event(|tray, _event| {
                // 每次点托盘都把状态刷新一遍（用户看到的就是"现在"）。
                refresh_tray(tray.app_handle());
            });
        if let Some(icon) = app.default_window_icon().cloned() {
            builder = builder.icon(icon);
        }
        let _tray = builder.build(app)?;
        refresh_tray(app);
        Ok(())
    };
    if let Err(error) = build() {
        log::warn!("[托盘] 没装起来（{error}）—— app 照旧可用，只是没有托盘那一格");
    }
}

/// **打开 app ⇒ 本机服务在跑**（§8.1：本地进程是本地 app 的孩子）。
///
/// 为什么在**启动**这一层而不是页面那一层（2026-10-03 真机发现的缺口）：回到 app 的用户
/// 手里已经有凭据，正门走的是"直进"、连本机计划都不做 ⇒ 页面那条 `enable` 根本不会被调到，
/// 于是"世界在跑而主体不在跑"这一态没人管。这条承诺是**机器级**的，就该由 app 自己在启动时兑现。
///
/// 三条判据（都不发明）：
///
/// - **只在表过态要跑时做**（§8.6-1：没表态 ⇒ 问一次，绝不替用户起）；
/// - **世界与住户都幂等**（探活优先 / 账本 + 身份核对）；
/// - **拿不到主人会话就不做 presence 那一问**（账本仍然照判）—— 启动这一层不为了报读数
///   多铸一支会话；页面那条路（`enable`）本来就有会话，安全闸在那里生效。
///
/// ⚠ 放到别的线程里：起世界可能要等 30 秒（就绪预算），**不能冻住窗口**。
fn ensure_household_at_startup(app: &tauri::AppHandle) {
    let config_dir = MachineConfigDir(
        app.path()
            .app_config_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from(".celira-app")),
    )
    .0;
    if machine_settings::read_run_local_world(&config_dir) != Some(true) {
        log::info!("[本机服务] 还没表态要在这台机器上跑世界 —— 启动时不动作（§8.6-1 问一次）");
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let facts = local_world::LocalFacts::load(None, None);
        // **家的形状迁移**（2026-10-05 主人裁：壳自己搬）—— 排在读住户**之前**（与 `up.js` 里
        // `migrateHomeShape` 同一处语义），幂等；搬不动（老宿主在跑 / 更老的世界级键）就**如实报**，
        // **一个字都不改**，这一趟照旧开机（世界与住户照旧按现状走）。
        let shape = home_shape::migrate_home_shape(&facts.home_dir, &facts.layout.host_exe, None);
        // 把这次的**原因码**记在进程里：状态口那一格（`legacyBlocker`）读它，
        // 页面才能说准是哪一种（"停掉它下次自己搬" vs "最老那一档要走 CLI"）。
        home_shape::remember(&shape);
        if let Some(why) = &shape.refuse {
            log::warn!("[家的形状] {}：{why}", shape.action.as_str());
        }
        for line in &shape.lines {
            log::info!("[家的形状] {line}");
        }
        match local_world::ensure_world(&facts) {
            local_world::EnsureOutcome::AlreadyRunning => {
                log::info!("[本机服务] 世界本来就在跑（没动它）")
            }
            local_world::EnsureOutcome::Started { pid } => {
                log::info!("[本机服务] 世界起来了：pid {pid}")
            }
            local_world::EnsureOutcome::NoCore => {
                log::warn!(
                    "[本机服务] 缺世界可执行件：{}",
                    facts.layout.im_exe.display()
                );
                refresh_tray(&app);
                return;
            }
            local_world::EnsureOutcome::Failed { detail } => {
                log::warn!("[本机服务] 世界起不来：{detail}");
                refresh_tray(&app);
                return;
            }
        }
        for report in local_residents::ensure_residents(&facts, None) {
            let detail = match &report.outcome {
                Some(outcome) => outcome.kind(),
                None => report.state.kind(),
            };
            log::info!("[本机服务] 住户 {}：{detail}", report.label);
        }
        // **家的两道门**（页面那三格读数与机器页三间的来路 ＋ 模型配置页的读）：探活优先。
        for (what, outcome) in [
            ("状态口", home_doors::ensure_status_door(&facts)),
            ("配置门", home_doors::ensure_config_door(&facts)),
        ] {
            match outcome {
                home_doors::DoorOutcome::AlreadyServed => {
                    log::info!("[本机服务] {what}已经有人在供（没动它）")
                }
                home_doors::DoorOutcome::Started { port } => {
                    log::info!("[本机服务] {what}起来了：127.0.0.1:{port}")
                }
                home_doors::DoorOutcome::Failed { detail } => {
                    log::warn!("[本机服务] {what}没起来：{detail}")
                }
            }
        }
        refresh_tray(&app);
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    // HTTP：只在桌面三平台上注册（手机壳走不到"本地家直连"那一格，见 Cargo.toml 的注释）。
    // **文件系统（下载落盘）同一条**：落点由 capability 的作用域强制（只放开 `$DOWNLOAD/**`）。
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    let builder = builder
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init());

    builder
        // **安家**那一道闸（进程内、易失；同一支 Agent 同时只许一轮——见 `anjia::commands`）。
        .manage(anjia::commands::AnjiaGate::default())
        // 未读总数（页面报上来；壳只读 —— 见 `unread.rs` 文件头）。
        .manage(unread::UnreadTotal::default())
        // **壳 ↔ 前端的最窄接口**：安家一个命令 ＋ 本机世界三个命令 ＋ 本机服务两个命令
        // （前端按能力探测分支：`client/src/api/anjia.ts` / `shellLocal.ts`）。
        .invoke_handler(tauri::generate_handler![
            // **这一支在这台机器上的手**（终端档，2026-10-20）：页面那一格只读写
            // `agent.conf` 的 `terminal=` 一行；生效在它下一次启动（见 `agent_hands` 的文件头）。
            agent_hands::hands_readout,
            agent_hands::hands_set,
            anjia::commands::anjia_start,
            // 关于页那一行"我在跑哪一版"：号从 exe 旁边的 `VERSION` 来（源码形态退回编译期那个，
            // 并如实说 source）—— 这一格从前是前端里写死的（见 `app_version.rs` 的文件头）。
            app_version::app_version_readout,
            local_world::local_world_readout,
            local_world::local_world_enable,
            local_world::local_world_disable,
            local_world::local_world_session,
            // **给这个世界的第一个 Human 起名并铸人**（2026-10-20：名字由人给，
            // 壳不再从机器用户名造名字 —— 见 `local_world::normalize_human_name`）。
            local_world::local_world_bootstrap,
            local_services::local_services_readout,
            local_services::local_services_stop,
            local_services::local_quit_behaviour_readout,
            local_services::local_quit_behaviour_set,
            // **本机持久存储的文件后端**（前端 `api/storage.ts` 那道窄口在壳里的实现）：
            // 四个动作与窄口一一对应；读不到/写失败一律如实返回（`None`/`false`），不抛。
            local_store::local_store_read,
            local_store::local_store_write,
            local_store::local_store_remove,
            local_store::local_store_keys,
            // **原生通知**（2026-10-08）：壳里 Web 通知用不了（见 `native_notify.rs` 文件头），
            // 消息提醒走系统通知。页面侧唯一调用点是 `api/notifications.ts`。
            native_notify::notify_message,
            // **未读角标**（2026-10-08）：页面报一个总数，壳让它在窗口外看得见
            // （托盘悬停那句 ＋ 任务栏小红点／系统角标）。见 `unread.rs` 文件头。
            unread::set_unread_total
        ])
        .setup(|app| {
            // **主窗口从这里建**（`tauri.conf.json` 里那一格写着 `"create": false`）。
            //
            // 为什么不能让它自己建：Tauri 建配置窗口发生在**用户的 `setup` 之前**
            // （`tauri/src/app.rs` 的 `setup()`：先建窗、再调用户的 setup）⇒ 拿到 `app`
            // 时窗口已经在树里，**没地方再挂导航与新窗口这两个处理器**（它们是 builder 方法）。
            // 所以按**同一份配置**自己建一次：窗口的形状仍然只有 `tauri.conf.json` 一个来源，
            // 这里只多做一件事 —— 把外链接管下来（`external_links.rs`）。
            let window_config = app
                .config()
                .app
                .windows
                .iter()
                .find(|each| each.label == "main")
                .cloned()
                .ok_or("tauri.conf.json 里没有 label=main 的窗口 —— 壳没有窗口可开")?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &window_config)?
                // **壳里永远不导航走**：自己的来源放行，其余一律拦下（http/https 交给系统浏览器）。
                .on_navigation(|url| {
                    if external_links::is_app_origin(url) {
                        return true;
                    }
                    match external_links::external_target(url) {
                        Some(target) => {
                            log::info!("[外链] {target} ⇒ 交给系统浏览器");
                            if let Err(error) =
                                tauri_plugin_opener::open_url(target.as_str(), None::<&str>)
                            {
                                log::warn!("[外链] 没交给系统浏览器：{error}");
                            }
                        }
                        None => log::warn!("[外链] 拦下一个不认的地址（不是 http/https）：{url}"),
                    }
                    false
                })
                // **`target="_blank"` 那条**：wry 在没人接的时候会把它吃掉（点了没反应）——
                // 这里接过来，同样交给系统浏览器，然后明确拒绝在壳里开新窗口。
                .on_new_window(|url, _features| {
                    match external_links::external_target(&url) {
                        Some(target) => {
                            log::info!("[外链] {target} ⇒ 交给系统浏览器（新窗口那条）");
                            if let Err(error) =
                                tauri_plugin_opener::open_url(target.as_str(), None::<&str>)
                            {
                                log::warn!("[外链] 没交给系统浏览器：{error}");
                            }
                        }
                        None => log::warn!("[外链] 拦下一个不认的地址（不是 http/https）：{url}"),
                    }
                    tauri::webview::NewWindowResponse::Deny
                })
                // **麦克风那一格**（2026-10-09 主人裁：放行）——语音消息要它，将来的通话也要它。
                //
                // 为什么必须有这一处：壳里那份 WebView2 的权限处理器（wry 装的）**只放行剪贴板读**
                // （`wry-0.57.0/src/webview2/mod.rs` 的 `PermissionRequested`：`if kind ==
                // CLIPBOARD_READ { ALLOW }`，其余一律不设状态 ⇒ 走 WebView2 默认）⇒ 麦克风请求
                // **永远不答**。真机实测（2026-10-09）：壳里 `getUserMedia({audio:true})` **10 秒无应答**，
                // 而**只改浏览器参数**（自动放行 ＋ 假设备）立刻 `granted` ⇒ 管道没问题，堵点就是这一格。
                // 代价是"按了麦克风按钮界面什么都不发生"——静默失败，用户与我都看不到原因。
                //
                // 边界（三条，缺一条这一处就不该这么写）：
                // 1. **只放行麦克风**：`Camera` 一律 `Default`（= 拒）。今天只有"语音消息"这一个
                //    消费者；将来接通话的视频面时，**在那时**同一处加一行（不为想象中的扩展预建能力）；
                // 2. **页面只在用户动作时才请求**（`client/src/lib/voice.ts` 的 `start()` 由
                //    "点/按住录音"触发）⇒ 这一格放行的是"用户刚刚按下去的那一次"，
                //    不是"页面可以随时偷偷开麦"；
                // 3. **拿得到它的只有我们自己的页面**：导航只放行自己的来源、外链交给系统浏览器
                //    （上面两条 `on_navigation` / `on_new_window`）。
                //
                // ⚠ 这是**一次明确的安全放行**，所以判据写在这里、读数记在 `app/README.md` §5；
                //   谁要再放宽（比如连 Camera 一起放），得先有真实消费者与一次新的裁决。
                .on_permission_request(|_, kind| match kind {
                    tauri::webview::PermissionKind::Microphone => {
                        tauri::webview::PermissionResponse::Allow
                    }
                    _ => tauri::webview::PermissionResponse::Default,
                })
                .build()?;

            // 机器级配置目录：解析不了就退回"用户主目录下的 .celira-app"
            // （比 panic 好：一条读数读不到，不该让整个 app 起不来）。
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".celira-app"));
            app.manage(MachineConfigDir(config_dir));

            // **日志：debug 与 release 都要有**（2026-10-05 记账，第一轮 release 构建时发现）。
            //
            // 原先只有 `debug_assertions` 才装它 ⇒ **发布出去的那一颗一个字都不留**：release 是
            // GUI 子系统（没有控制台），也没有日志文件 ⇒ 用户那边出问题时，**谁也看不到发生过什么**，
            // 连我自己验 release 都没有读数可读。桌面应用的常规做法是留一份本地日志
            // （`tauri_plugin_log` 默认写 Stdout ＋ LogDir），这里两档都装、级别 Info。
            // 日志里没有凭据（token 从不进日志，见 `AGENTS.md` 的凭据纪律）。
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .build(),
            )?;
            // 托盘与退出语义（§8.2-8.3）：装不起来也不拦 app 启动。
            setup_tray(app.handle());
            // **打开 app ⇒ 本机服务在跑**（§8.1）：表过态要跑就把世界与住户都带起来（幂等）。
            ensure_household_at_startup(app.handle());
            Ok(())
        })
        // **点窗口的 × ⇒ 收起，app 还在托盘，本地进程照旧跑**（§8.2 那张表的第一行）。
        //
        // ⚠ 这是**有意的**，不是"关不掉"：本地进程的出身就是这 app，而"停"是**一次明确表态**
        // （托盘菜单那一项把后果写在名字里，§8.3）。所以这里只拦"窗口关闭"，不拦"退出"。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                // 收起之后刷新一次托盘：用户刚"关"了窗口，托盘上那一句要告诉他还有东西在跑。
                refresh_tray(window.app_handle());
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
