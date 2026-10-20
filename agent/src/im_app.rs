//! im app：Celira IM 的**通信面**——通讯录、翻记录、说话、撤回、推进读位置、退出房间、群房治理
//! （开房 / 邀请 / 移除 / 转让）与群内呈现、房间进行态，以及**附件**的收与发。
//!
//! ⚠ **2026-10-02 拆包**：这个 App 从"整包"拆成三个**渐进式**的包——动词 id、参数表、回执文案、
//! 失败分类、wire 字段与幂等键规则**一字不改**，只换"哪个 App 拥有哪几条动词"：
//!
//! - `im`（本文件）：**预装常开**——通信面（上面那串）；
//! - [`crate::im_social_app`]（`im-social`）：**展开才上场**——关系面（联系人 / 黑名单与五个写）；
//! - [`crate::im_profile_app`]（`im-profile`）：**展开才上场**——表达面（读 / 写自己的资料与头像）。
//!
//! 三个包共用同一份通道机制（[`crate::im_channel`]），而 **App 之间没有实现依赖**：机制在中立层。
//!
//! 它仍是第三个真实 App（前两个是 files 与 notes），也是**第一条跨系统边界的通道**：对面是
//! Celira IM 服务端的**既有**端点（读面、写面、群房治理与附件收发都是它早就有的——缺的一直是
//! 这一侧的动作；权威契约见 `docs/contracts/core-v1/` 下的各份 Core v1 文档——
//! **wire 形状不由本文件定**）。
//!
//! 四条性质，一条边界：
//!
//! - **零本地存储**：事实的家在服务端，本 App 自己一个字节都不存——既不是缓存，也不是副本，
//!   所以"两份权威"这种形状在这里不可能出现。两半都要说清：**Run 内忠实保留**（读过的东西在
//!   同一个清醒期的**后续轮次里照样看得到**，有一条用例钉着它，名字里带人称代词、这里不写出来）；
//!   **跨清醒期不携带**
//!   （不持久进装配基底、不进行下一次唤醒的注入线——契约那句"用完即弃"说的正是这一层；通用侧
//!   由 `two_runs_share_one_environment_and_the_second_reads_history_on_purpose` 守着：新的一段
//!   首屏没有别处搬来的行动结果、观测条数恰好等于它自己发出的动作数）；
//! - **机械搬运**：只把端点给的字段按原样渲染（顺序照端点的顺序 = 最近活动倒序 / 序号升序），
//!   不摘要、不重排、不语义检索、不替主体判断该看哪个房间或哪条消息；预览的 60 字节截断是
//!   **呈现面的机械惯例**（契约明言服务端不截断）；
//! - **凭据与端点是宿主的声明**（[`ImAccess`]）：端点由命令行给出，凭据由环境变量给出；
//!   凭据只出现在请求头里，**不进视野、不进经历、不进事实、不进任何输出**（与模型凭据同一条
//!   边界），失败也只报"我的凭据不被接受"这一个分类，不带状态码、响应体或它的取值；
//! - **附件是渐进式的**（TASK_65）：元数据**免费**随历史来（`im_read_history` 给全
//!   id / file_name / content_type / size），**字节按需读**（`im_read_attachment`，注进主体的 Run
//!   视野：**Run 内保留、跨 Run 不携带**），**递出去要主体主动**（`im_say` 带 `file`）。白名单在**工具面**
//!   （主体读不动的类型就不伸手；媒体类只在**这一次装配用的模型**收得到图像块时才进清单，见
//!   [`IMAGE_ATTACHMENT_TYPES`]）：通道（`CORE_V1_IM_CONTRACT.md` §8）本来就不管种类也不嗅探，
//!   世界不拦——是主体的手不伸向自己看不了的东西。
//!
//! 明确**不**做（属别的刀）：SSE 门铃 / 常驻宿主；通知栏与已读水位；旧 Runtime 的 Maintenance
//! 窗口与"通讯录自动注入"（重建后不引入自动唤醒——主体要看得自己去看）；**自动存档与自动重读**
//! （存档经主体本体，是主体的动作）。附件这条通道也只到**工具面**为止：不做预览档、不为字节
//! 另存一份副本、不管 client 侧怎么呈现——媒体形态交出去的是**图像块**，不是预览图。
//!
//! ⚠ **契约本身没承诺的，这里也没有**（是边界，不是欠账）：编辑 / 删除 / 安全擦除 / 恢复撤回 /
//! 窗口延长 / 人工豁免 / 内容治理（群主代撤、举报、审查）；**写侧没有自动重试**——同键重放必须
//! 是**主体**的决定，不是系统的；凭据今天只走环境变量声明（没有钥匙串）；`next_before` 游标
//! **不跨 Run**（跨 Run 得从最新一页重新开始）。⚠ **"预装 = 常开"仍是开放问题**（主体不必点开
//! 就有动词）：将来若做，一刀统一改所有预装 App。
//!
//! ⚠ **未决效果门已退役**（`app_contract.rs` 的 `EffectClass::UnverifiableWrite`）：从前"一次
//! `unknown` 之后写类别一律被拒"，今天那次写有没有结果只由账本与开场那张纸条如实呈现——系统
//! **不自动复核、不自动重放、不自动补偿**，出路是主体自己去复核。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clock::localize_rfc3339;
use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, Material, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};
// 通道机制与共用判据在**中立层**（`im_channel`）：im 家族的三个 App 共用同一份实现，而
// App 之间不得有实现依赖 —— 因此这里取的是 `im_channel`，不是任何一个 App 的内部。
use crate::im_channel::{
    ByteReply, ImAccess, ImChannel, ImFailure, MAX_FILE_PARAM_BYTES, MAX_ID_BYTES,
    PARAM_BASE_VERSION, PARAM_FILE, Reply, canonical_uuid, idempotency_key, multipart_body,
    multipart_boundary, valid_base_version, valid_optional_text,
};
// 路径与字节的机制在**中立层**：资料 App、它的派生只读视图与 im app 共用同一份实现，而
// App 之间不得有实现依赖 —— 因此这里取的是 `sandbox`，不是任何一个 App 的内部。
use crate::sandbox::{SandboxError, read_sandbox_file, stat_sandbox_file};

/// **通讯录那一条路径**（`GET {ROOMS_PATH}`，`since` 可缺）：缺 = 无增量基线。
///
/// 单独立成一个小函数是**给门禁读的**：`scripts/check-boundary.mjs` 认这个形状
/// （`fn <小写名>(…) -> String { … }` 的体里那条 `/v1/` 字面量），而就地写 `match`
/// 会让调用点解析不出来——那时守卫**如实报未解析**，不许静默跳过。
fn rooms_path(since: Option<SystemTime>) -> String {
    match since {
        Some(at) => {
            let millis = at
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as i64)
                .unwrap_or(0);
            format!("{ROOMS_PATH}?{PARAM_SINCE}={millis}")
        }
        None => ROOMS_PATH.to_string(),
    }
}

// **历史那一页的路径**用的是既有的 `history_path`（本文件下部那个小函数，
// 翻页复核 `sweep` 与翻记录**共用同一条**）——这里不再写第二份。

/// 这个 App 对外的稳定标识（展开动作的目标就是它）。
pub const IM_APP_ID: &str = "im";

/// 通讯录动词（通讯录读取面）。
pub const CAPABILITY_ROOMS: &str = "im_rooms";
/// 翻记录动词（历史读取面）。
pub const CAPABILITY_READ_HISTORY: &str = "im_read_history";
/// **说话**动词（写面）：往自己是成员的房间发一条公共消息。
pub const CAPABILITY_SAY: &str = "im_say";
/// `im_say` 的**复核**能力（只读）：现在到底有没有那条消息。
pub const CAPABILITY_VERIFY_SAID: &str = "im_verify_said";
/// **撤回**动词（写面）：撤掉**自己刚说出的**那条公共消息。
pub const CAPABILITY_RECALL: &str = "im_recall";
/// `im_recall` 的**复核**能力（只读）：那条现在处于已撤回状态吗。
pub const CAPABILITY_VERIFY_RECALLED: &str = "im_verify_recalled";
/// **推进读位置**动词（写面，切片 M1）：告诉世界"我在这个房间读到这条了"。
pub const CAPABILITY_MARK_READ: &str = "im_mark_read";
/// **退出房间**动词（写面，切片 M2）：我不再是这间房的成员了。
pub const CAPABILITY_LEAVE_ROOM: &str = "im_leave_room";

/// **开一间群房**（2026-10-13）：开完**主体是 owner**。走 Agent 侧那条**既有**端点
/// （`POST /v1/backend/groups`——2026-09-27「owner = 创建者」那批给 Agent 的四条治理端点之一）。
///
/// ⚠ 这一格**不是新权利**：服务端与裁决早就有（原文：「**一支 Agent 也能建房、也能持有 owner**」），
/// 当年那批切片写着「**不**动客户端与执行端」⇒ 缺的一直是**这一侧的动作**。
pub const CAPABILITY_CREATE_GROUP: &str = "im_create_group";

/// **邀请一个主体进我持有的房**（owner 的邀请权；2026-09-27 主人裁决：与 Human owner 同规则）。
pub const CAPABILITY_INVITE: &str = "im_invite";

/// **移除一个成员**（2026-10-13）：群 owner 的治理权（2026-09-27 裁决原文「**移除权含 Human
/// 成员**」）。服务端那条 `DELETE /v1/backend/rooms/{room_id}/members/{participant_id}` 早就在，
/// 缺的是这一侧的动作。
pub const CAPABILITY_REMOVE_MEMBER: &str = "im_remove_member";

/// **把房主转让给另一个成员**（2026-10-13）：owner 想退出自己开的房时，`im_leave_room` 会如实回
/// `owner-transfer-required`——世界指了路，这一条就是那条路（服务端 `POST …/owner-transfer`）。
pub const CAPABILITY_TRANSFER_OWNER: &str = "im_transfer_owner";

/// **读一间房的成员清单**（2026-10-13）：主体此前的读面只有"谁说过话"（消息的 `actor`）与
/// "此刻谁在"（`im_room_live` 的在场态）——**列不出成员**。服务端那条
/// `GET /v1/backend/rooms/{room_id}/members` 早在 Agent 读面里（`CORE_V1_AGENT_READ_SURFACE.md`：
/// "requester 必须对该 Room 存在 active Membership 周期……投影与 Human 侧逐字段相同"），
/// 缺的一直是**这一侧的动作**。
pub const CAPABILITY_ROOM_MEMBERS: &str = "im_room_members";
/// **读附件**动词（只读面）：把一条消息里某个附件的字节读进**这一轮**视野。
pub const CAPABILITY_READ_ATTACHMENT: &str = "im_read_attachment";

/// **写自己在群里的呈现**动词（写面，批次四表达权第三刀）：我在某一间群房里的那一份。
///
/// 同一个我在不同群里可以是不同角色（在这里记账、在那里陪聊），这一份**归我自己**；房主也能写它
/// （治理面），但那是它的权限，不是它替我说话的理由。每（房 × 我）一条**独立版本链**。
///
/// ⚠ **覆盖边界如实登记**：今天**没有用例点名**这条动词——它只出现在能力面与效果声明里。
/// 别把"目录里有"读成"证明过"。
pub const CAPABILITY_WRITE_PRESENTATION: &str = "im_write_presentation";
/// **看房间的 live 快照**动词（只读面，批次四读取面对齐 V9）：**谁在这间房里、谁正在写**。
pub const CAPABILITY_ROOM_LIVE: &str = "im_room_live";
/// **声明"我正在这间房写"**动词（写面，2026-09-28 主体进行态）：一个**易失**信号，
/// 几秒后自己消失、被自己发出的消息取代。
///
/// 它不是"我在想 / 我在调工具 / 我的草稿长这样"——**执行过程不进 IM**（主人
/// 2026-09-28 裁决「不需要看到执行过程」）。它只说一件人也会说的事：我正在写。
pub const CAPABILITY_TYPING: &str = "im_typing";

/// 通讯录的 `since` 参数名（unix 毫秒，十进制）。
pub const PARAM_SINCE: &str = "since";
/// 翻记录的房间参数名（撤回也用它）。
pub const PARAM_ROOM: &str = "room";

/// 群名（`im_create_group` 用它）。
pub const PARAM_NAME: &str = "name";

/// 要拉进来的主体（与社交面那一个**同名**：`im_contacts` 给的身份）。
pub const PARAM_PARTICIPANT: &str = "participant";
/// 翻记录的条数参数名。
pub const PARAM_LIMIT: &str = "limit";
/// 说话的正文参数名。
pub const PARAM_BODY: &str = "body";
/// 翻记录的游标参数名（上一次返回的 `next_before`，原样回传）。
pub const PARAM_BEFORE: &str = "before";
/// 撤回的消息身份参数名（取自 `im_read_history` 给的消息身份）。
pub const PARAM_MESSAGE: &str = "message";
/// 附件身份参数名（取自 `im_read_history` 给每条消息的附件身份）。
pub const PARAM_ATTACHMENT: &str = "attachment";

/// 说话时**要引用的消息身份**参数名（取自 `im_read_history` 给的消息身份）。
///
/// ⚠ 工具面参数名（`reply_to_id`）与 wire 字段名（`reply_to_message_id`）**不同名**：
/// 参数名是主体看到的那个，wire 名只在连接处出现（同 `message` 与 `message_id` 的先例）。
pub const PARAM_REPLY_TO: &str = "reply_to_id";
/// 说话时**要@的成员身份**参数名（取自 `im_read_history` 的 `mention` 行或 `actor` 行）。
///
/// 取值是**逗号分隔**的一段文本：参数容器是"有序的具名标量"（`ActionParams`），模型侧
/// 的 `arguments` 也只收标量（对象与数组一律拒）——列表在这里必须落成可机械解析的文本。
pub const PARAM_MENTIONS: &str = "mentions";
/// 一条消息能@的成员数上限（契约 `CORE_V1_MENTION_TARGETS.md` §2：至多 64 个目标）。
///
/// **公开常量**：门禁用例拿它构造"刚好超限"的输入，不各自抄数字（抄的数字迟早与实现脱钩）。
pub const MAX_MENTION_TARGETS: usize = 64;

/// 读附件的**续读位置**参数名：从第几行开始（从 1 起；缺省 1）。
pub const PARAM_ATTACHMENT_OFFSET: &str = "offset";
/// 读附件时**可选**带上的附件文件名（取自 `im_read_history` 的附件清单）。
///
/// 它只是**引用回执**里的一条路条：工具**不校验**它是不是真的（名字不是身份，身份是 `attachment`），
/// 服务端也不在下载响应里给文件名（§8 的响应头是固定值）。带了就原样写回回执，方便主体下一步认人。
pub const PARAM_FILE_NAME: &str = "file_name";

/// 房呈现三个字段的参数名（契约：显示名 / 角色名 / 角色描述）。
pub const PARAM_ROOM_DISPLAY_NAME: &str = "room_display_name";
pub const PARAM_ROOM_ROLE_NAME: &str = "room_role_name";
pub const PARAM_ROOM_ROLE_DESCRIPTION: &str = "room_role_description";

/// 预览渲染的字节上限（**呈现面惯例**：契约明言服务端不截断，截断是这一侧的机械动作）。
pub const MAX_PREVIEW_BYTES: usize = 60;

/// 翻记录的条数上限：**严于路由面**（路由允许 1–100）。工具面先拒绝，因此非法值一次都不出网。
pub const MAX_HISTORY_LIMIT: u32 = 50;
/// 翻记录的条数缺省值。
pub const DEFAULT_HISTORY_LIMIT: u32 = 50;

/// 正文的字节上限（契约：16384 UTF-8 字节；超限**拒绝不截断**）。
const MAX_BODY_BYTES: usize = 16 * 1024;

/// 复核时**每一页**看多少条记录（**窗口是有限的，这一点必须说出来**）。
const VERIFY_WINDOW: u32 = MAX_HISTORY_LIMIT;
/// 复核最多往回翻几页（**有界读**，不是全量扫描）。
///
/// 为什么要有上界：复核要回答的是"**确定没有**"，而"没看见"绝不能直接说成"没有"——要么翻到房的
/// 起点（那才是真的没有），要么**如实说不知道**。而上界本身必须是**声明过的**：触顶不是失败，是
/// "没看到底"，回执里的 `more = older` 会把这件事说出来（照 §3.13 那把有界子串查找刀的
/// `MAX_SCANNED` 先例：同一层里另一个"有上界、触顶如实说"的读）。
///
/// 它**不**改变任何写动作的请求次数：写仍恰好一步，只有复核会走多页。
pub const MAX_VERIFY_PAGES: usize = 5;

/// 一次读附件的窗口上限：行数、字节数与单行字符数。
///
/// 这三个数**不是任选值**：三家独立收敛到同一组（2000 行 / 50 KB / 单行 2000 字符），
/// 见 `outputs/AGENT_FILE_CONTEXT_LIFECYCLE_SURVEY.md` §4。附件是**稳定可寻址的文件类**
/// （身份挂在消息上、随时可重取），所以超限的正确形态是**响亮截断 + 一个能继续读的位置**，
/// 而不是拒绝、更不是静默丢：回执里明写「共 N 行、已读 A–B、用 offset 继续」。
pub const ATTACHMENT_WINDOW_LINES: usize = 2000;
/// 窗口的字节上限（50 KB）。
pub const ATTACHMENT_WINDOW_BYTES: usize = 50 * 1024;
/// 单行字符上限：超长行**截断并标注**（那一行本身不能再 offset——只能看到它的开头）。
pub const ATTACHMENT_LINE_CHARS: usize = 2000;

/// **IM 通道的附件上限**（`CORE_V1_IM_CONTRACT.md` §8：文件 ≤ 16 MiB，超出 413）。
///
/// 它是**通道的事实**，所以只在这里写一遍：读入上限与递出上限都从它派生——两处各写一个
/// 数字，迟早一处跟着通道走、另一处不跟。
const CHANNEL_ATTACHMENT_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// 一次最多从通道拉多少字节。
///
/// 附件比这更大就根本不在这个通道里（`CORE_V1_IM_CONTRACT.md` §8 的上传上限是 16 MiB），
/// 那时明确报 `too-large`——**不是**截断：连一份完整的字节都没拿到，谈不上"读到哪一段"。
const MAX_ATTACHMENT_FETCH_BYTES: usize = CHANNEL_ATTACHMENT_LIMIT_BYTES;

/// 一次递出去的文件字节上限：**就是通道的上限**（契约 §8）。
///
/// ⚠️ 它从前等于**读入窗口**（50 KiB），依据是"读得到才递得出去"（2026-10-11 主人裁决改掉）。
/// 那条对称是错的：**读**是字节进主体的上下文（主体确实读不了像素），**递**是字节进世界
/// ——**递不需要理解**。主流 IM 的发送侧一律如此：**不限类型，只限通道的大小**。
pub const MAX_SAY_FILE_BYTES: usize = CHANNEL_ATTACHMENT_LIMIT_BYTES;

/// 递出去的文件名上限（契约：file_name UTF-8 1–255 bytes）。
const MAX_FILE_NAME_BYTES: usize = 255;

/// 消息视图里附件清单的字段名（外部冻结形状；只在**连接处**出现）。
const ATTACHMENTS_FIELD: &str = "attachments";
/// 附件清单里那几个字段名（外部冻结形状；只在**连接处**出现）。
const ATTACHMENT_ID_FIELD: &str = "id";
const ATTACHMENT_NAME_FIELD: &str = "file_name";
const ATTACHMENT_TYPE_FIELD: &str = "content_type";
const ATTACHMENT_SIZE_FIELD: &str = "size_bytes";
/// 消息视图里"这条回复了哪一条"与"点名了谁"的字段名（外部冻结形状；只在**连接处**出现）。
///
/// 两者都由 `CORE_V1_IM_CONTRACT.md` §5 保证**恒存在**（未回复为 `null`、无点名为空集合），
/// 因此缺了它们属于**翻译层漏掉**，不是世界没给（TASK_67 审计表 #1 / #2）。
const REPLY_TO_FIELD: &str = "reply_to_message_id";
const MENTIONS_FIELD: &str = "mentions";

/// **主体读得动的附件类型**（工具面的封闭白名单）。
///
/// ⚠️ 这是一条**能力边界**，不是通道规则：`CORE_V1_IM_CONTRACT.md` §8 明文「无 MIME 白名单、
/// 不嗅探」，人类那条路照旧什么都能传。主体看不了像素，所以主体的手不伸向自己看不了的东西——
/// 判据落在这一层，IM 服务端一个字节都不改。
pub const TEXT_ATTACHMENT_TYPES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/csv",
    "text/tab-separated-values",
    "text/yaml",
    "application/json",
    "application/x-ndjson",
    "application/toml",
];

/// **主体直接感知得动的媒体类型**（2026-10-17）：只在**这一次装配用的模型收得到图像块**时
/// 才进可读清单。
///
/// ⚠ 与 [`TEXT_ATTACHMENT_TYPES`] **分开摆**是有判据的：那一份是"读成文本"，这一份是"把字节
/// 折进这一次请求"——两条路的**回执形状不同**（文本有 `lines / read / more`，媒体没有"行"
/// 这回事）。两者合起来才是这个实例今天的可读清单，而**清单里有哪一半由模型事实决定**。
pub const IMAGE_ATTACHMENT_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

/// 递出去的文件按**扩展名**定类型；**认不出来就给 `application/octet-stream`**。
///
/// ⚠️ 它从前**认不出就拒收**（"清单外的类型不递"）——2026-10-11 按主人裁决改掉：那是把
/// **读得动读不动**当成了**递得出去递不出去**。主流 IM 的发送侧是：报得出的类型就报，
/// 报不出就是 `application/octet-stream`；**通道那边本来就不判类型**（契约 §8「无 MIME
/// 白名单、不嗅探」），接收端能渲染就渲染、不能就给文件卡。
///
/// **仍然不嗅探**：只看扩展名，绝不读内容猜类型（与通道同一条纪律）。代码类文件按
/// `text/plain` 报：它们是文本，收的机器读得动。
///
/// ⚠ **`.note` 是刻意不映射的一种**：它是笔记 App 的私有容器（`pinned` / `text` / `title`
/// 那三格的私有格式），**不**设自有类型——给它命名等于把主体的私有包装当成一种对外可呈现的
/// 格式放进房间。于是它按"认不出"走 `application/octet-stream`：
/// **递得出去**（通道不判类型），读侧却**读不动**（那一侧的白名单只收文本类与媒体类）
/// ——"递得出去 ≠ 我读得动"在它身上是同一件事的两半。
fn upload_content_type(file_name: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, tail)| tail).unwrap_or("");
    match extension {
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "yaml" | "yml" => "text/yaml",
        "json" => "application/json",
        "ndjson" | "jsonl" => "application/x-ndjson",
        "toml" => "application/toml",
        "rs" | "py" | "ts" | "tsx" | "js" | "mjs" | "sh" | "sql" | "html" | "css" | "xml" | "c"
        | "h" | "java" | "go" | "rb" | "lua" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        // 认不出来不是"这件东西我不递"，是"我不知道它是什么"——如实报
        // `application/octet-stream`。
        _ => "application/octet-stream",
    }
}

/// 类型的**本质**：切掉 `;` 之后的参数、去空白、转小写。
///
/// 两处都要它（判"读不读得动"、判"是不是媒体形态"），因此只有这一份实现。
fn content_type_essence(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// 服务端回的 `content_type` 是不是**这个实例**读得动的类型。
///
/// 比对的是**白名单逐字**（前缀放行会悄悄扩大 Agent 的能力面）。
///
/// `vision` 是**这一次装配用的模型**的事实（见 [`ImApp`] 的字段文档）：它决定清单里有没有媒体类。
/// **不嗅探**：只看服务端声明的类型，绝不读内容猜（与通道同一条纪律）。
fn is_readable_content_type(content_type: &str, vision: bool) -> bool {
    let essence = content_type_essence(content_type);
    TEXT_ATTACHMENT_TYPES.contains(&essence.as_str())
        || (vision && IMAGE_ATTACHMENT_TYPES.contains(&essence.as_str()))
}

/// **事前门的机械上限**：一张图最多这么多像素（宽 × 高）。
///
/// 为什么是**面积**，不是字节、也不是边：
///
/// - **字节是存储量**——一张高度压缩的大图字节很小、像素很多。拿它当事前门就是错的物理量
///   （契约 §8 的上传回执也只有 `size_bytes`，那一格本来就不是给这件事用的）；
/// - **边是形状**——它会把"很宽很矮"与"很大很方"混成一格；
/// - **面积才是那一轮要付的东西**：上下文成本随**采样量**走，而采样量随面积走——主流就是
///   **按面积折算上下文占用**（外部对照与出处见提案
///   `outputs/AGENT_MATERIAL_PERCEPTION_RULING_DRAFT_2026-10-17.md`；本层不引厂商名）。
///
/// ⚠ 它是**我们**的资源界限，**不是**"供应商收得下多少"（那是他们的事，而且会变——P7）。
/// 量级参照：`8000 × 8000`（主流单图上限那一档）。
const MAX_MEDIA_PIXELS: u64 = 64 * 1024 * 1024;

/// **感知体量**：一张图有多少像素。**从文件头读，不解码像素。**
///
/// 认不出（不是这四种、头被截断、形状不对）⇒ `None`。⚠ **"读不出来"与"很大"是两件事**：
/// 读不出尺寸**不拒绝**（我们还有字节上限那道门），但回执必须如实写 `pixels = unknown`，
/// 不冒充一个数。
///
/// ⚠ 它今天只有**一个消费者**（im 附件面）。资料面同形时该提到中立层去（与 `sandbox` 那次
/// 提取同一条纪律：两份还不值得一层抽象，第三份出现时再收）。
fn image_pixels(content_type: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    match content_type {
        "image/png" => png_pixels(bytes),
        "image/jpeg" => jpeg_pixels(bytes),
        "image/webp" => webp_pixels(bytes),
        "image/gif" => gif_pixels(bytes),
        _ => None,
    }
}

/// 大端 2 / 4 字节；长度不够就是"读不出来"。
fn be16(raw: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes(raw.try_into().ok()?))
}

