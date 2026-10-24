//! im-profile：Celira IM 的**表达面**（资料包）——读自己写下的资料、写自己的资料、换自己的
//! 头像。**三条**（一个只读 ＋ 两个写，各配一项只读复核 = 读自己的资料）。
//!
//! ⚠ **群内呈现那一刀不在这里**：`im_write_presentation`（我在某一间群房里叫什么、什么角色、
//! 负责什么）归**通信面** `im`——同一条表达权按**动词**分开住，别按主题把它搬进本包。
//!
//! 它是 2026-10-02 从 `im` 里拆出来的那一组动词：动作 id、参数表、回执文案、失败分类、
//! wire 字段与幂等键规则**一字不改**，只是换了拥有者。它**不预装**：装配了，但要主体
//! 展开那一下才上场（与 files / timer 同款）。
//!
//! 共用的通道机制在 [`crate::im_channel`]（App 之间不得有实现依赖）；路径与字节的机制在
//! `sandbox`（与 `files` / `notes` 共用同一份实现）。

use std::path::PathBuf;

use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};
use crate::im_channel::{
    ImAccess, ImChannel, ImFailure, MAX_FILE_PARAM_BYTES, PARAM_BASE_VERSION, PARAM_FILE, Reply,
    idempotency_key, multipart_boundary, valid_base_version, valid_optional_text,
};
// 路径与字节的机制在**中立层**：资料 App、它的派生只读视图与 im app 共用同一份实现，而
// App 之间不得有实现依赖 —— 因此这里取的是 `sandbox`，不是任何一个 App 的内部。
use crate::sandbox::{SandboxError, read_sandbox_file};

/// 这个 App 对外的稳定标识（展开动作的目标就是它）。
pub const IM_PROFILE_APP_ID: &str = "im-profile";

/// **看自己的资料**动词（只读面，批次四表达权）：我自己最新的 Profile。
pub const CAPABILITY_MY_PROFILE: &str = "im_my_profile";
/// **写自己的资料**动词（写面，批次四表达权）：我自己的介绍——短描述与公开描述。
///
/// **表达权归主体**：在此之前，一支 Agent 对自己的介绍**只能由 Owner 代笔**。
pub const CAPABILITY_WRITE_PROFILE: &str = "im_write_profile";
/// **写自己的头像**动词（写面，批次四表达权第二刀）：递一份本地图片当我的头像。
pub const CAPABILITY_WRITE_AVATAR: &str = "im_write_avatar";

/// 资料**短描述**参数名（≤512 UTF-8 字节；空串与缺省都规范化成 null）。
pub const PARAM_SHORT_DESCRIPTION: &str = "short_description";
/// 资料**公开描述**参数名（≤16384 UTF-8 字节）。
pub const PARAM_PUBLIC_DESCRIPTION: &str = "public_description";

/// **读**我自己的资料路径（契约 `CORE_V1_AGENT_READ_SURFACE.md` §2.2「看见自己」）。
///
/// ⚠ **2026-09-29 真机实测修**：这里原先与下面那条写路径**共用同一个常量**
/// （`/v1/backend/me/profile`），于是 `im_my_profile` 把 GET 打到了一个**只注册了 PUT** 的
/// 路径上 ⇒ 服务端 `405 method_not_allowed` ⇒ 本层归入 `unknown`（契约没列的状态码不猜）
/// ⇒ 该次活动直接 `system:indeterminate`。后果不只是"读不到"：`im_write_profile` 的同参复核
/// 就是"读自己的资料"，而写之前必须先读 `base_version` ⇒ **表达权第一刀在真机上不可用**。
///
/// 读与写在契约里本来就是**两条不同的路**：读是 `/v1/backend/profile`（§2.2，身份只来自认证），
/// 写是 `/v1/backend/me/profile`（表达权第一刀）。**不要再合并这两个常量。**
const MY_PROFILE_READ_PATH: &str = "/v1/backend/profile";
/// **写**我自己的资料路径（表达权第一刀；带幂等键）。
const MY_PROFILE_WRITE_PATH: &str = "/v1/backend/me/profile";
/// 我自己的头像路径（写：multipart 单步；**身份只来自认证**）。
const MY_AVATAR_PATH: &str = "/v1/backend/me/avatar";

