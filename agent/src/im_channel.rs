//! im 家族的**共用层**：端点与凭据的声明、请求的机械搬运、幂等键合成，以及工具面共用的
//! 参数判据。
//!
//! 三个 App——通信 [`crate::im_app`]、社交 [`crate::im_social_app`]、资料
//! [`crate::im_profile_app`]——共用这一份实现，而 **App 之间不得有实现依赖**：机制落在中立层，
//! 谁都不认识谁的内部（与 `sandbox` 同款，见 README 里那条结构性守卫）。
//!
//! 每个 App **各自建自己的请求客户端**（各自的连接池）：[`ImChannel::new`] 收下一份
//! [`ImAccess`] 声明并把客户端建起来。本模块只搬机械事实（状态码、响应体、字节）；**分类是
//! 契约知识**，由知道那条端点契约的 App 自己做（见 [`Reply`] 的文档）。

use std::io::Read;
use std::time::Duration;

use crate::core::ActionParams;
use crate::foundation::OperationId;

/// 单次请求的墙钟上限（与适配层同一量级）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// **标识**的字节上限（房间 / 参与者 / 消息目标这一族：世界签发的不透明标识）。
///
/// 契约：机械校验先于任何往返。64 对 UUID（36 个字符）是宽松的余量；**它不是内容上限**——
/// 消息正文、附件字节、文件名各自有各自的界。
///
/// 与历史条数上限、递文件上限同族：**公开常量**——门禁用例拿它构造"刚好超限"的输入，
/// 而不是各自抄一个数字（抄的数字迟早与实现脱钩）。
pub const MAX_ID_BYTES: usize = 64;

/// **本地文件路径**的字节上限（`file` 参数：主体世界观里的沙盒相对路径）。
///
/// 它**不是**标识那一族：路径的长度由主体的资料根长什么样决定，与"世界签发了一个多长的 id"
/// 无关。两个包（通信面递文件、表达面换头像）用**同一个**界——同一种形状只有一个答案，
/// 权威在这里，谁都不许再抄一个数（2026-10-06：头像那一处曾借用标识的上限 64，是同一个
/// 形状两个答案的实例，已收）。
pub const MAX_FILE_PARAM_BYTES: usize = 512;

/// 幂等键的长度上限（契约：128 个可见 ASCII 字符）。
const MAX_KEY_CHARS: usize = 128;

/// 消息身份的字符长度（契约：严格规范的小写连字符 UUID v4）。
const MESSAGE_ID_CHARS: usize = 36;

/// 要递出去的**本地文件**参数名：自己的世界观里的沙盒相对路径。
pub const PARAM_FILE: &str = "file";

/// 写入时的**基准版本**参数名（我认为的当前版本；并发裁决在授权之后）。
pub const PARAM_BASE_VERSION: &str = "base_version";

// ---------------------------------------------------------------------------
// 声明的词汇：端点与凭据、失败词表、回程形状
// ---------------------------------------------------------------------------