fn be32(raw: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(raw.try_into().ok()?))
}

fn le16(raw: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(raw.try_into().ok()?))
}

fn le24(raw: &[u8]) -> Option<u32> {
    let three: [u8; 3] = raw.try_into().ok()?;
    Some(u32::from(three[0]) | u32::from(three[1]) << 8 | u32::from(three[2]) << 16)
}

/// PNG：8 字节签名 ＋ `IHDR`（宽 4 字节大端、高 4 字节大端，各在固定偏移）。
fn png_pixels(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some((be32(&bytes[16..20])?, be32(&bytes[20..24])?))
}

/// GIF：`GIF87a` / `GIF89a` ＋ 宽 2 字节小端 ＋ 高 2 字节小端。
fn gif_pixels(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 || (&bytes[..6] != b"GIF87a" && &bytes[..6] != b"GIF89a") {
        return None;
    }
    Some((
        u32::from(le16(&bytes[6..8])?),
        u32::from(le16(&bytes[8..10])?),
    ))
}

/// JPEG：沿段表找 `SOFn`，宽高在它载荷的头几格里（高在前、宽在后，各 2 字节大端）。
///
/// `0xFFC0`–`0xFFCF` 里 `C4`（霍夫曼表）/ `C8`（保留）/ `CC`（算术编码表）**不是** SOF。
fn jpeg_pixels(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }
    let mut at = 2;
    while at + 9 <= bytes.len() {
        if bytes[at] != 0xff {
            return None;
        }
        let marker = bytes[at + 1];
        // 填充（连续的 0xFF）与无载荷的标记：往前走一格 / 两格。
        if marker == 0xff {
            at += 1;
            continue;
        }
        if marker == 0x01 || (0xd0..=0xd9).contains(&marker) {
            at += 2;
            continue;
        }
        let length = usize::from(be16(&bytes[at + 2..at + 4])?);
        if length < 2 {
            return None;
        }
        let is_sof = (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if is_sof {
            let height = u32::from(be16(&bytes[at + 5..at + 7])?);
            let width = u32::from(be16(&bytes[at + 7..at + 9])?);
            return Some((width, height));
        }
        at += 2 + length;
    }
    None
}