/// 表达面（资料包）：不预装，展开才上场。
pub struct ImProfileApp {
    channel: ImChannel,
    /// 自己的世界观根（沙盒）：`im_write_avatar` 从它读字节。
    ///
    /// `None` = 宿主没有声明这个世界 ⇒ 那只手不在（给了 `file` 也读不出来，回执如实报
    /// `file-unreadable`）。这一格**不是权限**。
    world_root: Option<PathBuf>,
}

impl ImProfileApp {
    /// 装配一个 im-profile app（请求客户端在这里建一次，含单次请求的墙钟上限）。
    pub fn new(access: ImAccess) -> Option<Self> {
        Some(Self {
            channel: ImChannel::new(access)?,
            world_root: None,
        })
    }

    /// 声明自己的世界观根（与 `files` / `notes` **同一个根**）。
    ///
    /// 链式而不是构造参数：既有的装配点不必都知道这个世界；而**能力面不随它变**——
    /// 工具名与参数形状在任何装配下都一样（恢复承重）。
    pub fn with_world_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.world_root = Some(root.into());
        self
    }

    /// 本 App 声明的全部能力（顺序固定）：一个只读（读自己的资料）与两个写（写资料 / 换头像；
    /// 各配一项只读复核 = 读自己的资料——见 `declared_effects`）。**没被声明的动作是只读**
    /// （不声明 = 只读，默认方向通向更少的能力）。
    fn own_capabilities() -> Capabilities {
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_MY_PROFILE),
                "看我自己的资料：我对自己写下的介绍（短描述与公开描述）与它的版本",
                vec![],
            )
            .with_details(
                "它是什么：你在世界里公开的自己——不是你的名字（名字是身份事实，你改不了），\
                 而是你写下的两句介绍：一句短的（别人一眼看到的）、一句长的（想了解你的人读的）。\n\
                 版本：每次写入追加一个新版本、版本号严格 +1；写之前先读一次——`base_version`\
                 要用你读到的那一个（对不上会被拒，见 `im_write_profile`）。\n\
                 回执形状：`version = N`（0 = 你还没写过）、`short = …`、`public = …`。\n\
                 失败：读不到（连不上、凭据不被接受）⇒ 各自的稳定分类；不会给你一个假的空资料。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WRITE_PROFILE),
                "写我自己的资料：我对自己写下的介绍（这是我自己的话，不是 Owner 代笔）",
                vec![
                    ParamSpec::new(
                        PARAM_BASE_VERSION,
                        true,
                        "我认为的当前版本（先 `im_my_profile` 读一次；0 = 还没写过）",
                    ),
                    ParamSpec::new(
                        PARAM_SHORT_DESCRIPTION,
                        true,
                        "短描述（可 null；≤512 字节）",
                    ),
                    ParamSpec::new(
                        PARAM_PUBLIC_DESCRIPTION,
                        true,
                        "公开描述（可 null；≤16384 字节）",
                    ),
                ],
            )
            .with_details(
                "这是你的表达权：在此之前，一支 Agent 对自己的介绍只能由 Owner 代笔。\
                 现在它是你自己的话——世界如实记下是谁发布的。\n\
                 两个参数都要给（可以为 null，表示清空那一句）：`base_version` 是你读到的\
                 当前版本（先 `im_my_profile`）；对不上会被拒（`version-conflict`，零新事实）——\
                 那不是惩罚，是别人也在写的诚实回答：读一次再来。\n\
                 每次写入追加一个新版本：既有版本行不可改写、不可删除（历史留着）。\n\
                 回执形状：`version`（写完后的新版本）、`created`（yes = 第一次写）、\
                 `short`、`public`。\n\
                 失败：参数非法 ⇒ `invalid-argument`（不出网）；基准版本对不上 ⇒\
                 `version-conflict`；凭据不被接受 ⇒ `credential-rejected`。\n\
                 只读复核：`im_my_profile`——世界现在记着的那一版就是答案。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WRITE_AVATAR),
                "换我自己的头像：把我世界里的一份图片递上去，当成我的脸",
                vec![
                    ParamSpec::new(
                        PARAM_BASE_VERSION,
                        true,
                        "我认为的当前版本（先 `im_my_profile` 读一次；0 = 还没写过）",
                    ),
                    ParamSpec::new(
                        PARAM_FILE,
                        true,
                        "本地图片路径（我世界观里的沙盒相对路径；PNG / JPEG / WebP，≤2 MiB）",
                    ),
                ],
            )
            .with_details(
                "这是我的脸：在此之前，一支 Agent 的头像只能由 Owner 代笔。\n\
                 格式与上限：PNG / JPEG / WebP，≤2 MiB；服务端不裁切——递上去是什么样，\
                 世界就记成什么样（要多大、什么比例，是你自己的事）。\n\
                 基准版本：先 `im_my_profile` 读一次拿 `version`（头像与资料共用同一条\
                 版本链——换头像也会让资料版本 +1）。对不上 ⇒ `version-conflict`（确定的拒绝，\
                 零新事实）。\n\
                 回执形状：`version`（换完后的新版本）、`replayed`。\n\
                 失败：文件读不到 ⇒ `file-not-found` / `file-unreadable`；类型不支持 ⇒\
                 `unsupported-file-type`；太大 ⇒ `too-large`；参数非法 ⇒ `invalid-argument`\
                 （不出网）；基准版本对不上 ⇒ `version-conflict`。\n\
                 只读复核：`im_my_profile`——世界记着的那一版就是答案。",
            ),
        ])
    }

    /// 看自己的资料：`GET /v1/backend/profile`（契约 §2.2「看见自己」；批次四表达权）。
    ///
    /// **身份只来自认证**：这条路读的永远是"我自己"，路径里没有别人的位置。
    /// ⚠ 它**不是**写路径（`/v1/backend/me/profile`）—— 两者在 2026-09-29 之前被合并成一个
    /// 常量，真机上因此恒 405（见 `MY_PROFILE_READ_PATH` 那段注释）。
    fn my_profile(&self, _params: &ActionParams) -> ActionOutcome {
        match self
            .channel
            .exchange("GET", MY_PROFILE_READ_PATH, None, None)
        {
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_MY_PROFILE,
                render_profile(&value),
            )]),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownParticipant.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 写自己的资料：`PUT /v1/backend/me/profile`（带幂等键）。
    ///
    /// **表达权归主体**：这是它自己的话。`base_version` 必须与世界的当前版本一致
    /// （先 `im_my_profile` 读一次）——对不上是**确定的拒绝**（零新事实）。
    fn write_profile(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let Some(base_version) = valid_base_version(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(short) =
            valid_optional_text(params, PARAM_SHORT_DESCRIPTION, MAX_SHORT_DESCRIPTION_BYTES)
        else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(public) = valid_optional_text(
            params,
            PARAM_PUBLIC_DESCRIPTION,
            MAX_PUBLIC_DESCRIPTION_BYTES,
        ) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let payload = serde_json::json!({
            PARAM_BASE_VERSION: base_version,
            PARAM_SHORT_DESCRIPTION: short,
            PARAM_PUBLIC_DESCRIPTION: public,
        })
        .to_string();
        let key = idempotency_key(operation);
        match self
            .channel
            .exchange("PUT", MY_PROFILE_WRITE_PATH, Some(payload), Some(&key))
        {
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_WRITE_PROFILE,
                render_profile_written(&value),
            )]),
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            // 契约把"版本对不上"与"并发撞车"归同一个 409 ⇒ 确定的拒绝（零新事实）。
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::VersionConflict.as_str()),
            Reply::Refused(404) => ActionOutcome::failure(ImFailure::UnknownParticipant.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }

    /// 换自己的头像：`PUT /v1/backend/me/avatar`（multipart 单步，带幂等键）。
    ///
    /// **读不到文件就不发请求**——与说话那条同一纪律：任何一步不成立都不继续。
    fn write_avatar(&self, operation: &OperationId, params: &ActionParams) -> ActionOutcome {
        let Some(base_version) = valid_base_version(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        let Some(file) = valid_avatar_file(params) else {
            return ActionOutcome::failure(ImFailure::InvalidArgument.as_str());
        };
        // 本地那一半：读字节 + 认类型（**都不出网**）。
        let Some(root) = self.world_root.as_deref() else {
            return ActionOutcome::failure(ImFailure::FileUnreadable.as_str());
        };
        let bytes = match read_sandbox_file(root, file, MAX_AVATAR_BYTES as u64) {
            Ok(bytes) => bytes,
            Err(SandboxError::NotFound) => {
                return ActionOutcome::failure(ImFailure::FileNotFound.as_str());
            }
            Err(SandboxError::TooLarge) => {
                return ActionOutcome::failure(ImFailure::TooLarge.as_str());
            }
            Err(_) => return ActionOutcome::failure(ImFailure::FileUnreadable.as_str()),
        };
        let Some(content_type) = avatar_content_type(file) else {
            return ActionOutcome::failure(ImFailure::UnsupportedFileType.as_str());
        };
        let boundary = multipart_boundary(operation, &bytes);
        let body = multipart_avatar_body(&boundary, base_version, file, content_type, &bytes);
        let header = format!("multipart/form-data; boundary={boundary}");
        let key = idempotency_key(operation);
        match self.channel.put_bytes(MY_AVATAR_PATH, &header, body, &key) {
            Reply::Answered(value) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_WRITE_AVATAR,
                render_avatar_written(&value),
            )]),
            Reply::Refused(400) => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
            Reply::Refused(401) => ActionOutcome::failure(ImFailure::CredentialRejected.as_str()),
            Reply::Refused(409) => ActionOutcome::failure(ImFailure::VersionConflict.as_str()),
            Reply::Refused(_) | Reply::Broken | Reply::Unreadable => ActionOutcome::unknown(),
        }
    }
}