/// im 家族（三个 App）共用的**封闭失败词汇表**。
///
/// 它是搬运用词，不是协议复述：协议词（状态码、错误码字符串、响应体）一个都不外泄——主体只看到
/// "这次读取属于哪一类失败"。同一种故障两次发生**逐字节相同**（分类之外不带任何动态细节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImFailure {
    /// 连不上、超时、响应读不出来（传输层）。
    Unreachable,
    /// 凭据不被接受。契约区分"缺头"与"坏凭据"，但对工具两者同义：**我的凭据不被接受**。
    CredentialRejected,
    /// 参数非法（工具面先拒绝：不出网）。
    ///
    /// ⚠️ 说话端点里**引用面与另一面同场**（回复对象与提及目标都给、或与 `file` 同场）而服务端回
    /// 400 时也归这一格：那些面的拒绝合成同一个状态码，本层**不指认**是哪一面（不分解，也不猜）。
    InvalidArgument,
    /// 房间不存在（含形状不合法——服务端不泄露存在性，本层也不猜）。
    ///
    /// ⚠️ 有两条端点把它当**唯一**的 404 含义，于是它也覆盖"我不是这间房的成员"：
    /// ·读游标推进（`PUT …/read-cursor`）；·退房（`POST …/leave`，含**已经退过**）。
    /// 服务端刻意把这几件事回成同一个状态码，本层不分解（分解就是替它说出它拒绝说出的事）。
    UnknownRoom,
    /// 房间存在但**我不是成员**（契约把这一态与上一态分开，正是为了让"存在但不在其中"可判）。
    NotAMember,
    /// 服务端明确报错（5xx 或本层不认识的其它状态）。
    ServerError,
    /// 同一个幂等键配的是**另一个请求摘要**（契约：409，零新事实、不改首次事实）。
    IdempotencyConflict,
    /// 复核时确定「现在一条都没有」（只读的一种**确定**答案，不是「读不出来」）。
    NotFound,
    /// 撤回时那个消息身份**不在可撤范围里**（契约：404 `message_not_found`）。
    ///
    /// ⚠️ 它**同时**覆盖"那条消息不存在"与"那条不是主体发的"——服务端刻意不给存在性 oracle
    /// （非本人与不存在回同一个码）。因此本层**不分解**它：不叫 `not-owner`、不叫 `forbidden`，
    /// 因为那两个名字都在替服务端说出它拒绝说出的事。
    MessageNotFound,
    /// 撤回超窗（契约：409 `message_recall_window_expired`，固定 120 秒；**已撤回的重撤不受窗口限制**）。
    WindowExpired,
    /// 那条消息**不能当读位置**（契约：400 `invalid_read_cursor`）。
    ///
    /// ⚠️ 它**同时**覆盖"那条消息不存在""那条在别的房间""那条在我加入之前"——服务端刻意不给
    /// 存在性 oracle（三种情况回同一个码）。因此本层**不分解**它：分类只到主体能据此改动作的
    /// 粒度（换一条**我读得到的**消息），再细分就是替服务端说出它拒绝说出的事。
    NotAReadTarget,
    /// 说话时**回复目标不能当回复对象**（契约 `CORE_V1_IM_CONTRACT.md` §6.5：400
    /// `invalid_reply_target`）。
    ///
    /// ⚠️ 它**同时**覆盖"目标已被撤回""目标未知""目标在别的房间""目标对我不在任一历史
    /// Membership 窗口内"——服务端刻意合成同一个码（§6.5 明文：撤回后禁新建回复与
    /// 未知/跨房/不可见**同一错误面**，不泄露是否因撤回）。因此本层**不分解**它：
    /// 分类只到主体能据此改动作的粒度（换一个我读得到的目标，或不回复），
    /// 再细分就是替服务端说出它拒绝说出的事。
    NotAReplyTarget,
    /// 说话时**提及目标不能被接受**（契约 `CORE_V1_MENTION_TARGETS.md` §5：400 `invalid_mention`）。
    ///
    /// ⚠️ 它**同时**覆盖"目标不是本房 active 成员""目标 participant 非 active""不是 group
    /// 房""重复目标 / 形状非法 / 超过 64 个"——服务端刻意不区分具体原因（不泄露目标是否存在、
    /// 是否成员）。因此本层**不分解**它：分类只到主体能据此改动作的粒度（换目标、或干脆不点名）。
    NotAMentionTarget,
    /// 回答关系申请时**那条不是我的事**（契约：404 `contact_not_found`）。
    ///
    /// ⚠️ 它**同时**覆盖"这对之间没有关系""我不是被申请的那一方""那段已经不是
    /// pending"——服务端刻意合成同一个码（不泄露关系状态）。因此本层**不分解**它：
    /// 分类只到主体能据此改动作的粒度（先 `im_contacts` 看清 `direction`），
    /// 再细分就是替服务端说出它拒绝说出的事。
    UnknownContact,
    /// 关系面/拉黑面的**目标主体不可用**（契约：404 `participant_not_found`）。
    ///
    /// ⚠️ 同样是一个刻意合成的码：不存在 / 非 active / **被对方拉黑**三者同形——
    /// 这是**不泄露**（没有存在性 oracle），不是"服务端不知道"。本层不分解。
    UnknownParticipant,
    /// 写资料时**基准版本对不上**（契约：409 `profile_version_conflict`——并发裁决）。
    ///
    /// 它是"确定的拒绝"（零新事实）：我手上的版本不是世界当前的版本，先读再写。
    VersionConflict,
    /// 退房时**我是 owner**：必须先转让再退出（契约：409 `owner_transfer_required`）。
    ///
    /// ⚠️ **今天 Agent 不可能成为群 owner**（转让目标不许是 Agent），所以这一格是按契约错误面
    /// 如实映射的**防御格**，不是可达路径——它存在只是为了不把"明确被拒"说成 `unknown`。
    OwnerTransferRequired,
    /// **owner 移不掉自己**（契约 `owner_not_removable`，2026-10-13 补）：它不是
    /// `not-a-member`——他不是"不在里面"，而是**不能把自己请出去**；出路是先转让
    /// （`im_transfer_owner`）或者整间房留给别人。
    OwnerNotRemovable,
    /// 要递出去的那个文件在主体的世界里**不存在**。
    FileNotFound,
    /// 要递出去的那个文件读不出来：路径非法（绝对路径 / `..` / 盘符 / 空）、不是普通文件、
    /// 链上有符号链接、被拒绝或底层 I/O 报错。
    ///
    /// 这几格**不分解**：主体指一个文件，读不到就是读不到——分类只到主体能据此改动作的粒度
    /// （改路径 / 换一个文件），再细分只是把实现的形状漏出去。
    FileUnreadable,
    /// 太大：超过这一侧的上限（读附件或递文件）。**明确拒绝，不截断、不静默丢**。
    TooLarge,
    /// 这个文件的类型不在**这一侧**的清单里（按扩展名判定）。
    ///
    /// ⚠️ 今天只有一个产生者：**表达面**（`im_profile_app` 的头像上传）。im 的「递文件」那一侧
    /// **没有类型门**（2026-10-11：认不出的类型名照原样递，不拒收）；读附件那一侧
    /// 是另一格 [`ImFailure::UnsupportedContentType`]（按响应头判定）。这一格留着也因为**解码端**：
    /// 它曾经由「递文件」产生，旧账里可能有这个取值。
    UnsupportedFileType,
    /// 服务端说这个附件的内容类型不在主体读得动的清单里（读进来时按响应头判定）。
    UnsupportedContentType,
    /// 内容不是合法 UTF-8 文本——主体看不了它（**不**替换字符冒充"读到了"）。
    NotText,
}