/// WebP：`RIFF….WEBP` 之后有三种块，宽高各在**不同的位置**。
fn webp_pixels(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 16 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        // 有损：`VP8 ` ＋ 块长 ＋ 帧标签(3) ＋ 起始码(3) ＋ 宽(2, 低 14 位) ＋ 高(2, 低 14 位)。
        b"VP8 " => {
            if bytes.len() < 30 {
                return None;
            }
            let width = u32::from(le16(&bytes[26..28])?) & 0x3fff;
            let height = u32::from(le16(&bytes[28..30])?) & 0x3fff;
            Some((width, height))
        }
        // 无损：`VP8L` ＋ 块长 ＋ 签名 `0x2f` ＋ 4 字节里塞着 14 位宽与 14 位高（都要加一）。
        b"VP8L" => {
            if bytes.len() < 25 || bytes[20] != 0x2f {
                return None;
            }
            let bits = u32::from_le_bytes(bytes[21..25].try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        // 扩展：`VP8X` ＋ 块长 ＋ 4 字节标志 ＋ 宽减一(3, 小端) ＋ 高减一(3, 小端)。
        b"VP8X" => {
            if bytes.len() < 30 {
                return None;
            }
            Some((le24(&bytes[24..27])? + 1, le24(&bytes[27..30])? + 1))
        }
        _ => None,
    }
}

/// 通讯录路径（纯读）。
const ME_PATH: &str = "/v1/backend/me";
const ROOMS_PATH: &str = "/v1/backend/rooms";
/// 房内记录路径前缀（纯读）。
const ROOM_HISTORY_PREFIX: &str = "/v1/backend/rooms/";
/// 房内记录路径后缀。
const ROOM_HISTORY_SUFFIX: &str = "/messages";
/// 退房路径后缀（写：我不再是这间房的成员了，切片 M2）。
const LEAVE_SUFFIX: &str = "/leave";

/// 开群房那条 Agent 侧端点（不带房间标识）。
const GROUPS_PATH: &str = "/v1/backend/groups";

/// 成员那条后缀（邀请用；移除是同一条再加主体标识）。
const MEMBERS_SUFFIX: &str = "/members";

/// 转让房主那条后缀（与 Human 侧同形）。
const TRANSFER_SUFFIX: &str = "/owner-transfer";

/// 邀请请求体里那个字段名（**线协议的名字**；主体看到的参数名是 `participant`）。
const PARTICIPANT_ID_FIELD: &str = "participant_id";

/// 群名上限（体上限 4 KiB 之内的一个稳妥值：单行、非空）。
const MAX_NAME_BYTES: usize = 256;

/// 主体标识上限（服务端的标识是 UUID 形状；给足余量，只为挡住病态输入）。
///
/// ⚠ 它**不是** [`MAX_ID_BYTES`]（标识那一族的 64）：参与者这一格刻意更宽，而关系面
/// （[`crate::im_social_app`]）那一处用自己的判断、取的是族界 ⇒ **同一类参数今天有两个数**，
/// 且**不合并**（App 之间不得有实现依赖）。契约 `CORE_V1_AGENT_SPEAK.md` §8 那句"房间 /
/// 参与者 / 消息目标共用"读的是**族名**，不是逐参数的现状——按契约冻结纪律，那一句不改。
const MAX_PARTICIPANT_BYTES: usize = 128;
/// 读游标路径后缀（写：推进我在这个房间的读位置，切片 M1）。
const READ_CURSOR_SUFFIX: &str = "/read-cursor";

/// 我在某一间群房里的呈现路径（写；`{}` 是房间标识）。
const MY_PRESENTATION_PATH: &str = "/v1/backend/rooms/{}/my-presentation";
/// 某间房的 live 快照路径（只读；`{}` 是房间标识）。
const ROOM_LIVE_PATH: &str = "/v1/backend/rooms/{}/live";
/// 某间房的 typing 声明路径（写面、无参数、204；`{}` 是房间标识）。
const ROOM_TYPING_PATH: &str = "/v1/backend/rooms/{}/typing";

/// 这个 App 的通道面。
pub struct ImApp {
    /// 到 im 服务端的通道（端点 + 凭据 + 自己的请求客户端）。
    channel: ImChannel,
    /// 自己的世界观根（沙盒）：`im_say` 的 `file` 从它读字节。
    ///
    /// `None` = 宿主没有声明这个世界 ⇒ 那只手不在（给了 `file` 也读不出来，回执如实报
    /// `file-unreadable`）。这一格**不是权限**：公共写权限在世界那一侧由 Membership 判。
    world_root: Option<PathBuf>,
    /// **Agent 在 im 里的身份**（`GET /v1/backend/me` 的 `agent.id`）——**一次活动只探一次**。
    ///
    /// TASK_71 桌面 P0 第 4 件引入：读面回执要在开头给主体一行 `me = …`，复核归因也要用它。
    /// 缓存放这里成立，是因为这个实例由 `assemble_machine` **每次活动构造一次**（活动结束就散），
    /// 于是"一次活动一次探针"是**结构**给的，不是靠清缓存：下一次活动是新的实例、新的一次探针
    /// （凭据可能在两次活动之间被换掉——那一份是宿主的，不是这里的）。
    ///
    /// 成功与失败**都**缓存：一次活动里只问一次，不因一次抖动把整段活动变成反复打点
    /// （与 `RunExpansion.pinned` 那次读盘的读法同款）。
    me: std::sync::OnceLock<Result<String, ImFailure>>,
    /// **这一次装配用的模型收不收得到媒体材料**（2026-10-17）。
    ///
    /// 它只决定一件事：**读附件那件动词的可读类型清单里有没有媒体类**。`false`（缺省，
    /// fail-closed）= 与从前逐字节相同——清单里只有文本类，图片报 `unsupported-content-type`。
    ///
    /// ⚠ **它不是权限**，也不是"有没有视觉"这句话本身：它是**这台机器配的那个模型**的事实
    /// （`SessionDeclarations::vision`，来自模型配置门的裁决）。宿主不声明就是不支持。
    vision: bool,
}

impl ImApp {
    /// 装配一个 im app（请求客户端在这里建一次，含单次请求的墙钟上限）。
    pub fn new(access: ImAccess) -> Option<Self> {
        Some(Self {
            channel: ImChannel::new(access)?,
            world_root: None,
            me: std::sync::OnceLock::new(),
            vision: false,
        })
    }

    /// 声明**这一次装配用的模型收不收得到媒体材料**（见字段文档）。
    ///
    /// 链式而不是构造参数，与 [`Self::with_world_root`] 同款：既有的装配点不必都知道它，
    /// 而**缺省（不调用）就是 fail-closed 那一档**——能力面的**名字与参数形状不随它变**，
    /// 变的只有"哪些类型读得动"那一行清单（恢复承重）。
    pub fn with_vision(mut self, vision: bool) -> Self {
        self.vision = vision;
        self
    }

    /// 声明自己的世界观根（与 `files` / `notes` **同一个根**）。
    ///
    /// 链式而不是构造参数：既有的装配点（门铃回调那条只读路）不必都知道这个世界；
    /// 而**能力面不随它变**——工具名与参数形状在任何装配下都一样（恢复承重）。
    pub fn with_world_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.world_root = Some(root.into());
        self
    }

    /// 本 App 声明的全部能力（顺序固定）：通信面——通讯录 / 翻记录 / 读附件 / 说话与它的复核 /
    /// 撤回与它的复核 / 推进读位置 / 退房 / 房间进行态 / 成员清单；群房治理四条（开房 / 邀请 /
    /// 移除 / 转让房主）；另两条写：`im_typing`（第四档、免复核）与**群内呈现**
    /// （复核面是通讯录——见 `declared_effects`）。**没被声明的动作是只读**（不声明 = 只读，
    /// 默认方向通向更少的能力），因此 `im_read_attachment` 与各次复核一样：不需要任何授权。
    ///
    /// ⚠ 社交与资料两组动词住在各自的新包里（`im-social` / `im-profile`）——它们**不预装**，
    /// 展开才上场；三个包共用一份通道机制（`im_channel`）。
    fn own_capabilities(&self) -> Capabilities {
        // **可读清单里有哪一半由模型事实决定**（2026-10-17）：这个模型收不到图像块时，清单里
        // 就没有媒体类——工具面因此不摆出「读得动图片」这句话（与从前逐字节相同）。
        let readable = if self.vision {
            [TEXT_ATTACHMENT_TYPES, IMAGE_ATTACHMENT_TYPES]
                .concat()
                .join(" / ")
        } else {
            TEXT_ATTACHMENT_TYPES.join(" / ")
        };
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_ROOMS),
                "列出我参与的房间：每个房间给标识、种类、显示名、我读到哪儿与还欠几条、\
                 最近一条预览、最近活动时间与自 `since` 以来的条数",
                vec![ParamSpec::new(
                    PARAM_SINCE,
                    false,
                    "只数这个时点之后的条数（unix 毫秒十进制）；不给就是不看增量",
                )],
            )
            .with_details(
                "读数形状：每房一行——标识、种类、显示名、我在这个房间的未读（`unread = n`）\
                 与读到的位置（`read = <消息标识>`）、最近一条的预览（最多 60 字节，超出即截断）、\
                 最近活动时间、自 `since` 以来的条数（含 0）。顺序由服务端给出（最近活动倒序）。\
                 只读：不改变任何东西。房间标识是 `im_read_history` / `im_say` 要用的那个。\
                 `unread` 与 `read` 是世界记得的那个位置（服务端按成员周期记着，跨你的 Run 活着）——\
                 `unread = 0` 是「这间房没有你还没看的」；`read = -` 是「你还没读过这间房的任何一条」；\
                 `?` 是读不到（这一版世界没给这一栏，或形状读不懂）——读不到不等于零。\
                 回执首行是你的身份（`me = …`：一次活动只问服务器一次；问不到就写 `unknown`，\
                 不拿本机身份或显示名顶替）——读到 `actor` 时用它分自己与别人。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_READ_HISTORY),
                format!(
                    "读一个房间的历史记录（从最新一页往回翻，最多 {MAX_HISTORY_LIMIT} 条）"
                ),
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_LIMIT,
                        false,
                        format!("条数 1–{MAX_HISTORY_LIMIT}，缺省 {DEFAULT_HISTORY_LIMIT}"),
                    ),
                    ParamSpec::new(
                        PARAM_BEFORE,
                        false,
                        "上一次回执给出的游标（原样回传，用于翻更旧的一页）",
                    ),
                ],
            )
            .with_details(format!(
                "顺序：按发生顺序（最新一页往回翻）。每条给：消息身份、发出者、时间与正文；\
                 撤回如实标出；有附件时逐条给附件清单（身份、文件名、类型、字节数——\
                 `im_read_attachment` 要用的正是那个身份）；有提及时给计数。分页：条数 \
                 {MAX_HISTORY_LIMIT} 以内（缺省\
                 {DEFAULT_HISTORY_LIMIT}）；要更旧的页，把上一次回执末尾的游标原样放回 `{PARAM_BEFORE}`\
                 ——它是不透明字符串，不解析、不猜、不构造。只读：不改变任何东西。\
                 回执首行是你的身份（`me = …`：一次活动只问服务器一次；问不到就写 `unknown`，\
                 不拿本机身份或显示名顶替）——「这条是不是我说的」看发出者那一格对不对得上它。\
                 ⚠️ 它顺带推进「我读到哪」（2026-10-10 裁决）：读完之后，世界会记下你读到这一页\
                 最新的那一条（与「打开会话」同形；`before=` 回翻不会把位置往回推——读位置是\
                 单调的）。回执末尾那一行 `read-cursor = …` 如实报结果：`advanced to <id>` / \
                 `unchanged` / `unknown` / `没推上去（<分类>）`——读到了 ≠ 世界知道你读到哪，\
                 两者分开说。",
            )),
            Capability::new(
                ActionId::new(CAPABILITY_READ_ATTACHMENT),
                format!(
                    "读一条消息里某个附件的文本（一次最多 {ATTACHMENT_WINDOW_LINES} 行 / \
                     {ATTACHMENT_WINDOW_BYTES} 字节；可分段续读）"
                ),
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_ATTACHMENT,
                        true,
                        "附件身份（取自 im_read_history 给的那条消息的附件清单）",
                    ),
                    ParamSpec::new(
                        PARAM_ATTACHMENT_OFFSET,
                        false,
                        "从第几行开始（从 1 起，缺省 1）；把上一次回执的 more 里那个数放回来",
                    ),
                    ParamSpec::new(
                        PARAM_FILE_NAME,
                        false,
                        "可选：这个附件的文件名（原样写回回执，方便你下一步认人；不校验）",
                    ),
                ],
            )
            .with_details(format!(
                "只读：把这条消息里某个附件的文本读进这一轮（走的是既有那条注入路）。\
                 看得到的类型：{}（其余类型看不了：服务端怎么存都照样拒绝，\
                 `unsupported-content-type`）。媒体形态（模型收得到图像块时清单里才有媒体类）：\
                 整份给、没有「行」这回事（对它给 `{PARAM_ATTACHMENT_OFFSET}` 是 `invalid-argument`）；\
                 回执另给一行 `pixels = 宽x高`（从文件头读、不解码像素；读不出来写 `unknown`，\
                 那一档不拒绝），而面积超过 {MAX_MEDIA_PIXELS} 像素的当场拒绝\
                 （`too-large`，读数写 `refused = perception-limit`——它与「通道字节超限」不是\
                 同一道门：那一道是连完整字节都没拿到）。分段读：一次最多 {ATTACHMENT_WINDOW_LINES} 行 / \
                 {ATTACHMENT_WINDOW_BYTES} 字节，单行超过 {ATTACHMENT_LINE_CHARS} 字符就截断并在那一行\
                 末尾标注它本来多长；回执的第二行给 `lines = 共多少行, read = 已读哪几行, \
                 more = offset 下一段`——用那个 offset 接着读（截断不是丢：附件随时可按身份重取）。\
                 `{}` 缺省从第 1 行开始。引用回执：回执里写着附件身份、文件名（你带来的那个，\
                 工具不校验它）与已读范围——读到的内容留在这一次清醒期里（后面的轮次照样看得到）；\
                 跨 Run不自动带过去，要用就照这个身份再读。撤回联动：那条消息被撤回后，\
                 它的附件同样读不到（与服务端同一答案）。只读：不改变任何东西。",
                readable,
                PARAM_ATTACHMENT_OFFSET,
            )),
            Capability::new(
                ActionId::new(CAPABILITY_SAY),
                "往我参与的房间里发一条公共消息（房里所有人都看得到）；可以引用某一条、\
                 点名成员、附上一份文件（什么类型都递得出去；我读不读得动它是另一件事）",
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_BODY,
                        false,
                        format!(
                            "消息正文（≤{MAX_BODY_BYTES} 字节、不含 NUL）；与 file 至少给一个"
                        ),
                    ),
                    ParamSpec::new(
                        PARAM_FILE,
                        false,
                        format!(
                            "要递出去的文件（我世界里的相对路径，≤{MAX_SAY_FILE_BYTES} 字节；\
                             什么类型都递得出去）"
                        ),
                    ),
                    ParamSpec::new(
                        PARAM_REPLY_TO,
                        false,
                        "要引用的消息身份（取自 im_read_history；不给就是普通消息）",
                    ),
                    ParamSpec::new(
                        PARAM_MENTIONS,
                        false,
                        format!(
                            "要@的成员身份（取自 im_read_history；逗号分隔、最多 {MAX_MENTION_TARGETS} 个；只在群房里能被接受）"
                        ),
                    ),
                ],
            )
            .with_details(format!(
                "写动作：它会真的落进那个房间，房内所有成员（人和其他 agent）都看得到。参数边界：\
                 正文与 `file` 至少给一个；正文最多 {MAX_BODY_BYTES} 字节（超限拒绝、不截断）；\
                 房间标识最多 {MAX_ID_BYTES} 字节。带 `file` 时：读那个文件的字节 → 上传 → 随消息\
                 绑定（两步）；类型按扩展名定，认不出的报 `application/octet-stream`\
                 （不嗅探：绝不读内容猜类型）；不限类型——上限 {MAX_SAY_FILE_BYTES} 字节就是\
                 IM 通道的上限（契约 §8）。⚠ 递得出去 ≠ 我读得动：`im_read_attachment`\
                 那一侧只收文本类，那是我读附件那一侧的形状，不是通道的规则。文件读不到或太大就\
                 明确拒绝，一个字节都不发。\n\
                 引用一条（可选 `{PARAM_REPLY_TO}`）：取 `im_read_history` 给的消息身份——那条必须是\
                 这间房里、对你看得见的已提交消息（可以回自己）；已撤回的不能当引用对象。\n\
                 点名成员（可选 `{PARAM_MENTIONS}`）：逗号分隔的成员身份（取自 `im_read_history` 的\
                 `mention` 行或 `actor` 行），最多 {MAX_MENTION_TARGETS} 个、互不相同；只在群房里\
                 能被接受。这两样给了就必须合规——非法一律本地拒绝，一个请求都不发。\n\
                 三态：已提交 / 被明确拒绝 / 提交结果未确认（最后一种不会自动重发；要确认就用\
                 `im_verify_said`，参数与这次完全相同——带 `file` 的那次也要带同一个 `file`）。\n\
                 失败：引用对象不能被接受（已撤回 / 看不到 / 不在本房）⇒ `not-a-reply-target`；\
                 提及目标不能被接受（不在群里 / 已离开）⇒ `not-a-mention-target`——两者都不分解\
                 （服务端把各自的多种原因合成同一个码，不泄露原因）；两类同场、或与 `file` 同场时\
                 世界拒绝无法归因 ⇒ `invalid-argument`（不猜）。",
            )),
            Capability::new(
                ActionId::new(CAPABILITY_VERIFY_SAID),
                format!(
                    "复核「我说过的那条现在在不在」：按身份在最近 {VERIFY_WINDOW} 条里找（只读）"
                ),
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（与那次 im_say 相同）"),
                    ParamSpec::new(PARAM_BODY, false, "正文（与那次 im_say 完全相同）"),
                    ParamSpec::new(
                        PARAM_FILE,
                        false,
                        "文件路径（与那次 im_say 完全相同；那次带了它就是必给）",
                    ),
                    ParamSpec::new(
                        PARAM_REPLY_TO,
                        false,
                        "回复对象（与那次 im_say 完全相同；那次带了它就是必给）",
                    ),
                    ParamSpec::new(
                        PARAM_MENTIONS,
                        false,
                        "提及集合（与那次 im_say 完全相同；那次带了它就是必给）",
                    ),
                ],
            )
            .with_details(format!(
                "它怎么判：先问服务器「这个凭据代表谁」，再在最近 {VERIFY_WINDOW} 条记录里找一条\
                 那个身份发的、与这次要复核的那条同形的消息（按身份判定，不按名字）。\
                 同形有四个维度：① 正文完全相同；② 附件——那次带了 `file` 的，要求那条的\
                 附件按文件名与字节数与这个文件对得上；那次没带 `file` 的，要求那条没有附件；\
                 ③ 回复对象——那次带了 `{PARAM_REPLY_TO}` 的，要求那条引用的就是同一条；\
                 那次没带的，要求那条不是回复；④ 提及集合——那次带了 `{PARAM_MENTIONS}` 的，\
                 要求那条点名的集合完全相同；那次没带的，要求那条没点名（少了后三条，\
                 一条带附件 / 带回复 / 带@的旧消息会被当成「这句话我说过了」——那是误判）。\
                 ⚠️ 三处弱，必须知道：① 弱在归因（同正文多条时无法指认是哪一次）；\
                 ② 弱在窗口（更旧的记录看不到，回执里的 `searched` 会告诉你看了多少条）；\
                 ③ 弱在附件判据（比的是文件名与字节数、不是内容：同名同大小而内容改过的\
                 两份文件在这一层分不开）。文件现在读不到时附件这一格不可比，回执如实报\
                 `unknown`（不当作「没找到」）。参数与 `im_say` 完全相同；它只读、\
                 不改变任何东西。",
            )),
            Capability::new(
                ActionId::new(CAPABILITY_RECALL),
                "撤回我刚说出的那条消息（只撤自己发的；有 120 秒窗口）".to_string(),
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_MESSAGE,
                        true,
                        "要撤回的消息身份（取自 im_read_history 给的消息身份）",
                    ),
                ],
            )
            .with_details(
                "只撤自己发的：别人的撤不了，服务端也会拒。写动作：一次调用只发一次请求，\
                 三态同 `im_say`（已撤回 / 被明确拒绝 / 提交结果未确认，最后一种不自动重发）。\
                 契约固定的 120 秒窗口：从那条消息发出算起，超窗就撤不了（不延长、不豁免）。\
                 撤回之后，房里所有人再读那条都只会看到「已撤回」。⚠️ 「那条不存在」与「那条不是自己发的」\
                 在服务端是同一个回答（不给存在性），所以这里也只给出同一个分类。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_VERIFY_RECALLED),
                format!(
                    "复核「那条消息现在撤回了没有」：按身份在最近 {VERIFY_WINDOW} 条里找那条（只读）"
                ),
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（与那次 im_recall 相同）"),
                    ParamSpec::new(PARAM_MESSAGE, true, "消息身份（与那次 im_recall 完全相同）"),
                ],
            )
            .with_details(format!(
                "它怎么判：在最近 {VERIFY_WINDOW} 条记录里找那条身份的消息，看它是不是已撤回\
                 状态；归因按身份（不看正文、不看名字）。⚠️ 弱在窗口：更旧的记录看不到，\
                 回执里的 `searched` 与 `more` 会告诉你看了多少条、以及有没有更旧的没看。\
                 参数与 `im_recall` 完全相同；它只读。",
            )),
            Capability::new(
                ActionId::new(CAPABILITY_MARK_READ),
                "告诉世界「我在这个房间读到这条了」——把我读到哪儿记在世界里，\
                 这样下一段清醒期不用从零重扫",
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_MESSAGE,
                        true,
                        "读到的那一条的消息身份（取自 im_read_history）",
                    ),
                ],
            )
            .with_details(
                "为什么要有它：`im_rooms` 里的 `unread` / `read` 是世界记着的位置——\
                 它跨你的清醒期活着，所以你不必自己记住「上次看到哪儿」。而世界只在你说的时候\
                 才推进它：读过没读过，是你自己的判断，系统不替你猜（它不会因为你调了\
                 `im_read_history` 就认为你读了）。\n\
                 位置只能往前走：给一个不比现在更新的消息 ⇒ 成功，`advanced = no`（不倒退、\
                 不报错）；给一条更新的 ⇒ `advanced = yes`，回执给出世界记下的新位置与未读。\n\
                 回执形状：`room`、`advanced`（这次真的推进了没有）、`read`（世界记的位置，\
                 还没读过写 `-`）、`unread`（推进后的未读；读不到写 `?`）。\n\
                 它只动你自己的位置：不改消息、不改别人的未读、不产生任何通知（这是你的\
                 位置，不是「告诉别人你读了」）。\n\
                 只读复核：`im_rooms` 的 `read` 那一栏就是它——读一次就知道世界记的是不是这条。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_LEAVE_ROOM),
                "退出一个房间——我不再是它的成员了（此后它的内容我读不到、也不能在里面说话）",
                vec![ParamSpec::new(
                    PARAM_ROOM,
                    true,
                    "房间标识（取自 im_rooms）",
                )],
            )
            .with_details(
                "它是不可逆的：退出一间房之后，那间房对你彻底消失——通讯录不再列它，\
                 历史、发言、读位置一律按「非成员」处理。想再进去得别人重新把你拉进去\
                 （你自己没有回去的路）。所以退之前想清楚。\n\
                 它只关你自己那条成员周期：不改房、不删消息、不动别人的成员资格；\
                 那间房与它的历史都还在（留在里面的人照旧读得到），只是你不在里面了。\n\
                 退出不是结束关系：在一间 1:1 房里退出，对方可以再开一间新的——\
                 「再也不要有关系」这件事今天没有对应的动作。\n\
                 别人会知道：退出是一条公共事实（留在房里的人会收到「某成员离开了」）。\n\
                 回执形状：`room`、`left`（`yes` = 世界已记下你退出了）。\n\
                 只读复核：`im_rooms` ——那间房还在不在你的通讯录里，就是答案。\n\
                 失败：房不存在、你不在里面（含已经退过）⇒ `unknown-room`；\
                 你是 owner（Agent 也能是 owner——转让目标可以是 Agent）⇒ `owner-transfer-required`（先转让）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_CREATE_GROUP),
                "开一间群房——开完我是它的 owner（房里一开始只有我）",
                vec![ParamSpec::new(
                    PARAM_NAME,
                    true,
                    format!("群名（单行、非空、≤{MAX_NAME_BYTES} 字节）"),
                )],
            )
            .with_details(
                "开完我是 owner：因此我能邀请别人进来（`im_invite`），也能在自己想走时先转让\
                 （`im_leave_room` 会如实回 `owner-transfer-required`）。\n\
                 回执形状：`room`（新房的标识）、`name`（群名）。\n\
                 只读复核：`im_rooms`——那间房在不在你的通讯录里，就是答案。\n\
                 ⚠ 弱在归因：再开一次同名的房会得到另一间房（两次是两间），因此复核只能说\
                 「有这样一间房在我的通讯录里」，不能说「就是那一次开的那一间」。\n\
                 失败：群名不合法 ⇒ `invalid-argument`；同一个幂等键配了另一个群名 ⇒\
                 `idempotency-conflict`（不猜是哪一个）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_INVITE),
                "把一个人拉进我持有的那间房（我必须是它的 owner）",
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_PARTICIPANT,
                        true,
                        "要拉进来的主体标识（取自 im_contacts）",
                    ),
                ],
            )
            .with_details(
                "资格判据在世界那一侧：目标要与你有 accepted 关系，或被你以 active Agent Owner\
                 拥有——本动作不复制这条判据，只把世界的答复如实分类。\n\
                 回执形状：`room`、`member`（他的主体标识）、`role`（他在房里的角色）。\n\
                 只读复核：`im_room_members`——那间房的成员清单里有没有他，就是答案\
                 （2026-10-13 起 Agent 侧有了成员读面，这一条因此从「核实不了」升回可复核）。\n\
                 失败：我不是这间房的 owner（或不是它的 active 成员）⇒ `not-a-member`；\
                 房不在、或目标不存在 ⇒ `unknown-room`（契约把这两种合成同一个 404，本层也不分解）；\
                 同一个键配了另一个目标 ⇒ `idempotency-conflict`。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_REMOVE_MEMBER),
                "把一个成员请出我持有的那间房（我必须是它的 owner）",
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_PARTICIPANT,
                        true,
                        "要请出去的主体标识（取自 im_room_members）",
                    ),
                ],
            )
            .with_details(
                "它是 `im_invite` 的反面，也是 owner 的基本治理权（与 Human owner 同规则：\
                 移除权含 Human 成员）。\n\
                 只关他那一份成员周期：不删消息、不改房、不动别人的成员资格；他此后读不到\
                 这间房的内容、也不能在里面说话，但那间房与它的历史都还在。\n\
                 owner 移不掉自己：想走就先转让（`im_transfer_owner`），或者把整间房留给别人。\n\
                 他会知道：移除是一条公共事实（留在房里的人会收到「某成员离开了」）。\n\
                 回执形状：`room`、`removed`。\n\
                 只读复核：`im_room_members`——他还在不在那份成员清单里，就是答案。\n\
                 失败：我不是这间房的 owner ⇒ `not-a-member`；房不在、他不在里面（含已经\
                 被移出）⇒ `unknown-room`（契约把这两类合成同一个 404，本层也不分解）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_TRANSFER_OWNER),
                "把房主让给另一个成员（我必须是现在的 owner）",
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_PARTICIPANT,
                        true,
                        "接手的主体标识（取自 im_room_members，必须是这间房的 active 成员）",
                    ),
                ],
            )
            .with_details(
                "原子转让：转完他立刻是 owner、我不再是。它不会改变任何人的成员资格。\n\
                 为什么需要它：owner 退房必须先转让（`im_leave_room` 会回\
                 `owner-transfer-required`）——它是那条路；转完之后你就能正常退出。\n\
                 转给谁：任意 active 成员（Human 或 Agent 都行）。转给自己 ⇒\
                 `invalid-argument`；目标不是本房的 active 成员 ⇒ `unknown-room`。\n\
                 回执形状：`room`、`owner`（新的 owner 标识）、`replayed`。\n\
                 只读复核：`im_room_members`——现在那一行里谁写着 `owner`，就是答案。\n\
                 失败：我不是这间房的 owner ⇒ `not-a-member`；同一个键配了另一个目标 ⇒\
                 `idempotency-conflict`。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_ROOM_MEMBERS),
                "读一间房的成员清单（谁在这间房里、各是什么角色）",
                vec![ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）")],
            )
            .with_details(
                "成员与在场是两件事：这一条读的是成员资格（谁在这间房里），\
                 `im_room_live` 读的是此刻谁在（进行态）。要邀请人进来用 `im_invite`，\
                 要知道谁在说话用 `im_room_live`。\n\
                 回执形状：`members = N` 开头，逐行 `身份 · 名字 · 角色 · 加入时刻`\
                 （owner 在前，其余按加入顺序；`direct` 房保持配对展示）。\n\
                 只读：不改变任何东西。\n\
                 失败：房不存在、房标识读不懂、我不是它的 active 成员 ⇒ 一律 `unknown-room`\
                 （契约把三者合成同一个 404 `room_not_found`，本层也不分解）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WRITE_PRESENTATION),
                "写我在某一间群房里的呈现：我在这间房里叫什么、是什么角色、负责什么",
                vec![
                    ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）"),
                    ParamSpec::new(
                        PARAM_BASE_VERSION,
                        true,
                        "我认为的当前版本（0 = 我在这一间还没写过）",
                    ),
                    ParamSpec::new(PARAM_ROOM_DISPLAY_NAME, true, "在这间房里显示的名字（可 null）"),
                    ParamSpec::new(PARAM_ROOM_ROLE_NAME, true, "我在这间房里的角色名（可 null）"),
                    ParamSpec::new(
                        PARAM_ROOM_ROLE_DESCRIPTION,
                        true,
                        "我在这间房里负责什么（可 null；≤16384 字节）",
                    ),
                ],
            )
            .with_details(
                "这是我在这一间房里的样子：同一个我，在不同的群里可以是不同的角色\
                 （在这里记账、在那里陪聊）——这一份归我自己。房主也能写它（治理面），\
                 但那是它的权限，不是它替我说话的理由。\n\
                 三个字段都要给（可以为 null = 清空那一项）。\n\
                 基准版本：每个（房 × 我）一条独立版本链——先看看现在第几版再写；\
                 对不上 ⇒ `version-conflict`（确定的拒绝，零新事实）。\n\
                 只能写我自己：这条路写不了别人在这间房里的样子。\n\
                 回执形状：`version`、三项、`replayed`。\n\
                 失败：房不存在 / 我不是成员 / 不是群房 ⇒ `unknown-room`（契约把三者合成\
                 同一个码，不分解）；参数非法 ⇒ `invalid-argument`（不出网）。\n\
                 只读复核：房成员呈现那条读面（`im_read_history` 的房成员清单里能看到我的那一份）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_ROOM_LIVE),
                "看这间房现在谁在、谁正在写（进行态；不含任何宿主读数）",
                vec![ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）")],
            )
            .with_details(
                "它是进行态，不是身体态：说的是「这间房里现在有谁、谁正在写」——\
                 这些是房间里看得见的事。宿主读数（在不在家、跑得怎么样、用哪个模型）不在\
                 这里，那些在执行端那一层（IM 里不展示投影，2026-09-26 主人裁决）。\n\
                 零正文：回执只说「谁 + 在不在 + 是不是正在写」，内容自己去读\
                 （`im_read_history`）——与通知栏同一条律。\n\
                 回执形状：`members = N, online = M, typing = K` 开头，逐行 `身份 · 名字 · 种类 · online = yes/no · typing = yes/no`。\n\
                 失败：房不存在 / 我不是成员 ⇒ `unknown-room`（契约把两者合成同一个码，不分解）。",
            ),
            // **主体进行态：我正在写**（契约 §6.2，2026-09-28 按主体形状重做）。
            //
            // ⚠ **它此前不在能力面里**（2026-10-06 接通）：`admit` 与 `execute` 两处早就有它，
            // 文档也把它当已交付，但 `own_capabilities` 里没有这一条 ⇒ OS 的归属索引按**声明**
            // 建表 ⇒ 这个动词**模型看不见、也路由不到**（"路由里有、目录里没有"）。
            // 这一刀改的只是"它进不进目录"：动词、端点、契约、客户端渲染全都早就存在。
            Capability::new(
                ActionId::new(CAPABILITY_TYPING),
                "声明「我正在这间房写」——一个几秒后自己消失的进行态信号（不是消息、不是事实）",
                vec![ParamSpec::new(PARAM_ROOM, true, "房间标识（取自 im_rooms）")],
            )
            .with_details(
                "它是一个易失信号，不是一条消息：世界不落库、不产生事件，几秒后自己过期；\
                 我在这间房发出消息会取代它。\n\
                 为什么要它：IM 里「对方正在输入…」人也有，人与 Agent 同一条规则\
                 （机制不对称：人是 UI 自动、我是显式动作；形状对称）。\n\
                 它的失败语义与别的写动词不同：传输层没答上来不当作失败\
                 （进行态丢了不影响任何消息、任何公共事实）；参数非法仍如实报 `invalid-argument`；\
                 房不存在 / 我不是成员 ⇒ `unknown-room`。\n\
                 回执：`已声明正在写（几秒后自动消失）`——没有正文、没有阶段、没有草稿\
                 （执行过程不进 IM）。\n\
                 没有同参复核可配（信号自灭，读回来归因不了）⇒ 效应类别是 \
                 `unverifiable-write`；要看此刻谁在写，读 `im_room_live`。",
            ),
        ])
    }

    /// 通讯录：`GET /v1/backend/rooms[?since=<unix ms>]`。
    fn rooms(&self, params: &ActionParams) -> ActionOutcome {
        let since = match parse_since(params.get(PARAM_SINCE)) {
            Ok(since) => since,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let path = match since {
            Some(millis) => format!("{ROOMS_PATH}?{PARAM_SINCE}={millis}"),
            None => ROOMS_PATH.to_string(),
        };
        let body = match self.read(&path) {
            Ok(body) => body,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        match render_rooms(&body, since) {
            // TASK_71 桌面 P0 第 4 件：回执**开头加一行身份**（`me = …`）——主体读到的每个 `actor`
            // 都是服务端身份，没有这一行主体分不出哪条是自己说的（见 [`ImApp::me_line`]）。
            // 次序是"**先读正文，再补那一行**"：读失败时不该为一行读数多花一次请求。
            Ok(text) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_ROOMS,
                format!("{me}{text}", me = self.me_line()),
            )]),
            // 读不懂的响应按传输层报：这一层没有"猜一个"的退路。
            Err(_) => ActionOutcome::failure(ImFailure::Unreachable.as_str()),
        }
    }

    /// **只读投影**（宿主用，不是主体的动词）：自 `since` 以来**有哪些房有新的、各几条**。
    ///
    /// 它与 [`ImApp::rooms`] 走**同一个端点、同一个读面**（B1），只是 `since` 必给——投影要知道的
    /// 只有一件事：**从锚算起有什么新的**。因此它刻意只搬**机械计数**：
    ///
    /// - **一行正文都不带**（不带预览、不带发送者、不带时间）——深度读永远是**主体的动作**；
    /// - **计数为 0 的房不逐行点名**（见 [`render_new_since`]）：按事件定价——分母（可见几间）＋
    ///   有新的房逐行 ＋ 余数那一句，"没出现的＝零新"。缺省不等于零这条纪律改由分母承担；
    /// - 一次调用恰好一次请求（不重试、不退避）；
    /// - **它不在 Agent 的能力面里**：宿主拿它做通知栏那块投影，主体看不见这个动词。
    ///
    /// `since` 早于 1970-01-01T00:00:00Z 时取 0（纪元），不编一个未来的时刻。
    /// 同一次拉取的**行**版本（宿主侧给事件卡片用：它要每房计数才能算增量，不要渲染文本）。
    ///
    /// 与 [`ImApp::new_since`] 走同一条路、同一份解析：`(房名, since 以来的条数)`，
    /// 按"条数多在前、同名按名字"机械排序——**零正文**，只有房名与计数。
    pub fn new_since_rows(&self, since: SystemTime) -> Result<Vec<(String, i64)>, ImFailure> {
        let millis = since
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0);
        let path = format!("{ROOMS_PATH}?{PARAM_SINCE}={millis}");
        let body = self.read(&path)?;
        Ok(parse_new_since_rows(&body)?
            .into_iter()
            .map(|row| (row.name, row.since))
            .collect())
    }

    /// **通知栏那块投影**：`since` 可缺（缺 = 没有锚，增量那一半问不出来）。
    ///
    /// ⚠ **没有锚也读**（2026-10-10）：未读那一半按**世界记着的读位置**算，**它不需要锚**——
    /// 从前"没锚就不读"让刚重启 / 第一次开机的那一段通知栏**整块失明**，而那一格恰恰是
    /// 主体最需要它的时候。契约 §3.1 明写 `since` 可选（缺省 = 无增量基线，`since_count` 恒 0）。
    pub fn new_since(&self, since: Option<SystemTime>) -> Result<String, ImFailure> {
        // ⚠ **路径必须由一个小函数交回**（不是就地 `match`）：`scripts/check-boundary.mjs`
        // 靠"最近的 `let path = <expr>;`"解析调用点，而它只读到**第一个分号**为止——
        // 就地 `match` 会让那一条解析不出来，守卫**如实报未解析**（它不许静默跳过）。
        // 2026-10-10 实测：那次改动把这条门禁弄红了，而当时只跑了 cargo 门禁。
        let path = rooms_path(since);
        let body = self.read(&path)?;
        render_new_since(&body, since.is_some())
    }

    /// 翻记录：`GET {`[`ROOM_HISTORY_PREFIX`]`}`{room_id}{`[`ROOM_HISTORY_SUFFIX`]`}`（可带
    /// `limit` 与 opaque 的 `before`）。
    ///
    /// ## ⚠ 2026-10-10：**读完顺带推进读位置**（主人裁决："进房读 = 打开会话"）
    ///
    /// 读位置从前**只在主体记得调 `im_mark_read` 时才动**，而真机上"读"的次数**总是多于**"标已读"
    /// 的次数（越到后面差得越多）⇒ 世界记着的"主体读到哪"**不是主体真读到哪**。这一格不修，任何
    /// "主体读没读"的读数（给人看的、给主体自己看的）都是**假的**。
    ///
    /// 判据三条：
    ///
    /// 1. **推进到这一页最新的那一条**（行是升序的，所以是最后一行）——"主体取进了视野"这件事
    ///    与"打开会话"同形，因此与世界对主体读位置的理解一致；
    /// 2. **绝不后退**：`before=` 回翻时那一页的最新一条可能比当前位置旧 ⇒ 世界答 `advanced=false`，
    ///    照旧如实说（契约的读位置本来就是**单调推进**的）；
    /// 3. **读到 ≠ 记住**：推进失败**不影响这次读的结果**（消息已经在主体视野里了），
    ///    但回执**必须如实说**"读位置没推上去"——两件事分开报，不许压成一档。
    ///
    /// 上面这三条也**摆进了那条动词的详情卡**（[`ImApp::own_capabilities`] 里 `im_read_history`
    /// 的 `with_details`）：那一段是主体自己读得到的 ⇒ 那里给的是**事实**，不替主体决定
    /// "要不要读"。
    ///
    /// ## 读数怎么取（别把它写成绝对条数）
    ///
    /// 经历账本里同一条调用会被记几次 ⇒ 只能比**比值**（"读"与"标已读"各出现多少回），
    /// **绝对条数不是调用次数**。任何写死成"读几次只标几次"的句子，下一次量就对不上——
    /// 要再说这件事，重新去账里折算，**不抄这一处**。
    ///
    /// ## 负空间（动这里之前先读完）
    ///
    /// - **"读过但不推位置"这条路因此不存在**：一次读就是一次"我读到这了"——那正是"客户端替主体
    ///   报已读"的意思。真要"看而不标记"，那是一件**新动词**（登记而不预建，本件不做）；
    ///   **不加设置项**：开关会让世界记着的读位置重新变成猜的。
    /// - **`im_mark_read` 一字不动**：它仍是主体自己说"记下我看到哪儿了"那条路，与这里**共用同一处
    ///   请求**（[`ImApp::advance_cursor`]），但回执与失败分类各归各的，谁也不借谁的语汇。
    /// - **推进失败不自愈**：不重试、不补推、不排队——那一次推进就永远不补，世界记着的位置与主体
    ///   视野的差只能靠下一次读自己收敛。所以回执那一行必须让人读得懂"没推上去" ≠ "没读到"。
    /// - **"给人看"那一格不属本件**：本件只让世界记着的那个位置变真；把位置摆到谁眼前，是世界与
    ///   呈现面的投影，窄面（只 `direct` 房、一格布尔、拉）见 `agent/README.md` §3.64 与
    ///   `CORE_V1_READ_RECEIPT.md`。**同一个位置也喂通知栏的未读那一半**（见 [`ImApp::new_since`]）
    ///   ——一处权威、两个消费者，不另存第二份读数。
    fn read_history(&self, params: &ActionParams) -> ActionOutcome {
        // ⚠ **2026-10-13 修**：这里从前是**自己内联**的非空判断（`!room.is_empty()`），
        // 因此漏了 [`MAX_ID_BYTES`] 那道长度界——而**同族的另外 14 处**都走 [`valid_room`]。
        // 后果两样都坏：① 契约（`CORE_V1_AGENT_HISTORY.md`）写着"机械校验先于任何往返"，
        // 而这条读路径**没有**；② 一个超长的房间标识会被拼进请求路径去打扰世界，
        // 再拿回一个 404——那一次往返本可以在本地省掉。
        // ⇒ 收敛到那一个函数：**同一种形状只有一个答案**（与 `MAX_FILE_PARAM_BYTES`
        // 那条注释同款：抄的数字迟早与实现脱钩）。
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let limit = match parse_limit(params.get(PARAM_LIMIT)) {
            Ok(limit) => limit,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let before = params.get(PARAM_BEFORE).filter(|cursor| !cursor.is_empty());

        let path = history_path(room, limit, before);

        let body = match self.read(&path) {
            Ok(body) => body,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        match render_history(&body, room, limit) {
            // 同上（第 4 件）：翻记录这一面也有那行身份——**主体看自己的话来核对时，就靠它**。
            Ok(text) => {
                let cursor_line = self.advance_after_read(room, &body);
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_READ_HISTORY,
                    format!("{me}{text}{cursor_line}", me = self.me_line()),
                )])
            }
            Err(_) => ActionOutcome::failure(ImFailure::Unreachable.as_str()),
        }
    }

    /// **读完历史之后那一句**：推进读位置并如实报结果（读到 ≠ 记住，两件事分开说）。
    ///
    /// 它**不改变这次读的结果**：读失败根本走不到这里（消息没进视野就谈不上"读到了"）；
    /// 而这里出任何岔子，读本身**仍然是成功的**——只是回执多一句实话。
    fn advance_after_read(&self, room: &str, body: &serde_json::Value) -> String {
        let newest = body
            .get("messages")
            .and_then(|rows| rows.as_array())
            .and_then(|rows| rows.last())
            .and_then(|last| last.get("id"))
            .and_then(|id| id.as_str())
            .filter(|id| !id.is_empty());
        let Some(newest) = newest else {
            // 空页（或行里没有 id）：**没有可推进的位置**——说出口，别让"没这一行"与"推上去了"混。
            return "read-cursor = unchanged（这一页没有可推进的消息）\n".to_string();
        };
        match self.advance_cursor(room, newest) {
            AdvanceReply::Answered { advanced: true, .. } => {
                format!("read-cursor = advanced to {newest}\n")
            }
            AdvanceReply::Answered {
                advanced: false, ..
            } => {
                format!("read-cursor = unchanged（世界说你早就读到 {newest} 了）\n")
            }
            AdvanceReply::Unreadable => {
                "read-cursor = unknown（2xx，但读不出「推进了没有」）\n".to_string()
            }
            AdvanceReply::Refused(failure) => format!(
                "read-cursor = 没推上去（{}）——读到了，但世界还不知道\n",
                failure.as_str()
            ),
            AdvanceReply::Unknown => {
                "read-cursor = unknown（不知道推上去没有）——读到了，但世界知不知道不确定\n"
                    .to_string()
            }
        }
    }

    /// 读附件：`GET /v1/backend/rooms/{room}/attachments/{attachment}`（纯读）。
    ///
    /// 它把**字节**搬成主体这一轮看得到的一段文本——**走的就是 `im_read_history` 那条注入路**
    /// （结果作为这一轮的观察进视野；本 App 不存、不缓存、不写任何地方）。本刀**不新立注入行为**：
    /// "读进去的东西在上下文里留多久"是 Runtime 的机制，已另立刀排队，落地后所有工具一起变。
    ///
    /// **分段读**（附件是稳定可寻址的文件类：身份挂在消息上、随时可重取）：
    ///
    /// - 一次最多 [`ATTACHMENT_WINDOW_LINES`] 行 / [`ATTACHMENT_WINDOW_BYTES`] 字节；
    /// - **单行**超过 [`ATTACHMENT_LINE_CHARS`] 字符 ⇒ 那一行**截断并标注**（明写这一行有多长）；
    /// - 被截掉的部分**不是丢**：回执的读数块给出 `lines = 共多少行, read = 已读哪几行,
    ///   more = offset <下一段的起始行>`，主体把那个数放回 `offset` 就能接着读。
    ///
    /// 三道边界：响应头的类型不在 [`TEXT_ATTACHMENT_TYPES`] 里 ⇒ `unsupported-content-type`
    /// （**缺 `Content-Type` 也算这一格**）；不是合法 UTF-8 ⇒ `not-text`（**不用替换字符充数**）；
    /// 连通道都拿不下的（> 16 MiB）⇒ `too-large`——**这不叫截断**：一份完整字节都没拿到，
    /// 谈不上"读到哪一段"。`offset` 非法（0 / 非十进制 / 超过总行数）⇒ `invalid-argument`，
    /// 一次请求都不发。
    ///
    /// 失败分类与别的读侧同源（同一套码同一个意思）：`404` 在这里的含义是"这个附件读不到"，
    /// 服务端把 pending / 未知 / 跨房 / 跨人 / 过期 / 撤回 / 非成员合成同一个答案，
    /// 这一层**同样不分解它**。
    fn read_attachment(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(attachment) = valid_attachment(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let offset = match parse_attachment_offset(params.get(PARAM_ATTACHMENT_OFFSET)) {
            Ok(offset) => offset,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let cited_name = match valid_file_name(params) {
            Ok(name) => name,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let path = format!("{ROOMS_PATH}/{room}/attachments/{attachment}");
        match self.fetch_bytes(&path, MAX_ATTACHMENT_FETCH_BYTES) {
            ByteReply::Fetched {
                content_type,
                bytes,
            } => {
                let Some(content_type) = content_type else {
                    return ActionOutcome::failure(ImFailure::UnsupportedContentType.as_str());
                };
                if !is_readable_content_type(&content_type, self.vision) {
                    return ActionOutcome::failure(ImFailure::UnsupportedContentType.as_str());
                }
                // **媒体形态**（2026-10-17）：媒体没有"行"这回事 ⇒ 窗口参数**明确失败**（不静默
                // 忽略——静默忽略会让人以为「读到的是某一段」）。回执给**引用**，字节随块出门：
                // **只进这一次请求，不进账本**（见 `core::ContentBlock` 的类型文档）。
                if IMAGE_ATTACHMENT_TYPES.contains(&content_type_essence(&content_type).as_str()) {
                    if params.get(PARAM_ATTACHMENT_OFFSET).is_some() {
                        return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
                    }
                    let size = bytes.len();
                    // **感知体量**（P3/P6）：从文件头读，**不解码像素**。读不出来就是 `None`
                    // ——那一档**不拒绝**（还有字节那道门），但回执必须如实写 `unknown`。
                    let pixels = image_pixels(&content_type_essence(&content_type), &bytes);
                    // **事前门**：读不下就**当场**拒绝（P5：采样不可逆，不能先给再抽走）。
                    if let Some((width, height)) = pixels
                        && u64::from(width) * u64::from(height) > MAX_MEDIA_PIXELS
                    {
                        return ActionOutcome::failure_with_content(
                            ImFailure::TooLarge.as_str(),
                            vec![ContentBlock::new(
                                CAPABILITY_READ_ATTACHMENT,
                                render_media_refusal(
                                    room,
                                    attachment,
                                    &content_type,
                                    size,
                                    (width, height),
                                ),
                            )],
                        );
                    }
                    return ActionOutcome::success(vec![
                        ContentBlock::new(
                            CAPABILITY_READ_ATTACHMENT,
                            render_media_attachment(
                                room,
                                attachment,
                                cited_name,
                                &content_type,
                                size,
                                pixels,
                            ),
                        )
                        .with_material(Material::new(content_type, bytes)),
                    ]);
                }
                let Ok(text) = String::from_utf8(bytes) else {
                    return ActionOutcome::failure(ImFailure::NotText.as_str());
                };
                let window = match slice_attachment(&text, offset) {
                    Ok(window) => window,
                    Err(failure) => return ActionOutcome::failure(failure.as_str()),
                };
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_READ_ATTACHMENT,
                    render_attachment(
                        room,
                        attachment,
                        cited_name,
                        &content_type,
                        text.len(),
                        &window,
                    ),
                )])
            }
            // 契约列出的明确拒绝：与其它读侧同一套解释。
            ByteReply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            ByteReply::Refused(401) => {
                ActionOutcome::failure(ImFailure::CredentialRejected.as_str())
            }
            ByteReply::Refused(403) => ActionOutcome::failure(ImFailure::NotAMember.as_str()),
            ByteReply::Refused(404) => ActionOutcome::failure(ImFailure::NotFound.as_str()),
            ByteReply::Refused(_) => ActionOutcome::failure(ImFailure::ServerError.as_str()),
            ByteReply::Broken => ActionOutcome::failure(ImFailure::Unreachable.as_str()),
            // 连通道都拿不下：明确说"太大"（**不是**截断——我们连完整字节都没有）。
            ByteReply::OverLimit => ActionOutcome::failure(ImFailure::TooLarge.as_str()),
        }
    }

    /// **说话**（写）：把这一句话落进那个房间；给了 `file` 时，**先把那份文件递上去**。
    ///
    /// 三态如实（这一件的核心）：
    ///
    /// - **2xx ⇒ 已提交**：回执给消息身份、**服务端说这条引用了哪一条**（是回复才有那一行），
    ///   并如实写出服务端说的"这是重放"这个事实；
    /// - **4xx ⇒ 被明确拒绝**：契约保证这类回程**零新事实**（含 409 同键异摘要），因此是
    ///   一次确定的失败，带本 App 的稳定分类；
    /// - **其余（传输失败 / 送出后读不到响应 / 超时 / 5xx / 2xx 读不懂）⇒ `unknown`**：
    ///   我们**不知道**那份效果落没落账。**绝不重发**——幂等键摆在那里是为了让服务端把重复
    ///   投递收成一条，而不是让我们再赌一次；要确认就自己用复核。
    ///
    /// **带 `file` 时是两步**（契约 §8 的两步式：上传 → 绑定），两步的语义**不一样**：
    ///
    /// 1. **读本地字节**（在主体的世界观里；读不到 / 太大 ⇒ **明确失败**，一个请求都不发）；
    ///    类型**不**在这条判据里：认不出的照 `application/octet-stream` 递（2026-10-11，见
    ///    [`upload_content_type`]）——**递得出去**与**读得动**是两件事；
    /// 2. **上传**（**不带幂等键**——契约明说上传不需要它；多余的 pending 是到期回收的私有
    ///    暂存租约）：拿不到附件身份 ⇒ 明确失败（**那句话确定没说出去**）；
    /// 3. **发消息**（带幂等键 + 那一个附件身份）。
    ///
    /// ⚠️ 第二步成功、第三步 `unknown` 时，回执**不**说"什么都没发生"（那是假话）：字节可能
    /// 已经上传成一个**临时**附件，服务端会在 24 小时内自行回收；而**那句话**的结果仍是未确认。
    ///
    /// **引用与点名**（可选的 `reply_to_id` / `mentions`）：两个参数都只判**形状**（非法
    /// 本地拒绝、零出网），合规就按契约的 wire 形状进请求体（`reply_to_message_id` /
    /// `mentions`），并参与服务端的幂等摘要（同键换引用对象 / 换提及集合 = 409，零新事实）。
    /// 失败面按**请求里带了哪一类引用**给分类（见下方 400 那一格的注释与词表）。
    fn say(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let file = match valid_file(params) {
            Ok(file) => file,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let Some(body) = valid_optional_body(params, file.is_some()) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        // 引用对象与提及集合：形状先于任何出网（非法一律本地拒绝）。
        let reply_to = match valid_reply_to(params) {
            Ok(reply_to) => reply_to,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let mentions = match valid_mentions(params) {
            Ok(mentions) => mentions,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };

        // 第一步 + 第二步：把要递的那份文件读成字节、上传、拿附件身份。任何一处不成立 ⇒
        // 明确失败，**消息一个字节都不发**。
        let attachment = match file {
            None => None,
            Some(path) => match self.stage_attachment(operation, room, path) {
                Ok(id) => Some(id),
                Err(failure) => return ActionOutcome::failure(failure.as_str()),
            },
        };

        let key = idempotency_key(operation);
        let mut payload = serde_json::Map::new();
        if let Some(body) = body {
            payload.insert(
                PARAM_BODY.to_string(),
                serde_json::Value::String(body.to_string()),
            );
        }
        // 引用与提及进体的**字段名是 wire 名**（`reply_to_message_id` / `mentions`），不是
        // 我们的参数名（`reply_to_id` / `mentions`）——同 `message` 与 `message_id` 的先例。
        // 它们参与服务端的幂等摘要：同键换引用对象或换提及集合 = 换摘要 = 409（零新事实）。
        if let Some(reply_to) = reply_to {
            payload.insert(
                REPLY_TO_FIELD.to_string(),
                serde_json::Value::String(reply_to.to_string()),
            );
        }
        if !mentions.is_empty() {
            payload.insert(
                MENTIONS_FIELD.to_string(),
                serde_json::Value::Array(
                    mentions
                        .iter()
                        .map(|id| serde_json::Value::String((*id).to_string()))
                        .collect(),
                ),
            );
        }
        if let Some(id) = &attachment {
            payload.insert(
                ATTACHMENTS_FIELD.to_string(),
                serde_json::Value::Array(vec![serde_json::Value::String(id.clone())]),
            );
        }
        let payload = serde_json::Value::Object(payload).to_string();
        let path = format!("{ROOMS_PATH}/{room}{ROOM_HISTORY_SUFFIX}");

        match self.exchange("POST", &path, Some(payload), Some(&key)) {
            Reply::Answered(value) => {
                let message = value
                    .get("message")
                    .and_then(|message| message.get("id"))
                    .and_then(|id| id.as_str());
                let replayed = value
                    .get("idempotency_replayed")
                    .and_then(|flag| flag.as_bool())
                    .unwrap_or(false);
                // 回执里"引用了哪一条"取**服务端回执的那一格**（MessageView 恒含），不是复述
                // 请求——那样它才能与读侧（历史投影 / 复核）直接对上。
                let reply_to = value
                    .get("message")
                    .and_then(|message| message.get(REPLY_TO_FIELD))
                    .and_then(|id| id.as_str())
                    .filter(|id| !id.is_empty());
                match message {
                    Some(id) => ActionOutcome::success(vec![ContentBlock::new(
                        CAPABILITY_SAY,
                        render_said(room, id, replayed, reply_to, attachment.as_deref()),
                    )]),
                    // 2xx 却拿不到消息身份：我们无法说出"落成了哪一条" ⇒ 未确认，不猜。
                    None => ActionOutcome::unknown(),
                }
            }
            // 说话侧：契约**列出**的那几个码是"明确的拒绝"（零新事实）⇒ 确定的失败；
            // 没列出的（例如 422）与 5xx 一样 ⇒ 它们没有告诉我们"没落账" ⇒ 未确认。
            //
            // 400 的归属：本地形状已先判过，能走到这里的 400 只剩三种来源——两个**引用面**
            // （契约把「回复目标不能当回复对象」（§6.5：撤回 / 未知 / 跨房 / 不可见合成同一个码）
            // 与「提及目标不能被接受」（MENTION_TARGETS §5，同样合成）各自收成一个码）与**附件面**
            // （`invalid_attachment`：附件身份来自本次上传，但仍由服务端在提交事务里裁决）。
            // 因此只在**恰好一个**引用面在场、且**没带附件**时指认它（给到可行动粒度）；其余
            // ——两类同场 / 与附件同场 / 一个引用都没带——一律合并分类：**不指认、不猜**。
            Reply::Refused(400) => {
                let class = match (
                    reply_to.is_some(),
                    mentions.is_empty(),
                    attachment.is_none(),
                ) {
                    (true, true, true) => ImFailure::NotAReplyTarget,
                    (false, false, true) => ImFailure::NotAMentionTarget,
                    _ => ImFailure::InvalidArgument,
                };
                ActionOutcome::failure(class.as_str())
            }
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(403) => ActionOutcome::failure(ImFailure::NotAMember.as_str()),
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::IdempotencyConflict.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 把一份本地文件**递上去**：读字节 → 上传 → 拿附件身份（契约 §8 的前半步）。
    ///
    /// 全程**零幂等键**（契约：上传不需要它）。任何一步不成立都**不**继续——读不到就不发请求，
    /// 上传没成就**不发消息**。因此这里的每种失败对"那句话说没说出去"都是**确定**的：没说
    /// （唯一的残留是可能多了一个 pending 附件，24 小时内由服务端自行回收）。
    fn stage_attachment(
        &self,
        operation: &OperationId,
        room: &str,
        file: &str,
    ) -> Result<String, ImFailure> {
        // 没有声明世界观 ⇒ 这只手不在：给了文件也读不出来（如实报，不猜）。
        let Some(root) = self.world_root.as_deref() else {
            return Err(ImFailure::FileUnreadable);
        };
        let bytes = read_sandbox_file(root, file, MAX_SAY_FILE_BYTES as u64).map_err(|class| {
            match class {
                SandboxError::NotFound => ImFailure::FileNotFound,
                SandboxError::TooLarge => ImFailure::TooLarge,
                // 路径非法 / 不是普通文件 / 链上有链接 / 被拒绝 / I/O：都是"这个文件读不出来"。
                _ => ImFailure::FileUnreadable,
            }
        })?;
        let file_name = file_name_of(file)?;
        // 类型按**扩展名**定；认不出来给 `application/octet-stream`——**不拒收**（2026-10-11：
        // 递得出去递不出去是**通道**的事，通道不判类型；读不读得动是**读附件那一侧**的事）。
        let content_type = upload_content_type(&file_name);

        let path = format!("{ROOMS_PATH}/{room}/attachments");
        match self.upload_multipart(operation, &path, &file_name, content_type, &bytes) {
            Reply::Answered(value) => value
                .get("attachment")
                .and_then(|attachment| attachment.get("id"))
                .and_then(|id| id.as_str())
                .map(str::to_owned)
                .ok_or(ImFailure::Unreachable),
            Reply::Refused(400) => Err(ImFailure::InvalidArgument),
            Reply::Refused(401) => Err(ImFailure::CredentialRejected),
            Reply::Refused(403) => Err(ImFailure::NotAMember),
            Reply::Refused(404) => Err(ImFailure::UnknownRoom),
            // 路由级与字节级的上限都是 413 ⇒ "太大"（我们这一侧已经先按 64 KiB 拦过一次，
            // 走到这里说明对面还有别的边界）。
            Reply::Refused(413) => Err(ImFailure::TooLarge),
            Reply::Refused(_) => Err(ImFailure::ServerError),
            // 上传这一步**没有逐字节的幂等兜底**：读不到响应 = 不知道传上去没有。
            // 但它对**那句话**仍然是确定的：消息还没发 ⇒ 没说过。
            Reply::Broken | Reply::Unreadable => Err(ImFailure::Unreachable),
        }
    }

    /// 上传一份附件：`POST /v1/backend/rooms/{room}/attachments`（multipart/form-data，
    /// **恰好一个** `name="file"` part——契约 §8 的严格形状）。
    ///
    /// 体在这一侧手工拼（`reqwest` 只开了 json，没有 multipart）；边界串由**操作身份**派生，
    /// 并且**先确认它没有出现在内容里**（撞上就换一个，有限步）——那样"内容里恰好有边界"
    /// 这种极端情形不会变成一次静默的形状错误。
    ///
    /// **不带幂等键**（契约：上传不需要），因此这里也**不重试**。
    fn upload_multipart(
        &self,
        operation: &OperationId,
        path: &str,
        file_name: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Reply {
        let boundary = multipart_boundary(operation, bytes);
        let body = multipart_body(&boundary, file_name, content_type, bytes);
        let header = format!("multipart/form-data; boundary={boundary}");
        self.channel.post_bytes(path, &header, body)
    }

    /// **复核**（只读）：现在有没有一条我发的、与这次**同形**的消息。
    ///
    /// 参数必须与 `im_say` **完全相同**——复核问的是"**这一次这一条**发生了没有"，
    /// 参数不同就不是同一个问题。
    ///
    /// **同形**有四个维度（少一个就会误判）：正文完全相同；**附件对得上**——那次带了 `file`
    /// 的，要求那条的附件按**文件名与字节数**与这个文件一致；那次没带 `file` 的，要求那条
    /// **没有附件**；**回复对象对得上**——那次带了 `reply_to_id` 的，要求那条引用的就是
    /// 同一条；那次没带的，要求那条**不是回复**；**提及集合对得上**——那次带了 `mentions`
    /// 的，要求那条点名的集合完全相同；那次没带的，要求那条**没点名**（少了这几条，一条
    /// 带附件 / 带回复 / 带@的旧消息会被当成"这句话我说过了"）。
    ///
    /// ⚠️ **附件这一格不可比时如实报 `unknown`**：要递的那个文件现在读不到（路径变了、被删了、
    /// 太大了）⇒ 手里没有能与那条消息比的东西 ⇒ 既不说"找到了"也不说"没有"。**这是对的**：
    /// 一条我们没有证据的写效果，不该因为"比不了"被静默说成"没有发生"。
    ///
    /// 两步都是**幂等读**：先问服务器"这个凭据代表谁"（[`ImApp::whoami`]，**恰好一次**），
    /// 再**有界回翻**（[`ImApp::sweep`]）。
    ///
    /// **"确定"凭什么**（TASK_24）：
    /// - 扫到 ⇒ success（页从新往旧走，因此取到的就是**最新**的那一条）；
    /// - **翻到房的起点**仍没有 ⇒ failure(`not-found`)——这是**唯一**允许说"没有"的依据；
    /// - **没看到底**（触页数上限 / 某一页读不出来 / 体不是我们认得的形状）⇒ `unknown`；
    /// - 连"我是谁"都问不出来 ⇒ `unknown`。
    ///
    /// ⚠️ 一次**明确的拒绝**（`400` / `404` / `403` …）在这里**也**是 `unknown`：房不存在与
    /// "我不再是成员"用的是同一个 `404`，而"我不是成员"**不证明**那条消息不存在。
    fn verify_said(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let file = match valid_file(params) {
            Ok(file) => file,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let Some(body) = valid_optional_body(params, file.is_some()) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        // 与 `im_say` 同一套形状判据（"参数完全相同"这条承诺在这里落地）。
        let reply_to = match valid_reply_to(params) {
            Ok(reply_to) => reply_to,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let mentions = match valid_mentions(params) {
            Ok(mentions) => mentions,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };

        // 附件这一格要拿**本地文件**的（名字, 字节数）去比；读不到就没有可比的东西 ⇒ unknown。
        let expecting = match file {
            None => Some(Vec::new()),
            Some(path) => self.attachment_expectation(path).ok(),
        };
        let Some(expecting) = expecting else {
            return ActionOutcome::unknown();
        };

        // 归因用**服务器给的身份**（见 [`ImApp::me`]），不是执行端给的本地 AgentId：
        // 连"我是谁"都问不出来 ⇒ 没有答案 ⇒ 不解除任何东西。
        // 它**一次活动只探一次**（同一个 `ImApp` 上的第二次复核照抄第一次的答案）。
        let Ok(me) = self.me() else {
            return ActionOutcome::unknown();
        };

        match self.sweep(room, |rows| {
            find_own_message(rows, &me, body, &expecting, reply_to, &mentions)
        }) {
            Sweep::Found {
                searched,
                whole,
                found,
            } => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_VERIFY_SAID,
                render_verification(
                    room,
                    searched,
                    whole,
                    Some(&found),
                    file.is_some(),
                    reply_to.is_some(),
                    !mentions.is_empty(),
                ),
            )]),
            // 翻到房的起点仍没有：这才叫"确定没有"。
            Sweep::Exhausted => ActionOutcome::failure(ImFailure::NotFound.as_str()),
            // 没看到底：不能声称"没有"，因此不能解除任何东西。
            Sweep::Undecided => ActionOutcome::unknown(),
        }
    }

    /// 复核用的一份**预期**：那个文件现在叫什么、有多大。
    ///
    /// 它**不读内容**：服务端不返回 hash（§8 明文），所以"同一份文件"在这一层只能用可观察的
    /// 元数据说——文件名与字节数。这两样对上**不等于**内容没换过，回执里如实写着这个边界
    /// （见 `im_verify_said` 的详情）。
    fn attachment_expectation(&self, file: &str) -> Result<Vec<(String, u64)>, ImFailure> {
        let Some(root) = self.world_root.as_deref() else {
            return Err(ImFailure::FileUnreadable);
        };
        let (name, size) = stat_sandbox_file(root, file).map_err(|class| match class {
            SandboxError::NotFound => ImFailure::FileNotFound,
            SandboxError::TooLarge => ImFailure::TooLarge,
            _ => ImFailure::FileUnreadable,
        })?;
        Ok(vec![(name, size)])
    }

    /// **撤回**（写）：撤掉自己刚说出的那条公共消息。
    ///
    /// 与 [`ImApp::say`] 的**关键差异**：撤回**不带**幂等键（契约：非空 body / 任意 query /
    /// `Idempotency-Key` **一律 400**）。幂等在这里由 **message 身份**承担（每条 Message 至多一条
    /// MessageRecall，重复撤回是 200 重放且**不刷新** `recalled_at`），不是由我们的键承担——
    /// 所以**不许**为了"统一"给它加键。
    ///
    /// 三态如实：`2xx` ⇒ 已提交（回执如实说这一次有没有产生新事实）；契约**列出**的
    /// `400`/`401`/`404`/`409` ⇒ 明确的失败；其余（传输失败 / 送出后读不到响应 / 超时 / 5xx /
    /// 读不懂）⇒ `unknown`，**绝不重发**。
    ///
    /// `404` 在这里的含义**不是**"房不存在"：契约把"那条消息不存在"与"那条不是主体发的"合流成
    /// 同一个 `404 message_not_found`（服务端刻意不给存在性 oracle），因此本层也只报一个分类。
    fn recall(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(message) = valid_message(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };

        // 无 body、无 query、**无键**（见本函数的文档）。
        let path = format!("{ROOMS_PATH}/{room}{ROOM_HISTORY_SUFFIX}/{message}");
        match self.exchange("DELETE", &path, None, None) {
            Reply::Answered(value) => {
                let message = value
                    .get("message_id")
                    .and_then(|id| id.as_str())
                    .filter(|id| !id.is_empty());
                let replayed = value
                    .get("idempotency_replayed")
                    .and_then(|flag| flag.as_bool())
                    .unwrap_or(false);
                let recalled_at = value.get("recalled_at").and_then(|at| at.as_str());
                match (message, recalled_at) {
                    (Some(id), Some(at)) => ActionOutcome::success(vec![ContentBlock::new(
                        CAPABILITY_RECALL,
                        render_recall(room, id, at, replayed),
                    )]),
                    // 2xx 却读不出身份或撤回时点：我们无法说出"撤掉了哪一条、什么时候" ⇒ 不猜。
                    _ => ActionOutcome::unknown(),
                }
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 撤回侧独有的 404 含义（不分解"不存在"与"非本人"）。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::MessageNotFound.as_str()),
            // 撤回侧独有的 409 含义（120 秒窗口）。
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::WindowExpired.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// **推进读位置**（写）：告诉世界"我在这个房间读到这条了"（切片 M1 的执行端第 2 件）。
    ///
    /// 契约：`PUT /v1/backend/rooms/{room_id}/read-cursor`，体只含 `message_id`，**无幂等键**
    /// （单调推进天然幂等：同一条重发是成功 no-op，`advanced = false`）。
    ///
    /// **为什么这条位置要由主体自己说**：`im_read_history` 是"看"，本动词是"记下我看到哪儿了"
    /// ——两件事。系统**不**因为主体翻过历史就替它认定"读过了"：读过没读过是主体自己的判断
    /// （见 `outputs/AGENT_IDEMPOTENT_WRITE_EFFECT_CLASS_DECISION_SHEET.md`）。
    ///
    /// **三态如实**（与另外两个写动词同一条纪律）：2xx 且回执里读得出 `advanced` ⇒ 成功
    /// （推进了没有是**确定**的答案）；被明确拒绝（400 / 401 / 404）⇒ 对应分类；其余（传输断、
    /// 读不懂、不认识的码）⇒ `unknown`，**绝不重发**。
    ///
    /// 回执里的 `read` / `unread` 与通讯录同款：读不到写 `?`，**不写 0**；位置为空写 `-`。
    fn mark_read(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(message) = valid_message(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        match self.advance_cursor(room, message) {
            AdvanceReply::Answered {
                advanced,
                read_state,
            } => {
                // 推进之后的世界读数：**同一份回执里带回来的**（读不出来就如实 `unknown`）。
                let (read, unread) = read_state_fields(read_state.as_ref());
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_MARK_READ,
                    render_mark_read(room, advanced, &read, &unread),
                )])
            }
            // 本地形状已判过；能走到这里的 400 只可能是"那条不能当读位置"。
            AdvanceReply::Refused(failure) => ActionOutcome::failure(failure.as_str()),
            // 2xx 却读不出"推进了没有"，或传输断 / 不认识的码：说不出这次到底动没动 ⇒ 不猜。
            AdvanceReply::Unreadable | AdvanceReply::Unknown => ActionOutcome::unknown(),
        }
    }

    /// **推进读位置那一次请求**（`im_mark_read` 与"读完历史顺带推进"**共用这一处**）。
    ///
    /// 无键（契约：单调推进天然幂等）。**体里的字段名是 wire 名**（`message_id`），
    /// 不是我们的参数名（`message`）——两者不同名这件事在这一行说清。
    ///
    /// 四态分开（与读侧同一条纪律：**不知道 ≠ 被拒 ≠ 没动**）：世界答了（`advanced` 是它说的
    /// "动没动"，`read_state` 是它给的读数）／2xx 但读不出那一格／明确被拒（稳定分类）／
    /// 其余（传输断、未分类）。读数**随回执一起带回来**，不另存一份（那会变成第二份可变权威）。
    fn advance_cursor(&self, room: &str, message: &str) -> AdvanceReply {
        let path = format!("{ROOMS_PATH}/{room}{READ_CURSOR_SUFFIX}");
        let body = serde_json::json!({ "message_id": message }).to_string();
        match self.exchange("PUT", &path, Some(body), None) {
            Reply::Answered(value) => {
                match value.get("advanced").and_then(|flag| flag.as_bool()) {
                    Some(advanced) => AdvanceReply::Answered {
                        advanced,
                        read_state: value.get("read_state").cloned(),
                    },
                    // 2xx 却读不出"推进了没有"：说不出这次到底动没动 ⇒ 不猜。
                    None => AdvanceReply::Unreadable,
                }
            }
            // 本地形状已判过；能走到这里的 400 只可能是"那条不能当读位置"。
            Reply::Refused(400) => AdvanceReply::Refused(ImFailure::NotAReadTarget),
            Reply::Refused(401) => AdvanceReply::Refused(ImFailure::CredentialRejected),
            // 契约把"房不在"与"我不是成员"合成同一个 404（不泄露存在性）——本层也不猜。
            Reply::Refused(404) => AdvanceReply::Refused(ImFailure::UnknownRoom),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => AdvanceReply::Unknown,
        }
    }

    /// **退出房间**（写，切片 M2）：我不再是这间房的成员了。
    ///
    /// 契约：`POST /v1/backend/rooms/{room_id}/leave`，**无体、无 query、无幂等键**，
    /// 成功 `204 No Content`。与 Human 侧同一条用例（`leave_membership`），语义逐条同形。
    ///
    /// **它不可逆、且只关自己那条周期**：房与历史都留着（留在里面的人照旧读得到），
    /// 只有**你**不在里面了；想回去得别人重新拉你。
    ///
    /// **2xx 的空体是预期形状**：这条端点按契约回 204（无响应体），因此
    /// `Reply::Unreadable`（2xx 但体读不出）在这里**就是成功**——不是"读不懂"。
    /// 分类是契约知识，由知道这条端点契约的调用方做（`Reply` 的文档就是这么分工的）。
    ///
    /// **三态如实**：2xx ⇒ 成功（世界已记下退出）；被明确拒绝（401 / 404 / 409）⇒ 对应分类；
    /// 其余（传输断、不认识的码）⇒ `unknown`，**绝不重发**。
    fn leave_room(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };

        // 无体、无键（幂等由"成员资格"承担：退过之后就不再是成员，第二次是 404）。
        let path = format!("{ROOMS_PATH}/{room}{LEAVE_SUFFIX}");
        match self.exchange("POST", &path, None, None) {
            // 204（空体）与任何别的 2xx：都表示"这次退出已被接受"。
            Reply::Answered(_) | Reply::Unreadable => {
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_LEAVE_ROOM,
                    render_left(room),
                )])
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 契约把「房不在 / 我不是成员（含已经退过）/ human_direct」合成同一个 404
            // ——本层也不猜是哪一个。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            // 契约给这条端点列了 409（owner 必须先转让）。**Agent 可以是群 owner**（§5.8 四条治理
            // 端点让"Agent 也能当 owner"成为可达且可退出的状态：转让目标可以是 Agent；黑盒
            // `backend_group_admin_blackbox::an_agent_can_own_and_govern_a_group_room` 走真进程
            // 证明过），因此这一格是**可达路径**（Agent owner 退房要先转让），不是防御格——但绝不
            // 把它吞成 unknown（那会把"明确被拒"说成"不知道"）。
            Reply::Refused(409) => {
                ActionOutcome::failure(ImFailure::OwnerTransferRequired.as_str())
            }
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// **移除一个成员**（`DELETE /v1/backend/rooms/{room}/members/{participant}`，不带键）。
    ///
    /// 与 Human 侧那条**同一 application 用例、同一套错误码**（只换认证面）——契约把
    /// 「房不在 / 我不是成员 / 目标不是 active 成员」合成同一个 404，本层也不分解。
    fn remove_member(&self, params: &ActionParams) -> ActionOutcome {
        let (Some(room), Some(participant)) = (valid_room(params), valid_participant(params))
        else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let path = format!("{ROOMS_PATH}/{room}{MEMBERS_SUFFIX}/{participant}");
        match self.exchange("DELETE", &path, None, None) {
            Reply::Answered(_) | Reply::Unreadable => {
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_REMOVE_MEMBER,
                    render_removed(room, participant),
                )])
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 403 = 我不是这间房的 owner。
            Reply::Refused(403) => ActionOutcome::failure(ImFailure::NotAMember.as_str()),
            // 409 = **owner 移不掉自己**（契约 `owner_not_removable`）——它与"我不在里面"是两件事，
            // 因此走自己的词（出路是先转让），不并进 `not-a-member`。
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::OwnerNotRemovable.as_str()),
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// **转让房主**（`POST /v1/backend/rooms/{room}/owner-transfer`，带幂等键）。
    ///
    /// 契约：目标必须是本房的 active 成员（Human 或 Agent 都行）；转给自己 ⇒ 400
    /// `invalid_transfer_target`；同 key 重放 ⇒ `replayed = yes`。
    fn transfer_owner(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let (Some(room), Some(participant)) = (valid_room(params), valid_participant(params))
        else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let payload = serde_json::json!({ PARTICIPANT_ID_FIELD: participant }).to_string();
        let path = format!("{ROOMS_PATH}/{room}{TRANSFER_SUFFIX}");
        let key = idempotency_key(operation);
        match self.exchange("POST", &path, Some(payload), Some(&key)) {
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_TRANSFER_OWNER,
                render_transferred(room, participant, &value),
            )]),
            Reply::Unreadable => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_TRANSFER_OWNER,
                format!(
                    "room = {room}\nowner = {participant}\n（这一版世界的回执形状我没读懂；用 im_room_members 看现在谁是 owner）\n"
                ),
            )]),
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(403) => ActionOutcome::failure(ImFailure::NotAMember.as_str()),
            // 404 = 房不在 / 目标不是本房的 active 成员（契约合成一个码）。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::IdempotencyConflict.as_str()),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// **读一间房的成员清单**（`GET /v1/backend/rooms/{room}/members`，只读）。
    ///
    /// 它与 Human 侧那条**同一可见性裁决**（服务端复用同一个 `get_room_members`）：
    /// 我必须是这间房的 active 成员；未知 / bad-id / 不是成员统一 404。
    fn room_members(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let path = format!("{ROOMS_PATH}/{room}{MEMBERS_SUFFIX}");
        match self.read(&path) {
            Ok(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_ROOM_MEMBERS,
                render_members(&value),
            )]),
            // 读侧的解释与 `read` 同一套：401 凭据、404 房层面、其余服务端错误。
            Err(failure) => ActionOutcome::failure(failure.as_str()),
        }
    }

    /// **开一间群房**（`POST /v1/backend/groups`，带幂等键）：**我成为它的 owner**。
    ///
    /// 这是 2026-09-27「owner = 创建者」那批给 Agent 的四条治理端点之一——**权利早就有**
    /// （裁决原文：「Agent owner 的邀请权完全给（与 Human owner 同规则）」），缺的是这一侧的动作。
    ///
    /// **复核 = 通讯录**（与这一包其余写动词同一条规矩）：那间房在不在我的通讯录里就是答案；
    /// ⚠ 同参第二次会**再开一间同名房**（幂等键不同 ⇒ 两间房）⇒ 复核**弱在归因**。
    fn create_group(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let Some(name) = valid_name(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let payload = serde_json::json!({ PARAM_NAME: name }).to_string();
        let key = idempotency_key(operation);
        match self.exchange("POST", GROUPS_PATH, Some(payload), Some(&key)) {
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_CREATE_GROUP,
                render_group_created(&value),
            )]),
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 契约给这条端点列了 409：同一个键配了另一个群名（幂等冲突）。
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::IdempotencyConflict.as_str()),
            // 2xx 但体读不懂：**这次开房已被接受**，只是回执形状没读懂——如实说（标识用 im_rooms 看）。
            Reply::Unreadable => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_CREATE_GROUP,
                "opened = yes\n（这一版世界的回执形状我没读懂：房开了，但标识没读出来——用 im_rooms 看）\n",
            )]),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// **邀请一个主体进我持有的房**（`POST /v1/backend/rooms/{room}/members`，带幂等键）。
    ///
    /// **资格判据在世界那一侧**（"与邀请者有 accepted 关系，或被我以 active Agent Owner 拥有"）
    /// ——本层不复制它，只把世界的答复如实分类（与这一包其余写动词同一条纪律）。
    fn invite(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let (Some(room), Some(participant)) = (valid_room(params), valid_participant(params))
        else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let payload = serde_json::json!({ PARTICIPANT_ID_FIELD: participant }).to_string();
        let path = format!("{ROOMS_PATH}/{room}{MEMBERS_SUFFIX}");
        let key = idempotency_key(operation);
        match self.exchange("POST", &path, Some(payload), Some(&key)) {
            // 201（新成员）/ 200（同 key 重放）：这次邀请已被接受。
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_INVITE,
                render_invited(room, &value),
            )]),
            // 2xx 但体读不懂：**这次邀请已被接受**，只是回执形状没读懂——如实说，不编一个成员出来。
            Reply::Unreadable => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_INVITE,
                format!(
                    "room = {room}\ninvited = yes\n（这一版世界的回执形状我没读懂；这一条没有成员读面可查——要确认只能等他说话）\n"
                ),
            )]),
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 403 = 我不是这间房的 owner（或不是它的 active 成员）。
            Reply::Refused(403) => ActionOutcome::failure(ImFailure::NotAMember.as_str()),
            // 404 = 房不在 / 目标不存在（契约把两种合成一个码，本层也不分解）。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::IdempotencyConflict.as_str()),
            Reply::Refused(_) | Reply::Broken => ActionOutcome::unknown(),
        }
    }

    /// 写自己在群里的呈现：`PUT /v1/backend/rooms/{room}/my-presentation`（带幂等键）。
    ///
    /// **它是"我在这一间房里的样子"**：房 owner 的写权限一字不动，这一份归我自己。
    fn write_presentation(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(base_version) = valid_base_version(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        // 三个字段**都要给**（可以为 null = 清空那一项），与契约同形。
        let mut fields = serde_json::Map::new();
        for name in [
            PARAM_ROOM_DISPLAY_NAME,
            PARAM_ROOM_ROLE_NAME,
            PARAM_ROOM_ROLE_DESCRIPTION,
        ] {
            match valid_optional_text(params, name, MAX_PRESENTATION_TEXT_BYTES) {
                Some(value) => {
                    fields.insert(
                        name.to_string(),
                        value.map_or(serde_json::Value::Null, |text| {
                            serde_json::Value::String(text.to_string())
                        }),
                    );
                }
                None => return ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            }
        }
        let mut payload = fields;
        payload.insert(
            PARAM_BASE_VERSION.to_string(),
            serde_json::Value::from(base_version),
        );
        let payload = serde_json::Value::Object(payload).to_string();
        let path = MY_PRESENTATION_PATH.replace("{}", room);
        let key = idempotency_key(operation);
        match self.exchange("PUT", &path, Some(payload), Some(&key)) {
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_WRITE_PRESENTATION,
                render_presentation_written(&value),
            )]),
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 契约：房不存在 / 我不是成员 / 不是群房 ⇒ 同一个 404（不分解）。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::VersionConflict.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 看房间的 live 快照：`GET /v1/backend/rooms/{room}/live`（读取面对齐 V9）。
    ///
    /// **进行态，不是身体态**：它说的是"这间房里谁在、谁正在写"，**不**包含宿主读数
    /// （那些在执行端那一层，不进 IM）。
    ///
    /// 它与 Human 侧 `/v1/rooms/{id}/live` 是**同一个 application 用例、同一段投影**
    /// （服务端同一份 `live_routes` 伺候两条口子），名单里**人与 Agent 同形**。
    fn room_live(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let path = ROOM_LIVE_PATH.replace("{}", room);
        match self.exchange("GET", &path, None, None) {
            Reply::Answered(value) => match render_room_live(&value) {
                Ok(text) => {
                    ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_ROOM_LIVE, text)])
                }
                Err(failure) => ActionOutcome::failure(failure.as_str()),
            },
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 契约：房不存在 / 我不是成员 ⇒ 同一个 404（不分解）。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 声明「我正在这间房写」：`POST /v1/backend/rooms/{room}/typing`（204）。
    ///
    /// **易失、best-effort**：它不建立任何事实（服务端不落库、不产生事件），
    /// 几秒后自己过期，我发出的消息会取代它。所以：
    /// - 传输层没答上来**不当作失败**——进行态丢了不影响任何消息、任何公共事实；
    /// - 但**参数非法**仍如实报错（那是调用方的问题，不是信号的问题）。
    fn typing(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let path = ROOM_TYPING_PATH.replace("{}", room);
        match self.exchange("POST", &path, None, None) {
            Reply::Answered(_) | Reply::Broken | Reply::Unreadable => {
                ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_TYPING,
                    "已声明正在写（几秒后自动消失）".to_owned(),
                )])
            }
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 契约：房不存在 / 我不是成员 ⇒ 同一个 404（不分解）。
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownRoom.as_str()),
            Reply::Refused(_) => ActionOutcome::unknown(),
        }
    }

    /// **撤回的复核**（只读）：那条现在处于已撤回状态吗。
    ///
    /// 参数与 `im_recall` **完全相同**（复核问的是"这一次这一条"，参数不同就不是同一个问题）。
    ///
    /// 归因是**精确的**（按 message 身份判定，不是按正文，也不问身份探针）——它是本 App 里唯一
    /// 不需要 `whoami` 的复核。它同样**有界回翻**（[`ImApp::sweep`]）：那条可能早就不在新的一页里了。
    ///
    /// 两个方向都是**确定的答案**：找得到那条 ⇒ 它撤没撤回一目了然；**翻到房的起点**仍没有那条
    /// ⇒ 它不在这间房的历史里 ⇒ 这次撤回没有发生。**没看到底** ⇒ `unknown`。
    fn verify_recalled(&self, params: &ActionParams) -> ActionOutcome {
        let Some(room) = valid_room(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(message) = valid_message(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };

        match self.sweep(room, |rows| find_recalled(rows, message)) {
            // 已撤回 ⇒ 这次效果确实发生了。
            Sweep::Found {
                searched,
                whole,
                found: true,
            } => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_VERIFY_RECALLED,
                // 撤回复核**不比任何一格**（按消息身份判）——三个 `false` 就是这个意思，
                // 因此回执里不会出现 `compared` 那一行。
                render_verification(room, searched, whole, Some(message), false, false, false),
            )]),
            // 那条在，但它没撤回 ⇒ 一个确定的答案：这次效果没发生。
            Sweep::Found { found: false, .. } => {
                ActionOutcome::failure(ImFailure::NotFound.as_str())
            }
            // 翻到房的起点仍没有那条 ⇒ 它不在这间房的历史里 ⇒ 也没撤回。
            Sweep::Exhausted => ActionOutcome::failure(ImFailure::NotFound.as_str()),
            // 没看到底：不能声称"没有"。
            Sweep::Undecided => ActionOutcome::unknown(),
        }
    }

    /// 问服务器：**这个凭据代表谁**（契约里唯一的 Backend 认证探针 `GET /v1/backend/me`）。
    ///
    /// 归因必须精确到身份：复核比对的是 `actor.id`，**不**比显示名（名字不是身份）。
    /// 而"凭据代表谁"只有服务器说得准。本 App **不假设**Agent 在这台机器上的 AgentId 等于 Agent 在 im 里
    /// 的参与者身份——那是两套命名空间（本地身份归环境，im 身份归 im），也**不**让宿主转抄一份
    /// 来对齐：第二个来源就是第二个可能过期的真相。真实联跑实测过这一点：本地身份是
    /// `agent-t22`，而 Agent 在 im 里的参与者身份是一个 UUID，两者**从不相等**。
    ///
    /// ⚠️ **它是没缓存的那一层**（探针本体）。要身份的都走 [`ImApp::me`]——一次活动一次探针。
    fn whoami(&self) -> Result<String, ImFailure> {
        let body = self.read(ME_PATH)?;
        body.get("agent")
            .and_then(|agent| agent.get("id"))
            .and_then(|id| id.as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            // 200 却读不出身份：没有答案（不是"没有"）。
            .ok_or(ImFailure::Unreachable)
    }

    /// [`ImApp::whoami`] 的**一次活动一次**版本：第一次问，之后照抄那次答案（成功或失败都照抄）。
    fn me(&self) -> Result<String, ImFailure> {
        self.me.get_or_init(|| self.whoami()).clone()
    }

    /// 回执头的**身份行**：`me = <Agent 在 im 里的身份>`。
    ///
    /// 为什么要有它（TASK_67 记下的缺口）：Agent 在 im 里读到的每个 `actor` 都是**服务端的身份**，
    /// 与本机那个 `AgentId` **从不相等**（见 [`ImApp::whoami`]）——没有这一行，主体只能靠
    /// "我在别处发过言"反推自己是谁。
    ///
    /// 它是**读数**，不是身份声明：探不到就如实说 `unknown`，
    /// **不**拿本机身份或显示名顶替（缺省要能判读，不能悄悄不给这一行）。
    fn me_line(&self) -> String {
        match self.me() {
            Ok(id) => format!("me = {id}\n"),
            Err(_) => {
                "me = unknown\n（这一次没探到你在 im 里的身份；本机身份与显示名都不能顶替）\n"
                    .to_string()
            }
        }
    }

    /// **有界回翻**：从最新一页往旧翻，把每一页交给 `look` 判。
    ///
    /// 三种结局**都不猜**（这一层最重要的一条纪律）：
    /// - [`Sweep::Found`]：找到了。页是从新往旧走的，因此这就是**最新**的那一条；
    /// - [`Sweep::Exhausted`]：**翻到了房的起点**（那一页说没有更旧的了）——这一整段历史里都没有；
    /// - [`Sweep::Undecided`]：**没看到底**——触到 [`MAX_VERIFY_PAGES`]，或某一页读不出来
    ///   （传输失败 / 服务端报错 / 明确拒绝 / 体不是我们认得的形状）。**这种时候不许说"没有"。**
    ///
    /// 形状纪律：每一页**恰好一次**请求（不重试、不跳页）；游标**原样**回传（opaque，不解析、不构造）；
    /// 说"还有更旧的"却没给游标 ⇒ 继续不了 ⇒ `Undecided`（不猜）。
    ///
    /// **为什么值得这么严**（不是洁癖）：这条断言要回答的是"**确定没有**"，而松哪一边都有人付钱——
    /// 放宽（"没看见"也说没有）会让主体**重发一条已经发出的公共消息**；一概装傻（什么都 `unknown`）
    /// 会让繁忙房间里的**全部写冻死**在复核上。所以既不放宽也不装傻，而是**去看到底**，并把
    /// "没看到底"做成一个**声明的上界**＋一句诚实的读数（`more = older`）。
    fn sweep<T>(&self, room: &str, look: impl Fn(&[serde_json::Value]) -> Option<T>) -> Sweep<T> {
        let mut searched = 0usize;
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_VERIFY_PAGES {
            let path = history_path(room, VERIFY_WINDOW, cursor.as_deref());
            let Ok(value) = self.read(&path) else {
                // 读不出来：没有回答"在不在"。
                return Sweep::Undecided;
            };
            let Some((window, rows)) = window_of(&value) else {
                return Sweep::Undecided;
            };
            searched += window.searched;
            if let Some(found) = look(rows) {
                return Sweep::Found {
                    searched,
                    whole: window.whole,
                    found,
                };
            }
            if window.whole {
                return Sweep::Exhausted;
            }
            match window.cursor {
                Some(next) => cursor = Some(next),
                // 说"还有更旧的"却没给游标：继续不了（不猜、不构造游标）。
                None => return Sweep::Undecided,
            }
        }
        // 触到页数上限：还有更旧的没看。
        Sweep::Undecided
    }

    /// 一次请求（读与写都走这里）。**不重试、不退避、不换端点**。
    ///
    /// **写**动作一次调用恰好一次请求——这是最要紧的那条（绝不重发）；只读复核是**有界回翻**
    /// （[`ImApp::sweep`]，最多 [`MAX_VERIFY_PAGES`] 页；`im_verify_said` 另加**恰好一次**
    /// [`ImApp::whoami`]），每一步都是幂等读、各自恰好一次。
    ///
    /// ⚠ **2026-10-10 起有一处例外，写在这里免得被读成违规**：`im_read_history` 一次调用发
    /// **两次**请求（读那一页 ＋ **顺带推进读位置**，见 [`ImApp::read_history`]）。
    /// 它**不是重试**——是**两件不同的事**（"我读到了"与"世界知道我读到哪"），因此回执把两者
    /// **分开报**：读成功而推进失败时，读仍然是成功的，回执多一句实话。
    ///
    /// 它只做「我们读到了什么」的机械搬运，**不替调用方决定这算不算失败、更不给分类**——读侧、说话侧
    /// 与撤回侧对同一个状态码的解释**不同**（分类是契约知识，见 [`Reply::Refused`]）。
    /// 凭据只在请求头里出现一次，任何分支都不把它带进返回值。
    ///
    /// ⚠ 机械搬运在**中立层**（[`crate::im_channel::ImChannel`]）：这一层只是本 App 的入口别名。
    fn exchange(
        &self,
        method: &str,
        path: &str,
        payload: Option<String>,
        key: Option<&str>,
    ) -> Reply {
        self.channel.exchange(method, path, payload, key)
    }

    /// 取**字节**（附件下载）：响应不是 JSON，是二进制 + `Content-Type`。
    ///
    /// 上限**在读之前先判一次**（`Content-Length` 已知且超限就直接停），再按 `max + 1` 限量读：
    /// "太大"因此既不会把 16 MiB 拉进内存，也不会被悄悄截断成一份看起来完整的内容。
    /// 类型只在响应头里取（**不嗅探字节**——通道那条纪律在这一侧同样成立）。
    fn fetch_bytes(&self, path: &str, max: usize) -> ByteReply {
        self.channel.fetch_bytes(path, max)
    }

    /// 读侧的回程解释：明确的拒绝 → 本 App 的分类；服务端报错 → `server-error`；
    /// 传输失败或读不懂 → `unreachable`。
    fn read(&self, path: &str) -> Result<serde_json::Value, ImFailure> {
        match self.exchange("GET", path, None, None) {
            Reply::Answered(value) => Ok(value),
            // 读侧（通讯录 / 翻记录 / 身份探针）对同一个码的解释。这里的 `404` 是**房**层面
            // 的（房不存在，或我不是成员）——撤回侧的 `404` 含义不同，它有自己的解释。
            Reply::Refused(400) => Err(ImFailure::InvalidArgument),
            Reply::Refused(401) => Err(ImFailure::CredentialRejected),
            Reply::Refused(403) => Err(ImFailure::NotAMember),
            Reply::Refused(404) => Err(ImFailure::UnknownRoom),
            // 契约没列出的其它非 2xx（例如 422 / 5xx）：服务端明确报错，但没告诉我们任何
            // 契约语义 ⇒ 读侧报 `server-error`。
            Reply::Refused(_) => Err(ImFailure::ServerError),
            Reply::Broken | Reply::Unreadable => Err(ImFailure::Unreachable),
        }
    }
}

