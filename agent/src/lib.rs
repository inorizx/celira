//! Celira Agent —— 最小骨架（阶段一）＋ 最小供应商适配层（阶段二）＋ 第一个只读文件 App
//! （阶段三）＋ 最小 OS 骨干的入口、能力展开与机械路由（阶段四/本刀）。
//!
//! 这个 crate 证明一条链路能独立成立：
//!
//! ```text
//! Agent 身份
//!   -> 一次 Run
//!   -> 一次 Turn
//!   -> 中立 View（初始输入 + 当前可用能力 + 只追加的注入/结果轨迹 + 机械资源事实）
//!   -> Model
//!   -> 中立 Action（能力标识 + 中立参数）
//!   -> 外层接缝在活动开始与每轮之前注入环境内容
//!   -> 由 OS 骨干按唯一归属校验可用性并路由 / 由 App 执行 / 由供应商适配层翻译
//!   -> 结果（类别 + 稳定失败分类 + 有界内容）
//!   -> Record（事实）/ Observation（运行内痕迹）
//!   -> Outcome
//!   -> 从 Record 恢复
//! ```
//!
//! **每个词只有一个含义**：`foundation` 只处理"什么是已经发生的事实、事实怎么追加、中断后怎么只凭事实重建"；
//! `core` 只处理"一次有限活动怎么按顺序跑完并诚实收场"；`os` 只处理窄契约的装配、发现、展开与机械路由；
//! 各 App 只处理自己那个世界（数据、动作、局部规则与失败词汇）；适配层只做协议翻译；
//! `composition` 只做构造、配置与显式接线；显示区各件只做机械投影。
//!
//! ⚠ **逐模块的事实不在本文件里**：本 crate 的代码地图（哪个文件是什么、当前接线与已知缺口）
//! 只有一处权威——`README.md` §2 的代码地图、§3 的逐条行为与 §4 的差距表；**第一版明确不做**的清单在 §6。
//! 本文件只陈述分层的形状与不变量：在这里再列一遍模块说明，就是制造第二份会过期的事实
//! （2026-10-06 之前它正是这样过期的——它把已实现的东西写成"未实现"，又漏掉了近三十个模块）。
//!
//! 分层守卫（由 `tests/minimal_agent.rs` 机械检查）：
//!
//! - 供应商语言只允许出现在 `provider_adapter` / `responses_adapter` / `model_choice`（两条线协议并排：
//!   抽象单位是**线协议**而不是供应商，两个实现各写各的报文与解析，只共用与协议无关的机械件，
//!   中间没有统一表示；"用哪一条 wire"是装配期事实，所以第三个文件是**选择**那一处）；
//! - 文件系统调用、起进程、出网、序列化各有一份**封闭名单**——谁新增一处，就得在守卫里被人看见
//!   （Core 与 Foundation 一个记号都不许碰）；
//! - Core 与 Foundation 不得引用这些边界模块（含只读诊断层），也不得引用它们的依赖；
//! - OS 骨干的发现、展开与路由只认识 `app_contract` 这份窄契约：不导入历史读取实现或 `store`，
//!   也不依赖任何 App 的内部。⚠ **这一条今天只对"发现 / 展开 / 路由"成立**：`MinimalOs::open` /
//!   `with_history` 这两个便利构造入口直接命名文件视图组件 `FileView`，`MachineFailure` 还嵌着
//!   `FileViewFailure`，`execute` 直调 `pinned_actions` / `self_concept`——**这条依赖没有守卫**，登记在
//!   `README.md` §4（"os 装了六类东西"那一行）；
//! - App 认识这份契约，但不认识 OS 骨干；**App 之间不得互相引用实现**（共用机制走中立件）。
//!   ⚠ **「App」的权威判据是「实现了 `AppPort`」**（今天九个：`file_view` ＋ im 三包 ＋
//!   `notes` / `timer` / `web` / `terminal` / `mcp`）。**一个域可以由两个模块组成**——`files`
//!   就是：机制在 `file_app`、接入面在 `file_view`；而 `file_app` **不是** App（它只实现
//!   `ActionPort`，装不进 OS，两者还同报标识 `files`）。因此 `file_view` 用它是**同一个 App
//!   内部的依赖**，不构成"App 之间互相引用"。⇒ 判据是**上面那个定义**，不是"看到 import
//!   另一个文件就算越界"（2026-10-06 写清：这条正是有人读错过的地方）；
//! - 适配层不得决定能力可用性，也不认识 OS 或 App。
//!
//! **仍未实现的**（只列第一版仍算欠债的）：聊天界面、流式输出、后台消息服务、自动调度 / 排程、
//! 多 App 安装与通用插件与动态装卸、逐活动的物理历史文件与其生命周期管理、展开状态的持久恢复。
//! **第一版明确不做**的清单（恢复自动化、检索层与索引、多 Agent / 多环境编排、加密与保留期限、
//! 用量遥测等）在 `README.md` §6——它们**不算欠债**，别再按"缺口"读。
//! 已实现但**不**承诺的部分见 `store` 的模块文档：只承诺已确认耐久的前缀，
//! 不承诺掉电零丢失、不承诺两份存储之间的跨存储原子性。

pub mod activity_origin;
pub mod app_contract;
pub mod clock;
pub mod commit;
pub mod composition;
pub mod context_reading;
pub mod core;
pub mod cost;
pub mod creation_grant;
pub mod diagnostics;
pub mod doorbell;
pub mod event_cards;
pub mod file_app;
pub mod file_view;
pub mod foundation;
pub mod handoff;
pub mod holdings;
pub mod host_loop;
pub mod im_app;
pub mod im_channel;
pub mod im_profile_app;
pub mod im_social_app;
pub mod machine;
pub mod manual_entry;
pub mod mcp;
pub mod model_choice;
pub mod model_readings;
pub mod model_retry;
pub mod model_source;
pub mod notes_app;
pub mod notification_bar;
pub mod orientation;
pub mod os;
pub mod pinned_actions;
pub mod pins_board;
pub mod platform;
pub mod previous_activity;
pub mod provider_adapter;
pub mod recovery;
pub mod resident_lifetime;
pub mod responses_adapter;
pub mod sandbox;
pub mod self_concept;
pub mod skills_desktop;
pub mod status_bar;
pub mod store;
pub mod terminal_app;
pub mod terminal_held;
pub mod timer_app;
pub mod wake;
pub mod web_app;