impl ImFailure {
    /// 稳定分类标记（进结果、给模型看）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ImFailure::Unreachable => "unreachable",
            ImFailure::CredentialRejected => "credential-rejected",
            ImFailure::InvalidArgument => crate::core::INVALID_ARGUMENT,
            ImFailure::UnknownRoom => "unknown-room",
            ImFailure::NotAMember => "not-a-member",
            ImFailure::ServerError => "server-error",
            ImFailure::IdempotencyConflict => "idempotency-conflict",
            ImFailure::NotFound => "not-found",
            ImFailure::MessageNotFound => "message-not-found",
            ImFailure::WindowExpired => "window-expired",
            ImFailure::NotAReadTarget => "not-a-read-target",
            ImFailure::NotAReplyTarget => "not-a-reply-target",
            ImFailure::NotAMentionTarget => "not-a-mention-target",
            ImFailure::UnknownContact => "unknown-contact",
            ImFailure::UnknownParticipant => "unknown-participant",
            ImFailure::VersionConflict => "version-conflict",
            ImFailure::OwnerTransferRequired => "owner-transfer-required",
            ImFailure::OwnerNotRemovable => "owner-not-removable",
            ImFailure::FileNotFound => "file-not-found",
            ImFailure::FileUnreadable => "file-unreadable",
            ImFailure::TooLarge => "too-large",
            ImFailure::UnsupportedFileType => "unsupported-file-type",
            ImFailure::UnsupportedContentType => "unsupported-content-type",
            ImFailure::NotText => "not-text",
        }
    }

    /// 从稳定标记还原（解码用）。认不出的取值就是 `None`——**不猜**，也不就近映射
    /// （调用方按 fail-closed 处理）。
    ///
    /// **标记表就是权威**：`as_str` 与它**逐条对称**，有测试钉住（`tests/action_failure_wordlist.rs`）。
    /// 新增取值必须**同一刀**补在这里——这一格落在盘上的**行动结果**里，缺了解码端就是
    /// 「写得进去、读不出来」。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Option<Self> {
        Some(match text {
            "unreachable" => ImFailure::Unreachable,
            "credential-rejected" => ImFailure::CredentialRejected,
            crate::core::INVALID_ARGUMENT => ImFailure::InvalidArgument,
            "unknown-room" => ImFailure::UnknownRoom,
            "not-a-member" => ImFailure::NotAMember,
            "server-error" => ImFailure::ServerError,
            "idempotency-conflict" => ImFailure::IdempotencyConflict,
            "not-found" => ImFailure::NotFound,
            "message-not-found" => ImFailure::MessageNotFound,
            "window-expired" => ImFailure::WindowExpired,
            "not-a-read-target" => ImFailure::NotAReadTarget,
            "not-a-reply-target" => ImFailure::NotAReplyTarget,
            "not-a-mention-target" => ImFailure::NotAMentionTarget,
            "unknown-contact" => ImFailure::UnknownContact,
            "unknown-participant" => ImFailure::UnknownParticipant,
            "version-conflict" => ImFailure::VersionConflict,
            "owner-transfer-required" => ImFailure::OwnerTransferRequired,
            "owner-not-removable" => ImFailure::OwnerNotRemovable,
            "file-not-found" => ImFailure::FileNotFound,
            "file-unreadable" => ImFailure::FileUnreadable,
            "too-large" => ImFailure::TooLarge,
            "unsupported-file-type" => ImFailure::UnsupportedFileType,
            "unsupported-content-type" => ImFailure::UnsupportedContentType,
            "not-text" => ImFailure::NotText,
            _ => return None,
        })
    }
}