impl ActionPort for ImApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.own_capabilities()
    }

    /// 局部准入：本包的动词一律放行。**本 App 不做效果类别判断**——类别是各 App 自己声明的，
    /// 装配期与展开期由 `check_declarations` 按它做自洽性检查（"写效应必配复核动词"）。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_ROOMS
            | CAPABILITY_READ_HISTORY
            | CAPABILITY_READ_ATTACHMENT
            | CAPABILITY_SAY
            | CAPABILITY_VERIFY_SAID
            | CAPABILITY_RECALL
            | CAPABILITY_VERIFY_RECALLED
            | CAPABILITY_MARK_READ
            | CAPABILITY_LEAVE_ROOM
            | CAPABILITY_CREATE_GROUP
            | CAPABILITY_INVITE
            | CAPABILITY_ROOM_MEMBERS
            | CAPABILITY_REMOVE_MEMBER
            | CAPABILITY_TRANSFER_OWNER
            | CAPABILITY_WRITE_PRESENTATION
            | CAPABILITY_ROOM_LIVE
            | CAPABILITY_TYPING => ActionAdmission::Admitted,
            _ => ActionAdmission::refused(crate::core::UNKNOWN_CAPABILITY),
        }
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match action.id().as_str() {
            CAPABILITY_ROOMS => self.rooms(action.params()),
            CAPABILITY_READ_HISTORY => self.read_history(action.params()),
            // 读附件：只读（未声明即只读）⇒ 不需要任何授权。
            CAPABILITY_READ_ATTACHMENT => self.read_attachment(action.params()),
            // 写：幂等键从**操作身份**合成，因此同一次操作重复投递至多落一条。
            CAPABILITY_SAY => self.say(operation, action.params()),
            // 复核：读的是"我发的消息"——**这个"我"由服务器回答**（凭据代表谁），
            // 因此执行端给的本地 AgentId 在这里用不上（见 [`ImApp::whoami`]）。
            CAPABILITY_VERIFY_SAID => self.verify_said(action.params()),
            // 写：**不带键**（契约要求；幂等由 message 身份承担，见 [`ImApp::recall`]）。
            CAPABILITY_RECALL => self.recall(action.params()),
            // 复核：按消息身份归因，因此既不需要本地 AgentId，也不需要身份探针。
            CAPABILITY_VERIFY_RECALLED => self.verify_recalled(action.params()),
            // 写：**不带键**（单调推进天然幂等，见 [`ImApp::mark_read`]）。
            CAPABILITY_MARK_READ => self.mark_read(action.params()),
            // 写：**不带键**（幂等由"成员资格"承担，见 [`ImApp::leave_room`]）。
            CAPABILITY_LEAVE_ROOM => self.leave_room(action.params()),
            // 开群房（2026-10-13）：**带幂等键**（同一次操作重复投递至多落一间房）。
            CAPABILITY_CREATE_GROUP => self.create_group(operation, action.params()),
            // 邀请（2026-10-13）：**带幂等键**（契约要求；同 key 重放不重复邀请）。
            CAPABILITY_INVITE => self.invite(operation, action.params()),
            // 读成员清单（2026-10-13）：只读，不出网之外什么都不动。
            CAPABILITY_ROOM_MEMBERS => self.room_members(action.params()),
            // 治理两条（2026-10-13）：移除**不带键**（幂等由"他还在不在"承担）；
            // 转让**带键**（契约要求，同 key 重放）。
            CAPABILITY_REMOVE_MEMBER => self.remove_member(action.params()),
            CAPABILITY_TRANSFER_OWNER => self.transfer_owner(operation, action.params()),
            // 表达权第三刀：写自己在群房里的呈现（**复核面是通讯录**——见 `declared_effects`）。
            CAPABILITY_WRITE_PRESENTATION => self.write_presentation(operation, action.params()),
            // 读取面对齐（V9）：看房间的 live 快照（只读）。
            CAPABILITY_ROOM_LIVE => self.room_live(action.params()),
            // 写：**不带键**——声明本身没有"这一条还是那一条"的身份，重发
            // 只是把同一个"我正在写"续期（单调、天然幂等，同 `im_mark_read`）。
            CAPABILITY_TYPING => self.typing(action.params()),
            _ => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
        }
    }
}