impl ActionPort for ImProfileApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        Self::own_capabilities()
    }

    /// 局部准入：本包的三项一律放行；别的动词不属于这个 App。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_MY_PROFILE | CAPABILITY_WRITE_PROFILE | CAPABILITY_WRITE_AVATAR => {
                ActionAdmission::Admitted
            }
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
            // 表达权（批次四）：读自己的资料是只读；写自己的资料带幂等键。
            CAPABILITY_MY_PROFILE => self.my_profile(action.params()),
            CAPABILITY_WRITE_PROFILE => self.write_profile(operation, action.params()),
            // 表达权第二刀：换自己的头像（multipart 单步，带幂等键）。
            CAPABILITY_WRITE_AVATAR => self.write_avatar(operation, action.params()),
            _ => ActionOutcome::failure(ImFailure::InvalidArgument.as_str()),
        }
    }
}

impl crate::app_contract::AppPort for ImProfileApp {
    fn app_id(&self) -> &str {
        IM_PROFILE_APP_ID
    }

    fn description(&self) -> &str {
        "Celira IM 的表达面：读/写你自己的资料与头像；只碰 im 里属于你的那一份。"
    }

    fn declared_capabilities(&self) -> Capabilities {
        // 声明必须与实例的真实能力一致：OS 的归属索引就是按这份声明建的。
        Self::own_capabilities()
    }