/// 一次请求的回程——**机械分类**，不是判断。
///
/// 分成四格只为一件事：让读侧与写侧各自能说出**它需要的那句话**，而不是被一个"成功/失败"的
/// 两分法逼着撒谎。
pub(crate) enum Reply {
    /// 2xx 且响应体读得出（写侧从体里的重放标记分辨是首次还是重放）。
    Answered(serde_json::Value),
    /// 非 2xx：**只带状态码，不带任何解释**。
    ///
    /// 为什么通用请求层不直接给分类：**同一个码在不同端点上的含义不同**——`404` 在翻记录里是
    /// 「房不在（或我不是成员）」，在撤回里是「那条消息不在可撤范围里」；`409` 在说话里是
    /// 「同键异摘要」，在撤回里是「超窗」。分类是**契约知识**，只能由知道那个端点契约的调用方做；
    /// 通用层只搬机械事实（状态码本身）。
    Refused(u16),
    /// 传输失败或读不到响应（连接、超时、半截响应）。
    Broken,
    /// 2xx 但响应体读不懂。
    Unreadable,
}

/// 一次**取字节**请求的回程（附件的字节不是 JSON：响应体是二进制，类型在响应头里）。
///
/// 与 [`Reply`] 分开是**故意的**：JSON 那条路要的是"读得懂的体"，这条路要的是"原样的字节"，
/// 两个形状塞进一个枚举会逼着每一侧都去处理自己用不上的格子。
pub(crate) enum ByteReply {
    /// 2xx：拿到了字节（`content_type` 是响应头的原文，可能带参数；缺头时是 `None`）。
    Fetched {
        content_type: Option<String>,
        bytes: Vec<u8>,
    },
    /// 非 2xx：只带状态码（分类是契约知识，由知道那个端点契约的调用方做）。
    Refused(u16),
    /// 传输失败或读不到响应。
    Broken,
    /// 响应体超过我们要的上限：**没读完就停**（既没把整份拉进内存，也没截断成半份）。
    OverLimit,
}

/// 宿主给 im 家族的**声明**：端点 + 凭据。
///
/// 两样都由宿主显式给出（命令行给端点、环境变量给凭据），这里不去找、不配置、不落盘。
/// 凭据只被写进请求头；本类型**不派生 `Debug`**，因此它不可能被顺手打进日志或报告。
#[derive(Clone)]
pub struct ImAccess {
    endpoint: String,
    credential: String,
}

impl ImAccess {
    /// 用端点与凭据建立一份声明；空值一律拒绝（"没配好"不是一种运行状态）。
    pub fn new(endpoint: impl Into<String>, credential: impl Into<String>) -> Option<Self> {
        let endpoint = endpoint.into();
        let credential = credential.into();
        if endpoint.trim().is_empty() || credential.trim().is_empty() {
            return None;
        }
        Some(Self {
            endpoint,
            credential,
        })
    }
}

/// 一条到 im 服务端的**通道**：一份访问声明 ＋ 自己的请求客户端。
///
/// 每个 App 各自建一份（多两个连接池，可接受）——"各自建自己的客户端"是装配事实，
/// 不是共享可变状态；本类型只做机械搬运，不做任何判断。
pub(crate) struct ImChannel {
    access: ImAccess,
    client: reqwest::blocking::Client,
}