impl crate::app_contract::AppPort for ImApp {
    fn app_id(&self) -> &str {
        IM_APP_ID
    }

    fn description(&self) -> &str {
        "Celira IM：看我参与的房间与房内记录、读房内消息的附件，可以在我参与的那些房间里说一句话（也可以引用某一条、点名成员、附上一份文件），也可以自己开一间群房、把有来往的人拉进来，还可以撤掉自己刚说出的那条（只能碰 im 自己的消息、附件与房间，不能改任何别的东西）"
    }

    fn declared_capabilities(&self) -> Capabilities {
        // 声明必须与实例的真实能力一致：OS 的归属索引就是按这份声明建的。
        self.own_capabilities()
    }

    /// 本包的写动词各配一项只读复核（前两项参数完全相同，第三项以通讯录那一栏为复核面）。
    ///
    /// - `im_say`：**可重复写**——同参数第二次是一条**新**消息（身份不是参数决定的，不是抢唯一
    ///   名字），因此必须有复核。
    /// - `im_recall`：也声明为**可重复写**，理由**不是**"它每次产生新效果"（它不会：第二次撤回
    ///   是同一条事实的 200 重放），而是**这一档在两个后果上都偏严**（必须有复核入口）。
    ///   三档里没有更准的一档——`exclusive-create` 的定义是"同参数第二次必然失败"，而撤回的第二次
    ///   是**成功**（重放）。**命名不精确这件事写在这里，不糊**：将来若出现第二个同形状的写
    ///   （幂等写），再带着两例证据一次裁清类别语义（见 `outputs/AGENT_NEXT_STEP_DECISION_SHEET.md` §A）。
    ///
    /// - `im_typing`（2026-10-06 进能力面）：**确实是写**（POST 改的是世界那张进程内易失表），
    ///   但**没有同参只读复核可配**——信号几秒自灭，等读回来已经归因不了 ⇒ 第四档
    ///   [`crate::app_contract::EffectClass::UnverifiableWrite`]（按定义**豁免复核**，
    ///   与 `terminal_run` 同档）。⚠ 它**不是**"不声明即只读"那一类：不声明会把一个 POST
    ///   说成只读，那是对世界撒谎。
    ///
    /// 只读动词不需要声明（未声明即只读）：通讯录、翻记录、**读附件**与各次复核。
    /// ⚠ 社交 / 资料两组写动词的声明跟着动词搬进了各自的新包（`im-social` / `im-profile`）。
    fn declared_effects(&self) -> Vec<crate::app_contract::EffectDeclaration> {
        use crate::app_contract::{EffectClass, EffectDeclaration};
        vec![
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_SAY),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_SAID)),
            ),
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_RECALL),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_RECALLED)),
            ),
            // 读位置推进（切片 M1）：**同参数第二次是成功 no-op**（`advanced = false`，零新事实）
            // ——它既不是"第二次必然失败"（`exclusive-create`），也不是"每次都产生新的一份效果"
            // （`repeatable-write` 的字面定义），更不是"没有同参复核可配"（`unverifiable-write`）。
            // 与 `im_recall` 同形状（同参第二次成功、什么也没多发生），因此**按同一先例**归
            // `repeatable-write`；不精确处写在 `outputs/AGENT_IDEMPOTENT_WRITE_EFFECT_CLASS_DECISION_SHEET.md`
            // （2026-09-27 主人裁：不为此加第五档——档位今天只驱动"要不要配复核"，加它不改变行为）。
            // **复核 = 通讯录的 `read` 栏**：读一次就知道世界记的是不是这条（同一条只读面，零新动词）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_MARK_READ),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_ROOMS)),
            ),
            // 退房（切片 M2）：**同参数第二次必然失败**（退过之后不再是成员 ⇒ 404），
            // 本次效果没有发生——这正是 `exclusive-create` 的定义（"第二次必然失败"），
            // 因此它归这一档，而不是 `repeatable-write`。
            // **复核 = 通讯录**：那间房还在不在我的通讯录里，就是答案（同一条只读面）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_LEAVE_ROOM),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(CAPABILITY_ROOMS)),
            ),
            // 写房呈现（表达权第三刀）：每（房 × 我）一条独立版本链；同 key 同摘要重放
            // 不产生新版本 ⇒ 同一档。**复核 = 通讯录**（房成员呈现那条读面就在它里面）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_WRITE_PRESENTATION),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_ROOMS)),
            ),
            // 开群房（2026-10-13）：**可重复写**——同参第二次会开出**另一间同名房**（幂等键不同），
            // 因此复核**弱在归因**。**复核 = 通讯录**（那间房在不在我的通讯录里）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_CREATE_GROUP),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_ROOMS)),
            ),
            // 邀请（2026-10-13）：**可重复写** ＋ **复核 = 成员清单**——同 key 重放不重复邀请，
            // 而异 key 再邀请一次仍是"他在房里"（同参第二次成功、零新事实）。
            // ⚠ 它当天先落在**第四档**（Agent 侧当时没有成员读面）；同一天补上 `im_room_members`
            // 之后按判据升回这一档——"系统能不能机械判断这一格"的答案变了，声明跟着变。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_INVITE),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_ROOM_MEMBERS)),
            ),
            // 移除成员（2026-10-13）：**同参数第二次必然失败**（他已经不在里面 ⇒ 404）⇒ 与退房同档。
            // **复核 = 成员清单**（他还在不在那份清单里）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_REMOVE_MEMBER),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(CAPABILITY_ROOM_MEMBERS)),
            ),
            // 转让房主（2026-10-13）：同参数第二次必然失败（**我已经不是 owner 了** ⇒ 403/404）
            // ⇒ 与退房同档。**复核 = 成员清单**（现在谁写着 owner）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_TRANSFER_OWNER),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(CAPABILITY_ROOM_MEMBERS)),
            ),
            // 正在写（`im_typing`）：第四档——**是写，但没有同参只读复核可配**
            // （信号 6 秒自灭，读回来归因不了"这一次声明发生过没有"）。
            // 它因此**不配复核能力**：写类别默认要求配复核，而这一档按定义豁免。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_TYPING),
                EffectClass::UnverifiableWrite,
                None,
            ),
        ]
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// 本包的只读动词（通讯录 / 翻记录 / 读附件 / 两次复核 / 房间进行态）带回的是**世界上别人
    /// 说过的话**，而且每一段都能指回一个明确的发出者（房成员、撤回标记、附件所有者），因此是
    /// [`crate::app_contract::SourceClass::WorldSigned`]。本包的写动词只回报 im 服务端对这次动作的
    /// 确认（消息标识 + 时刻 / 世界记下的新位置 / 房呈现的新版本），回执里没有外来字，因此是
    /// [`crate::app_contract::SourceClass::System`]——**正在写**（`im_typing`）同属这一档：
    /// 它回报的只是服务端收下了这次声明（204），回执那句是 App 自己写的固定字。
    ///
    /// ⚠️ `world-signed` **不等于可信**：一个署了名的对端照样可以骗主体。这一档只保证"混淆不是
    /// 系统造成的"——判断归自己。
    fn declared_sources(&self) -> Vec<crate::app_contract::SourceDeclaration> {
        use crate::app_contract::{SourceClass, SourceDeclaration};
        self.declared_capabilities()
            .items()
            .iter()
            .map(|capability| {
                let class = match capability.id().as_str() {
                    CAPABILITY_SAY
                    | CAPABILITY_RECALL
                    | CAPABILITY_MARK_READ
                    | CAPABILITY_LEAVE_ROOM
                    | CAPABILITY_CREATE_GROUP
                    | CAPABILITY_INVITE
                    | CAPABILITY_WRITE_PRESENTATION
                    | CAPABILITY_TYPING => SourceClass::System,
                    _ => SourceClass::WorldSigned,
                };
                SourceDeclaration::new(capability.id().clone(), class)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 参数解析（工具面先拒绝 ⇒ 非法值一次都不出网）
// ---------------------------------------------------------------------------

/// `since`：可选非负 unix 毫秒。非十进制、负数、溢出 ⇒ 参数非法。
fn parse_since(raw: Option<&str>) -> Result<Option<i64>, ImFailure> {
    match raw {
        None => Ok(None),
        Some(text) => match text.trim().parse::<i64>() {
            Ok(millis) if millis >= 0 => Ok(Some(millis)),
            _ => Err(ImFailure::InvalidArgument),
        },
    }
}

/// `limit`：可选 1–[`MAX_HISTORY_LIMIT`]；越界、非十进制、0 一律参数非法。
fn parse_limit(raw: Option<&str>) -> Result<u32, ImFailure> {
    match raw {
        None => Ok(DEFAULT_HISTORY_LIMIT),
        Some(text) => match text.trim().parse::<u32>() {
            Ok(limit) if (1..=MAX_HISTORY_LIMIT).contains(&limit) => Ok(limit),
            _ => Err(ImFailure::InvalidArgument),
        },
    }
}

// ---------------------------------------------------------------------------
// 回执渲染（只读原子工具的读数惯例：首行给机械读数，之后一行一条）
// ---------------------------------------------------------------------------

/// 房间进行态快照的回执：一行一个**成员**（人与 Agent 同形）。
///
/// **零正文**：只说"谁 · 在不在 · 是不是正在写"，内容自己去读——与通知栏同一条律。
/// **零执行面**：没有阶段、没有草稿、没有生成流——那些不进 IM（2026-09-28 裁决）。
fn render_room_live(body: &serde_json::Value) -> Result<String, ImFailure> {
    let members = body
        .get("members")
        .and_then(|members| members.as_array())
        .ok_or(ImFailure::Unreachable)?;
    let online = members
        .iter()
        .filter(|m| m.get("online").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    let typing = members
        .iter()
        .filter(|m| m.get("typing").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    let mut text = format!(
        "members = {}, online = {}, typing = {}\n",
        members.len(),
        online,
        typing
    );
    for member in members {
        let field = |name: &str| {
            member
                .get(name)
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        };
        let flag = |name: &str| {
            if member.get(name).and_then(|v| v.as_bool()).unwrap_or(false) {
                "yes"
            } else {
                "no"
            }
        };
        text.push_str(&format!(
            "{} · {} · {} · online = {} · typing = {}\n",
            field("participant_id"),
            field("display_name"),
            field("kind"),
            flag("online"),
            flag("typing"),
        ));
    }
    Ok(text)
}

/// 房呈现三个字段的字节上限（契约：显示名 512、角色名 512、角色描述 16384）。
const MAX_PRESENTATION_TEXT_BYTES: usize = 16 * 1024;

/// 写房呈现的回执：写完后的版本 + 是不是重放 + 落下的三项。
fn render_presentation_written(body: &serde_json::Value) -> String {
    let presentation = body.get("presentation");
    let field = |name: &str| {
        presentation
            .and_then(|p| p.get(name))
            .and_then(|value| value.as_str())
            .unwrap_or("-")
    };
    let version = presentation
        .and_then(|p| p.get("presentation_version"))
        .and_then(|value| value.as_u64())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".to_string());
    let replayed = body
        .get("idempotency_replayed")
        .and_then(|flag| flag.as_bool())
        .unwrap_or(false);
    format!(
        "version = {version}\nroom_display_name = {}\nroom_role_name = {}\nroom_role_description = {}\nreplayed = {}",
        field("room_display_name"),
        field("room_role_name"),
        field("room_role_description"),
        if replayed { "yes" } else { "no" },
    )
}

/// `im_create_group` 的群名：非空、单行（不含控制字符）、在上限之内。
///
/// 与 notes 的标题规则同一条理由：它要出现在通讯录那一行里，带换行的名字会伪造出另一行。
fn valid_name(params: &ActionParams) -> Option<&str> {
    let name = params.get(PARAM_NAME)?;
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return None;
    }
    Some(name)
}

/// `im_invite` 的目标标识：非空、在上限之内、不含控制字符。
///
/// **形状不在这里判**（标识是世界的词，UUID 只是它今天的样子）：这一层只挡住"明显不是标识"的
/// 输入，**认不认识这个主体归世界**（它会回 404）。
fn valid_participant(params: &ActionParams) -> Option<&str> {
    let participant = params.get(PARAM_PARTICIPANT)?;
    if participant.is_empty()
        || participant.len() > MAX_PARTICIPANT_BYTES
        || participant.chars().any(char::is_control)
    {
        return None;
    }
    Some(participant)
}

/// 开房那条端点的回执（`{room: {id, name, …}, …}`）。
///
/// 读不懂就**如实说读不懂**，不编一个标识出来（房确实开了，标识让主体用 `im_rooms` 看）。
fn render_group_created(value: &serde_json::Value) -> String {
    let room = value.get("room");
    let id = room
        .and_then(|room| room.get("id"))
        .and_then(|id| id.as_str());
    let name = room
        .and_then(|room| room.get("name"))
        .and_then(|name| name.as_str());
    match (id, name) {
        (Some(id), Some(name)) => format!("room = {id}\nname = {name}\nowner = me\n"),
        _ => {
            "opened = yes\n（这一版世界的回执形状我没读懂：房开了，但标识没读出来——用 im_rooms 看）\n"
                .to_string()
        }
    }
}

/// 成员清单那条端点的回执（`{members: [{participant: {id, display_name, …}, membership: {role, joined_at, …}}, …]}`）。
///
/// 读不懂就**如实说读不懂**（不编成员出来）：那说明这一版世界的回执形状变了，用 `im_rooms`
/// 与 `im_room_live` 交叉看。
fn render_removed(room: &str, participant: &str) -> String {
    format!("room = {room}\nremoved = {participant}\n（那间房与它的历史都还在；他会知道）")
}

fn render_transferred(room: &str, participant: &str, value: &serde_json::Value) -> String {
    let replayed = value
        .get("idempotency_replayed")
        .and_then(|flag| flag.as_bool())
        .map(|flag| if flag { "yes" } else { "no" })
        .unwrap_or("?");
    format!("room = {room}\nowner = {participant}\nreplayed = {replayed}")
}

fn render_members(value: &serde_json::Value) -> String {
    let Some(members) = value.get("members").and_then(|members| members.as_array()) else {
        return "members = ?\n（这一版世界的回执形状我没读懂：成员清单没读出来）\n".to_string();
    };
    let mut text = format!("members = {}\n", members.len());
    for member in members {
        let id = member
            .get("participant")
            .and_then(|participant| participant.get("id"))
            .and_then(|id| id.as_str())
            .unwrap_or("-");
        let name = member
            .get("participant")
            .and_then(|participant| participant.get("display_name"))
            .and_then(|name| name.as_str())
            .unwrap_or("-");
        let role = member
            .get("membership")
            .and_then(|membership| membership.get("role"))
            .and_then(|role| role.as_str())
            .unwrap_or("?");
        let joined = member
            .get("membership")
            .and_then(|membership| membership.get("joined_at"))
            .and_then(|joined| joined.as_str())
            .unwrap_or("-");
        text.push_str(&format!("{id} · {name} · {role} · {joined}\n"));
    }
    text
}

/// 邀请那条端点的回执（`{membership: {participant_id, role, …}, …}`）。
fn render_invited(room: &str, value: &serde_json::Value) -> String {
    let membership = value.get("membership");
    let participant = membership
        .and_then(|membership| membership.get("participant_id"))
        .and_then(|id| id.as_str());
    let role = membership
        .and_then(|membership| membership.get("role"))
        .and_then(|role| role.as_str());
    match (participant, role) {
        (Some(participant), Some(role)) => {
            format!("room = {room}\nmember = {participant}\nrole = {role}\n")
        }
        _ => format!(
            "room = {room}\ninvited = yes\n（这一版世界的回执形状我没读懂；这一条没有成员读面可查——要确认只能等他说话）\n"
        ),
    }
}

/// 房间标识的机械校验：**非空 ＋ ≤ [`MAX_ID_BYTES`] 字节**（超限 ⇒ `invalid-argument`，
/// **一次都不出网**；正反两条用例在 `tests/im_app.rs`）。
///
/// **房间这一个参数只有一个实现**：本 App 里凡是带房间标识的动词都走它——读历史那条路曾经
/// 自己另写一版（只判了非空），理由与收口经过见 [`ImApp::read_history`] 的注释；
/// `MAX_FILE_PARAM_BYTES` 那条"同一种形状只有一个答案"在这里同样成立。
///
/// **形状不在这里判**：标识是世界的词（今天的形状是 36 个字符的 UUID v4），这一层只挡住
/// "明显不是标识"的输入，认不认识这间房归世界（它会答 403 / 404）。因此这里的界比世界真正
/// 签发的形状宽——宽出来的那一段不构成放宽，合法的房号一个都不会被拦。
fn valid_room(params: &ActionParams) -> Option<&str> {
    params
        .get(PARAM_ROOM)
        .filter(|room| !room.is_empty() && room.len() <= MAX_ID_BYTES)
}

/// 说话的正文校验：非空、≤ [`MAX_BODY_BYTES`] 字节、不含 NUL（契约：超限**拒绝不截断**）。
fn valid_body(params: &ActionParams) -> Option<&str> {
    params
        .get(PARAM_BODY)
        .filter(|body| !body.is_empty() && body.len() <= MAX_BODY_BYTES && !body.contains('\0'))
}

/// 说话的正文（带 `file` 的版本）：**给了就必须合规**；**没给**是不是合法由 `has_file` 决定
/// ——契约要求"正文与附件至少一个"：只递一份文件是合法的，只有空正文不是。
///
/// 返回 `None` = 参数非法；`Some(None)` = 这次没有正文。
fn valid_optional_body(params: &ActionParams, has_file: bool) -> Option<Option<&str>> {
    match params.get(PARAM_BODY) {
        None => has_file.then_some(None),
        // 空串：带文件时等价于"没给正文"；没带文件就是非法（空正文不是"没说"）。
        Some("") => has_file.then_some(None),
        // 其余交给那一条形状规则（长度与 NUL 都在那里判，规则只有一份）。
        Some(_) => valid_body(params).map(Some),
    }
}

/// `file` 参数的**形状**校验：可选；给了就必须非空、≤ [`MAX_FILE_PARAM_BYTES`] 字节、不含 NUL。
///
/// ⚠️ 这里**只判形状**："它是不是一个可读的沙盒相对路径"由 `files` 的那条路径规则
/// （唯一权威）在执行那一步判——两处各判一半，谁也不重复谁。
fn valid_file(params: &ActionParams) -> Result<Option<&str>, ImFailure> {
    match params.get(PARAM_FILE) {
        None => Ok(None),
        Some(file)
            if !file.is_empty() && file.len() <= MAX_FILE_PARAM_BYTES && !file.contains('\0') =>
        {
            Ok(Some(file))
        }
        Some(_) => Err(ImFailure::InvalidArgument),
    }
}

/// 附件的身份校验：**形状**上必须是契约的严格规范小写连字符 UUID v4（与消息身份同一套规则）。
///
/// 只做形状判断——不做存在性判断：形状不对就在本地拒绝（零出网），形状对就如实发出去，
/// 由服务端裁决（服务端把一切不可读合成同一个 `404`，这一层不猜）。
fn valid_attachment(params: &ActionParams) -> Option<&str> {
    let attachment = params.get(PARAM_ATTACHMENT).filter(|id| !id.is_empty())?;
    canonical_uuid(attachment).then_some(attachment)
}

/// `file_name` 参数：可选。给了就必须是一条**形状合法的名字**（非空、≤ [`MAX_FILE_NAME_BYTES`]
/// 字节、不含 CR / LF / NUL）——它要进回执那张**按行组织的读数表**，形状不对会破坏行结构。
///
/// ⚠️ 这里**不校验它是不是真的**：名字不是身份（身份是 `attachment`），本函数只保证它是
/// 一条**形状合法的读数**，不假装自己验证过它。
fn valid_file_name(params: &ActionParams) -> Result<Option<&str>, ImFailure> {
    match params.get(PARAM_FILE_NAME) {
        None => Ok(None),
        Some(name)
            if !name.is_empty()
                && name.len() <= MAX_FILE_NAME_BYTES
                && !name.contains(['\r', '\n', '\0']) =>
        {
            Ok(Some(name))
        }
        Some(_) => Err(ImFailure::InvalidArgument),
    }
}

/// `offset`（读附件）：可选**行号**（从 1 起），缺省 1。非十进制 / 0 / 溢出 ⇒ 参数非法（零出网）。
fn parse_attachment_offset(raw: Option<&str>) -> Result<usize, ImFailure> {
    match raw {
        None => Ok(1),
        Some(text) => match text.trim().parse::<usize>() {
            Ok(offset) if offset >= 1 => Ok(offset),
            _ => Err(ImFailure::InvalidArgument),
        },
    }
}

/// 从沙盒相对路径里取**要递给服务端的文件名**（最后一段）。
///
/// 它必须是一段**单层名字**：不许含引号 / CR / LF / 制表 / NUL（那几样能改写 multipart 头的
/// 结构）、不许是空名。取不出来就拒绝——那是**递出去的名字**，不许把本地路径结构漏出去。
/// （`..` 与绝对路径已经在读那一步被 `files` 的路径规则拦掉，这里再拦一道，因为名字要进头。）
fn file_name_of(file: &str) -> Result<String, ImFailure> {
    let name = file
        .rsplit_once(['/', '\\'])
        .map(|(_, tail)| tail)
        .unwrap_or(file);
    if name.is_empty()
        || name.len() > MAX_FILE_NAME_BYTES
        || name.contains(['"', '\r', '\n', '\t', '\0'])
        || name == "."
        || name == ".."
    {
        return Err(ImFailure::InvalidArgument);
    }
    Ok(name.to_string())
}

/// 撤回的消息身份校验：**形状**上必须是契约的严格规范小写连字符 UUID v4。
///
/// 只做形状判断（长度、字符集、四个连字符的位置）——**不做存在性判断**，也不试图猜服务端会不会
/// 认它：形状不对就在本地拒绝（零出网），形状对就如实发出去，由服务端裁决。
fn valid_message(params: &ActionParams) -> Option<&str> {
    let message = params.get(PARAM_MESSAGE).filter(|id| !id.is_empty())?;
    canonical_uuid(message).then_some(message)
}

/// 说话的**引用对象**校验：可选；给了就必须是契约的严格规范小写连字符 UUID v4。
///
/// 与 `message` / `attachment` 同一条形状规则（`canonical_uuid`，规则只有一份）：只判形状——
/// 形状不对就在本地拒绝（零出网）；形状对就如实发出去，由服务端裁决（撤回 / 未知 / 跨房 /
/// 不可见合成同一个 `400 invalid_reply_target`，本层不猜、不分解）。
fn valid_reply_to(params: &ActionParams) -> Result<Option<&str>, ImFailure> {
    match params.get(PARAM_REPLY_TO) {
        None => Ok(None),
        Some(id) if !id.is_empty() && canonical_uuid(id) => Ok(Some(id)),
        Some(_) => Err(ImFailure::InvalidArgument),
    }
}

/// 说话的**提及集合**校验：可选；给了就是**逗号分隔**的成员身份列表（见 [`PARAM_MENTIONS`]）。
///
/// 每段去空白后必须是规范 UUID v4、非空、互不相同，且总数不超过 [`MAX_MENTION_TARGETS`]——
/// 任何一条不满足都在本地拒绝（零出网）。**只判形状**：目标是不是本房 active 成员、房间是不是
/// group，由服务端在提交事务内裁决（`400 invalid_mention`，同样不分解）。
fn valid_mentions(params: &ActionParams) -> Result<Vec<&str>, ImFailure> {
    let Some(raw) = params.get(PARAM_MENTIONS) else {
        return Ok(Vec::new());
    };
    if raw.is_empty() {
        return Err(ImFailure::InvalidArgument);
    }
    let mut targets: Vec<&str> = Vec::new();
    for piece in raw.split(',') {
        let id = piece.trim();
        if id.is_empty() || !canonical_uuid(id) || targets.contains(&id) {
            return Err(ImFailure::InvalidArgument);
        }
        targets.push(id);
    }
    if targets.len() > MAX_MENTION_TARGETS {
        return Err(ImFailure::InvalidArgument);
    }
    Ok(targets)
}

/// 说话的回执：房间、消息身份，服务端说的"这是重放"这个事实，**服务端回执里这条引用了哪一条**
/// （是回复才有这一行——它取的是世界给的那一格，不是复述请求，因此能与读侧直接对上），
/// 以及**这次递上去的附件身份**。
///
/// 附件那一行只在真的带了文件时出现：它就是"这份文件已经随这条消息绑定了"的证据（一步递出去
/// 的那次调用里，主体是看不到上传那一步的）。
fn render_said(
    room: &str,
    message: &str,
    replayed: bool,
    reply_to: Option<&str>,
    attachment: Option<&str>,
) -> String {
    let mut text = format!(
        "room = {}, message = {}\nreplayed = {}",
        escaped_field(room),
        escaped_field(message),
        if replayed { "yes" } else { "no" }
    );
    if let Some(reply_to) = reply_to {
        text.push_str(&format!("\nreply_to = {}", escaped_field(reply_to)));
    }
    if let Some(attachment) = attachment {
        text.push_str(&format!("\nattachment = {}", escaped_field(attachment)));
    }
    text
}

/// 撤回的回执：房间、消息身份、撤回时点，以及服务端说的"这是重放"这个事实。
///
/// 撤回时点是**世界给的时刻** ⇒ 摆给主体时换成本机刻度（见 [`localize_rfc3339`]）。
fn render_recall(room: &str, message: &str, recalled_at: &str, replayed: bool) -> String {
    format!(
        "room = {}, message = {}, recalled_at = {}\nreplayed = {}",
        escaped_field(room),
        escaped_field(message),
        escaped_field(&localize_rfc3339(recalled_at)),
        if replayed { "yes" } else { "no" }
    )
}

/// 退房的回执：房间、**世界已经记下你退出了**。
///
/// 这条端点回 204（无响应体），所以回执里的话**只有一件事**：这次退出被接受了。
/// 不补"你已经不在里面了"之类的话——那是**世界的状态**，主体自己用 `im_rooms` 读得到
/// （那也正是本动词的复核面）。
fn render_left(room: &str) -> String {
    format!("room = {}, left = yes", escaped_field(room))
}

/// 从一份 `read_state` 投影里取出**位置与未读**，用**同一套三态词**渲染。
///
/// - 位置：`<消息标识>` / `-`（世界说「还没读过」）/ `?`（**读不到**）；
/// - 未读：`<n>` / `?`（**读不到**）——**读不到不写 0**（把"没读到"说成"没有未读"是最坏的那种谎）。
///
/// 通讯录与推进回执**共用本函数**：同一份事实的两种渲染，词必须是同一套（两个来源、两套词，
/// 迟早只改一边）。
fn read_state_fields(read_state: Option<&serde_json::Value>) -> (String, String) {
    let read = match read_state.and_then(|state| state.get("last_read_message_id")) {
        Some(serde_json::Value::String(id)) if !id.is_empty() => escaped_field(id),
        Some(serde_json::Value::Null) => "-".to_string(),
        _ => "?".to_string(),
    };
    let unread = match read_state
        .and_then(|state| state.get("unread_count"))
        .and_then(|value| value.as_i64())
        .filter(|count| *count >= 0)
    {
        Some(count) => count.to_string(),
        None => "?".to_string(),
    };
    (read, unread)
}

/// 推进读位置的回执：房间、**这次真的推进了没有**、世界记下的新位置与未读。
///
/// 位置与未读那两栏由 [`read_state_fields`] 渲染（与通讯录同一套三态词）。
fn render_mark_read(room: &str, advanced: bool, read: &str, unread: &str) -> String {
    format!(
        "room = {}, advanced = {}\nread = {read}, unread = {unread}",
        escaped_field(room),
        if advanced { "yes" } else { "no" },
    )
}

/// 一条房内历史路径（可选往更旧的一页翻）。
///
/// 游标是**不透明**的：只原样回传上一页给的 `next_before`，不解析、不构造、不改写。
fn history_path(room: &str, limit: u32, before: Option<&str>) -> String {
    match before {
        Some(cursor) => format!(
            "{ROOM_HISTORY_PREFIX}{room}{ROOM_HISTORY_SUFFIX}?{PARAM_LIMIT}={limit}&{PARAM_BEFORE}={cursor}"
        ),
        None => format!("{ROOM_HISTORY_PREFIX}{room}{ROOM_HISTORY_SUFFIX}?{PARAM_LIMIT}={limit}"),
    }
}

/// 一次**有界回翻**的结局（见 [`ImApp::sweep`]）。
enum Sweep<T> {
    /// 找到了：`searched` 是累计看了多少条，`whole` 说明"找到它的那一页是否已经翻到房的起点"。
    Found {
        searched: usize,
        whole: bool,
        found: T,
    },
    /// **翻到房的起点**，这一整段历史里都没有。
    ///
    /// 它不带读数：失败结果**本来就不带正文块**（TASK_21 起的纪律），"翻了多少条"因此不可观测——
    /// 与其留一个没人读的字段，不如让它只是一个结局。
    Exhausted,
    /// **没看到底**（触页数上限 / 某一页读不出来）：不许说"没有"。
    Undecided,
}

/// 复核的回执：房间、**累计看了多少条**、**有没有看到房的起点**、找到了哪一条，以及
/// **这次比的是哪些维度**（正文之外的任何一维在场时才出现那一行）。
///
/// `more` 这个读数不是装饰：`more = none` = 已经翻到房的起点（整段历史都在这次读取里），
/// `more = older` = 还有更旧的记录**没看**。回执必须把这件事说出来，否则"没找到"会被读成"不存在"。
/// 判定口径（哪种情况才敢说"确定没有"）见 [`ImApp::sweep`] 与 README §3.16。
///
/// `compared` 那一行是**判据的自白**：它把这次真正参与比较的维度逐条说出来——附件那一维
/// 明说比的是"文件名 + 字节数"而不是内容（服务端不返回 hash），所以主体不会把这一格读成
/// "内容一模一样"；回复对象与提及集合同理，各自列名、不混成一团。
fn render_verification(
    room: &str,
    searched: usize,
    whole: bool,
    found: Option<&str>,
    with_file: bool,
    with_reply: bool,
    with_mentions: bool,
) -> String {
    let mut text = format!(
        "room = {}, searched = {searched}, more = {}, found = {}",
        escaped_field(room),
        if whole { "none" } else { "older" },
        found
            .map(escaped_field)
            .unwrap_or_else(|| "none".to_string())
    );
    let mut compared = vec!["body"];
    if with_file {
        compared.push("attachment file_name + size_bytes");
    }
    if with_reply {
        compared.push("reply target");
    }
    if with_mentions {
        compared.push("mention set");
    }
    if compared.len() > 1 {
        text.push_str(&format!("\ncompared = {}", compared.join(" + ")));
    }
    text
}

/// 读附件的一段**窗口**：从第几行开始给了哪几行、整个附件共几行。
///
/// 它存在的理由只有一个：**被截掉的部分不是丢**——只要这一段能说出"下一段从哪开始"，
/// 附件就还是那份随时可重取的文件（三家独立收敛的同一形态：截断 + 一个能继续读的位置）。
struct AttachmentWindow {
    /// 这一段的起始行（从 1 起）。
    start: usize,
    /// 这一段实际给出去的几行（超长行已按字符截断并标注）。
    lines: Vec<String>,
    /// 整个附件共几行。
    total: usize,
}

impl AttachmentWindow {
    /// 下一段的 `offset`；读到底了就是 `None`。
    fn next_offset(&self) -> Option<usize> {
        let read_end = self.start - 1 + self.lines.len();
        (read_end < self.total).then_some(read_end + 1)
    }
}

/// 把附件正文切出**一段**：从 `offset` 行（1 起）开始，至多 [`ATTACHMENT_WINDOW_LINES`] 行、
/// 至多 [`ATTACHMENT_WINDOW_BYTES`] 字节。
///
/// 两种截断都**不是丢**：行数或字节数触顶 ⇒ 由 [`AttachmentWindow::next_offset`] 给出下一段的
/// 起始行；单行超 [`ATTACHMENT_LINE_CHARS`] 字符 ⇒ [`truncate_attachment_line`] 就地标注。
/// `offset` 为 0 或超过总行数 ⇒ 参数非法（不猜主体想读哪一段）。
fn slice_attachment(text: &str, offset: usize) -> Result<AttachmentWindow, ImFailure> {
    let raw: Vec<&str> = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    };
    let total = raw.len();
    if offset == 0 || offset > total {
        return Err(ImFailure::InvalidArgument);
    }

    let mut lines = Vec::new();
    let mut bytes = 0usize;
    for line in raw.iter().skip(offset - 1) {
        if lines.len() >= ATTACHMENT_WINDOW_LINES {
            break;
        }
        let rendered = truncate_attachment_line(line);
        // 第一行**无条件收**：单行已经被截到有界（≤ [`ATTACHMENT_LINE_CHARS`] 个字符），
        // 所以"窗口装不下一行"这种死局在构造上不可能出现。
        if !lines.is_empty() && bytes + rendered.len() + 1 > ATTACHMENT_WINDOW_BYTES {
            break;
        }
        bytes += rendered.len() + 1;
        lines.push(rendered);
    }

    Ok(AttachmentWindow {
        start: offset,
        lines,
        total,
    })
}

/// 单行超长 ⇒ 截到 [`ATTACHMENT_LINE_CHARS`] 个字符并**标注这一行本来有多长**。
///
/// 按**字符边界**切（不是字节）：一个中文占三个字节，按字节切会把一个字切成两半。
/// 标注是必须的——安静地少给半行，正是这一刀要避免的"静默丢"。
fn truncate_attachment_line(line: &str) -> String {
    let chars = line.chars().count();
    if chars <= ATTACHMENT_LINE_CHARS {
        return line.to_string();
    }
    let end = line
        .char_indices()
        .nth(ATTACHMENT_LINE_CHARS)
        .map(|(index, _)| index)
        .unwrap_or(line.len());
    format!("{}…[line truncated: {chars} chars]", &line[..end])
}

/// 读附件的回执：**引用回执先行**（房间 / 附件身份 / 文件名 / 类型 / 总字节），再给这一段
/// **读到哪几行、怎么继续**，最后才是内容。
///
/// 为什么把身份与范围摆在这一段的最前面：**"读过"这件事不会自己留在上下文里**——三家一致
/// （内容会随历史在里面，但没有任何机制自动记住"这条消息里的这个附件我读过、读到哪了"）。
/// 主体下一步要用那个附件，路条只能是这段模型可见的文本：`attachment` 是能重取的**权威身份**，
/// `offset` 是能接着读的位置。
///
/// `cited_name` 是主体**自己带进来**的文件名（工具不校验它——名字不是身份，服务端也不在下载响应里
/// 给文件名，§8 的响应头是固定值）。带了就原样写回来，并在 `file_name_source` 一行里说清它从哪来。
fn render_attachment(
    room: &str,
    attachment: &str,
    cited_name: Option<&str>,
    content_type: &str,
    size_bytes: usize,
    window: &AttachmentWindow,
) -> String {
    let read_start = window.start;
    let read_end = window.start - 1 + window.lines.len();
    let mut text = format!(
        "room = {}, attachment = {}, content_type = {}, size = {size_bytes}\nlines = {}, read = {read_start}-{read_end}, more = {}\n{}",
        escaped_field(room),
        escaped_field(attachment),
        escaped_field(content_type),
        window.total,
        match window.next_offset() {
            Some(next) => format!("offset {next}"),
            None => "none".to_string(),
        },
        window.lines.join("\n"),
    );
    if let Some(name) = cited_name {
        // 插在读数块里（第三行之前），保持"读数在前、内容在后"的形状。
        let marker = "\nlines = ";
        if let Some(index) = text.find(marker) {
            text.insert_str(index, &format!("\nfile_name = {}", escaped_field(name)));
        }
    }
    text
}

/// 读**媒体附件**的回执：**引用先行**（房间 / 附件身份 / 文件名 / 类型 / 总字节），再给这一段的
/// 形态读数。
///
/// ⚠ 与文本附件**同一套引用形状**，少掉的只是 `lines / read / more`——媒体没有"行"这回事，
/// 整份就是整份。`attachment` 仍是**能重取的权威身份**：下一次要用它，路条还是这一行。
///
/// 为什么读数必须写在这里：**"读过"这件事不会自己留在上下文里**，而且**字节只活在这一轮请求里**
/// ——下一轮看到的就只剩这一行。它得自己说清「看过什么、怎么再看」。
fn render_media_attachment(
    room: &str,
    attachment: &str,
    cited_name: Option<&str>,
    content_type: &str,
    size_bytes: usize,
    pixels: Option<(u32, u32)>,
) -> String {
    let mut text = format!(
        "room = {}, attachment = {}, content_type = {}, size = {size_bytes}\n\
         pixels = {}\n\
         form = media, read = whole, more = none\n",
        escaped_field(room),
        escaped_field(attachment),
        escaped_field(content_type),
        pixels_text(pixels),
    );
    if let Some(name) = cited_name {
        // 插在读数块里（第二行之前），保持"读数在前"的形状——与文本那条回执同款。
        let marker = "\nform = ";
        if let Some(index) = text.find(marker) {
            text.insert_str(index, &format!("\nfile_name = {}", escaped_field(name)));
        }
    }
    text
}

/// 感知体量的读数：有就说 `宽x高`，**读不出来就说 `unknown`**。
///
/// 它不许省：省掉这一行，读的人就分不清"这张图很小"与"我们没量出来"——而那正是
/// 「没有」与「读不到」必须分开表达的那条纪律。
fn pixels_text(pixels: Option<(u32, u32)>) -> String {
    match pixels {
        Some((width, height)) => format!("{width}x{height}"),
        None => "unknown".to_string(),
    }
}

/// **感知体量超限**的读数：两个体量与上限一起摆出来（读到的人知道该换什么），并说清撞的是
/// **哪一道门**。
///
/// 为什么把它与"通道字节超限"分开写：那一道是"我们连完整字节都没拿到"（`too-large`，
/// 谈不上读到哪一段），这一道是"**字节拿到了，但这一轮装不下**"。两者的决定不一样
/// ——前者要换一份更小的东西，后者要换一张更小的**图**（同一份字节可以很小而像素很多）。
fn render_media_refusal(
    room: &str,
    attachment: &str,
    content_type: &str,
    size_bytes: usize,
    pixels: (u32, u32),
) -> String {
    format!(
        "room = {}, attachment = {}, content_type = {}, size = {size_bytes}\n\
         pixels = {}x{}, limit = {MAX_MEDIA_PIXELS}\n\
         refused = perception-limit\n",
        escaped_field(room),
        escaped_field(attachment),
        escaped_field(content_type),
        pixels.0,
        pixels.1,
    )
}

/// 一页读取的读数：**窗口是有限的**，这几件事必须一起说出来。
struct Window {
    /// 这一页实际看了多少条。
    searched: usize,
    /// 这一页是否已经**翻到房的起点**（`next_before = null`）。
    whole: bool,
    /// 更旧那一页的游标（原样回传给端点）；`whole` 为真时是 `None`。
    cursor: Option<String>,
}

/// **推进读位置那一次请求的结果**（四态，与读侧同一条纪律：**不知道 ≠ 被拒 ≠ 没动**）。
///
/// 它是 [`ImApp::advance_cursor`] 的交回形状，也是两个调用方共用的那一份判据：
/// `im_mark_read` 把它翻成回执与失败分类，`im_read_history` 把它翻成回执末尾那一句实话。
enum AdvanceReply {
    /// 世界答了：`advanced` 是它说的"动没动"，`read_state` 是它给的读数（读不出来就是 `None`）。
    Answered {
        advanced: bool,
        read_state: Option<serde_json::Value>,
    },
    /// 2xx 却读不出"推进了没有"：说不出这次到底动没动。
    Unreadable,
    /// 明确被拒（稳定分类）。
    Refused(ImFailure),
    /// 其余（传输断、不认识的码）：不知道。
    Unknown,
}

/// 从一页回执里读出窗口与这一页的行。`None` = 体读得出来但不是我们认得的形状（没有答案）。
///
/// `next_before` **缺字段按"还有更旧的没看、但没有游标"处理**：宁可说"窗口不是整段"，也不在没有
/// 证据时声称自己看完了整段历史；而"没有游标"会让回翻停在那里并**报不知道**（这是 read 侧唯一一处
/// fail-closed 的默认值）。
fn window_of(body: &serde_json::Value) -> Option<(Window, &Vec<serde_json::Value>)> {
    let rows = body.get("messages").and_then(|rows| rows.as_array())?;
    let cursor = body
        .get("next_before")
        .and_then(|cursor| cursor.as_str())
        .map(str::to_string);
    let absent = body.get("next_before").is_none();
    Some((
        Window {
            searched: rows.len(),
            whole: cursor.is_none() && !absent,
            cursor,
        },
        rows,
    ))
}

/// 在**这一页**记录里找一条**我发的**、与这次**同形**的消息。
///
/// `me` 是**服务器**回答的身份（[`ImApp::whoami`]），不是本地 AgentId——两套命名空间。
///
/// **同形** = 五个条件同时成立：
///
/// - **正文**：`body` 与给定的一致。`None` 表示"这次没有正文"，那就必须对上服务端的 `null`
///   （附件-only 消息的正文在投影里就是 `null`）；
/// - **附件**：`expecting` 是这次预期的附件 `(名字, 字节数)` 有序表。**没带文件时它是空表**，
///   于是要求那条消息**没有附件**——这一条不能少：否则一条带附件的旧消息会被当成
///   "这句话我说过了"，而那正是任务书要禁的**误判**；
/// - **回复对象**：那次带了 `reply_to_id` 的，要求那条引用的就是**同一条**；那次没带的，
///   要求那条**不是回复**（同上：少了这一条，一条带回复的旧消息会被认领）；
/// - **提及集合**：那次带了 `mentions` 的，要求那条点名的集合**完全相同**；那次没带的，
///   要求那条**没点名**（集合语义：顺序不计）；
/// - **未撤回**：撤回后的消息在公共投影里是 tombstone（正文与附件都被清空），把它当成证据
///   等于拿一份已经被抹掉的记录去解除门。
///
/// 取**最新**的一条匹配（多条时的确定性选择：行是升序的，因此取最后一条）。它只回答
/// "这一页里有没有"，"更早的记录里有没有"由 [`ImApp::sweep`] 负责。缺字段 / 形状读不懂的行
/// （`recalled_at` / `reply_to_message_id` / `mentions`）一律**不当证据**——不把读不出来的
/// 格子当成"对上了"。
fn find_own_message(
    rows: &[serde_json::Value],
    me: &str,
    body: Option<&str>,
    expecting: &[(String, u64)],
    reply_to: Option<&str>,
    mentions: &[&str],
) -> Option<String> {
    let mut found = None;
    for row in rows {
        let mine = row
            .get("actor")
            .and_then(|actor| actor.get("id"))
            .and_then(|id| id.as_str())
            == Some(me);
        let same_body = row.get("body").and_then(|body| body.as_str()) == body;
        // `recalled_at` 在契约里**字段恒存在**（§7.1）；缺字段的行不当证据（不指认）。
        let active = row
            .get("recalled_at")
            .map(|at| at.is_null())
            .unwrap_or(false);
        let same_attachments = attachments_match(row, expecting);
        let same_reply = reply_matches(row, reply_to);
        let same_mentions = mentions_match(row, mentions);
        if mine && same_body && active && same_attachments && same_reply && same_mentions {
            found = row.get("id").and_then(|id| id.as_str()).map(str::to_string);
        }
    }
    found
}

/// 这一条消息的**回复对象**与预期对得上吗。
///
/// 预期 `None` = 要求这条**不是回复**（那条消息必须原样带着 `null`——少了这一条，一条带回复的
/// 旧消息会被当成"这次这句我说过了"）；预期 `Some(id)` = 要求引用的就是**同一条**。字段缺失
/// 或形状读不懂 ⇒ **不当证据**（契约里该字段恒存在，读不出来就不是我们认得的形状）。
fn reply_matches(row: &serde_json::Value, expected: Option<&str>) -> bool {
    match row.get(REPLY_TO_FIELD) {
        Some(serde_json::Value::String(id)) if !id.is_empty() => expected == Some(id.as_str()),
        Some(serde_json::Value::Null) => expected.is_none(),
        _ => false,
    }
}

/// 这一条消息的**提及集合**与预期对得上吗（**集合语义**：顺序不计，逐项必须相同）。
///
/// 空预期 = 要求这条**没有提及**；任何一项读不出来也算不同——**不**把读不出来的格子当成
/// "对上了"（那是把不知道读成知道）。服务端返回的是规范排序去重后的列表，本层两侧都先排序再比。
fn mentions_match(row: &serde_json::Value, expected: &[&str]) -> bool {
    let items = row
        .get(MENTIONS_FIELD)
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if items.len() != expected.len() {
        return false;
    }
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    let mut found: Vec<&str> = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(id) if !id.is_empty() => found.push(id),
            _ => return false,
        }
    }
    found.sort_unstable();
    found == expected
}

/// 这一条消息的附件与预期**逐项**对得上吗（`(file_name, size_bytes)` 有序序列）。
///
/// 空预期 = 要求这条**没有附件**（见 [`find_own_message`]）。数目不等即不同；任何一项读不出来
/// 也算不同——**不**把读不出来的格子当成"对上了"（那是把不知道读成知道）。
fn attachments_match(row: &serde_json::Value, expecting: &[(String, u64)]) -> bool {
    let items = row
        .get(ATTACHMENTS_FIELD)
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if items.len() != expecting.len() {
        return false;
    }
    items.iter().zip(expecting).all(|(item, (name, size))| {
        let item_name = item.get(ATTACHMENT_NAME_FIELD).and_then(|v| v.as_str());
        let item_size = item.get(ATTACHMENT_SIZE_FIELD).and_then(|v| v.as_i64());
        item_name == Some(name.as_str()) && item_size == Some(*size as i64)
    })
}

/// 在**这一页**记录里找**那条消息**，并读出它是否已撤回。
///
/// 归因**只看身份**（`id` 相等），不看正文也不看作者——撤回的 id 是自己写下来的，
/// 因此这条复核不需要身份探针。
fn find_recalled(rows: &[serde_json::Value], message: &str) -> Option<bool> {
    let mut found = None;
    for row in rows {
        if row.get("id").and_then(|id| id.as_str()) == Some(message) {
            let recalled = row
                .get("recalled_at")
                .map(|at| !at.is_null())
                .unwrap_or(false);
            found = Some(recalled);
        }
    }
    found
}

/// 通讯录回执。
///
/// ```text
/// rooms = 2, since = 1758000000000
/// <room_id>\t<kind>\t<display_name>\tsince = 3\tlast = <RFC3339|->\tpreview = <≤60B 截断|-></>
/// ```
///
/// 字段全部照端点原样搬运：`kind` 不翻译、`since = N` 不折叠成 `99+`（折叠是将来**通知栏呈现面**
/// 的纪律，不是工具面的），预览只在超长时按**字符边界**截断。
fn render_rooms(body: &serde_json::Value, since: Option<i64>) -> Result<String, ImFailure> {
    let rooms = body
        .get("rooms")
        .and_then(|rooms| rooms.as_array())
        .ok_or(ImFailure::Unreachable)?;

    let mut text = format!(
        "rooms = {}, since = {}\n",
        rooms.len(),
        match since {
            Some(millis) => millis.to_string(),
            None => "none".to_string(),
        }
    );
    for room in rooms {
        let field = |name: &str| {
            room.get(name)
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        };
        let count = room
            .get("since_count")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        text.push_str(field("room_id"));
        text.push('\t');
        text.push_str(field("kind"));
        text.push('\t');
        text.push_str(&escaped_field(field("display_name")));
        // **我在这个房间的位置与未读**（切片 M1：世界记得"我读到哪儿了"，
        // 见 `CORE_V1_AGENT_READ_CURSOR.md`）。它回答的是"我还欠这间房什么"，
        // 与 `since`（调用方自己给的锚）是两件事，因此分列两栏。
        //
        // 三态词由 `read_state_fields` 统一渲染（**缺省不当零**：读不到写 `?`，
        // 绝不写 0；位置为空是另一件事，写 `-`）——推进回执用的是同一个函数。
        let (read, unread) = read_state_fields(room.get("read_state"));
        text.push_str(&format!("\tunread = {unread}\tread = {read}"));
        text.push_str(&format!("\tsince = {count}"));
        text.push_str("\tlast = ");
        text.push_str(field("last_activity_at"));
        text.push_str("\tpreview = ");
        text.push_str(
            &match room.get("preview").and_then(|value| value.as_str()) {
                Some(preview) => escaped_field(&truncate_preview(preview)),
                None => "-".to_string(),
            },
        );
        text.push('\n');
    }
    Ok(text)
}

/// 一次 B1 回执里**一行**的两半（**两个锚各算各的**，契约 §3.2 的 D3 记账）。
///
/// 两个都是"有多少条"，但锚不同 ⇒ **不许压成一个数**：
/// - `since`：自**锚**（上一次去看的时刻）以来、**别人**说的条数——**没有锚就问不出来**；
/// - `unread`：按**世界记着的读位置**算的未读——**它不需要锚**（刚重启 / 第一次开机照样答得出来）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct RoomCounts {
    /// **房号**——它是**把手**，不是给人读的名字：主体拿它去调 `im_read_history`。
    ///
    /// `None` = 这一次没读到它（**不编一个号**）；那时那一行**不写括号**，计数照给
    /// ——"把手缺了"与"有几条"是两件事，不许因为前者把后者也吞掉。
    room_id: Option<String>,
    name: String,
    since: i64,
    unread: i64,
}