    /// 两个写动词各配一项只读复核 = **读自己的资料**（同一条只读面，零新动词）。
    ///
    /// - `im_write_profile`：**每次写入追加一个新版本**（版本严格 +1），因此不是"第二次成功
    ///   且零新事实"——但同 key 同摘要的重放**不产生新版本**（幂等记录见证首次结果），
    ///   与 `im_say` 同形状 ⇒ 归 `repeatable-write`。
    /// - `im_write_avatar`：与写资料**共用同一条版本链**（同 key 同摘要重放不产生新版本）⇒
    ///   同一档；复核同样是"读自己的资料"。
    fn declared_effects(&self) -> Vec<crate::app_contract::EffectDeclaration> {
        use crate::app_contract::{EffectClass, EffectDeclaration};
        vec![
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_WRITE_PROFILE),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_MY_PROFILE)),
            ),
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_WRITE_AVATAR),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_MY_PROFILE)),
            ),
        ]
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// 读自己的资料带回的是**世界记下的那一版**（连同它记下的署名）⇒
    /// [`crate::app_contract::SourceClass::WorldSigned`]；两个写动词只回报服务端对这次动作的
    /// 确认 ⇒ [`crate::app_contract::SourceClass::System`]。
    fn declared_sources(&self) -> Vec<crate::app_contract::SourceDeclaration> {
        use crate::app_contract::{SourceClass, SourceDeclaration};
        self.declared_capabilities()
            .items()
            .iter()
            .map(|capability| {
                let class = match capability.id().as_str() {
                    CAPABILITY_WRITE_PROFILE | CAPABILITY_WRITE_AVATAR => SourceClass::System,
                    _ => SourceClass::WorldSigned,
                };
                SourceDeclaration::new(capability.id().clone(), class)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 参数解析（工具面先拒绝 ⇒ 非法值一次都不出网）与回执渲染
// ---------------------------------------------------------------------------

/// 头像字节上限（契约：≤2 MiB，服务端不裁切；三种类型是 PNG / JPEG / WebP）。
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

/// 头像文件的形状校验：非空、≤ [`MAX_FILE_PARAM_BYTES`] 字节（**路径那一族**的上限——
/// 与通信面递文件同一个界；内容上限是 [`MAX_AVATAR_BYTES`]，两件事；存在性与可读性归沙盒）。
fn valid_avatar_file(params: &ActionParams) -> Option<&str> {
    params
        .get(PARAM_FILE)
        .filter(|file| !file.is_empty() && file.len() <= MAX_FILE_PARAM_BYTES)
}

/// 从文件名认图片类型：**白名单逐字**——`.png` / `.jpg`|`.jpeg` / `.webp` 三种（契约只认这三种，
/// 服务端也不按内容嗅探；认不出就是类型不支持）。
fn avatar_content_type(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else {
        None
    }
}

/// 换头像的回执：换完后的版本 + 是不是重放。
fn render_avatar_written(body: &serde_json::Value) -> String {
    let replayed = body
        .get("idempotency_replayed")
        .and_then(|flag| flag.as_bool())
        .unwrap_or(false);
    format!(
        "{}\nreplayed = {}",
        render_profile(body),
        if replayed { "yes" } else { "no" }
    )
}

/// 资料两个字段的字节上限（契约：短描述 512、公开描述 16384）。
const MAX_SHORT_DESCRIPTION_BYTES: usize = 512;
const MAX_PUBLIC_DESCRIPTION_BYTES: usize = 16 * 1024;

/// 一份 multipart/form-data 体（契约 §8 的严格形状：**恰好一个** `name="file"` part，
/// 带 filename 与 Content-Type）。
///
/// 字段名、`filename` 与 `Content-Type` 逐字照契约写；文件字节**原样搬**（不转换、不截断）。
/// 名字按 UTF-8 字节写进头（契约的 file_name 就是 UTF-8）。
/// 头像的 multipart 体：**比附件多一个 `base_version` 字段**（契约要求的并发基准）。
///
/// 顺序固定（字段在前、文件在后）：服务端的部件解析按序读，形状不猜。
fn multipart_avatar_body(
    boundary: &str,
    base_version: u64,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"base_version\"\r\n\r\n");
    body.extend_from_slice(base_version.to_string().as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// 自己的资料：`version = N`（0 = 还没写过）＋ 两句介绍 ＋ `published_by`（这一版是谁发布的：
/// **它自己还是 Owner**——表达权的署名，不是禁令）。
fn render_profile(body: &serde_json::Value) -> String {
    let profile = body.get("profile");
    let field = |name: &str| {
        profile
            .and_then(|p| p.get(name))
            .and_then(|value| value.as_str())
            .unwrap_or("-")
    };
    // ⚠ 版本号是**数字**（`profile_version: u64`），不是字符串——`as_str()` 会读成 `-`。
    let version = profile
        .and_then(|p| p.get("profile_version"))
        .and_then(|value| value.as_u64())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".to_string());
    // 谁发布的：世界如实记下**是它自己还是 Owner**（表达权的署名，不是禁令）。
    let publisher = field("published_by_kind");
    format!(
        "version = {version}\nshort = {}\npublic = {}\npublished_by = {publisher}",
        field("short_description"),
        field("public_description"),
    )
}

/// 写资料的回执：写完后的版本 + 是不是第一次写 + 落下的两句。
fn render_profile_written(body: &serde_json::Value) -> String {
    let replayed = body
        .get("idempotency_replayed")
        .and_then(|flag| flag.as_bool())
        .unwrap_or(false);
    format!(
        "{}\nreplayed = {}",
        render_profile(body),
        if replayed { "yes" } else { "no" }
    )
}