impl ImChannel {
    /// 建一条通道（请求客户端在这里建一次，含单次请求的墙钟上限）。
    pub(crate) fn new(access: ImAccess) -> Option<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self { access, client })
    }

    /// 一次请求（读与写都走这里）。**不重试、不退避、不换端点**。
    ///
    /// **写**动作一次调用恰好一次请求——这是最要紧的那条（绝不重发）；只读复核是**有界回翻**
    /// （im_app 的 `sweep`，最多 [`crate::im_app::MAX_VERIFY_PAGES`] 页；`im_verify_said`
    /// 另加**恰好一次**身份探针），每一步都是幂等读、各自恰好一次。
    ///
    /// 它只做「我们读到了什么」的机械搬运，**不替调用方决定这算不算失败、更不给分类**——读侧、说话侧
    /// 与撤回侧对同一个状态码的解释**不同**（分类是契约知识，见 [`Reply::Refused`]）。
    /// 凭据只在请求头里出现一次，任何分支都不把它带进返回值。
    pub(crate) fn exchange(
        &self,
        method: &str,
        path: &str,
        payload: Option<String>,
        key: Option<&str>,
    ) -> Reply {
        let target = format!("{}{path}", self.access.endpoint);
        let mut request = match method {
            "POST" => self.client.post(target),
            "PUT" => self.client.put(target),
            "DELETE" => self.client.delete(target),
            _ => self.client.get(target),
        };
        request = request
            .header(
                "Authorization",
                format!("Bearer {}", self.access.credential),
            )
            .header("Accept", "application/json");
        if let Some(key) = key {
            // 契约要求写请求必须带幂等键；键由这次操作的身份合成，见 [`idempotency_key`]。
            request = request.header("Idempotency-Key", key);
        }
        if let Some(payload) = payload {
            request = request
                .header("Content-Type", "application/json")
                .body(payload);
        }

        let Ok(response) = request.send() else {
            return Reply::Broken;
        };
        let status = response.status().as_u16();
        let Ok(text) = response.text() else {
            return Reply::Broken;
        };
        match status {
            200..=299 => match serde_json::from_str(&text) {
                Ok(value) => Reply::Answered(value),
                Err(_) => Reply::Unreadable,
            },
            // 非 2xx 一律只交状态码：分类由知道该端点契约的调用方做（见 [`Reply::Refused`]）。
            _ => Reply::Refused(status),
        }
    }

    /// 取**字节**（附件下载）：响应不是 JSON，是二进制 + `Content-Type`。
    ///
    /// 上限**在读之前先判一次**（`Content-Length` 已知且超限就直接停），再按 `max + 1` 限量读：
    /// "太大"因此既不会把 16 MiB 拉进内存，也不会被悄悄截断成一份看起来完整的内容。
    /// 类型只在响应头里取（**不嗅探字节**——通道那条纪律在这一侧同样成立）。
    pub(crate) fn fetch_bytes(&self, path: &str, max: usize) -> ByteReply {
        let target = format!("{}{path}", self.access.endpoint);
        let Ok(response) = self
            .client
            .get(target)
            .header(
                "Authorization",
                format!("Bearer {}", self.access.credential),
            )
            .header("Accept", "*/*")
            .send()
        else {
            return ByteReply::Broken;
        };
        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return ByteReply::Refused(status);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if let Some(length) = response.content_length()
            && length > max as u64
        {
            return ByteReply::OverLimit;
        }
        let mut bytes = Vec::new();
        let mut limited = response.take(max as u64 + 1);
        if limited.read_to_end(&mut bytes).is_err() {
            return ByteReply::Broken;
        }
        if bytes.len() > max {
            return ByteReply::OverLimit;
        }
        ByteReply::Fetched {
            content_type,
            bytes,
        }
    }

    /// 发一段**字节**并带幂等键（表达权的 multipart 写用）：`PUT` + `Idempotency-Key`。
    ///
    /// 与 [`ImChannel::post_bytes`] 同族，只多两件：方法是 PUT、带上幂等键——头像写入按
    /// 契约**必带**键（并发裁决在授权之后；同 key 同摘要重放不产生新版本）。
    pub(crate) fn put_bytes(
        &self,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
        key: &str,
    ) -> Reply {
        let target = format!("{}{path}", self.access.endpoint);
        let Ok(response) = self
            .client
            .put(target)
            .header(
                "Authorization",
                format!("Bearer {}", self.access.credential),
            )
            .header("Accept", "application/json")
            .header("Content-Type", content_type)
            .header("Idempotency-Key", key)
            .body(body)
            .send()
        else {
            return Reply::Broken;
        };
        let status = response.status().as_u16();
        let Ok(text) = response.text() else {
            return Reply::Broken;
        };
        match status {
            200..=299 => match serde_json::from_str(&text) {
                Ok(value) => Reply::Answered(value),
                Err(_) => Reply::Unreadable,
            },
            other => Reply::Refused(other),
        }
    }

    /// 发一段**字节**（multipart 用；`exchange` 那条路只发 JSON 文本）。
    pub(crate) fn post_bytes(&self, path: &str, content_type: &str, body: Vec<u8>) -> Reply {
        let target = format!("{}{path}", self.access.endpoint);
        let Ok(response) = self
            .client
            .post(target)
            .header(
                "Authorization",
                format!("Bearer {}", self.access.credential),
            )
            .header("Accept", "application/json")
            .header("Content-Type", content_type)
            .body(body)
            .send()
        else {
            return Reply::Broken;
        };
        let status = response.status().as_u16();
        let Ok(text) = response.text() else {
            return Reply::Broken;
        };
        match status {
            200..=299 => match serde_json::from_str(&text) {
                Ok(value) => Reply::Answered(value),
                Err(_) => Reply::Unreadable,
            },
            _ => Reply::Refused(status),
        }
    }
}