/// 把一次 B1 回执解析成一行一行的**两半**（**两个锚各算各的**，契约 §3.2 的 D3 记账）。
///
/// **顺序在这里定死**：条数降序，同数按房名升序（零因此自然落在末尾；同一份回执两次渲染逐字节
/// 相同）。房名走既有的字段转义（含空格/等号/换行的显示名破坏不了行结构）。
///
/// **缺省不当零**：某间房没有 `since_count`、或它不是非负整数 ⇒ 整份回执**读不懂**
/// （`Unreachable`，宿主侧如实写 `counts = unknown`）——"这个字段缺了"与"这间房零新"是两件事，
/// 把前者渲染成后者正是最坏的那种谎。
///
/// `read_state.unread_count` 同一条纪律（缺 / 形状读不懂 ⇒ **整份读不懂**，不冒充零）。
fn parse_new_since_rows(body: &serde_json::Value) -> Result<Vec<RoomCounts>, ImFailure> {
    let rooms = body
        .get("rooms")
        .and_then(|rooms| rooms.as_array())
        .ok_or(ImFailure::Unreachable)?;

    let mut rows: Vec<RoomCounts> = Vec::new();
    for room in rooms {
        let count = room
            .get("since_count")
            .and_then(|value| value.as_i64())
            .filter(|count| *count >= 0)
            .ok_or(ImFailure::Unreachable)?;
        // ⚠ **未读那一半按世界记着的读位置算，它不需要锚**（契约 D3：`since_count` 与
        // `read_state` 是两个锚，**不合并**）。缺省不当零：读不懂 ⇒ 整份读不懂。
        let unread = room
            .get("read_state")
            .and_then(|state| state.get("unread_count"))
            .and_then(|value| value.as_i64())
            .filter(|count| *count >= 0)
            .ok_or(ImFailure::Unreachable)?;
        let name = room
            .get("display_name")
            .and_then(|value| value.as_str())
            .unwrap_or("-");
        // ⚠ **房号是把手**（2026-10-10）：名字给人读，**号码给手用**——从前那一格只给名字，
        // 于是主体判断得出"要不要去看"，却**递不出参数**（`im_read_history` 要的是房号），
        // 还得先花一次 `im_rooms`；而**两间房同名**时那一格连"是哪一间"都说不清。
        let room_id = room
            .get("room_id")
            .and_then(|value| value.as_str())
            .filter(|id| !id.is_empty())
            .map(escaped_field);
        rows.push(RoomCounts {
            room_id,
            name: escaped_field(name),
            since: count,
            unread,
        });
    }
    rows.sort_by(|left, right| {
        right
            .since
            .cmp(&left.since)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(rows)
}

/// 投影回执（**机械计数，一行正文都没有**）——**按事件定价**。
///
/// **这块投影的形状只此一处**（唯一装配点）：宿主侧 `new_since` 只搬它的结果、`notification_bar`
/// 只渲染它——顺序、分母、余数、未读那一行都**不许在别处再判断一遍**。
///
/// ```text
/// 可见 3 间 · 全部 = 0                     # 安静日（k = 0）：一行说尽"都看了、都没有"
/// 未读 0 间
///
/// 可见 3 间 · 有新 2 间                    # 忙日（0 < k < N）：分母 ＋ 名单 ＋ 余数
/// 值班室（b5fe4d96-…） = 2                 # **名字给人读，号码给手用**（见下）
/// 项目群（7f735c93-…） = 1
/// 其余 1 间 = 0                            # 缺席消歧：分母在，没出现在名单里的房就是零新
/// 未读 1 间：值班室（b5fe4d96-…） = 4      # **另一行、另一个锚**（见下）
///
/// 可见 3 间 · 有新 3 间                    # 每间都有新的（k = N）：名单本身就是全量
/// 值班室（b5fe4d96-…） = 2
/// 项目群（7f735c93-…） = 1
/// 清静间（1b212cf1-…） = 1                 # 不再补一句"其余 0 间 = 0"（那件事已经说完了）
/// 未读 0 间
///
/// 可见 2 间                                # **没有锚**：增量那一半问不出来（不编一个 0）
/// 未读 1 间：Inori（b5fe4d96-…） = 2       # 而未读那一半**不需要锚**——它照样答得出来
///
/// 可见 0 间                                # 没有可见的房（如实说零，不编分母、不编比例）
/// ```
///
/// **房号也写上**（2026-10-10）：**名字给人读，号码给手用**——`im_read_history` 要的是**房号**，
/// 而从前那一格只给名字 ⇒ 主体判断得出"要不要去看"，却**递不出参数**（还得先花一次 `im_rooms`）；
/// **两间房同名**时更糟：两行一模一样的"乙房 = 2"，连"是哪一间"都说不清。
/// 房号没读到 ⇒ 那一行**不写括号**（**不编一个号**），计数照给。
///
/// **为什么不再逐间点名零**（TASK_73）：快照的大小必须随**事件**走，不随**世界大小**走。
/// "什么都没发生"的信息量不随房间数变化，而旧形态每间一行（含 `= 0`）⇒ 安静的一天要为「无事」
/// 按库存付税：五百间房每次醒来付五百行，上千间则撞上批次上限、整块变成"超限未给"——通知栏
/// 恰在主体最需要它的时刻失明。这是**结构性**的坏，不是优雅降级。
///
/// ⚠ **未覆盖（如实登记）**：这笔账在**世界规模**上**没有量过**（"有界"由批预算保证，而真机
/// 实际占用离那道上限还有余量）——那一条的权威在 `agent/README.md` §4，别在这里再抄一遍数字。
///
/// **TASK_29 那一刀的用意一条不丢**：它要求「每间可见的房都有一行、含 `= 0`」，理由是
/// "这一间没出现"分不清「零新」与「这一次没读到它」。前半句改由**分母**承担（见下）；后半句
/// 由**整份投影**那一层承担——`counts = unknown`，**不与「零」混**（某间房缺那个字段 ⇒
/// **整份读不懂**，见 [`parse_new_since_rows`]）。
///
/// **缺席消歧一条不丢**（TASK_28 的意图照旧）：分母（`可见 N 间`）＋非零清单已经说清了"每一间
/// 各有多少"——名字没在清单里出现的房，就是零新。代价是**零房间的名字**不再出现在快照里；
/// 某间房"还在不在"是罕见事件，要查主体就 `im_rooms` 拉一次（已裁：值得那一次拉取）。
/// **余数那句只在还有零房间时给**（`0 < k < N`，裁决 ②）：`k = N` 时名单就是全量，
/// 再补一句"其余 0 间 = 0"只是把一件已经说完的事又说一遍。
///
/// ⚠ **如实登记一条没达成的判据（它的对象已被本形状改掉）**：「主体只读非零房间」——真机实测里
/// 主体**会**读那几间非零的，**也**会顺手读零新的那一间；当时的诊断是**信号没给全**（零没有说
/// 出口）。TASK_73 起快照里**没有零房可读**：零只由余数那句 `其余 M 间 = 0` 承担、名字不出现
/// ⇒ 这条判据**今天没有可挑的对象**，因此既不能说它达成，也不再是一条可检验的判据。
/// **不假装已达成**，也不把它写成待办（当时的读数与形状归 Git 历史）。
///
/// **两半各有各的锚**（2026-10-10）：契约 §3.2 的 D3 记账写着「**"我欠多少"问 `read_state`；
/// "自某个时刻以来有多少"问 `since_count`**」——两个锚，**不合并**。锚不知道时（刚重启 / 第一次
/// 开机）增量那一半**根本问不出来**，而未读那一半照样答得出来 ⇒ 通知栏不再整块失明。
///
/// 一行里的**房名（房号）**：**名字给人读，号码给手用**——主体拿那个号码去调 `im_read_history`
/// （它要的是房号，不是名字）。⚠ 名字还会**重名**（两间"乙房"是真实形状）：只有号码分得开。
///
/// 没读到房号 ⇒ **不写那个括号**（**不编一个号**），名字与计数照给。
fn room_label(row: &RoomCounts) -> String {
    match &row.room_id {
        Some(id) => format!("{}（{}）", row.name, id),
        None => row.name.clone(),
    }
}

/// 顺序**确定**（见 [`parse_new_since_rows`]：条数降序、同数按房名升序），因此同一份回执两次渲染
/// **逐字节相同**；房名走既有字段转义，含空格/等号/换行的显示名破坏不了行结构。
///
/// ⚠ 形状的**完整说法**（四档示例、"为什么不再逐间点名零"那笔账与代价、两半各有各的锚）写在它
/// **上面那段 `///` 注释**里（`room_label` 那一格）——本函数只钉**顺序与转义**这两条。
fn render_new_since(body: &serde_json::Value, has_anchor: bool) -> Result<String, ImFailure> {
    let rows = parse_new_since_rows(body)?;

    let visible = rows.len();
    if visible == 0 {
        return Ok("可见 0 间".to_string());
    }
    // **未读那一行**：只列未读 > 0 的房（安静的不点名——同一条"按事件定价"）。
    //
    // ⚠ **它只在有未读（或没有锚）时出场**：没出场就是**都读了**——与"没出现在名单里的房就是
    // 零新"同一条缺席消歧。这样安静日仍然是**一行**（尺寸定律不破），而"读不到"由**整份投影**
    // 那一层表达（`counts = unknown`），不与"零"混。
    let unread: Vec<&RoomCounts> = rows.iter().filter(|row| row.unread > 0).collect();
    let unread_line = if unread.is_empty() {
        "未读 0 间".to_string()
    } else {
        let listed: Vec<String> = unread
            .iter()
            .map(|row| format!("{} = {}", room_label(row), row.unread))
            .collect();
        format!("未读 {} 间：{}", unread.len(), listed.join("，"))
    };
    // **没有锚**：增量那一半问不出来（**不编一个 0**），未读那一半照给——它是这一次唯一的读数。
    if !has_anchor {
        return Ok(format!("可见 {visible} 间\n{unread_line}"));
    }
    let busy: Vec<&RoomCounts> = rows.iter().filter(|row| row.since > 0).collect();
    let mut text = if busy.is_empty() {
        // 安静日：一行说尽"都看了、都没有"。
        format!("可见 {visible} 间 · 全部 = 0")
    } else {
        let mut text = format!("可见 {visible} 间 · 有新 {} 间", busy.len());
        for row in &busy {
            text.push('\n');
            text.push_str(&room_label(row));
            text.push_str(" = ");
            text.push_str(&row.since.to_string());
        }
        // 还有零房间时才给余数那句（`0 < k < N`）：k = N 时名单即全量，无需再证书一遍"其余是零"。
        if busy.len() < visible {
            text.push_str(&format!("\n其余 {} 间 = 0", visible - busy.len()));
        }
        text
    };
    if !unread.is_empty() {
        text.push('\n');
        text.push_str(&unread_line);
    }
    Ok(text)
}

/// 翻记录回执。
///
/// ```text
/// room = <room_id>, limit = <n>, count = <n>
/// <message_id>\t<actor_kind>\t<actor_name>\t<created_at>\t<state>[\tmentions = n]
/// \tattachment\t<第几个>\t<附件身份>\t<文件名>\t<类型>\t<字节数>
/// more = none | next <游标>
/// ```
///
/// **附件逐条列**（缩进一行一个）：`im_read_attachment` 要的正是那个**附件身份**——只给计数，
/// 主体看得到"有东西"却拿不到"读哪一个"。**元数据免费随历史来、字节要主体主动读**（渐进式铁律）。
///
/// 顺序就是端点给的顺序（序号升序，服务端排好）。撤回**如实**写 `recalled`（正文与附件在撤回后
/// 由服务端投影为空，本层不还原、不猜）；`more` 行搬运游标，不解释它的内容。
///
/// ⚠ 那一串**追加**在行尾的读数（`mentions` / `actor` / `reply_to`、`mention` 与 `attachment`
/// 两行）是 TASK_67 分批加上去的：前面各列的形状一个字没动，旧断言的前缀与列序照旧成立。
/// 逐条理由与审计表见 `outputs/AGENT_REBUILD_TASK_67_SEMANTIC_PROJECTION_AUDIT_REPORT.md`。
fn render_history(body: &serde_json::Value, room: &str, limit: u32) -> Result<String, ImFailure> {
    // 外部字段名只在**连接处**出现这一次（外部冻结形状）；本层自己的词是 `rows`。
    let rows = body
        .get("messages")
        .and_then(|rows| rows.as_array())
        .ok_or(ImFailure::Unreachable)?;

    let mut text = format!("room = {room}, limit = {limit}, count = {}\n", rows.len());
    for message in rows {
        let field = |name: &str| message.get(name).and_then(|value| value.as_str());
        let actor = message.get("actor");
        text.push_str(field("id").unwrap_or("-"));
        text.push('\t');
        text.push_str(
            actor
                .and_then(|actor| actor.get("kind"))
                .and_then(|value| value.as_str())
                .unwrap_or("-"),
        );
        text.push('\t');
        text.push_str(&escaped_field(
            actor
                .and_then(|actor| actor.get("display_name"))
                .and_then(|value| value.as_str())
                .unwrap_or("-"),
        ));
        text.push('\t');
        // **世界给的时刻摆给主体时换成本机刻度**（与人那边 `toLocaleString()` 是同一步）：
        // 世界的事实不变（wire 上仍是 UTC），但那次换算不该由主体自己做。读不出来就原样摆着。
        text.push_str(&match field("created_at") {
            Some(at) => localize_rfc3339(at),
            None => "-".to_string(),
        });

        let recalled = message
            .get("recalled_at")
            .map(|value| !value.is_null())
            .unwrap_or(false);
        text.push('\t');
        if recalled {
            // 撤回**带时间**：人类那边撤回是一件有时间的公共事实（"谁在什么时候撤的"）；
            // 只给一个裸布尔，主体就只剩"它没了"，没有"什么时候没的"（TASK_67 审计表 #3）。
            text.push_str("recalled = ");
            text.push_str(&match field("recalled_at") {
                Some(at) => localize_rfc3339(at),
                None => "-".to_string(),
            });
        } else {
            text.push_str("body = ");
            text.push_str(&match field("body") {
                Some(body) if !body.is_empty() => escaped_field(body),
                _ => "-".to_string(),
            });
        }

        let mentions = message
            .get("mentions")
            .and_then(|value| value.as_array())
            .map(|items| items.len())
            .unwrap_or(0);
        if mentions > 0 {
            text.push_str(&format!("\tmentions = {mentions}"));
        }
        // **作者身份**与**回复目标**都是追加在行尾的读数（TASK_67 审计表 #6 / #1）：
        // 前面各列的形状一个字不动，旧断言的前缀与列序零改动继续成立。
        // - 为什么要有 id：显示名不是身份（同名者分不开、也判不出"这条是不是在回我"）；
        // - 为什么要有 reply_to：人类一眼看得出"回复挂在哪条下面"，主体此前只能靠正文猜。
        if let Some(actor_id) = actor
            .and_then(|actor| actor.get("id"))
            .and_then(|value| value.as_str())
        {
            text.push_str(&format!("\tactor = {}", escaped_field(actor_id)));
        }
        if let Some(reply_to) = field(REPLY_TO_FIELD) {
            text.push_str(&format!("\treply_to = {}", escaped_field(reply_to)));
        }
        text.push('\n');

        // @ 提及**逐条列**（与附件同款，TASK_67 审计表 #2）：只给计数的话，主体看得到"有人被点名"，
        // 却不知道是**谁**被点名——而"谁"正是主体要用的那个身份（`im_mention` 一族将来的入参来源）。
        if let Some(mentions) = message
            .get(MENTIONS_FIELD)
            .and_then(|value| value.as_array())
        {
            for (position, mention) in mentions.iter().enumerate() {
                text.push_str(&format!(
                    "\tmention\t{position}\t{}\n",
                    escaped_field(mention.as_str().unwrap_or("-"))
                ));
            }
        }

        // 附件**逐条列**（各占一行）：只给计数的话，主体看得到"有东西"却拿不到"读哪一个"——
        // `im_read_attachment` 要的正是这里的**附件身份**。
        if let Some(attachments) = message
            .get(ATTACHMENTS_FIELD)
            .and_then(|value| value.as_array())
        {
            for (position, attachment) in attachments.iter().enumerate() {
                let named = |name: &str| {
                    attachment
                        .get(name)
                        .and_then(|value| value.as_str())
                        .unwrap_or("-")
                };
                let size = attachment
                    .get(ATTACHMENT_SIZE_FIELD)
                    .and_then(|value| value.as_i64())
                    .map(|size| size.to_string())
                    .unwrap_or_else(|| "-".to_string());
                text.push_str(&format!(
                    "\tattachment\t{position}\t{}\t{}\t{}\t{size}\n",
                    named(ATTACHMENT_ID_FIELD),
                    escaped_field(named(ATTACHMENT_NAME_FIELD)),
                    escaped_field(named(ATTACHMENT_TYPE_FIELD)),
                ));
            }
        }
    }

    text.push_str("more = ");
    match body.get("next_before").and_then(|value| value.as_str()) {
        Some(cursor) => {
            text.push_str("next ");
            text.push_str(cursor);
        }
        None => text.push_str("none"),
    }
    text.push('\n');
    Ok(text)
}

/// 预览的机械截断：只按**字符边界**取不超过 [`MAX_PREVIEW_BYTES`] 字节的前缀，截断了就加一个
/// 标记字符。它不是摘要、不是省略号改写——被丢掉的部分本层一个字都不解释。
fn truncate_preview(preview: &str) -> String {
    if preview.len() <= MAX_PREVIEW_BYTES {
        return preview.to_string();
    }
    let mut end = MAX_PREVIEW_BYTES;
    while end > 0 && !preview.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = preview[..end].to_string();
    truncated.push('…');
    truncated
}

/// 把一段可能含换行/制表符的文本转义成**一个**字段值（行结构因此不可能被内容破坏）。
///
/// 与适配层那份转义同族：只处理会破坏行结构的字符，可逆、不改语义。
fn escaped_field(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{gif_pixels, image_pixels, jpeg_pixels, png_pixels, webp_pixels};

    /// 一条最小的 PNG 头：签名 ＋ IHDR（长度 ＋ 类型 ＋ 宽高）。
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    /// `SOI` ＋ 一个 `SOF0`（段长 / 精度 / 高 / 宽 / 分量数）。
    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xc0];
        bytes.extend_from_slice(&17u16.to_be_bytes());
        bytes.push(8);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[0x03, 0x01, 0x11, 0x00]);
        bytes
    }

    fn gif(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes
    }

    /// 扩展档 `VP8X`：宽高各 3 字节小端、**都存的是减一**。
    fn webp_vp8x(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"VP8X");
        bytes.extend_from_slice(&10u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        bytes.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        bytes
    }

    /// 无损档 `VP8L`：14 位宽 ＋ 14 位高塞在同一个 4 字节里，**都存的是减一**。
    fn webp_vp8l(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"VP8L");
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.push(0x2f);
        let bits = ((width - 1) & 0x3fff) | (((height - 1) & 0x3fff) << 14);
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes
    }

    /// 四种形态各读得出自己的宽高（`VP8X` 与 `VP8L` 那两档存的是**减一**，最容易写错）。
    #[test]
    fn every_supported_format_yields_its_pixels() {
        assert_eq!(png_pixels(&png(1200, 800)), Some((1200, 800)));
        assert_eq!(jpeg_pixels(&jpeg(640, 480)), Some((640, 480)));
        assert_eq!(gif_pixels(&gif(32, 24)), Some((32, 24)));
        assert_eq!(webp_pixels(&webp_vp8x(2000, 1500)), Some((2000, 1500)));
        assert_eq!(webp_pixels(&webp_vp8l(300, 200)), Some((300, 200)));
    }

    /// **"读不出来"与"很大"是两件事**：截断 / 形状不对 / 不在四种里 ⇒ `None`，**不猜一个数**。
    ///
    /// 猜的代价是具体的：猜小了就放过一张装不下的图，猜大了就把一张小图当成大图拒掉。
    #[test]
    fn a_header_we_cannot_read_is_none_and_never_a_guess() {
        let full = png(1200, 800);
        assert_eq!(png_pixels(&full[..10]), None, "截断的头");
        assert_eq!(png_pixels(b"not a png at all........"), None, "不是 PNG");
        assert_eq!(jpeg_pixels(&full), None, "不是 JPEG");
        assert_eq!(webp_pixels(&full), None, "不是 WebP");
        assert_eq!(gif_pixels(&full), None, "不是 GIF");
        assert_eq!(image_pixels("image/tiff", &full), None, "不在四种里");
        assert_eq!(
            image_pixels("image/png", &full),
            Some((1200, 800)),
            "对的类型走对的那一支"
        );
    }
}