/// 一次写的**幂等键**：由 Core 给的**操作身份**合成。
///
/// 同一个操作 ⇒ 同一个键 ⇒ 契约保证服务端对该 `(actor, key)` **至多一条**公共消息；主体有意说两遍
/// 是两次不同操作 ⇒ 两个键 ⇒ 两条消息（这是对的）。键的字符规则（可见 ASCII、≤128、无逗号与空白）
/// 在这里落成**机械可逆**的转义：允许的字符原样保留，其余按 UTF-8 字节写成 `%XX`——不引摘要函数，
/// 而且服务端那边一眼看得出这是哪一次操作。超长只可能是超长活动标识，截到上限即可：真撞键的后果
/// 是 409（明确拒绝、零新事实），不是静默重复。
pub(crate) fn idempotency_key(operation: &OperationId) -> String {
    let mut key = String::from("celira-agent-");
    for byte in operation.run().as_str().bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            key.push(ch);
        } else {
            key.push_str(&format!("%{byte:02X}"));
        }
    }
    key.push_str(&format!(
        "-{}-{}",
        operation.turn().number(),
        operation.index()
    ));
    key.truncate(MAX_KEY_CHARS);
    key
}

/// 严格规范的小写连字符 UUID v4 形状（长度、字符集、四个连字符的位置）。
///
/// 消息身份与附件身份共用这一条：**形状规则只有一份**，谁都不许各写一遍。
pub(crate) fn canonical_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != MESSAGE_ID_CHARS {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => *byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
    })
}

/// 基准版本：非负十进制整数（缺省或非法 ⇒ 参数非法，**一次都不出网**）。
pub(crate) fn valid_base_version(params: &ActionParams) -> Option<u64> {
    params.get(PARAM_BASE_VERSION)?.trim().parse::<u64>().ok()
}

/// 可选文本参数：缺省 ⇒ `None`（清空那一句）；给了就必须非空、≤ 上限、不含 NUL。
/// 返回 `None` 表示**参数非法**（与"值是 null"是两件事，靠 `Option<Option<&str>>` 区分）。
pub(crate) fn valid_optional_text<'a>(
    params: &'a ActionParams,
    name: &str,
    max_bytes: usize,
) -> Option<Option<&'a str>> {
    match params.get(name) {
        None | Some("") => Some(None),
        Some(text) => {
            if text.len() <= max_bytes && !text.contains('\0') {
                Some(Some(text))
            } else {
                None
            }
        }
    }
}

/// 一次上传的 multipart 边界串：由**操作身份**派生，并先确认它没出现在内容里。
///
/// 撞上（内容里恰好含这个边界）就往后加字符直到不撞——内容有限长，这一步一定会停。边界是
/// **机械材料**、不是身份，因此不需要随机性；需要的是"不与内容同形"这一条。
pub(crate) fn multipart_boundary(operation: &OperationId, bytes: &[u8]) -> String {
    let mut boundary = format!("----celira-im-{}", idempotency_key(operation));
    while contains_bytes(bytes, boundary.as_bytes()) {
        boundary.push('x');
    }
    boundary
}

/// 这段字节序列在内容里出现过吗。
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(crate) fn multipart_body(
    boundary: &str,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}
