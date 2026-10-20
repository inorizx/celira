//! TASK_21 + TASK_22 验收：im app 的两个只读动词（`im_rooms` / `im_read_history`）与
//! 一个写动词加它的复核（`im_say` / `im_verify_said`）。
//!
//! 这一份文件用**离线回环 im 服务**（脚本化回执 + 请求原文捕获 + 连接计数）证明六件事：
//!
//! 1. **请求形状**：路径、查询串与 `Authorization` 头逐字段对得上权威契约（B1/B2）；
//! 2. **失败词表**：400 / 两 401 / 403 / 404 / 5xx / 未知状态 / 读不懂 / 连不上，各自归到本 App
//!    自己的稳定分类，**同一种故障两次发生逐字节相同**，且一个协议词都不外泄；
//! 3. **工具面严于路由面**：`since` 与 `limit` 的非法值在本地就被拒绝——**零出网**；
//! 4. **请求次数是形状**：写动作一次调用**恰好一次**（连接计数逐次核对，不重试）；只读复核
//!    **恰好两步**（先问"我是谁"、再读那一页），一步不多；
//! 5. **身份来自服务器**：复核的归因比对的是 `actor.id` 与服务器回答的身份——本地 AgentId
//!    出现在 `actor.id` 上**不算**主体发的（真实联跑抓到过这个洞，见 `agent()` 的注释）；
//! 6. **能力与效应声明**：目录恰好两个读动词 + 一个写动词 + 它的复核；只有写被声明为
//!    `repeatable-write` 且必配复核；未声明的动词一律拒绝。
//!
//! 真实 im 服务端的端到端读数（注册流 + 真消息）在交付报告里，不在这份文件里（它不联网）。

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use celira_agent::app_contract::AppPort;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort,
};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::im_app::{
    ATTACHMENT_LINE_CHARS, ATTACHMENT_WINDOW_BYTES, ATTACHMENT_WINDOW_LINES,
    CAPABILITY_CREATE_GROUP, CAPABILITY_INVITE, CAPABILITY_LEAVE_ROOM, CAPABILITY_MARK_READ,
    CAPABILITY_READ_ATTACHMENT, CAPABILITY_READ_HISTORY, CAPABILITY_RECALL,
    CAPABILITY_REMOVE_MEMBER, CAPABILITY_ROOM_LIVE, CAPABILITY_ROOM_MEMBERS, CAPABILITY_ROOMS,
    CAPABILITY_SAY, CAPABILITY_TRANSFER_OWNER, CAPABILITY_TYPING, CAPABILITY_VERIFY_RECALLED,
    CAPABILITY_VERIFY_SAID, CAPABILITY_WRITE_PRESENTATION, IM_APP_ID, ImApp, MAX_HISTORY_LIMIT,
    MAX_MENTION_TARGETS, MAX_SAY_FILE_BYTES, MAX_VERIFY_PAGES, PARAM_ATTACHMENT,
    PARAM_ATTACHMENT_OFFSET, PARAM_BEFORE, PARAM_BODY, PARAM_FILE_NAME, PARAM_LIMIT,
    PARAM_MENTIONS, PARAM_MESSAGE, PARAM_NAME, PARAM_REPLY_TO, PARAM_ROOM, PARAM_SINCE,
};
use celira_agent::im_channel::{
    ImAccess, MAX_FILE_PARAM_BYTES, MAX_ID_BYTES, PARAM_BASE_VERSION, PARAM_FILE,
};
use celira_agent::im_profile_app::{
    CAPABILITY_MY_PROFILE, CAPABILITY_WRITE_AVATAR, CAPABILITY_WRITE_PROFILE, IM_PROFILE_APP_ID,
    ImProfileApp, PARAM_PUBLIC_DESCRIPTION, PARAM_SHORT_DESCRIPTION,
};
use celira_agent::im_social_app::{
    CAPABILITY_ANSWER_CONTACT, CAPABILITY_BLOCK, CAPABILITY_BLOCKS, CAPABILITY_CONTACTS,
    CAPABILITY_CONTACTS_REQUEST, CAPABILITY_END_CONTACT, CAPABILITY_UNBLOCK, IM_SOCIAL_APP_ID,
    ImSocialApp, PARAM_DECISION, PARAM_PARTICIPANT,
};
use celira_agent::os::{MinimalOs, RunExpansion};

/// 哨兵凭据：任何回执、失败分类或经历里出现它就说明越界了。
const SENTINEL: &str = "fixture-im-sentinel-credential";

// ---------------------------------------------------------------------------
// 假 im 服务：一次连接回一条脚本回执，并把请求原文交给用例
// ---------------------------------------------------------------------------

struct FakeIm {
    endpoint: String,
    requests: mpsc::Receiver<String>,
    connections: Arc<AtomicUsize>,
    _handle: JoinHandle<()>,
}

/// 一条脚本回执。
///
/// 默认是 JSON（既有用例的形状）；附件下载那条路回的是**二进制 + Content-Type**，因此它得能
/// 原样给字节——把字节塞进 JSON 里会连"这不是 JSON"这个事实都盖掉。
enum Scripted {
    Json(u16, String),
    Raw {
        status: u16,
        content_type: Option<String>,
        body: Vec<u8>,
    },
}

impl Scripted {
    /// 一条 200 的**文本附件**回执（下载用例最常用的一条）。
    fn text(body: &str) -> Self {
        Self::raw(200, "text/plain", body.as_bytes())
    }

    fn raw(status: u16, content_type: &str, body: &[u8]) -> Self {
        Scripted::Raw {
            status,
            content_type: Some(content_type.to_string()),
            body: body.to_vec(),
        }
    }
}

impl FakeIm {
    /// 按顺序回执；脚本用完之后再多来的连接一律 500 并计数（用来抓"偷偷重试"）。
    fn spawn(replies: Vec<(u16, String)>) -> Self {
        Self::spawn_scripted(
            replies
                .into_iter()
                .map(|(status, body)| Scripted::Json(status, body))
                .collect(),
        )
    }

    /// 同 [`FakeIm::spawn`]，但回执可以是**原样字节**（附件下载要的形状）。
    fn spawn_scripted(replies: Vec<Scripted>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
        let port = listener.local_addr().expect("有地址").port();
        let (sender, requests) = mpsc::channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);

        let handle = std::thread::spawn(move || {
            let mut queue = replies.into_iter();
            while let Ok((stream, _)) = listener.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut reader_stream = stream.try_clone().expect("克隆流");
                let request = read_request(&mut reader_stream);
                let _ = sender.send(request);
                match queue.next() {
                    Some(scripted) => respond_scripted(stream, &scripted),
                    None => respond(stream, 500, "{}"),
                }
            }
        });

        Self {
            endpoint: format!("http://127.0.0.1:{port}"),
            requests,
            connections,
            _handle: handle,
        }
    }

    fn app(&self) -> ImApp {
        let access = ImAccess::new(self.endpoint.clone(), SENTINEL).expect("声明可以建立");
        ImApp::new(access).expect("可以装配 im app")
    }

    /// 带**世界观根**的那只（递文件要用）：根由一个临时目录给出，用例自己往里放文件。
    ///
    /// 它与 [`FakeIm::app`] 的**能力面完全相同**——世界根只影响"那只手够不够得着文件"，
    /// 不影响工具名与参数形状（恢复承重）。
    fn app_with_world(&self, world: &Path) -> ImApp {
        let access = ImAccess::new(self.endpoint.clone(), SENTINEL).expect("声明可以建立");
        ImApp::new(access)
            .expect("可以装配 im app")
            .with_world_root(world)
    }

    /// 带**模型视觉声明**的那只（2026-10-17）：可读清单里因此多出媒体类。
    ///
    /// 与 [`FakeIm::app`] 的**能力面名字与参数形状完全相同**——变的只有"哪些类型读得动"
    /// 那一行清单（恢复承重）。
    fn app_with_vision(&self, vision: bool) -> ImApp {
        let access = ImAccess::new(self.endpoint.clone(), SENTINEL).expect("声明可以建立");
        ImApp::new(access)
            .expect("可以装配 im app")
            .with_vision(vision)
    }

    /// 社交面那只（`im-social`）：与 [`FakeIm::app`] 同端点、同凭据，**另一份实例**。
    ///
    /// 2026-10-02 拆包后关系面动词住在这里；它与通信面的**能力面互不相交**（这是拆包要证的）。
    fn social_app(&self) -> ImSocialApp {
        let access = ImAccess::new(self.endpoint.clone(), SENTINEL).expect("声明可以建立");
        ImSocialApp::new(access).expect("可以装配 im-social app")
    }

    /// 资料面那只（`im-profile`）。
    fn profile_app(&self) -> ImProfileApp {
        let access = ImAccess::new(self.endpoint.clone(), SENTINEL).expect("声明可以建立");
        ImProfileApp::new(access).expect("可以装配 im-profile app")
    }

    /// 带**世界观根**的资料面那只（换头像要读本地字节）。
    fn profile_app_with_world(&self, world: &Path) -> ImProfileApp {
        let access = ImAccess::new(self.endpoint.clone(), SENTINEL).expect("声明可以建立");
        ImProfileApp::new(access)
            .expect("可以装配 im-profile app")
            .with_world_root(world)
    }

    fn next_request(&self) -> String {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .expect("这一次调用应当发出一个请求")
    }

    /// 取下一个**不是身份探针、也不是读位置推进**的请求。
    ///
    /// TASK_71 桌面 P0 第 4 件起，**读面回执**会在读完正文之后补一次 `GET /v1/backend/me`
    /// （回执开头那行 `me = …`）⇒ 请求序列里会多出一条。判"这一次**读**请求带了什么"的地方
    /// 用这个取，才不会把那条探针当成下一次读。
    ///
    /// ⚠ **2026-10-10 起还多一条**：一次 `im_read_history` = 读那一页（GET）＋ **顺带推进读位置**
    /// （`PUT …/read-cursor`）两次请求。那一条也跳过——本函数问的是"**读**请求带了什么"，
    /// 而推进那一次有自己的用例（`reading_history_also_advances_the_read_cursor`）。
    fn next_read_request(&self) -> String {
        loop {
            let request = self.next_request();
            if !request.starts_with("GET /v1/backend/me ")
                && !request.starts_with("PUT /v1/backend/rooms/")
            {
                return request;
            }
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// 取下一个以 `prefix` 开头的请求（把别的请求**丢掉**，只在"我只关心这一类"时用）。
    ///
    /// ⚠ 它会把跳过的那些从通道里**取走**（通道是消费式的）⇒ 只在断言不再需要那些请求时用。
    fn next_request_of(&self, prefix: &str) -> Option<String> {
        loop {
            let request = self.requests.recv_timeout(Duration::from_secs(5)).ok()?;
            if request.starts_with(prefix) {
                return Some(request);
            }
        }
    }

    /// 请求序列里**有没有**这一类请求（取走式：问完就没了）。
    fn any_request_of(&self, prefix: &str) -> bool {
        self.next_request_of(prefix).is_some()
    }
}

/// 读一条完整的请求（请求行 + 头 + **体**），原样返回。
///
/// **必须把体排空再回**：写请求是 POST，客户端还在写体的时候服务端就把连接关了，客户端会看到
/// 连接被重置（于是读到的是"连不上"而不是回执）——这一条在 TASK_22 的写侧用例里实测踩到过。
fn read_request(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut raw = String::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end().to_string();
                if trimmed.is_empty() {
                    break;
                }
                if let Some(value) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                raw.push_str(&trimmed);
                raw.push('\n');
            }
            Err(_) => break,
        }
    }
    if length > 0 {
        let mut body = vec![0u8; length];
        let mut filled = 0usize;
        while filled < length {
            match reader.read(&mut body[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(_) => break,
            }
        }
        raw.push_str(&String::from_utf8_lossy(&body[..filled]));
    }
    raw
}

/// 回一条 HTTP 响应（Content-Length + 立刻关闭）。
fn respond(mut stream: TcpStream, status: u16, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        reason_of(status),
        body.len()
    );
    let _ = stream.flush();
}

/// 回一条**原样字节**响应（附件下载那条路要的形状：二进制体 + 给定的 Content-Type）。
fn respond_scripted(mut stream: TcpStream, scripted: &Scripted) {
    match scripted {
        Scripted::Json(status, body) => respond(stream, *status, body),
        Scripted::Raw {
            status,
            content_type,
            body,
        } => {
            let mut head = format!("HTTP/1.1 {status} {}\r\n", reason_of(*status));
            if let Some(content_type) = content_type {
                head.push_str(&format!("Content-Type: {content_type}\r\n"));
            }
            head.push_str(&format!(
                "Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ));
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    }
}

fn reason_of(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    }
}

// ---------------------------------------------------------------------------
// 用例辅助
// ---------------------------------------------------------------------------

/// 主体在这台机器上的身份（环境给的）——**故意**与 [`IM_PARTICIPANT`] 不同。
///
/// TASK_22 真实联跑抓到的缺陷就是这里：本地 AgentId（真的长这样）与 Agent 在 im 里的参与者身份
/// （服务器签发的 UUID）是**两套命名空间**，从不相等。早先的离线用例把两边写成同一个字符串，
/// 于是"归因错了"这件事按构造自洽、永远照不见。围栏：任何地方拿本地 AgentId 去比 `actor.id`，
/// 这条用例就会红。
fn agent() -> AgentId {
    AgentId::new("agent-t22")
}

// ---------------------------------------------------------------------------
// 房治理（2026-10-13）：她自己开一间群房、把有来往的人拉进来
// ---------------------------------------------------------------------------

/// **她能自己开房、能邀请**（她清单第⑦条：「绑定了主人却没有出站通道」）。
///
/// 两条动词走的是服务端 **2026-09-27 就有**的 Agent 侧治理端点——缺的从来是**这一侧的动作**
/// （当年那批切片的原文写着「不动客户端与执行端」）。这一条钉三件事：路径与体对得上契约、
/// 一次调用恰好一次请求、回执给的是世界回的标识。
#[test]
fn she_can_open_a_group_and_invite_someone_into_it() {
    let fake = FakeIm::spawn(vec![
        (
            201,
            r#"{"room":{"id":"room-1","kind":"group","status":"active","name":"收件箱","created_at":"t"},"memberships":[],"idempotency_replayed":false,"request_id":"r"}"#.to_string(),
        ),
        (
            200,
            r#"{"membership":{"membership_id":"m-1","participant_id":"p-9","role":"member","joined_at":"t","left_at":null,"closed_by":null},"idempotency_replayed":false,"request_id":"r"}"#.to_string(),
        ),
    ]);
    let mut app = fake.app();
    let operation = OperationId::new(RunId::new("run-group"), Turn::FIRST);

    // ① 开一间群房：回执给出房的标识与群名（**她是 owner**）。
    let opened = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_CREATE_GROUP),
            ActionParams::none().with(PARAM_NAME, "收件箱"),
        ),
    );
    assert_eq!(opened.state(), ResultState::Success, "{opened:?}");
    assert_eq!(
        text_of(&opened),
        "room = room-1\nname = 收件箱\nowner = me\n"
    );
    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/groups "),
        "路径必须是 Agent 侧开群那条端点: {request}"
    );
    assert!(
        request.contains("\"name\":\"收件箱\""),
        "体是契约那个字段: {request}"
    );
    assert_eq!(fake.connections(), 1, "一次调用恰好一次请求");

    // ② 邀请一个人进来：回执给出他的标识与角色。
    let invited = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_INVITE),
            ActionParams::none()
                .with(PARAM_ROOM, "room-1")
                .with(PARAM_PARTICIPANT, "p-9"),
        ),
    );
    assert_eq!(invited.state(), ResultState::Success, "{invited:?}");
    assert_eq!(
        text_of(&invited),
        "room = room-1\nmember = p-9\nrole = member\n"
    );
    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/rooms/room-1/members "),
        "路径必须是 Agent 侧邀请那条端点: {request}"
    );
    assert!(
        request.contains("\"participant_id\":\"p-9\""),
        "线协议的字段名是 participant_id: {request}"
    );
    assert_eq!(fake.connections(), 2, "两次调用两次请求");
}

/// 房治理两条的**失败面**：契约的码如实分类，而**参数不合法时一次请求都不发**。
#[test]
fn the_room_governance_failures_are_classified_and_local_checks_come_first() {
    let fake = FakeIm::spawn(vec![
        (409, "{}".to_string()),
        (403, "{}".to_string()),
        (404, "{}".to_string()),
    ]);
    let mut app = fake.app();
    let operation = OperationId::new(RunId::new("run-group-fail"), Turn::FIRST);

    // 参数不合法 ⇒ 明确失败，而且**不出网**（连接数不涨）。
    for (verb, params) in [
        (
            CAPABILITY_CREATE_GROUP,
            ActionParams::none().with(PARAM_NAME, ""),
        ),
        (
            CAPABILITY_CREATE_GROUP,
            ActionParams::none().with(PARAM_NAME, "带\n换行"),
        ),
        (
            CAPABILITY_INVITE,
            ActionParams::none()
                .with(PARAM_ROOM, "room-1")
                .with(PARAM_PARTICIPANT, ""),
        ),
    ] {
        let failed = app.execute(
            &agent(),
            &operation,
            &Action::with_params(ActionId::new(verb), params),
        );
        assert_eq!(failed.failure_class(), Some("invalid-argument"), "{verb}");
        assert_eq!(fake.connections(), 0, "参数不合法时一次请求都不发");
    }

    // 同一个键配了另一个群名 ⇒ `idempotency-conflict`（契约给这条端点列了 409）。
    let conflict = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_CREATE_GROUP),
            ActionParams::none().with(PARAM_NAME, "另一个名字"),
        ),
    );
    assert_eq!(conflict.failure_class(), Some("idempotency-conflict"));

    // 我不是这间房的 owner ⇒ `not-a-member`（403）。
    let not_owner = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_INVITE),
            ActionParams::none()
                .with(PARAM_ROOM, "room-1")
                .with(PARAM_PARTICIPANT, "p-9"),
        ),
    );
    assert_eq!(not_owner.failure_class(), Some("not-a-member"));

    // 房不在 / 目标不存在 ⇒ `unknown-room`（契约把两种合成一个码，本层也不分解）。
    let unknown = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_INVITE),
            ActionParams::none()
                .with(PARAM_ROOM, "room-x")
                .with(PARAM_PARTICIPANT, "p-9"),
        ),
    );
    assert_eq!(unknown.failure_class(), Some("unknown-room"));
    assert_eq!(fake.connections(), 3, "三次失败各一次请求");
}

/// **成员清单**（2026-10-13）：她要能列出"谁在这间房里"——此前只有"谁说过话"（消息的 `actor`）
/// 与"此刻谁在"（`im_room_live` 的在场态）。
///
/// 服务端那条端点在 **Agent 读面**里早就有（`CORE_V1_AGENT_READ_SURFACE.md`：requester 必须对该房
/// 存在 active Membership 周期，投影与 Human 侧逐字段相同），缺的一直是**这一侧的动作**。
/// 它的到来还让 `im_invite` 从"核实不了"升回可复核（见 `declared_effects`）。
#[test]
fn she_can_list_who_is_in_a_room() {
    let fake = FakeIm::spawn(vec![
        (
            200,
            r#"{"members":[{"participant":{"id":"p-1","kind":"agent","display_name":"小欣","status":"active"},"membership":{"membership_id":"m-1","participant_id":"p-1","role":"owner","joined_at":"1000","left_at":null,"closed_by":null}},{"participant":{"id":"p-2","kind":"human","display_name":"Inori","status":"active"},"membership":{"membership_id":"m-2","participant_id":"p-2","role":"member","joined_at":"2000","left_at":null,"closed_by":null}}],"request_id":"r"}"#.to_string(),
        ),
        (
            404,
            r#"{"error":{"code":"room_not_found"}}"#.to_string(),
        ),
    ]);
    let mut app = fake.app();
    let operation = OperationId::new(RunId::new("run-members"), Turn::FIRST);

    let listed = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_ROOM_MEMBERS),
            ActionParams::none().with(PARAM_ROOM, "room-1"),
        ),
    );
    assert_eq!(listed.state(), ResultState::Success, "{listed:?}");
    assert_eq!(
        text_of(&listed),
        "members = 2\np-1 · 小欣 · owner · 1000\np-2 · Inori · member · 2000\n",
        "owner 在前、逐行给身份 · 名字 · 角色 · 加入时刻"
    );
    let request = fake.next_request();
    assert!(
        request.starts_with("GET /v1/backend/rooms/room-1/members "),
        "路径必须是 Agent 读面那条成员端点: {request}"
    );

    // 不是成员 / 房不存在 / 房标识读不懂 ⇒ 一律 `unknown-room`（契约合成同一个 404，本层不分解）。
    let refused = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_ROOM_MEMBERS),
            ActionParams::none().with(PARAM_ROOM, "room-x"),
        ),
    );
    assert_eq!(refused.failure_class(), Some("unknown-room"));

    // 参数不合法 ⇒ 本地拒绝，**一次请求都不发**。
    let before = fake.connections();
    let bad = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_ROOM_MEMBERS),
            ActionParams::none().with(PARAM_ROOM, ""),
        ),
    );
    assert_eq!(bad.failure_class(), Some("invalid-argument"));
    assert_eq!(fake.connections(), before, "参数不合法时一次请求都不发");
}

/// **群 owner 的治理两条**（2026-10-13）：踢人与转让房主——服务端四条治理端点里的另外两条
/// （2026-09-27 裁决原文「**移除权含 Human 成员**」），缺的一直是这一侧的动作。
///
/// ⚠ 两条的复核都是**成员清单**（§3.87 刚给的读面）：他还在不在、现在谁写着 owner。
#[test]
fn the_owner_can_remove_a_member_and_transfer_the_room() {
    let fake = FakeIm::spawn(vec![
        (204, String::new()),
        (
            200,
            r#"{"membership":{"membership_id":"m-9","participant_id":"p-9","role":"owner","joined_at":"3000","left_at":null,"closed_by":null},"idempotency_replayed":false,"request_id":"r"}"#.to_string(),
        ),
        (
            409,
            r#"{"error":{"code":"owner_not_removable"}}"#.to_string(),
        ),
    ]);
    let mut app = fake.app();
    let operation = OperationId::new(RunId::new("run-govern"), Turn::FIRST);
    let call = |verb: &str, room: &str, participant: &str| {
        Action::with_params(
            ActionId::new(verb),
            ActionParams::none()
                .with(PARAM_ROOM, room)
                .with(PARAM_PARTICIPANT, participant),
        )
    };

    // ① 移除一个成员：**不带幂等键**（幂等由"他还在不在"承担）。
    let removed = app.execute(
        &agent(),
        &operation,
        &call(CAPABILITY_REMOVE_MEMBER, "room-1", "p-9"),
    );
    assert_eq!(removed.state(), ResultState::Success, "{removed:?}");
    assert_eq!(
        text_of(&removed),
        "room = room-1\nremoved = p-9\n（那间房与它的历史都还在；他会知道）"
    );
    let request = fake.next_request();
    assert!(
        request.starts_with("DELETE /v1/backend/rooms/room-1/members/p-9 "),
        "路径必须是 Agent 侧移除那条端点: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("idempotency-key:"),
        "移除**不带键**: {request}"
    );

    // ② 转让房主：**带键**，回执给出新 owner 与是否重放。
    let transferred = app.execute(
        &agent(),
        &operation,
        &call(CAPABILITY_TRANSFER_OWNER, "room-1", "p-9"),
    );
    assert_eq!(transferred.state(), ResultState::Success, "{transferred:?}");
    assert_eq!(
        text_of(&transferred),
        "room = room-1\nowner = p-9\nreplayed = no"
    );
    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/rooms/room-1/owner-transfer "),
        "路径必须是 Agent 侧转让那条端点: {request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("idempotency-key:"),
        "转让**带键**: {request}"
    );
    assert!(
        request.contains("\"participant_id\":\"p-9\""),
        "体是契约那个字段: {request}"
    );

    // ③ **owner 移不掉自己**（契约 409 `owner_not_removable`）——它是自己的一个词，
    //    不并进 `not-a-member`（"我不在里面"与"我不能把自己请出去"是两件事）。
    let myself = app.execute(
        &agent(),
        &operation,
        &call(CAPABILITY_REMOVE_MEMBER, "room-1", "p-me"),
    );
    assert_eq!(myself.failure_class(), Some("owner-not-removable"));

    // ④ 参数不合法 ⇒ 本地拒绝，**一次请求都不发**。
    let before = fake.connections();
    for (verb, room, participant) in [
        (CAPABILITY_REMOVE_MEMBER, "room-1", ""),
        (CAPABILITY_TRANSFER_OWNER, "", "p-9"),
    ] {
        let bad = app.execute(&agent(), &operation, &call(verb, room, participant));
        assert_eq!(bad.failure_class(), Some("invalid-argument"), "{verb}");
    }
    assert_eq!(fake.connections(), before, "参数不合法时一次请求都不发");
}

/// **删好友**（2026-10-13）：标准 IM 里"加好友"的另一半——能加就得能删。
///
/// 走服务端那五条关系端点里的第五条（`DELETE /v1/backend/contacts/{participant_id}`，批次三第二刀），
/// **不带幂等键**（幂等由"有没有 active 段"承担：没有也是成功 204）。
#[test]
fn she_can_end_a_relationship_she_started() {
    let fake = FakeIm::spawn(vec![
        (204, String::new()),
        (204, String::new()),
        (
            404,
            r#"{"error":{"code":"participant_not_found"}}"#.to_string(),
        ),
    ]);
    let mut social = fake.social_app();
    let operation = OperationId::new(RunId::new("run-end"), Turn::FIRST);
    let end = |participant: &str| {
        Action::with_params(
            ActionId::new(CAPABILITY_END_CONTACT),
            ActionParams::none().with(PARAM_PARTICIPANT, participant),
        )
    };

    let ended = social.execute(&agent(), &operation, &end("p-9"));
    assert_eq!(ended.state(), ResultState::Success, "{ended:?}");
    assert_eq!(
        text_of(&ended),
        "peer = p-9\nended = yes（单方面生效、不通知对方；旧房永久封存。拉黑是另一条）"
    );
    let request = fake.next_request();
    assert!(
        request.starts_with("DELETE /v1/backend/contacts/p-9 "),
        "路径必须是 Agent 侧解除那条端点: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("idempotency-key:"),
        "这条端点**不带键**（幂等由成员段承担）: {request}"
    );

    // **没有 active 段也是成功**（幂等 no-op）——同参第二次照样 204。
    let again = social.execute(&agent(), &operation, &end("p-9"));
    assert_eq!(again.state(), ResultState::Success, "{again:?}");

    // 目标不可用 ⇒ `unknown-participant`（与拉黑同一条口径）。
    let ghost = social.execute(&agent(), &operation, &end("p-404"));
    assert_eq!(ghost.failure_class(), Some("unknown-participant"));

    // 参数不合法 ⇒ 本地拒绝，**一次请求都不发**。
    let before = fake.connections();
    let bad = social.execute(&agent(), &operation, &end(""));
    assert_eq!(bad.failure_class(), Some("invalid-argument"));
    assert_eq!(fake.connections(), before, "参数不合法时一次请求都不发");
}

/// 服务器回答的"这个凭据代表谁"（`GET /v1/backend/me` 的 `agent.id`）。
const IM_PARTICIPANT: &str = "981a6d7d-9636-4606-9765-30c304ee5982";

/// 认证探针的回执体（真实形状：agent / binding / credential 三段公共元数据）。
fn me_body(id: &str) -> String {
    serde_json::json!({
        "agent": {"id": id, "kind": "agent", "display_name": "小满", "status": "active"},
        "binding": {"id": "b-1", "label": "fixture", "status": "active"},
        "credential": {"id": "c-1", "status": "active", "created_at": "2026-09-16T11:00:00+00:00"},
        "request_id": "req-me"
    })
    .to_string()
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("direct"), Turn::FIRST)
}

/// 直接驱动一次只读动作（不经 Core 循环：这一层只证明 App 的搬运、词表与参数网关）。
fn call<A: ActionPort>(app: &mut A, action: Action) -> ActionOutcome {
    app.execute(&agent(), &operation(), &action)
}

/// 一个 App 声明的能力 id（顺序 = 目录顺序）。
fn declared_ids(app: &impl AppPort) -> Vec<String> {
    app.declared_capabilities()
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect()
}

/// 一个 App 声明的写效应 id（顺序 = 声明顺序）。
fn declared_effect_ids(app: &impl AppPort) -> Vec<String> {
    app.declared_effects()
        .iter()
        .map(|effect| effect.action().as_str().to_string())
        .collect()
}

/// `&[&str]` → `Vec<String>`（让断言两侧同型）。
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// **每个包的能力清单**（顺序 = 目录顺序）。2026-10-02 拆包：动词本身一条不改，只是换了拥有者。
/// ⚠️ **2026-10-06**：通信面末尾多了一条 `im_typing`——它**不是拆包带来的**，是那一刀把
/// "早就实现、早就写进文档、却没进能力面"的进行态动词接上（见 [`ADDED_AFTER_THE_SPLIT`]）。
const IM_PACK: [&str; 17] = [
    CAPABILITY_ROOMS,
    CAPABILITY_READ_HISTORY,
    CAPABILITY_READ_ATTACHMENT,
    CAPABILITY_SAY,
    CAPABILITY_VERIFY_SAID,
    CAPABILITY_RECALL,
    CAPABILITY_VERIFY_RECALLED,
    CAPABILITY_MARK_READ,
    CAPABILITY_LEAVE_ROOM,
    // **2026-10-13 加**：房治理两条（开群房 / 邀请）——服务端 2026-09-27 就有那批端点，
    // 执行端一直没给动词（见 `agent/README.md` §3.86）。
    CAPABILITY_CREATE_GROUP,
    CAPABILITY_INVITE,
    // 2026-10-13：治理两条（移除成员 / 转让房主——2026-09-27 裁决「移除权含 Human 成员」）。
    CAPABILITY_REMOVE_MEMBER,
    CAPABILITY_TRANSFER_OWNER,
    // 2026-10-13：成员读面（服务端早在 Agent 读面里，缺的是这一侧的动作）。
    CAPABILITY_ROOM_MEMBERS,
    CAPABILITY_WRITE_PRESENTATION,
    CAPABILITY_ROOM_LIVE,
    CAPABILITY_TYPING,
];
const SOCIAL_PACK: [&str; 7] = [
    CAPABILITY_CONTACTS,
    CAPABILITY_BLOCKS,
    CAPABILITY_CONTACTS_REQUEST,
    CAPABILITY_ANSWER_CONTACT,
    CAPABILITY_BLOCK,
    // 2026-10-13：解除关系（删好友）——服务端那五条关系端点里的第五条。
    CAPABILITY_END_CONTACT,
    CAPABILITY_UNBLOCK,
];
const PROFILE_PACK: [&str; 3] = [
    CAPABILITY_MY_PROFILE,
    CAPABILITY_WRITE_PROFILE,
    CAPABILITY_WRITE_AVATAR,
];

/// **拆包前那份目录**（20 条，逐条同序）：三个包的并集必须等于它——拆包只换拥有者，
/// 不加一条、不减一条、不改一条。
const DIRECTORY_BEFORE_THE_SPLIT: [&str; 20] = [
    CAPABILITY_ROOMS,
    CAPABILITY_READ_HISTORY,
    CAPABILITY_READ_ATTACHMENT,
    CAPABILITY_SAY,
    CAPABILITY_VERIFY_SAID,
    CAPABILITY_RECALL,
    CAPABILITY_VERIFY_RECALLED,
    CAPABILITY_MARK_READ,
    CAPABILITY_LEAVE_ROOM,
    CAPABILITY_CONTACTS,
    CAPABILITY_BLOCKS,
    CAPABILITY_CONTACTS_REQUEST,
    CAPABILITY_ANSWER_CONTACT,
    CAPABILITY_BLOCK,
    CAPABILITY_UNBLOCK,
    CAPABILITY_MY_PROFILE,
    CAPABILITY_WRITE_PROFILE,
    CAPABILITY_WRITE_AVATAR,
    CAPABILITY_WRITE_PRESENTATION,
    CAPABILITY_ROOM_LIVE,
];

/// **拆包之后加进目录的那几条**（逐条点名，不在 [`DIRECTORY_BEFORE_THE_SPLIT`] 里）。
///
/// 为什么单列一份而不是把那 20 条改掉：`DIRECTORY_BEFORE_THE_SPLIT` 是一条**历史事实**
/// （"拆包前那份目录长什么样"），它的用处正是"拆包只换拥有者、一条不多一条不少"这条回归。
/// 本刀（2026-10-06）**合法地**加了一条 `im_typing`，因此判据拆成两半：
/// **前 20 条逐条同序仍在** ＋ **多出来的恰好是这一份点名的**。
/// ⇒ 将来谁再加一条，必须在这里写出来（而不是让"并集等于 20 条"那条断言静默失效）。
// ⚠ **2026-10-13 起这一格不只一条**：`im_typing`（那一刀把"早就实现却没进能力面"的进行态接上）
// 与房治理那两条（开群房 / 邀请）。名字保留原样——它说的是"拆包之后点名的那些"。
const ADDED_AFTER_THE_SPLIT: [&str; 7] = [
    CAPABILITY_TYPING,
    CAPABILITY_CREATE_GROUP,
    CAPABILITY_INVITE,
    CAPABILITY_ROOM_MEMBERS,
    CAPABILITY_END_CONTACT,
    CAPABILITY_REMOVE_MEMBER,
    CAPABILITY_TRANSFER_OWNER,
];

/// **拆包前那 11 条写效应声明**（逐条同序）。
const EFFECTS_BEFORE_THE_SPLIT: [&str; 11] = [
    CAPABILITY_SAY,
    CAPABILITY_RECALL,
    CAPABILITY_MARK_READ,
    CAPABILITY_LEAVE_ROOM,
    CAPABILITY_CONTACTS_REQUEST,
    CAPABILITY_ANSWER_CONTACT,
    CAPABILITY_BLOCK,
    CAPABILITY_UNBLOCK,
    CAPABILITY_WRITE_PROFILE,
    CAPABILITY_WRITE_AVATAR,
    CAPABILITY_WRITE_PRESENTATION,
];

/// 逐条核对一个 App 的写效应声明：动作 / 类别 / 复核（`None` = 这一档按定义不配复核）。
///
/// **装配期纪律**：写类别必须配复核（`check_declarations` 会把"说了写却没配复核"拒掉）
/// ——**唯一的豁免是 `unverifiable-write`**（"确实是写，但没有同参只读复核可配"，
/// 见 `terminal_run` 与 `im_typing`）。因此这里的 `None` 只对那一档合法。
fn assert_effects(app: &impl AppPort, expected: &[(&str, &str, Option<&str>)]) {
    let effects = app.declared_effects();
    assert_eq!(effects.len(), expected.len(), "写效应条数");
    for (index, (action, class, verification)) in expected.iter().enumerate() {
        assert_eq!(effects[index].action().as_str(), *action);
        assert_eq!(effects[index].class().as_str(), *class);
        assert_eq!(
            effects[index].verification().map(|id| id.as_str()),
            *verification,
            "写类别必须配复核（`unverifiable-write` 按定义豁免）"
        );
        if verification.is_none() {
            assert_eq!(*class, "unverifiable-write", "不配复核的只有那一档");
        }
    }
}

fn rooms(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_ROOMS), params)
}

fn history(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_READ_HISTORY), params)
}

/// 一次**成功的读位置推进**回执（`im_read_history` 读完之后顺带发的那一次 PUT 的答案）。
///
/// 它同时是那条纪律的读数：**一次读历史 = 两次请求**（读那一页 ＋ 顺带推进），
/// 所以凡喂历史回执的夹具都要在后面跟一条它。
fn cursor_advanced_body() -> String {
    serde_json::json!({
        "advanced": true,
        "read_state": {"last_read_message_id": null, "unread_count": 0},
        "request_id": "req-cursor"
    })
    .to_string()
}

fn recall(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_RECALL), params)
}

fn verify_recalled(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_VERIFY_RECALLED), params)
}

fn read_attachment(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_READ_ATTACHMENT), params)
}

fn say(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_SAY), params)
}

fn verify_said(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_VERIFY_SAID), params)
}

/// 一个**形状合法**的附件身份（严格规范小写连字符 UUID v4）。
const ATTACHMENT: &str = "7b2f5a91-2c4d-4f6e-8a10-5d9c3e77bb21";

/// 一个临时的**世界观根**（每个用例自己一份，`tag` 区分）。
///
/// 存在与否、里面有什么，都是用例自己布置的——主体那只手只看这个根里的东西。
fn world(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("celira-im-world-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("可以建世界观根");
    base
}

/// 往世界观里放一份文件（可以带子目录）。
fn put(world: &Path, relative: &str, bytes: &[u8]) {
    let path = world.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("可以建子目录");
    }
    fs::write(&path, bytes).expect("可以写文件");
}

/// 一个**形状合法**的消息身份（严格规范小写连字符 UUID v4）。
const MESSAGE: &str = "632474f3-95c0-4adb-9649-30209ac5012d";

/// 撤回成功的回执体（真实形状：`message_id` / `room_id` / `recalled_at` / `idempotency_replayed`）。
fn recall_body(id: &str, replayed: bool) -> String {
    serde_json::json!({
        "message_id": id,
        "room_id": "r-1",
        "recalled_at": "2026-09-16T11:01:00+00:00",
        "idempotency_replayed": replayed,
        "request_id": "req-recall"
    })
    .to_string()
}

fn text_of(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 一份房间目录回执：一个房有**超长预览 + 计数 150**，一个房是撤回（预览 null）+ 换行显示名。
/// 一次说话成功的回执体。
fn say_body(id: &str, replayed: bool) -> String {
    serde_json::json!({
        "message": {"id": id, "room_id": "r-1", "body": "你好", "created_at": "2026-09-16T11:00:00+00:00"},
        "idempotency_replayed": replayed,
        "request_id": "req-say"
    })
    .to_string()
}

fn rooms_body() -> String {
    let long = "很长的预览".repeat(20);
    serde_json::json!({
        "rooms": [
            {
                "room_id": "6f0f4f3a-1111-4111-8111-111111111111",
                "kind": "human_direct",
                "display_name": "主人",
                "last_activity_at": "2026-09-16T05:20:00+00:00",
                "preview": long,
                "since_count": 150,
                "read_state": {
                    "last_read_message_id": "6f0f4f3a-9999-4999-8999-999999999999",
                    "unread_count": 3
                }
            },
            {
                "room_id": "6f0f4f3a-2222-4222-8222-222222222222",
                "kind": "group",
                "display_name": "带\n换行的群名",
                "last_activity_at": null,
                "preview": null,
                "since_count": 0,
                "read_state": {"last_read_message_id": null, "unread_count": 0}
            }
        ],
        "request_id": "req-1"
    })
    .to_string()
}

/// 一份历史回执。
fn history_body(rows: &[(&str, &str, bool)], next: Option<&str>) -> String {
    let messages: Vec<serde_json::Value> = rows
        .iter()
        .map(|(id, created, recalled)| {
            serde_json::json!({
                "id": id,
                "room_id": "6f0f4f3a-1111-4111-8111-111111111111",
                "actor": {"id": "a-1", "kind": "human", "display_name": "主人", "status": "active"},
                "body": if *recalled { serde_json::Value::Null } else { serde_json::json!("你好") },
                "mentions": [],
                "attachments": [],
                "reply_to_message_id": null,
                "created_at": created,
                "recalled_at": if *recalled { serde_json::json!("2026-09-16T05:24:00+00:00") } else { serde_json::Value::Null }
            })
        })
        .collect();
    serde_json::json!({
        "messages": messages,
        "next_before": next,
        "request_id": "req-2"
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// 1. 通讯录：字段搬运、预览截断、行转义
// ---------------------------------------------------------------------------

#[test]
fn the_directory_renders_what_the_endpoint_gave() {
    // TASK_71 桌面 P0 第 4 件：读面回执会在读完正文之后补一次身份探针 ⇒ 脚本排在正文之后。
    let fake = FakeIm::spawn(vec![(200, rooms_body()), (200, me_body(IM_PARTICIPANT))]);
    let mut app = fake.app();

    let outcome = call(&mut app, rooms(ActionParams::none()));
    assert_eq!(outcome.state(), ResultState::Success);

    let request = fake.next_request();
    assert!(
        request.starts_with("GET /v1/backend/rooms "),
        "路径必须是通讯录端点: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {SENTINEL}").to_ascii_lowercase()),
        "凭据只走请求头（头名大小写不敏感）: {request}"
    );
    let probe = fake.next_request();
    assert!(
        probe.starts_with("GET /v1/backend/me "),
        "读完正文之后那一次是**身份探针**（回执开头那行 `me = …`）: {probe}"
    );
    assert_eq!(
        fake.connections(),
        2,
        "一次调用 = 读正文 + 身份探针各一次（探针**一次活动只探一次**，多读一次不会多探）"
    );

    let text = text_of(&outcome);
    // 回执**首行**是身份：Agent 在 im 里读到的每个 `actor` 都是服务端身份，没有这一行主体分不出自己。
    assert!(
        text.starts_with(&format!("me = {IM_PARTICIPANT}\n")),
        "首行要给身份（服务器说的那个，不是本机 AgentId）: {text}"
    );
    assert!(text.contains("rooms = 2, since = none\n"), "{text}");
    // 房间行**按形状取**（不按行号：回执头多了一行身份之后，行号就不再是判据）。
    let first = text
        .lines()
        .find(|line| line.contains("\tpreview = "))
        .expect("第一行房间");
    let preview = first.split("\tpreview = ").nth(1).expect("有预览字段");
    assert!(
        preview.ends_with('…') && preview.trim_end_matches('…').len() <= 60,
        "预览必须按字符边界截到 60 字节以内: {first}"
    );
    assert!(
        first.contains("\tsince = 150\t"),
        "计数照原值渲染、不折叠成 99+: {first}"
    );
    // 世界记得的"我读到哪儿、还欠几条"（切片 M1）：照原值渲染，与 `since` 分列两栏。
    assert!(
        first.contains("\tunread = 3\tread = 6f0f4f3a-9999-4999-8999-999999999999\t"),
        "未读与位置照原值渲染: {first}"
    );
    assert!(
        first.contains("\tlast = 2026-09-16T05:20:00+00:00"),
        "{first}"
    );

    let room_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("\tpreview = "))
        .collect();
    let second = room_lines.get(1).copied().expect("第二行房间");
    assert!(
        second.ends_with("\tpreview = -"),
        "撤回/空预览如实写 -: {second}"
    );
    assert!(
        second.contains("\tunread = 0\tread = -\t"),
        "未读为零与「还没读过」是两件事，分别写 0 与 -: {second}"
    );
    assert!(
        second.contains("带\\n换行的群名"),
        "换行必须被转义: {second}"
    );
    assert_eq!(room_lines.len(), 2, "两行房间，没有多余的房间行: {text}");
}

/// **缺省不当零**（切片 M1）：世界没给 `read_state`、或形状读不懂 ⇒ 写 `?`（读不到），
/// 绝不写 0——那会把"没读到"说成"没有未读"。位置**为空**是另一件事（还没读过），写 `-`。
/// 三态 `0` / `-` / `?` 在同一份回执里各不相同，逐格判。
#[test]
fn the_directory_keeps_unknown_read_state_apart_from_zero() {
    let body_with = |read_state: Option<serde_json::Value>| {
        let mut room = serde_json::json!({
            "room_id": "6f0f4f3a-1111-4111-8111-111111111111",
            "kind": "human_direct",
            "display_name": "主人",
            "last_activity_at": null,
            "preview": null,
            "since_count": 0
        });
        if let Some(state) = read_state {
            room["read_state"] = state;
        }
        serde_json::json!({"rooms": [room], "request_id": "req"}).to_string()
    };
    let render = |body: String| {
        let fake = FakeIm::spawn(vec![(200, body), (200, me_body(IM_PARTICIPANT))]);
        let mut app = fake.app();
        let outcome = call(&mut app, rooms(ActionParams::none()));
        assert_eq!(outcome.state(), ResultState::Success);
        text_of(&outcome)
    };

    let cases: Vec<(Option<serde_json::Value>, &str)> = vec![
        // 零未读 + 还没读过：两件事各写各的。
        (
            Some(serde_json::json!({"last_read_message_id": null, "unread_count": 0})),
            "\tunread = 0\tread = -\t",
        ),
        // 正常值照原值渲染。
        (
            Some(serde_json::json!({
                "last_read_message_id": "6f0f4f3a-9999-4999-8999-999999999999",
                "unread_count": 2
            })),
            "\tunread = 2\tread = 6f0f4f3a-9999-4999-8999-999999999999\t",
        ),
        // 这一版世界没给这一栏 ⇒ 两栏都 `?`（读不到），不是 0。
        (None, "\tunread = ?\tread = ?\t"),
        // 形状读不懂：负数 / 非整数 / 位置类型不对 / 整个 read_state 不是对象。
        (
            Some(serde_json::json!({"last_read_message_id": null, "unread_count": -1})),
            "\tunread = ?\tread = -\t",
        ),
        (
            Some(serde_json::json!({"last_read_message_id": null, "unread_count": "三条"})),
            "\tunread = ?\tread = -\t",
        ),
        (
            Some(serde_json::json!({"last_read_message_id": 7, "unread_count": 2})),
            "\tunread = 2\tread = ?\t",
        ),
        (
            Some(serde_json::json!("读不到")),
            "\tunread = ?\tread = ?\t",
        ),
    ];

    for (state, expected) in cases {
        let text = render(body_with(state.clone()));
        assert!(
            text.contains(expected),
            "read_state = {state:?} 应渲染出 {expected:?}：{text}"
        );
    }
}

#[test]
fn the_directory_passes_since_as_milliseconds() {
    let fake = FakeIm::spawn(vec![(200, rooms_body()), (200, me_body(IM_PARTICIPANT))]);
    let mut app = fake.app();

    let outcome = call(
        &mut app,
        rooms(ActionParams::none().with(PARAM_SINCE, "1758000000000")),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    assert!(
        fake.next_request()
            .starts_with("GET /v1/backend/rooms?since=1758000000000 "),
        "since 原样进查询串"
    );
    let text = text_of(&outcome);
    assert!(
        text.starts_with(&format!("me = {IM_PARTICIPANT}\n"))
            && text.contains("rooms = 2, since = 1758000000000\n"),
        "回执回显这次请求的基线: {}",
        text_of(&outcome)
    );
}

#[test]
fn an_invalid_since_is_refused_without_a_request() {
    for bad in ["abc", "-1", "1.5", "99999999999999999999"] {
        let fake = FakeIm::spawn(vec![]);
        let mut app = fake.app();
        let outcome = call(&mut app, rooms(ActionParams::none().with(PARAM_SINCE, bad)));
        assert_eq!(
            outcome.state(),
            ResultState::Failure,
            "since = {bad} 必须被拒绝"
        );
        assert_eq!(
            outcome.failure_class(),
            Some("invalid-argument"),
            "since = {bad}"
        );
        assert_eq!(fake.connections(), 0, "非法参数一次都不许出网");
    }
}

/// **超长的 room 在本地就被拒**——读历史那条路 2026-10-13 之前**漏了这道界**。
///
/// 那一族（房间 / 参与者 / 消息目标）的长度上限是 [`MAX_ID_BYTES`]，14 处走 `valid_room`，
/// 而 `im_read_history` 从前自己内联判了个"非空"就放行 ⇒ 两样都坏：
/// ① 契约（`CORE_V1_AGENT_HISTORY.md`）写着"机械校验先于任何 Core 往返"，而它没有；
/// ② 一个超长 room 会被拼进 URL 去打扰世界，再拿回一个 404。
///
/// 判据两条：**分类是 `invalid-argument`**（与同族一致）＋ **一次都不出网**。
#[test]
fn an_overlong_room_is_refused_without_a_request_on_the_history_path() {
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        history(ActionParams::none().with("room", "x".repeat(MAX_ID_BYTES + 1))),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));
    assert_eq!(fake.connections(), 0, "超限的标识一次都不许出网");
}

/// **正对照**：界上那一个值照旧放行——否则上面那条可以靠"一律拒绝"假绿。
///
/// 判据不是"它成功了"（夹具没喂回执，世界那一侧当然不成），而是**它真的走出去了**：
/// 长度检查不能顺手把合法的值也拦掉。
#[test]
fn a_room_at_the_length_limit_still_reaches_the_world() {
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app();
    let _ = call(
        &mut app,
        history(ActionParams::none().with("room", "x".repeat(MAX_ID_BYTES))),
    );
    assert!(
        fake.connections() > 0,
        "刚好在界上的 room 必须照旧被送出去（界是 ≤，不是 <）"
    );
}

/// **锚行原文直接可用**（2026-09-17 主人裁决的判据）：状态行第四行/通知栏首行的那个值，
/// 原样喂给 `im_rooms(since=…)` 必须被接受，并且**原样**进查询串。
///
/// 两侧**不同源**，所以这条断言承重：左手是**处境那一栏的格式化点**
/// （[`celira_agent::clock::anchor_value`]——状态行第四行与通知栏首行都走它，只有这一处），
/// 右手是**这个 App 的参数网关与查询串**。哪天那一栏换回可读形状、或在值上带任何装饰，
/// 这里就会红——而"主体把那一行原样递回去"这条路会静静断掉（实测断过：`invalid-argument`，代价一轮）。
#[test]
fn the_anchor_line_value_is_accepted_verbatim_as_since() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_millis(1_789_625_378_407);
    let anchor = celira_agent::clock::anchor_value(at);
    // 那一栏的原文就是 `since = <这个值>`：值本身必须是**裸的十进制毫秒**（无单位、无括号）。
    assert_eq!(anchor, "1789625378407", "锚行的值就是十进制 unix 毫秒");

    let fake = FakeIm::spawn(vec![(200, rooms_body())]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        rooms(ActionParams::none().with(PARAM_SINCE, anchor.as_str())),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "锚行原样作 since 必须被接受: {}",
        text_of(&outcome)
    );
    assert!(
        fake.next_request()
            .starts_with(&format!("GET /v1/backend/rooms?since={anchor} ")),
        "锚原样进查询串"
    );
}

// ---------------------------------------------------------------------------
// 1b. 「我是谁」（TASK_71 桌面 P0 第 4 件）：读面回执开头那行身份
// ---------------------------------------------------------------------------

/// 读面回执**开头**是 Agent 在 im 里的身份，而且**一次活动只探一次**。
///
/// 为什么这一行非有不可：主体读到的每个 `actor` 都是服务端身份（UUID），与本机 `AgentId`
/// 是两套命名空间、从不相等 ⇒ 没有它，主体只能靠"我在别处发过言"反推自己是谁（TASK_67 的缺口）。
/// 判据落在**请求序列**上（读、探针、读）：第二次读**不再出网**。
#[test]
fn the_read_receipt_opens_with_her_im_identity_and_probes_it_once() {
    let fake = FakeIm::spawn(vec![
        (200, rooms_body()),
        (200, me_body(IM_PARTICIPANT)),
        (200, rooms_body()),
    ]);
    let mut app = fake.app();

    let first = call(&mut app, rooms(ActionParams::none()));
    let second = call(&mut app, rooms(ActionParams::none()));

    assert_eq!(first.state(), ResultState::Success);
    assert_eq!(second.state(), ResultState::Success);
    assert!(
        text_of(&first).starts_with(&format!("me = {IM_PARTICIPANT}\n")),
        "首行是**服务器说的**身份（不是本机 AgentId）: {}",
        text_of(&first)
    );
    assert!(
        text_of(&second).starts_with(&format!("me = {IM_PARTICIPANT}\n")),
        "第二次读照抄第一次探到的身份: {}",
        text_of(&second)
    );

    assert!(
        fake.next_request().starts_with("GET /v1/backend/rooms "),
        "① 读正文"
    );
    assert!(
        fake.next_request().starts_with("GET /v1/backend/me "),
        "② 身份探针（读完正文之后才补那一行）"
    );
    assert!(
        fake.next_request().starts_with("GET /v1/backend/rooms "),
        "③ 第二次读 —— 后面**没有**第二次探针"
    );
    assert_eq!(
        fake.connections(),
        3,
        "两次读 + **一次**探针：探针一次活动只探一次（缓存挂在 ImApp 上，活动结束就散）"
    );
}

/// 探不到身份 ⇒ 如实说 `unknown`（**不**拿本机身份或显示名顶替），而且**不**拖垮这次读。
#[test]
fn a_receipt_without_an_identity_says_unknown_instead_of_borrowing_one() {
    // 探针 500：读正文照样成功，只是那一行没有答案。
    let fake = FakeIm::spawn(vec![(200, rooms_body()), (500, "{}".to_string())]);
    let mut app = fake.app();

    let outcome = call(&mut app, rooms(ActionParams::none()));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "身份探不到不等于这次读失败: {outcome:?}"
    );
    let text = text_of(&outcome);
    assert!(text.starts_with("me = unknown\n"), "{text}");
    assert!(
        text.contains("本机身份与显示名都不能顶替"),
        "缺省要能判读：说清那行是什么、不能拿什么顶替: {text}"
    );
    assert!(text.contains("rooms = 2, since = none"), "正文照给: {text}");
    assert!(
        !text.contains(&agent().as_str().to_string()),
        "本机 AgentId（`{}`）不得被拿来顶替 im 身份: {text}",
        agent().as_str()
    );
}

// ---------------------------------------------------------------------------
// 2. 翻记录：分页往返、游标原样、更多读数
// ---------------------------------------------------------------------------

#[test]
fn history_pages_do_not_overlap_and_the_cursor_is_passed_back() {
    let first_page = history_body(
        &[
            ("m-2", "2026-09-16T05:22:00+00:00", false),
            ("m-3", "2026-09-16T05:23:00+00:00", true),
        ],
        Some("m:9f3c4d5e"),
    );
    let second_page = history_body(&[("m-1", "2026-09-16T05:21:00+00:00", false)], None);
    // ⚠ 2026-10-10：**一次 `im_read_history` = 两次请求**——读那一页（GET）＋ 顺带推进读位置
    // （PUT `read-cursor`，见 `im_app::read_history`）。因此每条历史回执后面都跟一条推进回执。
    // 再加一次身份探针（第二次读是**缓存命中**，不再出网）。
    let fake = FakeIm::spawn(vec![
        (200, first_page),
        (200, cursor_advanced_body()),
        (200, me_body(IM_PARTICIPANT)),
        (200, second_page),
        (200, cursor_advanced_body()),
    ]);
    let mut app = fake.app();

    let params = ActionParams::none()
        .with(PARAM_ROOM, "6f0f4f3a-1111-4111-8111-111111111111")
        .with(PARAM_LIMIT, "2");
    let first = call(&mut app, history(params.clone()));
    let first_text = text_of(&first);
    assert!(
        first_text.contains("more = next m:9f3c4d5e"),
        "滑到底之前要搬运游标: {first_text}"
    );
    assert!(first_text.contains("m-3"), "{first_text}");
    assert!(
        first_text.contains("\trecalled"),
        "撤回如实标出: {first_text}"
    );
    assert!(
        fake.next_read_request().starts_with(
            "GET /v1/backend/rooms/6f0f4f3a-1111-4111-8111-111111111111/messages?limit=2 "
        ),
        "路径与查询串"
    );

    let second = call(&mut app, history(params.with(PARAM_BEFORE, "m:9f3c4d5e")));
    let second_text = text_of(&second);
    assert!(
        second_text.contains("more = none"),
        "到底了要明说: {second_text}"
    );
    assert!(
        !second_text.contains("m-3"),
        "第二页不得重复第一页: {second_text}"
    );
    assert!(
        fake.next_read_request().contains("before=m:9f3c4d5e"),
        "游标原样透传（不解析、不改写）"
    );
    assert_eq!(
        fake.connections(),
        5,
        "两次读各两次请求（读那一页 ＋ 顺带推进读位置）+ 一次身份探针（**探针只探一次**，第二次读不再出网）"
    );
}

/// **读完历史顺带推进读位置**（2026-10-10 裁决：进房读 = 打开会话）。
///
/// 判据三条，逐条落在那一次 PUT 上：
///
/// 1. **推进到这一页最新的那一条**（行是升序的 ⇒ 最后一行）——不是第一条、也不是随便一条；
/// 2. **`before=` 回翻不把位置往回推**：那一页最新的一条比当前位置旧 ⇒ 世界答 `advanced=false`，
///    回执照旧如实说（读位置是**单调**的）；
/// 3. **空页没有可推进的位置** ⇒ 说出口（`unchanged（这一页没有可推进的消息）`），
///    而且**一次请求都不发**——别让"没这一行"与"推上去了"混。
///
/// ⚠ **读到 ≠ 记住**：推进失败**不改变这次读的结果**（消息已经在视野里了），只多一句实话。
/// 这一条只验"成功那一半"，失败那几态由下面 `read-cursor = unknown` 那条断言覆盖。
#[test]
fn reading_history_also_advances_the_read_cursor() {
    let room = "6f0f4f3a-1111-4111-8111-111111111111";
    let page = history_body(
        &[
            ("m-1", "2026-09-16T05:21:00+00:00", false),
            ("m-2", "2026-09-16T05:22:00+00:00", false),
            ("m-3", "2026-09-16T05:23:00+00:00", false),
        ],
        None,
    );
    let fake = FakeIm::spawn(vec![
        (200, page.clone()),
        (200, cursor_advanced_body()),
        (200, me_body(IM_PARTICIPANT)),
    ]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        history(ActionParams::none().with(PARAM_ROOM, room)),
    );
    let text = text_of(&outcome);

    // ① 回执里如实报"推上去了、推到哪一条"。
    assert!(
        text.contains("read-cursor = advanced to m-3"),
        "回执要说清推到哪一条（这一页**最新**的那一条）: {text}"
    );
    // ② 那一次请求带的是**最新那条**的 id，而且落在读游标那条端点上。
    let put = fake
        .next_request_of("PUT ")
        .expect("读完之后必须发一次推进请求");
    assert!(
        put.starts_with(&format!("PUT /v1/backend/rooms/{room}/read-cursor ")),
        "走的是读游标那条既有端点: {put}"
    );
    assert!(
        put.contains("\"message_id\":\"m-3\""),
        "推进的是这一页**最新**那条（不是第一条）: {put}"
    );

    // ③ 回翻：世界答 `advanced=false` ⇒ 如实说"早就读到那儿了"，不是"推上去了"。
    let fake = FakeIm::spawn(vec![
        (
            200,
            history_body(&[("m-1", "2026-09-16T05:21:00+00:00", false)], None),
        ),
        (
            200,
            serde_json::json!({
                "advanced": false,
                "read_state": {"last_read_message_id": "m-3", "unread_count": 0}
            })
            .to_string(),
        ),
        (200, me_body(IM_PARTICIPANT)),
    ]);
    let mut app = fake.app();
    let back = text_of(&call(
        &mut app,
        history(
            ActionParams::none()
                .with(PARAM_ROOM, room)
                .with(PARAM_BEFORE, "m:9f3c4d5e"),
        ),
    ));
    assert!(
        back.contains("read-cursor = unchanged（世界说你早就读到 m-1 了）"),
        "回翻**不许**把位置往回推，而且要说清是「早就读到了」: {back}"
    );

    // ④ 空页：没有可推进的位置 ⇒ 说出口，而且**一次请求都不发**。
    let fake = FakeIm::spawn(vec![
        (200, history_body(&[], None)),
        (200, me_body(IM_PARTICIPANT)),
    ]);
    let mut app = fake.app();
    let empty = text_of(&call(
        &mut app,
        history(ActionParams::none().with(PARAM_ROOM, room)),
    ));
    assert!(
        empty.contains("read-cursor = unchanged（这一页没有可推进的消息）"),
        "空页要如实说，别让「没这一行」与「推上去了」混: {empty}"
    );
    assert!(
        !fake.any_request_of("PUT "),
        "空页不该发推进请求: 请求序列里出现了 PUT"
    );
}

#[test]
fn the_history_limit_is_stricter_than_the_route() {
    let fake = FakeIm::spawn(vec![
        (200, history_body(&[], None)),
        (200, cursor_advanced_body()),
        (200, me_body(IM_PARTICIPANT)),
        (200, history_body(&[], None)),
        (200, cursor_advanced_body()),
    ]);
    let mut app = fake.app();
    let room = ActionParams::none().with(PARAM_ROOM, "r-1");

    let _ = call(&mut app, history(room.clone()));
    assert!(
        fake.next_read_request()
            .contains(&format!("limit={MAX_HISTORY_LIMIT} ")),
        "缺省 = 工具面上限"
    );
    let _ = call(&mut app, history(room.with(PARAM_LIMIT, "50")));
    assert!(fake.next_read_request().contains("limit=50 "));

    for bad in ["0", "51", "abc", "-1"] {
        let fake = FakeIm::spawn(vec![]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            history(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_LIMIT, bad),
            ),
        );
        assert_eq!(
            outcome.failure_class(),
            Some("invalid-argument"),
            "limit = {bad}"
        );
        assert_eq!(fake.connections(), 0, "limit = {bad} 不许出网");
    }

    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app();
    let outcome = call(&mut app, history(ActionParams::none()));
    assert_eq!(outcome.failure_class(), Some("invalid-argument"), "缺房间");
    assert_eq!(fake.connections(), 0);
}

// ---------------------------------------------------------------------------
// 3. 失败词表：全表映射、逐字节稳定、协议词不外泄
// ---------------------------------------------------------------------------

#[test]
fn the_failure_table_covers_every_status() {
    let table: [(u16, &str); 6] = [
        (400, "invalid-argument"),
        (401, "credential-rejected"),
        (403, "not-a-member"),
        (404, "unknown-room"),
        (500, "server-error"),
        (503, "server-error"),
    ];
    for (status, expected) in table {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = call(&mut app, rooms(ActionParams::none()));
        assert_eq!(outcome.state(), ResultState::Failure, "HTTP {status}");
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
        assert!(
            text_of(&outcome).is_empty(),
            "失败不带正文块（状态码与响应体不外泄）: {}",
            text_of(&outcome)
        );
    }

    // 读不懂的 2xx：非 JSON / 缺字段 / 字段类型不对，一律归"读不出"。
    for body in ["this is not json", "{}", "{\"rooms\":\"not-a-list\"}"] {
        let fake = FakeIm::spawn(vec![(200, body.to_string())]);
        let mut app = fake.app();
        let outcome = call(&mut app, rooms(ActionParams::none()));
        assert_eq!(
            outcome.failure_class(),
            Some("unreachable"),
            "读不懂的响应必须归到 unreachable: {body}"
        );
    }

    // 连不上：地址本身就发不出去（端口越界 ⇒ 请求构造失败），走的是 `send()` 的同一个错误分支——
    // 与"连接被拒 / 域名解析不了"同一条 `unreachable`。**不**用"绑一个端口再放掉"的写法：
    // 本机进程带着环境变量代理（`HTTP_PROXY`），那样请求会被代理接走并回一个非 2xx，
    // 于是我们看到的是 `server-error` —— 那是环境的形状，不是这一层的行为。
    let mut app =
        ImApp::new(ImAccess::new("http://127.0.0.1:99999/not-a-port", SENTINEL).expect("声明"))
            .expect("可以装配");
    let outcome = call(&mut app, rooms(ActionParams::none()));
    assert_eq!(outcome.failure_class(), Some("unreachable"));
}

#[test]
fn the_same_failure_twice_is_byte_identical() {
    let mut readings = Vec::new();
    for _ in 0..2 {
        let fake = FakeIm::spawn(vec![(500, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = call(&mut app, rooms(ActionParams::none()));
        readings.push((
            outcome.state(),
            outcome.failure_class().map(str::to_string),
            text_of(&outcome),
        ));
    }
    assert_eq!(readings[0], readings[1], "同一种故障两次发生必须逐字节相同");
}

// ---------------------------------------------------------------------------
// 4. 只读面本身：能力、准入、效果声明、凭据不外泄
// ---------------------------------------------------------------------------

#[test]
fn the_app_declares_its_verbs_with_their_verifications() {
    let fake = FakeIm::spawn(vec![]);
    let app = fake.app();
    let social = fake.social_app();
    let profile = fake.profile_app();

    // **2026-10-02 拆包**：动词一条不少，只是换了拥有者——每个包各自声明自己那一组，
    // 三个包的并集**逐条等于**拆包前那份目录（顺序也一样）。
    assert_eq!(declared_ids(&app), strings(&IM_PACK), "通信面 17 条");
    assert_eq!(declared_ids(&social), strings(&SOCIAL_PACK), "关系面 6 条");
    assert_eq!(
        declared_ids(&profile),
        strings(&PROFILE_PACK),
        "表达面 3 条"
    );
    assert_eq!(app.app_id(), IM_APP_ID);
    assert_eq!(social.app_id(), IM_SOCIAL_APP_ID);
    assert_eq!(profile.app_id(), IM_PROFILE_APP_ID);

    // 拥有者映射：拆包前那份目录里，谁拥有哪一段（可读，不需要去猜）。
    // ⚠ 这里按**拆包当时**的切点取段（`IM_PACK[14..16]` = 房呈现 ＋ 房间进行态——房治理四条与成员读面在它们前面，
    // 因此 `im_typing`（本刀加的那一条，在末尾）**不参与**这条历史重建。
    let mut in_directory_order: Vec<&str> = Vec::new();
    in_directory_order.extend_from_slice(&IM_PACK[..9]);
    // ⚠ 关系面那一条**拆包之后加的**（2026-10-13 解除关系）不参与这条历史重建——
    // 它在 `ADDED_AFTER_THE_SPLIT` 里单独点名。
    in_directory_order.extend(
        SOCIAL_PACK
            .iter()
            .copied()
            .filter(|id| *id != CAPABILITY_END_CONTACT),
    );
    in_directory_order.extend_from_slice(&PROFILE_PACK);
    in_directory_order.extend_from_slice(&IM_PACK[14..16]);
    assert_eq!(
        in_directory_order,
        DIRECTORY_BEFORE_THE_SPLIT.to_vec(),
        "拆包只换拥有者：逐条同序（拆包前那 20 条一条不多一条不少）"
    );
    // 当前并集 = 拆包前那 20 条 ＋ 拆包之后点名的（本刀：`im_typing`）。
    let mut union = declared_ids(&app);
    union.extend(declared_ids(&social));
    union.extend(declared_ids(&profile));
    let mut expected = strings(&DIRECTORY_BEFORE_THE_SPLIT);
    expected.extend(strings(&ADDED_AFTER_THE_SPLIT));
    union.sort();
    expected.sort();
    assert_eq!(
        union, expected,
        "三个包的并集 = 拆包前那 20 条 ＋ 之后点名的"
    );

    // 效果声明：拆包前那 11 条写动词**各配了复核**（装配期纪律）——按新拥有者分开列，并集不变。
    // ⚠ `im_mark_read`（幂等写）与 `im_leave_room`（第二次必然失败）归在**不同**两档，
    // 因为档位的判据就是"同参数第二次会怎样"；两者的复核都是**通讯录**——它们问的是
    // "世界的状态"，不是"同参数的只读动作"。
    assert_effects(
        &app,
        &[
            (
                CAPABILITY_SAY,
                "repeatable-write",
                Some(CAPABILITY_VERIFY_SAID),
            ),
            (
                CAPABILITY_RECALL,
                "repeatable-write",
                Some(CAPABILITY_VERIFY_RECALLED),
            ),
            (
                CAPABILITY_MARK_READ,
                "repeatable-write",
                Some(CAPABILITY_ROOMS),
            ),
            (
                CAPABILITY_LEAVE_ROOM,
                "exclusive-create",
                Some(CAPABILITY_ROOMS),
            ),
            (
                CAPABILITY_WRITE_PRESENTATION,
                "repeatable-write",
                Some(CAPABILITY_ROOMS),
            ),
            // 开群房（2026-10-13）：**可重复写**——同参第二次会开出**另一间同名房**（幂等键不同），
            // 因此复核**弱在归因**；**复核 = 通讯录**（那间房在不在我的通讯录里）。
            (
                CAPABILITY_CREATE_GROUP,
                "repeatable-write",
                Some(CAPABILITY_ROOMS),
            ),
            // 邀请（2026-10-13）：**可重复写** ＋ **复核 = 成员清单**——它当天先落在第四档
            // （Agent 侧当时没有成员读面），同一天补上 `im_room_members` 之后升回这一档。
            (
                CAPABILITY_INVITE,
                "repeatable-write",
                Some(CAPABILITY_ROOM_MEMBERS),
            ),
            // 移除成员（2026-10-13）：**同参数第二次必然失败**（他已经不在里面）⇒ 与退房同档；
            // 复核 = 成员清单（他还在不在那份清单里）。
            (
                CAPABILITY_REMOVE_MEMBER,
                "exclusive-create",
                Some(CAPABILITY_ROOM_MEMBERS),
            ),
            // 转让房主（2026-10-13）：同参数第二次必然失败（**我已经不是 owner 了**）⇒ 同档；
            // 复核 = 成员清单（现在谁写着 owner）。
            (
                CAPABILITY_TRANSFER_OWNER,
                "exclusive-create",
                Some(CAPABILITY_ROOM_MEMBERS),
            ),
            // 正在写（2026-10-06 进能力面）：**第四档**——确实是写，但没有同参只读复核可配
            // （信号几秒自灭，读回来归因不了"这一次声明发生过没有"）⇒ 按定义豁免复核。
            (CAPABILITY_TYPING, "unverifiable-write", None),
            // 翻记录（2026-10-20）：**隐式写**——读成功后顺带 PUT 一次读位置，与 `im_mark_read`
            // 改的是同一个公共事实（同伴看到 `peer_read`）。同档同复核：复核 = 通讯录的 `read` 栏。
            (
                CAPABILITY_READ_HISTORY,
                "repeatable-write",
                Some(CAPABILITY_ROOMS),
            ),
        ],
    );
    // 关系面四个写动词同属"同参数第二次成功、零新事实"那一档（发起申请回到既有段 /
    // accept 幂等 / 拉黑与解除都是幂等 204），复核分别是关系账与拉黑名单。
    assert_effects(
        &social,
        &[
            (
                CAPABILITY_CONTACTS_REQUEST,
                "repeatable-write",
                Some(CAPABILITY_CONTACTS),
            ),
            (
                CAPABILITY_ANSWER_CONTACT,
                "repeatable-write",
                Some(CAPABILITY_CONTACTS),
            ),
            (
                CAPABILITY_BLOCK,
                "repeatable-write",
                Some(CAPABILITY_BLOCKS),
            ),
            // 解除关系（2026-10-13）：**可重复写**（没有 active 段也是成功 no-op）＋ 复核 = 关系账。
            (
                CAPABILITY_END_CONTACT,
                "repeatable-write",
                Some(CAPABILITY_CONTACTS),
            ),
            (
                CAPABILITY_UNBLOCK,
                "repeatable-write",
                Some(CAPABILITY_BLOCKS),
            ),
        ],
    );
    // 表达面两个写动词：写资料（每次追加新版本；同 key 同摘要重放不产生新版本）与换头像
    // （与写资料共用版本链）⇒ 同一档；复核都是"读自己的资料"。
    assert_effects(
        &profile,
        &[
            (
                CAPABILITY_WRITE_PROFILE,
                "repeatable-write",
                Some(CAPABILITY_MY_PROFILE),
            ),
            (
                CAPABILITY_WRITE_AVATAR,
                "repeatable-write",
                Some(CAPABILITY_MY_PROFILE),
            ),
        ],
    );
    let im_effects = declared_effect_ids(&app);
    let mut union_effects: Vec<String> = Vec::new();
    union_effects.extend_from_slice(&im_effects[..4]);
    // ⚠ 关系面那条**拆包之后加的**（2026-10-13 解除关系）不参与这条历史重建。
    union_effects.extend(
        declared_effect_ids(&social)
            .into_iter()
            .filter(|id| id != CAPABILITY_END_CONTACT),
    );
    union_effects.extend(declared_effect_ids(&profile));
    union_effects.extend_from_slice(&im_effects[4..5]);
    assert_eq!(
        union_effects,
        strings(&EFFECTS_BEFORE_THE_SPLIT),
        "并集逐条等于拆包前那 11 条声明（同序；`im_typing` 不参与这条历史重建）"
    );
    // 拆包之后加的那些：房治理四条（2026-10-13）与「正在写」；最后一条是**翻记录的隐式写**
    // （2026-10-20 补声明——它从前被"未声明即只读"误读成读，而它读完之后会推进读位置）。
    assert_eq!(
        im_effects[5..],
        [
            CAPABILITY_CREATE_GROUP.to_string(),
            CAPABILITY_INVITE.to_string(),
            CAPABILITY_REMOVE_MEMBER.to_string(),
            CAPABILITY_TRANSFER_OWNER.to_string(),
            CAPABILITY_TYPING.to_string(),
            CAPABILITY_READ_HISTORY.to_string(),
        ],
        "拆包之后多了六条声明：房治理四条（2026-10-13）、「正在写」与翻记录的隐式写"
    );

    // 准入：每个包只放行**自己**那几条；别人的动词与契约里有、本层**故意没做**的那件事
    // （编辑）一律被拒——目录就是权限面，拆包后三个包的归属互不重叠。
    let owners: [(&str, &dyn ActionPort, &[&str]); 3] = [
        (IM_APP_ID, &app, &IM_PACK),
        (IM_SOCIAL_APP_ID, &social, &SOCIAL_PACK),
        (IM_PROFILE_APP_ID, &profile, &PROFILE_PACK),
    ];
    for (app_id, owner, verbs) in owners {
        for verb in verbs {
            assert!(
                matches!(
                    owner.admit(
                        &agent(),
                        &RunId::new("run-im"),
                        &Action::new(ActionId::new(*verb))
                    ),
                    ActionAdmission::Admitted
                ),
                "{app_id} 必须放行自己的动词 {verb}"
            );
        }
        for foreign in DIRECTORY_BEFORE_THE_SPLIT
            .iter()
            .chain(ADDED_AFTER_THE_SPLIT.iter())
            .chain(["im_edit"].iter())
        {
            if verbs.contains(foreign) {
                continue;
            }
            match owner.admit(
                &agent(),
                &RunId::new("run-im"),
                &Action::new(ActionId::new(*foreign)),
            ) {
                ActionAdmission::Refused { class, .. } => assert_eq!(class, "unknown-capability"),
                other => panic!("不属于 {app_id} 的动词必须被拒绝: {other:?}"),
            }
        }
    }
}

#[test]
fn the_credential_never_leaves_the_request_header() {
    let fake = FakeIm::spawn(vec![
        (200, rooms_body()),
        (500, "{\"error\":\"synthetic\"}".to_string()),
    ]);
    let mut app = fake.app();

    let mut seen = Vec::new();
    for action in [rooms(ActionParams::none()), rooms(ActionParams::none())] {
        let outcome = call(&mut app, action);
        seen.push(text_of(&outcome));
        seen.push(format!(
            "{:?} {:?}",
            outcome.state(),
            outcome.failure_class()
        ));
    }

    assert!(fake.next_request().contains(SENTINEL), "凭据确实进了请求头");
    assert!(fake.next_request().contains(SENTINEL));
    let everything = seen.join("\n");
    assert!(
        !everything.contains(SENTINEL),
        "凭据不得出现在任何回执或失败读数里: {everything}"
    );
}

/// **预装常开**：装配期声明 im 常开之后，它的六个动词**从第一轮起就在场**——不需要
/// `open_app`；`open_app` 对它只回答"本来就在场"，而且**一次网都不出**（在场 ≠ 说过话）。
#[test]
fn preinstalled_im_is_open_from_the_first_turn_and_says_nothing_to_anyone() {
    let fake = FakeIm::spawn(vec![]);
    let mut os = MinimalOs::assemble_preinstalled(vec![Box::new(fake.app())], &[IM_APP_ID])
        .expect("预装装配");
    let agent = AgentId::new("agent-im");
    let run = RunId::new("run-im");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    // 首轮就在场：六个动词一个不少。
    let capabilities = scope.capabilities(&agent, &run);
    for verb in [
        CAPABILITY_ROOMS,
        CAPABILITY_READ_HISTORY,
        CAPABILITY_SAY,
        CAPABILITY_VERIFY_SAID,
        CAPABILITY_RECALL,
        CAPABILITY_VERIFY_RECALLED,
    ] {
        assert!(
            capabilities.find(&ActionId::new(verb)).is_some(),
            "预装常开的 im 应当首轮即含 {verb}"
        );
    }

    // 展开机制原样保留：只回答"本来就在场"。
    let op = OperationId::new(run.clone(), Turn::FIRST);
    let expanded = scope.execute(
        &agent,
        &op,
        &Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("target", IM_APP_ID),
        ),
    );
    assert_eq!(expanded.state(), ResultState::Success);
    assert_eq!(expanded.content()[0].label(), "already-expanded");

    // **在场不等于说过话**：这一整套都没有让 im 出过一次网。
    assert_eq!(fake.connections(), 0, "预装/展开本身不得产生任何出网");
}

/// 与 files/notes 同形的**渐进披露**：没展开时目录里没有它的动词、调用被准入拒绝；
/// 展开之后动词出现，调用才真的发出去（这一条同时是"未展开零出网"的读数）。
#[test]
fn the_verbs_only_exist_after_expanding_im() {
    let fake = FakeIm::spawn(vec![(200, rooms_body()), (200, me_body(IM_PARTICIPANT))]);
    let mut os = MinimalOs::assemble(vec![Box::new(fake.app())]).expect("可以装配");
    let agent = AgentId::new("agent-im");
    let run = RunId::new("run-im");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);
    // 操作身份必须与这次作用域的身份、活动一致（OS 会核对，不同就是 identity-mismatch）。
    let op = OperationId::new(run.clone(), Turn::FIRST);

    let capabilities = scope.capabilities(&agent, &run);
    assert!(
        capabilities
            .find(&ActionId::new(CAPABILITY_ROOMS))
            .is_none(),
        "未展开时目录里不该有 im 的动词"
    );
    let refused = scope.execute(&agent, &op, &rooms(ActionParams::none()));
    assert_eq!(refused.failure_class(), Some("not-expanded"));
    assert_eq!(fake.connections(), 0, "未展开不许出网");

    let expanded = scope.execute(
        &agent,
        &op,
        &Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("target", IM_APP_ID),
        ),
    );
    assert_eq!(expanded.state(), ResultState::Success, "展开应当成功");

    let capabilities = scope.capabilities(&agent, &run);
    assert!(
        capabilities
            .find(&ActionId::new(CAPABILITY_ROOMS))
            .is_some(),
        "展开之后两个只读动词才出现"
    );
    let ok = scope.execute(&agent, &op, &rooms(ActionParams::none()));
    assert_eq!(ok.state(), ResultState::Success);
    assert_eq!(
        fake.connections(),
        2,
        "展开之后这一次调用发两个请求：读正文 + 回执开头那行身份（TASK_71 第 4 件）"
    );
}

/// **拆包后的渐进式**（2026-10-02）：社交面（`im-social`）与资料面（`im-profile`）**不预装**——
/// 装配了也不在场，要 `open_app` 展开那一下才上场；展开之前调用它们一律被准入拒绝、**零出网**。
///
/// 通信面（`im`）照旧预装常开（上面那条用例守着），三个包共用同一份端点与凭据声明
/// （各自建自己的请求客户端）。这条用例同时是装配期的读数：写效应必配复核、每个能力必有来源，
/// 三个包一起过 `check_declarations` 才算装配得起来。
#[test]
fn the_social_and_profile_packs_only_show_up_after_expanding_them() {
    let fake = FakeIm::spawn(vec![]);
    let mut os = MinimalOs::assemble_preinstalled(
        vec![
            Box::new(fake.app()),
            Box::new(fake.social_app()),
            Box::new(fake.profile_app()),
        ],
        &[IM_APP_ID],
    )
    .expect("预装装配");
    let agent = AgentId::new("agent-im");
    let run = RunId::new("run-im");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);
    let op = OperationId::new(run.clone(), Turn::FIRST);

    // 预装的只有通信面：它的动词在场，社交 / 资料两组的动词**一条都不在场**。
    let capabilities = scope.capabilities(&agent, &run);
    for verb in IM_PACK {
        assert!(
            capabilities.find(&ActionId::new(verb)).is_some(),
            "预装常开的 im 应当首轮即含 {verb}"
        );
    }
    for verb in SOCIAL_PACK.iter().chain(PROFILE_PACK.iter()) {
        assert!(
            capabilities.find(&ActionId::new(*verb)).is_none(),
            "未展开时目录里不该有 {verb}"
        );
    }
    // **2026-10-10 收口（主人裁决）**：`im_typing` **不再开箱免查**——出厂默认集已删，
    // **常驻只能是主体自己 pin 的**。⇒ 它此刻调用会撞"先看格式"那道门（`os.rs::may_call`），
    // 而**门语义一个字没动**：拒绝的语义是"先看格式"，不是"你没有这个能力"
    // （它的动词就在上面那份目录里）。
    //
    // ⚠ 从前这条断言钉的是相反的事（`im_typing` 在出厂默认集里 ⇒ 免查）。那一格随本刀消失：
    // 主人当次的判据是「**正在输入这种无所谓，甚至说不需要**」——所以它现在与别的动词**同待遇**，
    // 谁都不特殊。要免查，主体自己 `pin_action` 钉它。
    let refused = scope.admit(
        &agent,
        &run,
        &Action::with_params(
            ActionId::new(CAPABILITY_TYPING),
            ActionParams::none().with(PARAM_ROOM, "r-1"),
        ),
    );
    assert!(
        matches!(
            &refused,
            ActionAdmission::Refused { class, .. } if class == "needs-inspection"
        ),
        "没有任何动作开箱免查（出厂默认集已删）：要免查就自己钉住它: {refused:?}"
    ); // 未展开调用：准入拒绝，**一次网都不出**。
    for action in [
        contacts(ActionParams::none()),
        my_profile(ActionParams::none()),
    ] {
        let refused = scope.execute(&agent, &op, &action);
        assert_eq!(refused.failure_class(), Some("not-expanded"));
    }
    assert_eq!(fake.connections(), 0, "未展开不许出网");

    // 展开社交面：只有关系面的动词出现（资料面还没展开）。
    let expanded = scope.execute(
        &agent,
        &op,
        &Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("target", IM_SOCIAL_APP_ID),
        ),
    );
    assert_eq!(
        expanded.state(),
        ResultState::Success,
        "展开 im-social 应当成功"
    );
    let capabilities = scope.capabilities(&agent, &run);
    for verb in SOCIAL_PACK {
        assert!(
            capabilities.find(&ActionId::new(verb)).is_some(),
            "展开之后 {verb} 才出现"
        );
    }
    assert!(
        capabilities
            .find(&ActionId::new(CAPABILITY_MY_PROFILE))
            .is_none(),
        "资料面没展开，它的动词不该出现"
    );

    // 展开资料面。
    let expanded = scope.execute(
        &agent,
        &op,
        &Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("target", IM_PROFILE_APP_ID),
        ),
    );
    assert_eq!(
        expanded.state(),
        ResultState::Success,
        "展开 im-profile 应当成功"
    );
    let capabilities = scope.capabilities(&agent, &run);
    for verb in PROFILE_PACK {
        assert!(
            capabilities.find(&ActionId::new(verb)).is_some(),
            "展开之后 {verb} 才出现"
        );
    }

    // **在场不等于说过话**：装配与两次展开本身一次网都不出。
    assert_eq!(fake.connections(), 0, "装配 / 展开不得产生任何出网");
}

// ---------------------------------------------------------------------------
// 5. 写侧：说话（幂等键、三态、参数网关）
// ---------------------------------------------------------------------------

/// 一页消息（复核用）：每条给（id、actor.id、正文）。`next` 非空 ⇒ 还有更旧的没看。
fn page_with_next(rows: &[(&str, &str, &str)], next: Option<&str>, recalled: &[&str]) -> String {
    let messages: Vec<serde_json::Value> = rows
        .iter()
        .map(|(id, actor, body)| {
            serde_json::json!({
                "id": id,
                "room_id": "r-1",
                "actor": {"id": actor, "kind": "agent", "display_name": "小满", "status": "active"},
                "body": if recalled.contains(id) { serde_json::Value::Null } else { serde_json::json!(body) },
                "mentions": [],
                "attachments": [],
                "reply_to_message_id": null,
                "created_at": "2026-09-16T11:00:00+00:00",
                "recalled_at": if recalled.contains(id) { serde_json::json!("2026-09-16T11:01:00+00:00") } else { serde_json::Value::Null }
            })
        })
        .collect();
    serde_json::json!({
        "messages": messages,
        "next_before": next,
        "request_id": "req-page"
    })
    .to_string()
}

/// 一页"看完了整段历史"的消息（`next_before = null`）。
fn said_page(rows: &[(&str, &str, &str)]) -> String {
    page_with_next(rows, None, &[])
}

#[test]
fn saying_posts_once_with_a_key_derived_from_the_operation() {
    let fake = FakeIm::spawn(vec![
        (201, say_body("m-1", false)),
        (200, say_body("m-1", true)),
    ]);
    let mut app = fake.app();
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_BODY, "你好");
    let operation = OperationId::new(RunId::new("run-say"), Turn::FIRST);

    let first = app.execute(
        &agent(),
        &operation,
        &Action::with_params(ActionId::new(CAPABILITY_SAY), params.clone()),
    );
    assert_eq!(first.state(), ResultState::Success);
    assert_eq!(
        text_of(&first),
        "room = r-1, message = m-1\nreplayed = no",
        "首次提交如实写 replayed = no"
    );

    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/rooms/r-1/messages "),
        "路径必须是后台发消息端点: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {SENTINEL}")),
        "凭据只走请求头: {request}"
    );
    let key = request
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("idempotency-key:"))
        .map(|line| line.split(':').nth(1).expect("有取值").trim().to_string())
        .expect("写请求必须带幂等键");
    assert_eq!(key, "celira-agent-run-say-1-0", "键由操作身份合成: {key}");
    assert_eq!(fake.connections(), 1, "一次调用恰好一次请求");

    // 同一次操作再投递一次：**同一个键**（服务端据此把重复收成一条）。
    let again = app.execute(
        &agent(),
        &operation,
        &Action::with_params(ActionId::new(CAPABILITY_SAY), params),
    );
    let second = fake.next_request();
    assert!(
        second
            .to_ascii_lowercase()
            .contains(&format!("idempotency-key: {key}").to_ascii_lowercase()),
        "同一次操作必须用同一个键: {second}"
    );
    assert_eq!(
        text_of(&again),
        "room = r-1, message = m-1\nreplayed = yes",
        "重放也是成功，而且要如实说出这是重放"
    );
}

#[test]
fn a_rejected_send_is_a_failure_while_an_unconfirmed_one_is_unknown() {
    // 明确的拒绝（4xx）：契约保证零新事实 ⇒ 一次确定的失败，带本 App 的分类。
    let table: [(u16, &str); 5] = [
        (400, "invalid-argument"),
        (401, "credential-rejected"),
        (403, "not-a-member"),
        (404, "unknown-room"),
        (409, "idempotency-conflict"),
    ];
    for (status, expected) in table {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = app.execute(
            &agent(),
            &OperationId::new(RunId::new("run-say"), Turn::FIRST),
            &Action::with_params(
                ActionId::new(CAPABILITY_SAY),
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_BODY, "你好"),
            ),
        );
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
    }

    // 读不到确定的答案：**未确认**，绝不是失败（也绝不重发）。含"契约没列出的 4xx"：
    // 那个码对我们是一种不认识的回程形状，因此门必须站着（fail-closed），不能当成"被拒绝"。
    let unconfirmed: [(u16, &str); 5] = [
        (422, "{\"error\":\"unlisted\"}"),
        (500, "{\"error\":\"synthetic\"}"),
        (503, "{}"),
        (200, "this is not json"),
        (200, "{\"request_id\":\"only\"}"),
    ];
    for (status, body) in unconfirmed {
        let fake = FakeIm::spawn(vec![(status, body.to_string())]);
        let mut app = fake.app();
        let outcome = app.execute(
            &agent(),
            &OperationId::new(RunId::new("run-say"), Turn::FIRST),
            &Action::with_params(
                ActionId::new(CAPABILITY_SAY),
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_BODY, "你好"),
            ),
        );
        assert_eq!(
            outcome.state(),
            ResultState::Unknown,
            "HTTP {status} + {body} 必须报未确认（不是失败）"
        );
        assert_eq!(fake.connections(), 1, "未确认也不重发：恰好一次请求");
    }

    // 连不上：同样未确认。
    let mut app =
        ImApp::new(ImAccess::new("http://127.0.0.1:99999/not-a-port", SENTINEL).expect("声明"))
            .expect("可以装配");
    let outcome = app.execute(
        &agent(),
        &OperationId::new(RunId::new("run-say"), Turn::FIRST),
        &Action::with_params(
            ActionId::new(CAPABILITY_SAY),
            ActionParams::none()
                .with(PARAM_ROOM, "r-1")
                .with(PARAM_BODY, "你好"),
        ),
    );
    assert_eq!(outcome.state(), ResultState::Unknown);
}

#[test]
fn the_send_gateway_refuses_before_any_request() {
    let long_room = "r".repeat(65);
    let long_body = "字".repeat(20_000); // 每字 3 字节 ⇒ 60 KB，远超 16384 字节上限
    let cases: Vec<(&str, String, String)> = vec![
        ("缺房间", String::new(), "你好".to_string()),
        ("房间超长", long_room, "你好".to_string()),
        ("空正文", "r-1".to_string(), String::new()),
        ("正文超限", "r-1".to_string(), long_body),
        ("正文含 NUL", "r-1".to_string(), "你\0好".to_string()),
    ];
    for (label, room, body) in cases {
        let fake = FakeIm::spawn(vec![]);
        let mut app = fake.app();
        let outcome = app.execute(
            &agent(),
            &OperationId::new(RunId::new("run-say"), Turn::FIRST),
            &Action::with_params(
                ActionId::new(CAPABILITY_SAY),
                ActionParams::none()
                    .with(PARAM_ROOM, room)
                    .with(PARAM_BODY, body),
            ),
        );
        assert_eq!(
            outcome.failure_class(),
            Some("invalid-argument"),
            "{label} 必须在本地被拒"
        );
        assert_eq!(fake.connections(), 0, "{label} 一次都不许出网");
    }
}

// ---------------------------------------------------------------------------
// 6. 复核（只读）：只从窗口里回答，读不出答案就不解除任何东西
// ---------------------------------------------------------------------------

/// 复核的三态、请求形状，以及**归因必须用服务器给的身份**。
#[test]
fn the_verification_attributes_by_identity_before_judging_absence() {
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_BODY, "你好");
    let verify = |app: &mut ImApp| {
        app.execute(
            &agent(),
            &OperationId::new(RunId::new("run-check"), Turn::FIRST),
            &Action::with_params(ActionId::new(CAPABILITY_VERIFY_SAID), params.clone()),
        )
    };

    // 窗口里有**主体**（服务器给的身份）发的那一条 ⇒ 成功，并**明说看了多少条**。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (
            200,
            said_page(&[
                ("m-9", "other-agent", "你好"),
                ("m-1", IM_PARTICIPANT, "你好"),
            ]),
        ),
    ]);
    let mut app = fake.app();
    let found = verify(&mut app);
    assert_eq!(found.state(), ResultState::Success);
    assert_eq!(
        text_of(&found),
        "room = r-1, searched = 2, more = none, found = m-1",
        "窗口大小与'有没有更旧的没看'都必须写出来"
    );
    // 形状：**恰好两步**只读请求——先问"我是谁"，再读那一页；一步都不多（不重试）。
    let first = fake.next_request();
    assert!(
        first.starts_with("GET /v1/backend/me "),
        "复核的第一步是问服务器我是谁: {first}"
    );
    let second = fake.next_request();
    assert!(
        second.starts_with("GET /v1/backend/rooms/r-1/messages?limit=50 "),
        "复核的第二步是只读翻记录: {second}"
    );
    assert_eq!(fake.connections(), 2, "复核恰好两步只读请求");

    // 窗口里没有主体发的 ⇒ 一个**确定的**答案：not-found。
    // 中间那条是最要紧的围栏：**本地 AgentId 出现在 actor.id 上不算 Agent 发的**（两套命名空间）。
    for rows in [
        vec![("m-9", "other-agent", "你好")],
        vec![("m-9", agent().as_str(), "你好")],
        vec![],
    ] {
        let fake = FakeIm::spawn(vec![
            (200, me_body(IM_PARTICIPANT)),
            (200, said_page(&rows)),
        ]);
        let mut app = fake.app();
        let outcome = verify(&mut app);
        assert_eq!(
            outcome.failure_class(),
            Some("not-found"),
            "这些都不算 Agent 发的: {rows:?}"
        );
    }

    // 读不出确定答案的：**一律 unknown**（没有答案就不能解除任何东西）。
    for (status, body) in [
        (401u16, "{\"error\":\"synthetic\"}".to_string()),
        (403, "{\"error\":\"synthetic\"}".to_string()),
        (409, "{\"error\":\"synthetic\"}".to_string()),
        (500, "{\"error\":\"synthetic\"}".to_string()),
        (200, "this is not json".to_string()),
    ] {
        let fake = FakeIm::spawn(vec![(200, me_body(IM_PARTICIPANT)), (status, body.clone())]);
        let mut app = fake.app();
        let outcome = verify(&mut app);
        assert_eq!(
            outcome.state(),
            ResultState::Unknown,
            "HTTP {status} 没有回答那条消息在不在"
        );
    }

    // 房间不存在 / 请求非法：**也**是 unknown（TASK_24 收紧的那一处）。
    // 理由：房不存在与"我不再是成员"用的是同一个 404，而"我不是成员"**不证明**那条消息不存在；
    // "确定没有"的唯一依据是**翻到房的起点**（见下一组用例）。
    for status in [400u16, 404] {
        let fake = FakeIm::spawn(vec![
            (200, me_body(IM_PARTICIPANT)),
            (status, "{\"error\":\"synthetic\"}".to_string()),
        ]);
        let mut app = fake.app();
        let outcome = verify(&mut app);
        assert_eq!(
            outcome.state(),
            ResultState::Unknown,
            "HTTP {status} 没有说清那条到底在不在"
        );
    }

    // **连"我是谁"都问不出来 ⇒ unknown**：没有身份就没有归因，于是没有答案。
    for (status, body) in [
        (401u16, "{\"error\":\"synthetic\"}".to_string()),
        (500, "{\"error\":\"synthetic\"}".to_string()),
        (200, "{}".to_string()),
        (200, me_body("").to_string()),
    ] {
        let fake = FakeIm::spawn(vec![(status, body.clone())]);
        let mut app = fake.app();
        let outcome = verify(&mut app);
        assert_eq!(
            outcome.state(),
            ResultState::Unknown,
            "身份探针的读数：HTTP {status} / {body}"
        );
        assert_eq!(fake.connections(), 1, "问不出身份就不该再去读那一页");
    }
}

/// 复核必须**如实报出窗口是不是整段**（`more = none | older`）。
///
/// 这条是 B1（裁决单 §B）：回执不许把"我只看了一页"藏起来。**判定口径本身不动**——所以这里同时
/// 把当前口径钉住（截断时"没找到"仍算 not-found），好让 TASK_24 改它的时候看得见自己改了什么。
#[test]
fn the_verification_reports_whether_it_saw_the_whole_history() {
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_BODY, "你好");
    let verify = |app: &mut ImApp| {
        app.execute(
            &agent(),
            &OperationId::new(RunId::new("run-window"), Turn::FIRST),
            &Action::with_params(ActionId::new(CAPABILITY_VERIFY_SAID), params.clone()),
        )
    };

    // 还有更旧的没看 ⇒ `more = older`（哪怕这一条已经找到了，也要说）。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (
            200,
            page_with_next(&[("m-1", IM_PARTICIPANT, "你好")], Some("c1.m.m-1"), &[]),
        ),
    ]);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(
        text_of(&outcome),
        "room = r-1, searched = 1, more = older, found = m-1"
    );

    // 缺 `next_before` 字段 ⇒ 不声称看完了整段历史、也**没有游标可以继续** ⇒ 只能报 unknown。
    // （这是 fail-closed 的默认值：在没有证据时宁可说"不知道"，也不说"没有"。）
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (
            200,
            serde_json::json!({"messages": [], "request_id": "req-page"}).to_string(),
        ),
    ]);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(outcome.state(), ResultState::Unknown);
    assert_eq!(
        fake.connections(),
        2,
        "只试了一页（没有游标就不猜、不构造），恰好两步（身份探针 + 一页）"
    );
}

/// **有界回翻**：翻到起点才敢说"确定没有"，触顶或读不出来只能说"不知道"。
///
/// 这是 TASK_24 的核心判据——改的是"确定"这个词**凭什么**。
#[test]
fn the_verification_pages_back_until_it_finds_or_reaches_the_start() {
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_BODY, "你好");
    let verify = |app: &mut ImApp| {
        app.execute(
            &agent(),
            &OperationId::new(RunId::new("run-page"), Turn::FIRST),
            &Action::with_params(ActionId::new(CAPABILITY_VERIFY_SAID), params.clone()),
        )
    };
    let cursor = |page: usize| format!("c1.m.page-{page}");

    // ① 第 1 页没有、第 2 页有 ⇒ 成功，且**游标原样回传**、`searched` 是累计。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (
            200,
            page_with_next(
                &[
                    ("m-9", "other-agent", "你好"),
                    ("m-8", IM_PARTICIPANT, "别的话"),
                ],
                Some(&cursor(1)),
                &[],
            ),
        ),
        (
            200,
            page_with_next(&[("m-1", IM_PARTICIPANT, "你好")], None, &[]),
        ),
    ]);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(outcome.state(), ResultState::Success);
    assert_eq!(
        text_of(&outcome),
        "room = r-1, searched = 3, more = none, found = m-1"
    );
    assert!(fake.next_request().starts_with("GET /v1/backend/me "));
    assert!(
        fake.next_request()
            .starts_with("GET /v1/backend/rooms/r-1/messages?limit=50 "),
        "第 1 页不带游标"
    );
    let second = fake.next_request();
    assert!(
        second.contains(&format!("before={}", cursor(1))),
        "第 2 页必须把上一页给的游标**原样**带回去: {second}"
    );
    assert_eq!(fake.connections(), 3, "身份探针 + 两页，一步不多");

    // ② 每一页都没有、最后一页翻到起点 ⇒ **这才叫"确定没有"** ⇒ not-found。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (
            200,
            page_with_next(&[("m-9", "other-agent", "你好")], Some(&cursor(1)), &[]),
        ),
        (
            200,
            page_with_next(&[("m-8", "other-agent", "你好")], None, &[]),
        ),
    ]);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(outcome.failure_class(), Some("not-found"));
    assert!(
        text_of(&outcome).is_empty(),
        "失败不带正文块（窗口读数只在成功时给）"
    );
    assert_eq!(fake.connections(), 3);

    // ③ 每一页都还有更旧的 ⇒ 走满页数上限 ⇒ **unknown**（"没看见"不是"没有"）。
    //    页数从常量派生，不写死。
    let mut replies = vec![(200u16, me_body(IM_PARTICIPANT))];
    for page in 0..MAX_VERIFY_PAGES {
        replies.push((
            200,
            page_with_next(
                &[("m-9", "other-agent", "你好")],
                Some(&cursor(page + 1)),
                &[],
            ),
        ));
    }
    let fake = FakeIm::spawn(replies);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(outcome.state(), ResultState::Unknown, "触顶不许说'没有'");
    assert_eq!(
        fake.connections(),
        1 + MAX_VERIFY_PAGES,
        "恰好 1 次身份探针 + MAX_VERIFY_PAGES 页（不重试）"
    );

    // ④ 中途某一页读不出来（这里用 5xx）⇒ **立刻 undecided**，不重试、不接着翻。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (
            200,
            page_with_next(&[("m-9", "other-agent", "你好")], Some(&cursor(1)), &[]),
        ),
        (500, "{\"error\":\"synthetic\"}".to_string()),
    ]);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(outcome.state(), ResultState::Unknown);
    assert_eq!(fake.connections(), 3, "读不出来就停，不重试、不跳页");
}

// ---------------------------------------------------------------------------
// 6. 撤回（写）：形状、三态、它自己那两个码，以及按身份归因的复核
// ---------------------------------------------------------------------------

/// 撤回的**形状**：`DELETE` 那条路径、**没有 body / query / Idempotency-Key**；201 与 200 都是 success。
///
/// "没有键"这条是契约要求（带了就 400），因此它必须被**钉住**——否则将来有人"顺手统一一下"就会把
/// 端点打成 400，而且那时候看起来还很像一次正常失败。
#[test]
fn recalling_deletes_once_without_any_key() {
    let fake = FakeIm::spawn(vec![
        (201, recall_body(MESSAGE, false)),
        (200, recall_body(MESSAGE, true)),
    ]);
    let mut app = fake.app();
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_MESSAGE, MESSAGE);

    let first = call(&mut app, recall(params.clone()));
    assert_eq!(first.state(), ResultState::Success);
    // ⚠ 撤回时点是**世界给的时刻** ⇒ 回执里摆的是**本机刻度**（与人那边 `toLocaleString()` 同一步）。
    // 期望值因此**按同一台机器算出来**，而不是写死一个偏移——写死它就会变成"只有 +08:00 的机器才绿"。
    // "换过之后仍是同一个瞬间"那条判据在 `tests/status_bar.rs` 里钉着。
    assert_eq!(
        text_of(&first),
        format!(
            "room = r-1, message = 632474f3-95c0-4adb-9649-30209ac5012d, recalled_at = {}\nreplayed = no",
            celira_agent::clock::localize_rfc3339("2026-09-16T11:01:00+00:00")
        )
    );

    let request = fake.next_request();
    assert!(
        request.starts_with(
            "DELETE /v1/backend/rooms/r-1/messages/632474f3-95c0-4adb-9649-30209ac5012d "
        ),
        "撤回是一次 DELETE: {request}"
    );
    assert!(request.contains("authorization:"), "凭据进请求头");
    let lowered = request.to_ascii_lowercase();
    assert!(
        !lowered.contains("idempotency-key"),
        "契约要求撤回**不带**幂等键（带了就 400）: {request}"
    );
    assert!(
        !lowered.contains("content-length") && !request.contains('{'),
        "撤回没有请求体: {request}"
    );

    // 重复撤回（同一条事实的 200 重放）⇒ **也是 success**，回执如实说这一次没产生新事实。
    let again = call(&mut app, recall(params));
    assert_eq!(again.state(), ResultState::Success);
    assert!(
        text_of(&again).ends_with("replayed = yes"),
        "重放必须如实说出来: {}",
        text_of(&again)
    );
    assert_eq!(fake.connections(), 2, "两次调用恰好两次请求");
}

/// 撤回的失败表：**同一个状态码在这里的含义与说话侧不同**——`404` 是"那条不在可撤范围里"，
/// `409` 是"超窗"，都不是"房不存在"或"同键异摘要"。
#[test]
fn the_recall_failure_table_maps_its_own_404_and_409() {
    for (status, expected) in [
        (400u16, "invalid-argument"),
        (401, "credential-rejected"),
        (404, "message-not-found"),
        (409, "window-expired"),
    ] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            recall(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_MESSAGE, MESSAGE),
            ),
        );
        assert_eq!(outcome.state(), ResultState::Failure, "HTTP {status}");
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
        assert!(
            text_of(&outcome).is_empty(),
            "失败不带正文块（状态码与响应体不外泄）: {}",
            text_of(&outcome)
        );
    }

    // 契约**没列出**的码（422）与 5xx、读不懂：一律"没告诉我们" ⇒ 未确认（fail-closed）。
    for (status, body) in [
        (422u16, "{\"error\":\"synthetic\"}".to_string()),
        (500, "{\"error\":\"synthetic\"}".to_string()),
        (503, "{\"error\":\"synthetic\"}".to_string()),
        (200, "this is not json".to_string()),
        (200, "{}".to_string()),
    ] {
        let fake = FakeIm::spawn(vec![(status, body.clone())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            recall(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_MESSAGE, MESSAGE),
            ),
        );
        assert_eq!(
            outcome.state(),
            ResultState::Unknown,
            "HTTP {status} / {body} 没有告诉我们那一次的提交结果"
        );
    }

    // 同一种失败两次发生逐字节相同。
    let mut readings = Vec::new();
    for _ in 0..2 {
        let fake = FakeIm::spawn(vec![(409, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            recall(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_MESSAGE, MESSAGE),
            ),
        );
        readings.push((
            outcome.state(),
            outcome.failure_class().map(str::to_string),
            text_of(&outcome),
        ));
    }
    assert_eq!(readings[0], readings[1]);
}

/// 撤回的参数网关：形状不对就在本地拒绝，**一次都不出网**。
#[test]
fn the_recall_gateway_refuses_before_any_request() {
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app();

    let long_room = "r".repeat(65);
    for params in [
        ActionParams::none().with(PARAM_ROOM, "r-1"),
        ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_MESSAGE, ""),
        ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_MESSAGE, "632474f3-95c0-4adb-9649-30209ac5012"),
        ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_MESSAGE, "632474F3-95C0-4ADB-9649-30209AC5012D"),
        ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_MESSAGE, "632474f3-95c0-4adb-9649-30209ac5012z"),
        ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_MESSAGE, "632474f395c04adb964930209ac5012d00"),
        ActionParams::none()
            .with(PARAM_ROOM, "")
            .with(PARAM_MESSAGE, MESSAGE),
        ActionParams::none()
            .with(PARAM_ROOM, long_room.as_str())
            .with(PARAM_MESSAGE, MESSAGE),
    ] {
        let outcome = call(&mut app, recall(params.clone()));
        assert_eq!(
            outcome.failure_class(),
            Some("invalid-argument"),
            "形状不对必须在本地拒绝: {params:?}"
        );
    }
    assert_eq!(fake.connections(), 0, "非法参数一次都不许出网");
}

/// 撤回的复核**按消息身份归因**（不看正文、不看作者），而且**只有一步请求**（不需要身份探针）。
#[test]
fn the_recall_verification_judges_by_message_identity() {
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_MESSAGE, MESSAGE);
    let verify = |app: &mut ImApp| call(app, verify_recalled(params.clone()));

    // 那条在窗口里、且已撤回 ⇒ 成功；回执给出窗口读数与那条身份。
    let fake = FakeIm::spawn(vec![(
        200,
        page_with_next(
            &[
                ("m-9", "other-agent", "别的话"),
                (MESSAGE, IM_PARTICIPANT, "你好"),
            ],
            None,
            &[MESSAGE],
        ),
    )]);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(outcome.state(), ResultState::Success);
    assert_eq!(
        text_of(&outcome),
        format!("room = r-1, searched = 2, more = none, found = {MESSAGE}")
    );
    let request = fake.next_request();
    assert!(
        request.starts_with("GET /v1/backend/rooms/r-1/messages?limit=50 "),
        "复核是只读翻记录: {request}"
    );
    assert_eq!(
        fake.connections(),
        1,
        "按身份归因的复核只有一步（不需要身份探针）"
    );

    // 那条在窗口里、但**没有**撤回 ⇒ 一个确定的答案：这次效果没发生。
    // 第二条是围栏：**同正文、不同身份**的那种不算（归因看身份，不看内容）。
    for rows in [
        vec![(MESSAGE, IM_PARTICIPANT, "你好")],
        vec![("m-other", IM_PARTICIPANT, "你好")],
        vec![],
    ] {
        let fake = FakeIm::spawn(vec![(200, page_with_next(&rows, None, &[]))]);
        let mut app = fake.app();
        let outcome = verify(&mut app);
        assert_eq!(
            outcome.failure_class(),
            Some("not-found"),
            "这些都不算'那条已撤回': {rows:?}"
        );
    }

    // 读不出确定答案的：**一律 unknown**。
    for (status, body) in [
        (401u16, "{\"error\":\"synthetic\"}".to_string()),
        (409, "{\"error\":\"synthetic\"}".to_string()),
        (500, "{\"error\":\"synthetic\"}".to_string()),
        (200, "this is not json".to_string()),
    ] {
        let fake = FakeIm::spawn(vec![(status, body.clone())]);
        let mut app = fake.app();
        let outcome = verify(&mut app);
        assert_eq!(outcome.state(), ResultState::Unknown, "HTTP {status}");
    }

    // 请求非法 / 房不存在：**也**是 unknown（与 `im_verify_said` 同一口径）。
    for status in [400u16, 404] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = verify(&mut app);
        assert_eq!(outcome.state(), ResultState::Unknown, "HTTP {status}");
    }
}

/// **一条写声明配一个复核**：两个写动词各自的复核能力逐字对齐。
///
/// ⚠ **旧规矩 → 新规矩**（2026-09-26，两刀）：这条用例原来还接着断言"未决效果门只被同参数复核
/// 解除（异参数、`unknown` 都不解除）"——那半边随**未决效果门**一起退役了（门是指令，而世界只该
/// 给信息）。它读的那张**效果声明表**也在本刀退役（门是那张表唯一的生产读者），因此读数回到
/// **权威源**：App 自己那句声明。**声明侧一个字没松**：它仍然钉住"谁的复核是谁"，那是装配期
/// 自洽性检查的读数，与门无关。
#[test]
fn each_write_declares_its_own_verification() {
    let fake = FakeIm::spawn(vec![]);
    let app = fake.app();
    let declared = app.declared_effects();
    let verification_of = |action: &str| {
        declared
            .iter()
            .find(|effect| effect.action().as_str() == action)
            .and_then(|effect| effect.verification().map(|id| id.as_str().to_string()))
    };
    assert_eq!(
        verification_of(CAPABILITY_RECALL).as_deref(),
        Some(CAPABILITY_VERIFY_RECALLED)
    );
    assert_eq!(
        verification_of(CAPABILITY_SAY).as_deref(),
        Some(CAPABILITY_VERIFY_SAID)
    );
}

/// 撤回的复核同样**有界回翻**：那条也许早就不在新的那一页里了（它在更旧的一页上）。
#[test]
fn the_recall_verification_pages_back_too() {
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_MESSAGE, MESSAGE);
    let verify = |app: &mut ImApp| call(app, verify_recalled(params.clone()));

    // 第 1 页（更新的一页）里没有那条、也没有更旧的 ⇒ 退到第 2 页；第 2 页里它在、且已撤回。
    let fake = FakeIm::spawn(vec![
        (
            200,
            page_with_next(
                &[("m-9", IM_PARTICIPANT, "新的话")],
                Some("c1.m.page-1"),
                &[],
            ),
        ),
        (
            200,
            page_with_next(&[(MESSAGE, IM_PARTICIPANT, "旧的那条")], None, &[MESSAGE]),
        ),
    ]);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(outcome.state(), ResultState::Success);
    assert_eq!(
        text_of(&outcome),
        format!("room = r-1, searched = 2, more = none, found = {MESSAGE}")
    );
    let first = fake.next_request();
    assert!(
        !first.contains("before="),
        "第 1 页不带游标（而且这一条复核**不问身份**）: {first}"
    );
    let second = fake.next_request();
    assert!(
        second.contains("before=c1.m.page-1"),
        "第 2 页原样带回上一页的游标: {second}"
    );
    assert_eq!(fake.connections(), 2, "两页、恰好两次（没有身份探针）");

    // 每页都没那条、一路翻到起点 ⇒ not-found（它不在这间房的历史里 ⇒ 也没撤回）。
    let mut replies = vec![];
    for page in 0..MAX_VERIFY_PAGES {
        replies.push((
            200u16,
            page_with_next(
                &[("m-9", IM_PARTICIPANT, "新的话")],
                Some(&format!("c1.m.page-{page}")),
                &[],
            ),
        ));
    }
    let fake = FakeIm::spawn(replies);
    let mut app = fake.app();
    let outcome = verify(&mut app);
    assert_eq!(
        outcome.state(),
        ResultState::Unknown,
        "触顶只能说不知道（与 im_verify_said 同一口径）"
    );
    assert_eq!(fake.connections(), MAX_VERIFY_PAGES);
}

// ---------------------------------------------------------------------------
// 6b. 读位置推进（写，切片 M1）：形状、三态、它自己那个码
// ---------------------------------------------------------------------------

fn mark_read(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_MARK_READ), params)
}

/// 推进回执体（真实形状：`read_state` + `advanced` + `request_id`）。
fn read_cursor_body(advanced: bool, last_read: Option<&str>, unread: serde_json::Value) -> String {
    serde_json::json!({
        "read_state": {"last_read_message_id": last_read, "unread_count": unread},
        "advanced": advanced,
        "request_id": "req-read-cursor"
    })
    .to_string()
}

/// 推进：一次写 = 恰好一次请求，**不带幂等键**（单调推进天然幂等），回执给出世界记下的位置。
#[test]
fn the_mark_read_advance_reports_where_the_world_now_puts_me() {
    let fake = FakeIm::spawn(vec![(200, read_cursor_body(true, Some(MESSAGE), 3.into()))]);
    let mut app = fake.app();

    let outcome = call(
        &mut app,
        mark_read(
            ActionParams::none()
                .with(PARAM_ROOM, "r-1")
                .with(PARAM_MESSAGE, MESSAGE),
        ),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");

    let request = fake.next_request();
    assert!(
        request.starts_with("PUT /v1/backend/rooms/r-1/read-cursor "),
        "方法必须是 PUT、路径必须是读游标端点: {request}"
    );
    assert!(
        request.contains(&format!("\"message_id\":\"{MESSAGE}\"")),
        "体只带 message_id: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("idempotency-key"),
        "单调推进天然幂等 ⇒ 契约要求**不带键**: {request}"
    );
    assert_eq!(fake.connections(), 1, "一次写 = 恰好一次请求（绝不重发）");

    let text = text_of(&outcome);
    assert!(text.contains("room = r-1, advanced = yes"), "{text}");
    assert!(text.contains(&format!("read = {MESSAGE}")), "{text}");
    assert!(text.contains("unread = 3"), "{text}");
}

/// 同一条重发 / 给一条更旧的：世界答 `advanced = no` —— 那是**成功**，不是失败。
#[test]
fn the_mark_read_noop_is_a_success_not_a_failure() {
    let fake = FakeIm::spawn(vec![(
        200,
        read_cursor_body(false, Some(MESSAGE), 0.into()),
    )]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        mark_read(
            ActionParams::none()
                .with(PARAM_ROOM, "r-1")
                .with(PARAM_MESSAGE, MESSAGE),
        ),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "no-op 是成功: {outcome:?}"
    );
    let text = text_of(&outcome);
    assert!(text.contains("advanced = no"), "{text}");
    assert!(text.contains("unread = 0"), "零未读如实写 0: {text}");
}

/// 回执里那两栏用的是**与通讯录同一套三态词**：位置 `<id>` / `-`（还没读过）/ `?`（读不到）；
/// 未读 `<n>` / `?`（读不到）——**读不到不写 0**。
#[test]
fn the_mark_read_receipt_uses_the_same_three_state_words_as_the_directory() {
    for (body, expected) in [
        // 正常：位置与未读都读得到。
        (
            read_cursor_body(true, Some(MESSAGE), 4.into()),
            format!("read = {MESSAGE}, unread = 4"),
        ),
        // 位置为空（世界说"你还没读过"）：`-`，未读照给。
        (
            read_cursor_body(true, None, 4.into()),
            "read = -, unread = 4".to_string(),
        ),
        // 形状读不懂（负数 / 非整数 / 位置类型不对）：`?`，**不写 0**。
        (
            read_cursor_body(true, None, (-1).into()),
            "read = -, unread = ?".to_string(),
        ),
        (
            read_cursor_body(true, None, "三条".into()),
            "read = -, unread = ?".to_string(),
        ),
        (
            read_cursor_body(true, None, 2.into()),
            "read = -, unread = 2".to_string(),
        ),
        // 整份 `read_state` 缺席：两栏都 `?`（读不到），仍是一次**成功**（2xx 已确认推进与否）。
        (
            serde_json::json!({"advanced": true, "request_id": "req"}).to_string(),
            "read = ?, unread = ?".to_string(),
        ),
    ] {
        let fake = FakeIm::spawn(vec![(200, body.clone())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            mark_read(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_MESSAGE, MESSAGE),
            ),
        );
        assert_eq!(outcome.state(), ResultState::Success, "{body}");
        assert!(
            text_of(&outcome).contains(&expected),
            "体 {body} 应渲染出 {expected:?}：{}",
            text_of(&outcome)
        );
    }
}

/// 失败面：本地形状先拒（零出网）；服务端的三个码各归各的分类；其余一律"没告诉我们"。
#[test]
fn the_mark_read_failure_table_maps_its_own_code() {
    // 形状非法 / 缺参数：**一次都不出网**。
    for params in [
        ActionParams::none().with(PARAM_ROOM, "r-1"),
        ActionParams::none().with(PARAM_MESSAGE, MESSAGE),
        ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_MESSAGE, "not-a-uuid"),
    ] {
        let fake = FakeIm::spawn(vec![]);
        let mut app = fake.app();
        let outcome = call(&mut app, mark_read(params.clone()));
        assert_eq!(outcome.state(), ResultState::Failure, "{params:?}");
        assert_eq!(outcome.failure_class(), Some("invalid-argument"));
        assert_eq!(fake.connections(), 0, "参数非法一次都不出网");
    }

    for (status, expected) in [
        (400u16, "not-a-read-target"),
        (401, "credential-rejected"),
        (404, "unknown-room"),
    ] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            mark_read(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_MESSAGE, MESSAGE),
            ),
        );
        assert_eq!(outcome.state(), ResultState::Failure, "HTTP {status}");
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
        assert!(
            text_of(&outcome).is_empty(),
            "失败不带正文块（状态码与响应体不外泄）"
        );
    }

    // 契约没列出的码、5xx、读不懂、以及 2xx 却读不出"推进了没有"：一律未确认（fail-closed）。
    for (status, body) in [
        (422u16, "{\"error\":\"synthetic\"}".to_string()),
        (500, "{\"error\":\"synthetic\"}".to_string()),
        (200, "this is not json".to_string()),
        (200, "{}".to_string()),
        (
            200,
            serde_json::json!({"read_state": {"last_read_message_id": null, "unread_count": 0}})
                .to_string(),
        ),
    ] {
        let fake = FakeIm::spawn(vec![(status, body.clone())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            mark_read(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_MESSAGE, MESSAGE),
            ),
        );
        assert_eq!(
            outcome.state(),
            ResultState::Unknown,
            "HTTP {status} / {body} 没有告诉我们这一次的提交结果"
        );
    }
}

// ---------------------------------------------------------------------------
// 6c. 退房（写，切片 M2）：形状、204 空体即成功、它自己那个码
// ---------------------------------------------------------------------------

fn leave_room(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_LEAVE_ROOM), params)
}

/// 退房：一次写 = 恰好一次请求（POST、**无体、无幂等键**）；**204 的空体就是成功**
/// ——这条端点按契约不回体，因此"读不出体"在这里不是"读不懂"。
#[test]
fn the_leave_room_call_is_a_bare_post_and_an_empty_204_is_success() {
    // 204 + **空体**：`exchange` 会把它归为 `Unreadable`（2xx 但体读不出），
    // 而本端点按契约就该是空的 ⇒ 成功。
    let fake = FakeIm::spawn(vec![(204, String::new())]);
    let mut app = fake.app();

    let outcome = call(
        &mut app,
        leave_room(ActionParams::none().with(PARAM_ROOM, "r-1")),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");

    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/rooms/r-1/leave "),
        "方法必须是 POST、路径必须是退房端点: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("idempotency-key"),
        "无体、无键（幂等由成员资格承担）: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("content-length"),
        "无体 ⇒ 不该有 content-length，也不该有体: {request:?}"
    );
    assert_eq!(fake.connections(), 1, "一次写 = 恰好一次请求（绝不重发）");

    let text = text_of(&outcome);
    assert_eq!(text, "room = r-1, left = yes", "{text}");

    // 200 + 空体同样是"被接受"（别的 2xx 也一样）。
    let fake = FakeIm::spawn(vec![(200, String::new())]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        leave_room(ActionParams::none().with(PARAM_ROOM, "r-2")),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    assert_eq!(text_of(&outcome), "room = r-2, left = yes");
}

/// 失败面：本地形状先拒（**零出网**）；服务端的三个码各归各的分类；其余一律未确认。
#[test]
fn the_leave_room_failure_table_maps_its_own_codes() {
    // 本地只判**形状的下限**（非空、不超长）——房标识是不是合法 UUID 由**服务端**裁决
    // （与其余房间动词同一纪律：本层不做存在性/形状的猜测）。
    for params in [
        ActionParams::none(),
        ActionParams::none().with(PARAM_ROOM, ""),
        ActionParams::none().with(PARAM_ROOM, "x".repeat(MAX_ID_BYTES + 1)),
    ] {
        let fake = FakeIm::spawn(vec![]);
        let mut app = fake.app();
        let outcome = call(&mut app, leave_room(params.clone()));
        assert_eq!(outcome.state(), ResultState::Failure, "{params:?}");
        assert_eq!(outcome.failure_class(), Some("invalid-argument"));
        assert_eq!(fake.connections(), 0, "参数非法一次都不出网");
    }

    for (room, status, expected) in [
        // 形状不是 UUID 的房标识：出网，由服务端回 404（不在这里猜）。
        ("not-a-uuid", 404u16, "unknown-room"),
        // 房不在 / 我不在里面（含**已经退过**）/ human_direct —— 同一个 404，不分解。
        ("r-1", 404, "unknown-room"),
        ("r-1", 401, "credential-rejected"),
        // 契约给这条端点列了 409；今天 Agent 不可能是 owner，但绝不吞成 unknown。
        ("r-1", 409, "owner-transfer-required"),
        ("r-1", 400, "invalid-argument"),
    ] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            leave_room(ActionParams::none().with(PARAM_ROOM, room)),
        );
        assert_eq!(outcome.state(), ResultState::Failure, "HTTP {status}");
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
        assert!(
            text_of(&outcome).is_empty(),
            "失败不带正文块（状态码与响应体不外泄）"
        );
    }

    // 契约没列出的码与传输断：一律"没告诉我们" ⇒ 未确认（fail-closed，绝不重发）。
    for (status, body) in [
        (422u16, "{\"error\":\"synthetic\"}".to_string()),
        (500, "{\"error\":\"synthetic\"}".to_string()),
        (503, "{\"error\":\"synthetic\"}".to_string()),
    ] {
        let fake = FakeIm::spawn(vec![(status, body.clone())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            leave_room(ActionParams::none().with(PARAM_ROOM, "r-1")),
        );
        assert_eq!(
            outcome.state(),
            ResultState::Unknown,
            "HTTP {status} 没有告诉我们这一次的提交结果"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. 只读投影（宿主那一侧用的入口——**不是主体的动词**）
// ---------------------------------------------------------------------------

/// 自锚以来"每间房几条新的"：**按事件定价**（TASK_73）——分母 ＋ 有新的那几间逐行 ＋ 余数，
/// 顺序确定、**一个字节正文都没有**。
#[test]
fn the_projection_states_every_room_in_a_deterministic_order() {
    let body = serde_json::json!({
        "rooms": [
            {"room_id": "r-1", "kind": "direct", "display_name": "乙房", "last_activity_at": null,
             "preview": "这段预览绝不该出现在投影里", "since_count": 2,
             "read_state": {"last_read_message_id": null, "unread_count": 0}},
            {"room_id": "r-2", "kind": "group", "display_name": "甲房", "last_activity_at": null,
             "preview": "也不该", "since_count": 3,
             "read_state": {"last_read_message_id": null, "unread_count": 0}},
            {"room_id": "r-3", "kind": "group", "display_name": "丙房", "last_activity_at": null,
             "preview": "更不该", "since_count": 0,
             "read_state": {"last_read_message_id": null, "unread_count": 0}},
            {"room_id": "r-4", "kind": "group", "display_name": "乙房", "last_activity_at": null,
             "preview": "同数同名时按名字排", "since_count": 2,
             "read_state": {"last_read_message_id": null, "unread_count": 0}}
        ],
        "request_id": "req-projection"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(200, body)]);
    let app = fake.app();

    let since =
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_789_560_000_000);
    let text = app.new_since(Some(since)).expect("投影读得出来");

    assert_eq!(
        text,
        "可见 4 间 · 有新 3 间\n甲房（r-2） = 3\n乙房（r-1） = 2\n乙房（r-4） = 2\n其余 1 间 = 0",
        "顺序必须确定（条数降序、同数按房名升序）；非零的**全部**逐行列出，余数说在末尾；\
         而且每行**带房号**——两间同名（都是「乙房」）只有房号分得开"
    );
    assert!(
        !text.contains("预览") && !text.contains("不该"),
        "投影里一个字节正文都没有: {text}"
    );

    let request = fake.next_request();
    assert!(
        request.starts_with("GET /v1/backend/rooms?since=1789560000000 "),
        "走的正是主体那条只读路径、带的是锚: {request}"
    );
    assert_eq!(fake.connections(), 1, "一次调用恰好一次请求");
}

/// **缺省不当零**：某间房没有 `since_count`、或它不是非负整数 ⇒ 整份回执**读不懂**
/// （`unreachable`，宿主侧读成 `counts = unknown`），**不**把它渲染成"这间房零新"。
///
/// 这是「缺省不等于零」那条教训在这一层的围栏：把"字段缺了"说成"零条新的"就是在撒谎，
/// 而主体正是拿这份回执决定"要不要去看"。
#[test]
fn a_projection_row_without_a_count_is_unreadable_not_zero() {
    for row in [
        serde_json::json!({"room_id": "r-1", "kind": "direct", "display_name": "值班室",
                           "last_activity_at": null, "preview": null}),
        serde_json::json!({"room_id": "r-2", "kind": "direct", "display_name": "值班室",
                           "last_activity_at": null, "preview": null, "since_count": -1}),
        serde_json::json!({"room_id": "r-3", "kind": "direct", "display_name": "值班室",
                           "last_activity_at": null, "preview": null, "since_count": "两条"}),
    ] {
        let body = serde_json::json!({"rooms": [row], "request_id": "req"}).to_string();
        let fake = FakeIm::spawn(vec![(200, body)]);
        let app = fake.app();
        let error = app
            .new_since(Some(std::time::SystemTime::UNIX_EPOCH))
            .expect_err("缺了计数就该读不懂，而不是当成零");
        assert_eq!(error.as_str(), "unreachable");
    }
}

/// 房间都在、但个个零新 ⇒ **一行说尽**：分母 ＋ "全部 = 0"（不是空白回执，也不再逐间点名零）。
///
/// **契约变更（TASK_73）**：零被说出口这条**意图**照旧（"没有新的"必须是看得见的事实），
/// 但形态从"每间一行零"升级成"一行说尽全世界的安静"——否则安静的一天要为「无事」按房间数付税。
/// 缺席消歧仍在：分母（可见 N 间）在场，未出现的房就是零新。
#[test]
fn an_empty_projection_says_the_whole_world_is_quiet_in_one_line() {
    let fake = FakeIm::spawn(vec![(
        200,
        serde_json::json!({
            "rooms": [
                {"room_id": "r-1", "kind": "direct", "display_name": "清静间",
                 "last_activity_at": null, "preview": null, "since_count": 0,
                 "read_state": {"last_read_message_id": null, "unread_count": 0}},
                {"room_id": "r-2", "kind": "group", "display_name": "值班室",
                 "last_activity_at": null, "preview": null, "since_count": 0,
                 "read_state": {"last_read_message_id": null, "unread_count": 0}}
            ],
            "request_id": "req-empty"
        })
        .to_string(),
    )]);
    let app = fake.app();
    let text = app
        .new_since(Some(std::time::SystemTime::UNIX_EPOCH))
        .expect("读得出来只是没有新的");
    assert_eq!(
        text, "可见 2 间 · 全部 = 0",
        "没有新的也是**看得见的事实**：一行说尽（分母 ＋ 全部 = 0），而不是一片空白"
    );
    assert_eq!(text.lines().count(), 1, "安静日**一行**，不随房间数走");
}

/// **两半各有各的锚**（2026-10-10 裁决）：未读那一半按**世界记着的读位置**算，**它不需要锚**。
///
/// 从前通知栏只答"自锚以来有几条新的"，"有几条等你回的"看不见——而契约 §3.2 的 D3 记账把这两件
/// 分开说（`read_state` 与 `since_count`，**两个锚、不合并**），§3.1 又明写 `since` **可选**。
/// ⇒ 没有锚时照样答得出来，而那正是**刚重启 / 第一次开机**那一段——从前那一整块是失明的。
#[test]
fn the_projection_states_the_unread_half_even_without_an_anchor() {
    let body = serde_json::json!({
        "rooms": [
            {"room_id": "r-1", "kind": "direct", "display_name": "Inori",
             "last_activity_at": null, "preview": null, "since_count": 0,
             "read_state": {"last_read_message_id": null, "unread_count": 2}},
            {"room_id": "r-2", "kind": "group", "display_name": "清静间",
             "last_activity_at": null, "preview": null, "since_count": 0,
             "read_state": {"last_read_message_id": null, "unread_count": 0}}
        ],
        "request_id": "req-no-anchor"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(200, body)]);
    let app = fake.app();

    let text = app.new_since(None).expect("没有锚也读得出来");

    assert_eq!(
        text, "可见 2 间\n未读 1 间：Inori（r-1） = 2",
        "没有锚 ⇒ 增量那一半**问不出来**（不编一个 0），未读那一半照给，而且**带上房号**"
    );
    let request = fake.next_request();
    assert!(
        !request.contains("since="),
        "没有锚就不带那个参数（契约 §3.1：缺省 = 无增量基线）: {request}"
    );
    assert_eq!(fake.connections(), 1, "一次调用恰好一次请求");
}

/// 有锚时，未读那一半**只在真有未读时出场**——没出场就是"都读了"（与"没出现在名单里的房就是
/// 零新"同一条缺席消歧，安静日仍然一行）。而**读不到**由整份投影那一层表达，**不冒充零**。
#[test]
fn the_unread_half_appears_only_when_something_is_unread() {
    let body_with = |unread: i64| {
        serde_json::json!({
            "rooms": [
                {"room_id": "r-1", "kind": "direct", "display_name": "值班室",
                 "last_activity_at": null, "preview": null, "since_count": 1,
                 "read_state": {"last_read_message_id": null, "unread_count": unread}}
            ],
            "request_id": "req-unread"
        })
        .to_string()
    };
    let read = |unread: i64| {
        let fake = FakeIm::spawn(vec![(200, body_with(unread))]);
        let app = fake.app();
        app.new_since(Some(std::time::SystemTime::UNIX_EPOCH))
            .expect("读得出来")
    };

    assert_eq!(
        read(0),
        "可见 1 间 · 有新 1 间\n值班室（r-1） = 1",
        "未读为零 ⇒ 那一行不出场（安静日仍然一行）"
    );
    assert_eq!(
        read(4),
        "可见 1 间 · 有新 1 间\n值班室（r-1） = 1\n未读 1 间：值班室（r-1） = 4",
        "有未读 ⇒ 另起一行、**另一个锚**"
    );

    // **缺省不当零**：世界没给 `read_state` ⇒ 整份读不懂（不冒充"未读零"）。
    let fake = FakeIm::spawn(vec![(
        200,
        serde_json::json!({
            "rooms": [{"room_id": "r-1", "kind": "direct", "display_name": "值班室",
                       "last_activity_at": null, "preview": null, "since_count": 0}],
            "request_id": "req"
        })
        .to_string(),
    )]);
    let error = fake
        .app()
        .new_since(Some(std::time::SystemTime::UNIX_EPOCH))
        .expect_err("缺了 read_state 就该读不懂，而不是当成未读零");
    assert_eq!(error.as_str(), "unreachable");
}

/// **尺寸定律（TASK_73）**：快照的大小随**非零房间数**走，不随**世界大小**走。
///
/// 关系式断言（不写固定字节锚点）：世界规模只出现在**分母**里，正文体量只跟"有多少间真有新的"有关。
/// 这就是"按事件定价"——安静的一天不该为「无事」按房间数付税（五百间付五百行、上千间撞批次上限
/// ⇒ 整块"超限未给"，通知栏恰在主体最需要它的时刻失明）。
#[test]
fn the_projection_is_priced_by_events_not_by_the_size_of_the_world() {
    /// 一份投影回执：`visible` 间房，**前 `news` 间**各有 `count` 条新的，其余为零。
    fn projection_body(visible: usize, news: usize, count: i64) -> String {
        let rooms: Vec<serde_json::Value> = (0..visible)
            .map(|index| {
                serde_json::json!({
                    "room_id": format!("r-{index}"),
                    "kind": "group",
                    "display_name": format!("房{index:03}"),
                    "last_activity_at": null,
                    "preview": null,
                    "since_count": if index < news { count } else { 0 },
                    "read_state": {"last_read_message_id": null, "unread_count": 0}
                })
            })
            .collect();
        serde_json::json!({"rooms": rooms, "request_id": "req-size"}).to_string()
    }

    fn read(body: String) -> String {
        let fake = FakeIm::spawn(vec![(200, body)]);
        let app = fake.app();
        app.new_since(Some(std::time::SystemTime::UNIX_EPOCH))
            .expect("投影读得出来")
    }

    // ① 安静日：一行说尽，房间数只贡献**分母的位数**。
    let quiet_small = read(projection_body(5, 0, 0));
    let quiet_big = read(projection_body(500, 0, 0));
    assert_eq!(quiet_small.lines().count(), 1);
    assert_eq!(
        quiet_big.lines().count(),
        1,
        "五百间全零 ⇒ **也只有一行**（不是五百零一行）: {quiet_big}"
    );
    assert_eq!(
        quiet_big.len() - quiet_small.len(),
        "500".len() - "5".len(),
        "世界的规模只贡献分母那几位数字，不进正文体量"
    );
    assert_eq!(quiet_big, "可见 500 间 · 全部 = 0");

    // ② 忙日：正文行数只跟非零房间数有关（三间有新的 ⇒ 三行），与总房间数无关。
    let (visible, news) = (500usize, 3usize);
    let busy_big = read(projection_body(visible, news, 2));
    let busy_small = read(projection_body(5, news, 2));
    assert_eq!(
        busy_big.lines().count(),
        1 + news + 1,
        "分母 ＋ 有新的那几间 ＋ 余数: {busy_big}"
    );
    let body_of = |text: &str| {
        text.lines()
            .skip(1)
            .take(news)
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        body_of(&busy_small),
        body_of(&busy_big),
        "正文（逐行的非零清单）不随世界大小变"
    );

    // ③ 缺席消歧照旧（TASK_28 的意图）：分母在 ⇒ 名单里没出现的房就是**零新**。
    let header = busy_big.lines().next().expect("有分母行");
    let tail = busy_big.lines().last().expect("有余数行");
    assert_eq!(header, format!("可见 {visible} 间 · 有新 {news} 间"));
    for index in 0..news {
        assert!(
            busy_big.contains(&format!("房{index:03}（r-{index}） = 2")),
            "有新的房**一个不漏**地在名单里（**带房号**——名字给人读、号码给手用）: {busy_big}"
        );
    }
    assert!(
        !busy_big.contains("房003"),
        "零新的房不再点名（契约变更），由余数那句话承担: {busy_big}"
    );
    assert_eq!(tail, format!("其余 {} 间 = 0", visible - news));

    // ④ 同一份回执两次渲染**逐字节相同**。
    assert_eq!(
        read(projection_body(visible, news, 2)),
        busy_big,
        "同数据同渲染"
    );
}

/// **每间都有新的（`k = N`）⇒ 不再补余数那一行**（裁决 ②，TASK_73 补丁）。
///
/// 名单本身就是全量，`其余 0 间 = 0` 只是把一件已经说完的事再说一遍。安静日（`k = 0`）与空世界
/// （`N = 0`）的形态不受这条影响——那两处的"零"是**唯一**被说出口的东西，不能省。
#[test]
fn a_projection_where_every_room_has_news_stops_certifying_the_remainder() {
    let body = serde_json::json!({
        "rooms": [
            {"room_id": "r-1", "kind": "direct", "display_name": "乙房", "last_activity_at": null,
             "preview": null, "since_count": 2,
             "read_state": {"last_read_message_id": null, "unread_count": 0}},
            {"room_id": "r-2", "kind": "group", "display_name": "甲房", "last_activity_at": null,
             "preview": null, "since_count": 3,
             "read_state": {"last_read_message_id": null, "unread_count": 0}},
            {"room_id": "r-3", "kind": "group", "display_name": "丙房", "last_activity_at": null,
             "preview": null, "since_count": 1,
             "read_state": {"last_read_message_id": null, "unread_count": 0}}
        ],
        "request_id": "req-all-busy"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(200, body)]);
    let app = fake.app();
    let text = app
        .new_since(Some(std::time::SystemTime::UNIX_EPOCH))
        .expect("投影读得出来");

    assert_eq!(
        text, "可见 3 间 · 有新 3 间\n甲房（r-2） = 3\n乙房（r-1） = 2\n丙房（r-3） = 1",
        "分母 ＋ 名单已经说尽（非零全列、机械排序），余数那句不再出现"
    );
    assert!(!text.contains("其余"), "k = N 时不给余数行: {text}");
    assert_eq!(
        text.lines().count(),
        1 + 3,
        "行数 = 分母 ＋ 每间一行（没有余数行）: {text}"
    );
}

/// 读不出来就是**失败分类**（宿主据此把它读成 `unknown`，而不是"没有新的"）。
#[test]
fn a_projection_that_cannot_be_read_is_a_failure_not_a_zero() {
    for (status, expected) in [
        (401u16, "credential-rejected"),
        (404, "unknown-room"),
        (500, "server-error"),
        // 读侧对"200 却读不懂"的既有口径就是"读不出"（`unreachable`）——与运输层失败同类，
        // 因此它**不是** `total = 0`（宿主据 Err 读成 `unknown`，不冒充"没有新的"）。
        (200, "unreachable"),
    ] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let app = fake.app();
        let error = app
            .new_since(Some(std::time::SystemTime::UNIX_EPOCH))
            .expect_err("这一次没有答案");
        assert_eq!(error.as_str(), expected, "HTTP {status}");
    }
}

/// 投影**不是主体的动词**：它不在能力目录里（宿主用它，主体的工具面一个字节都不多）。
#[test]
fn the_projection_is_not_one_of_her_verbs() {
    let fake = FakeIm::spawn(vec![]);
    let app = fake.app();
    let social = fake.social_app();
    let profile = fake.profile_app();
    // 拆包后：投影仍然**不在任何一个包**的目录里（宿主用它，主体的工具面一个字节都不多）。
    let mut ids = declared_ids(&app);
    ids.extend(declared_ids(&social));
    ids.extend(declared_ids(&profile));
    let mut expected = strings(&DIRECTORY_BEFORE_THE_SPLIT);
    expected.extend(strings(&ADDED_AFTER_THE_SPLIT));
    ids.sort();
    expected.sort();
    assert_eq!(ids, expected, "投影不增加主体的动词（本刀那一条已点名）");
    assert!(
        !ids.iter()
            .any(|id| id.contains("since") || id.contains("new")),
        "主体的目录里没有「看新的」这种动词: {ids:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. 附件（TASK_65）：读 / 递 / 复核
//
// 门禁里的**判错即红四条**都落在这一节：
//   ① 类型白名单拒绝（读）——清单外的类型，一个字节都不进视野；
//   ② 超限响亮失败——不截断、不静默丢；
//   ③ 复核不误判——带附件的消息不会被"只按正文找"的那次复核认领；
//   ④ 注入路径走现状——结果只经能力名那条路回、不缓存（读两次要问两次）、主体那边一个字节都不留；
//      而"读到的内容在**这一次 Run 的后续轮次**里还在"，由 manual_entry 的 TASK_66 钉子用例守着。
// ---------------------------------------------------------------------------

/// 一条**我发的、带附件**的历史消息（复核用例的舞台）。
fn history_with_my_attachment(name: &str, size: u64) -> String {
    history_rows(serde_json::json!([{
        "id": MESSAGE,
        "room_id": "r-1",
        "actor": {"id": IM_PARTICIPANT, "kind": "agent", "display_name": "小满", "status": "active"},
        "body": "带文件的问候",
        "mentions": [],
        "attachments": [{
            "id": ATTACHMENT,
            "file_name": name,
            "content_type": "text/markdown",
            "size_bytes": size
        }],
        "reply_to_message_id": null,
        "created_at": "2026-09-16T11:00:00+00:00",
        "recalled_at": null
    }]))
}

/// 一条**我发的、没有附件**的历史消息（负对照：这一条纯正文复核**应当**找到）。
fn history_with_my_message() -> String {
    history_rows(serde_json::json!([{
        "id": MESSAGE,
        "room_id": "r-1",
        "actor": {"id": IM_PARTICIPANT, "kind": "agent", "display_name": "小满", "status": "active"},
        "body": "带文件的问候",
        "mentions": [],
        "attachments": [],
        "reply_to_message_id": null,
        "created_at": "2026-09-16T11:00:00+00:00",
        "recalled_at": null
    }]))
}

/// 一条历史消息的舞台，带可覆盖的字段（TASK_67 的三条补漏各要改一处）。
fn history_with_extra(extra: serde_json::Value) -> String {
    let mut message = serde_json::json!({
        "id": MESSAGE,
        "room_id": "r-1",
        "actor": {"id": IM_PARTICIPANT, "kind": "agent", "display_name": "小满", "status": "active"},
        "body": "带文件的问候",
        "mentions": [],
        "attachments": [],
        "reply_to_message_id": null,
        "created_at": "2026-09-16T11:00:00+00:00",
        "recalled_at": null
    });
    if let (Some(target), Some(patch)) = (message.as_object_mut(), extra.as_object()) {
        for (key, value) in patch {
            target.insert(key.clone(), value.clone());
        }
    }
    history_rows(serde_json::json!([message]))
}

/// 把一份历史响应喂进去，取回主体的投影正文。
fn projection_of(body: &str) -> String {
    // ⚠ 2026-10-10：一次读历史 = 读那一页 ＋ **顺带推进读位置**两次请求 ⇒ 两条回执都要喂
    // （否则第二次会落到"队列空了 ⇒ 500"，用例就在一条**失败**的推进上验投影——那是假绿）。
    let fake = FakeIm::spawn(vec![(200, body.to_string()), (200, cursor_advanced_body())]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        history(
            ActionParams::none()
                .with(PARAM_ROOM, "6f0f4f3a-1111-4111-8111-111111111111")
                .with(PARAM_LIMIT, "2"),
        ),
    );
    text_of(&outcome)
}

#[test]
fn the_projection_names_the_participants_a_message_mentions() {
    // 审计表 #2（与附件那处**同款**的漏）：人类那边 @ 是**结构化 id 列表**，投影此前只给
    // `mentions = n` 这个计数——主体看得到"有人被点名"，却不知道是**谁**。
    // 判错：拿掉逐条的 `mention` 行 ⇒ 这条红（计数还在，所以只有这条会红）。
    const FIRST: &str = "11111111-1111-4111-8111-111111111111";
    const SECOND: &str = "22222222-2222-4222-8222-222222222222";
    let text = projection_of(&history_with_extra(serde_json::json!({
        "mentions": [FIRST, SECOND]
    })));
    assert!(
        text.contains(&format!("\tmention\t0\t{FIRST}")),
        "第一个被点名的人要按位置逐条现身: {text}"
    );
    assert!(
        text.contains(&format!("\tmention\t1\t{SECOND}")),
        "第二个也要，顺序就是集合顺序: {text}"
    );
    assert!(
        text.contains("\tmentions = 2"),
        "既有计数保留（旧读者不受影响）: {text}"
    );

    // 没有人被点名 ⇒ 一条 `mention` 行都不该出现（不编空位）。
    let text = projection_of(&history_with_extra(serde_json::json!({ "mentions": [] })));
    assert!(
        !text.contains("\tmention\t"),
        "没人被点名就没有这一格: {text}"
    );
}

#[test]
fn the_projection_says_which_message_a_reply_targets() {
    // 审计表 #1：人类 UI 靠 `reply_to_message_id` 画引用块；投影此前**完全不出现**这个字段——
    // 主体只能靠正文猜"这条在回谁"。判错：拿掉行尾那截 `reply_to = …` ⇒ 这条红。
    const TARGET: &str = "632474f3-95c0-4adb-9649-30209ac5012d";
    let text = projection_of(&history_with_extra(serde_json::json!({
        "reply_to_message_id": TARGET
    })));
    assert!(
        text.contains(&format!("\treply_to = {TARGET}")),
        "回复目标必须现身: {text}"
    );

    // 不是回复 ⇒ 不出现这一格（不编 `reply_to = -`）。
    let text = projection_of(&history_with_extra(serde_json::json!({
        "reply_to_message_id": null
    })));
    assert!(!text.contains("reply_to"), "不是回复就没有这一格: {text}");
}

#[test]
fn the_projection_says_when_a_message_was_recalled() {
    // 审计表 #3：人类那边撤回是一件**有时间**的公共事实；投影此前只给裸布尔 `recalled`。
    // 判错：把它改回裸 `recalled`（不写时间）⇒ 这条红。
    let text = projection_of(&history_with_extra(serde_json::json!({
        "body": null,
        "recalled_at": "2026-09-16T11:01:00+00:00"
    })));
    // ⚠ 撤回时点是**世界给的时刻** ⇒ 投影里摆的是**本机刻度**（与人那边 `toLocaleString()` 同一步）。
    // 期望值因此按同一台机器算出来，而不是写死一个偏移（写死它就只有 +08:00 的机器才绿）。
    assert!(
        text.contains(&format!(
            "\trecalled = {}",
            celira_agent::clock::localize_rfc3339("2026-09-16T11:01:00+00:00")
        )),
        "撤回要带上发生的时间: {text}"
    );
    assert!(
        !text.contains("body = "),
        "撤回后不拿伪正文占位（既有纪律）: {text}"
    );
}

#[test]
fn the_projection_carries_the_author_identity_not_only_the_name() {
    // 审计表 #6：**名字不是身份**（项目自己的原则）——人类 UI 用 `actor.id` 区分同名者、
    // 判断"连续同一个人"。投影此前只给 display_name。判错：拿掉 `actor = …` ⇒ 这条红。
    const OTHER: &str = "33333333-3333-4333-8333-333333333333";
    let first = projection_of(&history_with_extra(serde_json::json!({})));
    assert!(
        first.contains(&format!("\tactor = {IM_PARTICIPANT}")),
        "作者身份（稳定 id）必须在: {first}"
    );
    assert!(
        first.contains("\tagent\t小满\t"),
        "既有的 kind 与显示名列**形状不变**（新读数只追加）: {first}"
    );

    // **同名不同人**：两条消息显示名一样，但身份分得开——这正是 id 存在的理由。
    let same_name = serde_json::json!({
        "id": MESSAGE,
        "room_id": "r-1",
        "actor": {"id": OTHER, "kind": "agent", "display_name": "小满", "status": "active"},
        "body": "带文件的问候",
        "mentions": [],
        "attachments": [],
        "reply_to_message_id": null,
        "created_at": "2026-09-16T11:00:00+00:00",
        "recalled_at": null
    });
    let second = projection_of(&history_rows(serde_json::json!([same_name])));
    assert!(
        second.contains(&format!("\tactor = {OTHER}")),
        "同名者靠身份分得开: {second}"
    );
    // ⚠️ 判据落在**消息行**上，不按行号：回执开头现在有身份那行（TASK_71 第 4 件），
    // 而且探不到身份时它还会多一行说明 ⇒ 行号不再是稳定读数，**形状**才是。
    let message_row = |text: &str| {
        text.lines()
            .find(|line| line.contains("\tagent\t"))
            .unwrap_or_default()
            .to_string()
    };
    assert_ne!(
        message_row(&first),
        message_row(&second),
        "显示名相同而身份不同的两条，投影的消息行必须不同"
    );
}

fn history_rows(messages: serde_json::Value) -> String {
    serde_json::json!({
        "messages": messages,
        "next_before": null,
        "request_id": "req-att"
    })
    .to_string()
}

/// 一次上传成功的回执体（契约 §8 的形状：201 只给元数据与 `state = pending`）。
fn upload_body(id: &str) -> String {
    serde_json::json!({
        "attachment": {
            "id": id,
            "file_name": "hello.md",
            "content_type": "text/markdown",
            "size_bytes": 8,
            "state": "pending",
            "uploaded_at": "2026-09-16T11:00:00+00:00"
        },
        "request_id": "req-up"
    })
    .to_string()
}

/// 一个目录的机械快照（相对路径 → 字节数）：用来证明"什么都没留下"。
fn snapshot(root: &Path) -> Vec<(String, u64)> {
    let mut items = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).expect("可以读目录") {
            let entry = entry.expect("可以读条目");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("在根之下")
                    .to_string_lossy()
                    .replace('\\', "/");
                items.push((relative, entry.metadata().expect("有元数据").len()));
            }
        }
    }
    items.sort();
    items
}

fn attachment_params() -> ActionParams {
    ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_ATTACHMENT, ATTACHMENT)
}

/// 一条最小的 PNG 头：签名 ＋ IHDR（长度 ＋ 类型 ＋ 宽高）。**字节很小而像素很多**——正是
/// "拿 `size_bytes` 当事前门就是错的物理量"那个形状。
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes
}

/// **媒体形态的读取路**（2026-10-17）：清单里有没有媒体类，由**这一次装配用的模型**决定。
///
/// 四条判据一起钉：① **不声明视觉 ⇒ 与从前逐字节相同**（`unsupported-content-type`）；
/// ② 声明了 ⇒ 回执给**引用**（`form = media`，没有「行」这回事），而**字节随块出门**；
/// ③ **感知体量从文件头读**，面积超限**当场拒绝**（与"通道字节超限"不是同一道门）；
/// ④ 尺寸读不出来 ⇒ 如实写 `unknown`，**不拒绝也不猜**；对媒体给 `offset` ⇒ `invalid-argument`。
#[test]
fn a_media_attachment_is_readable_only_when_the_model_can_take_it() {
    let png = png_bytes(1200, 800);

    // ① 不声明视觉：与从前逐字节相同——清单里没有媒体类。
    let blind = FakeIm::spawn_scripted(vec![Scripted::raw(200, "image/png", &png)]);
    let mut app = blind.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some("unsupported-content-type"),
        "不声明视觉 ⇒ 图片读不动（与从前一致）"
    );

    // ② 声明了视觉：回执给引用（含**感知体量**），字节随块出门。
    let sighted = FakeIm::spawn_scripted(vec![Scripted::raw(200, "image/png", &png)]);
    let mut app = sighted.app_with_vision(true);
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(text.contains("content_type = image/png"), "{text}");
    assert!(
        text.contains("form = media, read = whole, more = none"),
        "媒体没有「行」这回事，整份就是整份: {text}"
    );
    assert!(
        !text.contains("lines ="),
        "媒体形态不得给文本那一套分段读数: {text}"
    );
    assert!(
        text.contains("pixels = 1200x800"),
        "感知体量必须从文件头读出来（不是 size_bytes）: {text}"
    );
    let material = outcome.content()[0]
        .material()
        .expect("字节随块出门（只进这一次请求）");
    assert_eq!(material.content_type(), "image/png");
    assert_eq!(material.bytes(), png, "字节逐字节相同");

    // ③ 面积超限：当场拒绝，而且读数说清撞的是**哪一道门**。
    //    注意这一份字节数与上面**完全相同**（都是 24 字节的头）——差别只在像素数，
    //    所以它同时证明了"拿字节当事前门"会漏掉这一张。
    let huge = png_bytes(9000, 9000);
    assert_eq!(huge.len(), png.len(), "两份字节数相同，只有像素数不同");
    let over = FakeIm::spawn_scripted(vec![Scripted::raw(200, "image/png", &huge)]);
    let mut app = over.app_with_vision(true);
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("too-large"));
    let refusal = text_of(&outcome);
    assert!(
        refusal.contains("refused = perception-limit"),
        "读数要说清撞的是感知体量那道门: {refusal}"
    );
    assert!(refusal.contains("pixels = 9000x9000"), "{refusal}");

    // ④ 尺寸读不出来：如实写 `unknown`，**不拒绝、也不猜一个数**。
    let truncated = &png[..12];
    let unknown = FakeIm::spawn_scripted(vec![Scripted::raw(200, "image/png", truncated)]);
    let mut app = unknown.app_with_vision(true);
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Success);
    assert!(
        text_of(&outcome).contains("pixels = unknown"),
        "读不出来就说读不出来: {}",
        text_of(&outcome)
    );

    // ⑤ 对媒体给 offset：明确失败（不静默忽略——静默忽略会让人以为"读到的是某一段"）。
    let third = FakeIm::spawn_scripted(vec![Scripted::raw(200, "image/png", &png)]);
    let mut app = third.app_with_vision(true);
    let outcome = call(
        &mut app,
        read_attachment(attachment_params().with(PARAM_ATTACHMENT_OFFSET, "2")),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));
}

#[test]
fn reading_an_attachment_brings_its_bytes_into_this_turn() {
    let body = "第一行\n第二行\n";
    let fake = FakeIm::spawn_scripted(vec![Scripted::text(body)]);
    let mut app = fake.app();

    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Success);
    assert!(outcome.failure_class().is_none());

    let request = fake.next_request();
    assert!(
        request.starts_with(&format!(
            "GET /v1/backend/rooms/r-1/attachments/{ATTACHMENT} "
        )),
        "读的是那个附件: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {SENTINEL}").to_ascii_lowercase()),
        "凭据只走请求头: {request}"
    );
    assert_eq!(fake.connections(), 1, "一次调用恰好一次请求");

    let text = text_of(&outcome);
    assert!(text.contains("content_type = text/plain"), "{text}");
    assert!(
        text.contains(&format!("size = {}", body.len())),
        "读数说的是这一次拿到的总字节: {text}"
    );
    assert!(
        text.contains("lines = 3, read = 1-3, more = none"),
        "窗口读数必须说出共几行、已读哪几行、怎么继续: {text}"
    );
    assert!(
        text.contains(&format!("attachment = {ATTACHMENT}")),
        "引用回执必须带可重取的身份: {text}"
    );
    assert!(text.ends_with(body), "正文原样搬、不摘要不改写: {text}");
}

#[test]
fn the_reference_receipt_carries_the_name_she_brought() {
    // 文件名不在下载响应里（§8：响应头是固定值）——主体把从附件清单里看到的名字带进来，
    // 工具**原样写回**回执（不校验：名字不是身份，身份是 attachment）。
    let fake = FakeIm::spawn_scripted(vec![Scripted::text("一行\n")]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        read_attachment(attachment_params().with(PARAM_FILE_NAME, "notes/hello.md")),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(text.contains("file_name = notes/hello.md"), "{text}");
    assert!(
        text.find("file_name =").expect("有序") < text.find("lines =").expect("有序"),
        "引用回执在读数块里、正文之前: {text}"
    );

    // 不带就没有那一行（不编一个名字出来）。
    let fake = FakeIm::spawn_scripted(vec![Scripted::text("一行\n")]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert!(!text_of(&outcome).contains("file_name ="));
}

#[test]
fn a_type_she_cannot_read_is_refused_and_never_reaches_her_view() {
    // ① 判错即红：白名单拒绝。
    let fake = FakeIm::spawn_scripted(vec![Scripted::raw(200, "image/png", b"\x89PNG binary")]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(
        outcome.failure_class(),
        Some("unsupported-content-type"),
        "清单外的类型不读"
    );
    assert!(
        !text_of(&outcome).contains("PNG"),
        "失败结果里一个字节都不许出现"
    );

    // 缺 Content-Type 也算这一格：没有类型就没有"这是文本"的证据。
    let fake = FakeIm::spawn_scripted(vec![Scripted::Raw {
        status: 200,
        content_type: None,
        body: b"text without a type".to_vec(),
    }]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.failure_class(), Some("unsupported-content-type"));

    // Content-Type 带参数（`; charset=utf-8`）仍然是同一个类型 ⇒ 过（负对照）。
    let fake = FakeIm::spawn_scripted(vec![Scripted::raw(
        200,
        "text/plain; charset=utf-8",
        b"hello",
    )]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Success, "参数不改变类型本身");
}

#[test]
fn an_oversized_attachment_is_truncated_loudly_with_a_way_to_continue() {
    // ② 判错即红：超限 ⇒ **响亮截断 + 继续读的位置在场**（改成静默截断——不写 read/more——
    // 这条就红；改成"整份拒绝"也红，因为那时 state 是 failure）。
    let lines: Vec<String> = (1..=(ATTACHMENT_WINDOW_LINES + 3))
        .map(|index| format!("第 {index} 行"))
        .collect();
    let body = lines.join("\n");
    let fake = FakeIm::spawn_scripted(vec![Scripted::text(&body)]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "超限不是失败，是截断"
    );
    let text = text_of(&outcome);
    assert!(
        text.contains(&format!(
            "lines = {}, read = 1-{ATTACHMENT_WINDOW_LINES}, more = offset {}",
            ATTACHMENT_WINDOW_LINES + 3,
            ATTACHMENT_WINDOW_LINES + 1
        )),
        "必须明示共 N、已读 A–B、怎么继续: {text}"
    );
    assert!(text.contains(&format!("第 {ATTACHMENT_WINDOW_LINES} 行")));
    assert!(
        !text.contains(&format!("第 {} 行", ATTACHMENT_WINDOW_LINES + 1)),
        "窗口外的行不在这一段的正文里"
    );

    // 用回执给的位置接着读：拿到剩下的，并且如实说读到底了。
    let fake = FakeIm::spawn_scripted(vec![Scripted::text(&body)]);
    let mut app = fake.app();
    let next = (ATTACHMENT_WINDOW_LINES + 1).to_string();
    let outcome = call(
        &mut app,
        read_attachment(attachment_params().with(PARAM_ATTACHMENT_OFFSET, next.as_str())),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(
        text.contains(&format!(
            "lines = {}, read = {}-{}, more = none",
            ATTACHMENT_WINDOW_LINES + 3,
            ATTACHMENT_WINDOW_LINES + 1,
            ATTACHMENT_WINDOW_LINES + 3
        )),
        "第二段读到底: {text}"
    );
    assert!(text.contains(&format!("第 {} 行", ATTACHMENT_WINDOW_LINES + 3)));

    // 字节上限也拦得住：2000 行 × 每行 100 字节 ≈ 200 KB > 50 KB ⇒ 按字节停，仍给继续位置。
    let wide: Vec<String> = (1..=ATTACHMENT_WINDOW_LINES)
        .map(|index| format!("{index:0>4}{}", "字".repeat(48)))
        .collect();
    let body = wide.join("\n");
    let fake = FakeIm::spawn_scripted(vec![Scripted::text(&body)]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(
        text.contains(&format!("lines = {ATTACHMENT_WINDOW_LINES}, read = 1-")),
        "{text}"
    );
    assert!(
        !text.contains("more = none"),
        "50 KB 上限先到 ⇒ 必须还有下一段: {text}"
    );
}

#[test]
fn an_overlong_line_is_truncated_in_place_and_says_how_long_it_was() {
    // 单行超 2000 字符 ⇒ 就地截断 + 标注（**不静默**少给半行）。
    let long = "甲".repeat(ATTACHMENT_LINE_CHARS + 500);
    let body = format!("短行\n{long}\n末行");
    let fake = FakeIm::spawn_scripted(vec![Scripted::text(&body)]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(
        text.contains(&format!(
            "…[line truncated: {} chars]",
            ATTACHMENT_LINE_CHARS + 500
        )),
        "截断必须标注这一行本来多长: {text}"
    );
    assert!(
        text.contains("lines = 3, read = 1-3, more = none"),
        "行的结构不变（截断的是行内容，不是行数）: {text}"
    );
    assert!(
        text.len() < ATTACHMENT_WINDOW_BYTES + 8 * 1024,
        "截断之后这一段仍然有界（实测 {} 字节）",
        text.len()
    );
}

#[test]
fn a_body_that_is_not_text_is_refused_not_replaced() {
    let fake = FakeIm::spawn_scripted(vec![Scripted::raw(
        200,
        "text/plain",
        &[0xff, 0xfe, 0x00, 0x01],
    )]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(
        outcome.failure_class(),
        Some("not-text"),
        "非 UTF-8 不用替换字符充数"
    );
}

#[test]
fn an_unreadable_attachment_is_one_answer_without_an_existence_oracle() {
    // 服务端把 pending / 未知 / 跨房 / 跨人 / 撤回 / 非成员合成同一个 404——这一层也不分解它。
    let fake = FakeIm::spawn(vec![(404, "{}".to_string())]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.failure_class(), Some("not-found"));

    let fake = FakeIm::spawn(vec![(401, "{}".to_string())]);
    let mut app = fake.app();
    let outcome = call(&mut app, read_attachment(attachment_params()));
    assert_eq!(outcome.failure_class(), Some("credential-rejected"));
}

#[test]
fn saying_with_a_file_uploads_then_binds_it() {
    let world_root = world("say-file");
    put(&world_root, "notes/hello.md", b"# hello\n");

    let fake = FakeIm::spawn_scripted(vec![
        Scripted::Json(201, upload_body(ATTACHMENT)),
        Scripted::Json(201, say_body(MESSAGE, false)),
    ]);
    let mut app = fake.app_with_world(&world_root);

    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "给你一份笔记")
            .with(PARAM_FILE, "notes/hello.md")),
    );
    assert_eq!(outcome.state(), ResultState::Success);

    // 第一步：上传（multipart，恰好一个 file part，**不带**幂等键）。
    let upload = fake.next_request();
    assert!(
        upload.starts_with("POST /v1/backend/rooms/r-1/attachments "),
        "先上传: {upload}"
    );
    let lowered = upload.to_ascii_lowercase();
    assert!(
        lowered.contains("content-type: multipart/form-data; boundary="),
        "{upload}"
    );
    assert!(upload.contains("name=\"file\""), "{upload}");
    assert!(upload.contains("filename=\"hello.md\""), "{upload}");
    assert!(upload.contains("Content-Type: text/markdown"), "{upload}");
    assert!(upload.contains("# hello"), "文件字节原样递上去: {upload}");
    assert!(
        !lowered.contains("idempotency-key"),
        "契约 §8：上传**不带**幂等键: {upload}"
    );

    // 第二步：发消息（带幂等键 + 那一个附件身份）。
    let sent = fake.next_request();
    assert!(
        sent.starts_with("POST /v1/backend/rooms/r-1/messages "),
        "再发消息: {sent}"
    );
    assert!(
        sent.to_ascii_lowercase()
            .contains("idempotency-key: celira-agent-"),
        "发消息必须带幂等键: {sent}"
    );
    assert!(
        sent.contains(&format!("\"attachments\":[\"{ATTACHMENT}\"]")),
        "附件身份随消息绑定: {sent}"
    );
    assert!(sent.contains("\"body\":\"给你一份笔记\""), "{sent}");
    assert_eq!(fake.connections(), 2, "一步递出去 = 恰好两次请求");

    let text = text_of(&outcome);
    assert!(
        text.contains(&format!("attachment = {ATTACHMENT}")),
        "回执要说清是哪一份: {text}"
    );
}

#[test]
fn saying_with_only_a_file_is_allowed() {
    let world_root = world("file-only");
    put(&world_root, "notes/only.txt", b"only the file\n");
    let fake = FakeIm::spawn_scripted(vec![
        Scripted::Json(201, upload_body(ATTACHMENT)),
        Scripted::Json(201, say_body(MESSAGE, false)),
    ]);
    let mut app = fake.app_with_world(&world_root);

    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_FILE, "notes/only.txt")),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "契约：正文与附件至少一个"
    );
    let upload = fake.next_request();
    assert!(upload.contains("filename=\"only.txt\""), "{upload}");
    let sent = fake.next_request();
    assert!(
        !sent.contains("\"body\""),
        "没有正文就一个 body 字段都不发（服务端投影为 null）: {sent}"
    );
    assert!(sent.contains(&format!("\"attachments\":[\"{ATTACHMENT}\"]")));
}

#[test]
fn a_file_that_is_missing_or_too_large_never_reaches_the_wire() {
    let world_root = world("no-wire");

    // 不存在。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_FILE, "notes/nope.md")),
    );
    assert_eq!(outcome.failure_class(), Some("file-not-found"));

    // 太大（**通道**的上限，契约 §8：文件 ≤ 16 MiB）。
    put(
        &world_root,
        "notes/big.txt",
        &vec![b'x'; MAX_SAY_FILE_BYTES + 1],
    );
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_FILE, "notes/big.txt")),
    );
    assert_eq!(outcome.failure_class(), Some("too-large"));

    // 越界的路径（`..`）：形状规则在 files 那一侧，同一个根、同一条规则。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_FILE, "../outside.txt")),
    );
    assert_eq!(outcome.failure_class(), Some("file-unreadable"));

    // 宿主没声明这个世界 ⇒ 那只手不在。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_FILE, "notes/nope.md")),
    );
    assert_eq!(outcome.failure_class(), Some("file-unreadable"));

    assert_eq!(
        fake.connections(),
        0,
        "读不到 / 太大 / 越界 / 没世界：**一个请求都不发**（类型不在这一列——\
         递得出去递不出去与读不读得动是两件事，见 \
         `anything_she_can_hand_over_goes_out_typed_by_its_extension`）"
    );
}

/// **递得出去 ≠ 我读得动**（2026-10-11 主人裁决：发送侧跟主流 IM 一样）。
///
/// 判据两半，缺一不可：
/// ① **图片递得出去**——按扩展名报 `image/png`，字节原样上传（她不读像素，但那不是"递"的事）；
/// ② **认不出的类型也递得出去**——报 `application/octet-stream`，**不拒收**（认不出 ≠ 不许）。
///
/// 判据为什么这么定：**读**是字节进她的上下文（她确实读不了像素），**递**是字节进世界
/// ——**递不需要理解**。通道那边本来就不判类型（契约 §8「无 MIME 白名单、不嗅探」），
/// 接收端能渲染就渲染、不能就给文件卡——与主流 IM 同形。
///
/// 反例在旁边：**读**那一侧照旧只收文本类（那是**读附件那一侧**的形状，不是通道的规则）。
#[test]
fn anything_she_can_hand_over_goes_out_typed_by_its_extension() {
    let world_root = world("send-any-type");

    for (path, wire_name, bytes, expected) in [
        (
            "pictures/cat.png",
            "cat.png",
            b"\x89PNG\r\n\x1a\n".to_vec(),
            "image/png",
        ),
        (
            "notes/blob.bin",
            "blob.bin",
            b"\x00\x01\x02".to_vec(),
            "application/octet-stream",
        ),
        (
            "notes/report.pdf",
            "report.pdf",
            b"%PDF-1.4".to_vec(),
            "application/pdf",
        ),
    ] {
        put(&world_root, path, &bytes);
        let fake = FakeIm::spawn_scripted(vec![
            Scripted::Json(201, upload_body(ATTACHMENT)),
            Scripted::Json(201, say_body(MESSAGE, false)),
        ]);
        let mut app = fake.app_with_world(&world_root);
        let outcome = call(
            &mut app,
            say(ActionParams::none()
                .with(PARAM_ROOM, "r-1")
                .with(PARAM_FILE, path)),
        );
        assert_eq!(outcome.state(), ResultState::Success, "{path}: {outcome:?}");

        let upload = fake.next_request();
        assert!(
            upload.starts_with("POST /v1/backend/rooms/r-1/attachments "),
            "{path}: 该先上传: {upload}"
        );
        assert!(
            upload.contains(&format!("Content-Type: {expected}")),
            "{path}: 该按扩展名报 {expected}: {upload}"
        );
        assert!(
            upload.contains(&format!("filename=\"{wire_name}\"")),
            "{path}: 递出去的名字是**最后一段**（本地路径结构不漏出去）: {upload}"
        );
    }
}

#[test]
fn a_failed_upload_never_becomes_a_message() {
    // 上传被明确拒绝 ⇒ 那句话确定没说出去（**不发**第二条请求）。
    let world_root = world("upload-refused");
    put(&world_root, "notes/hello.md", b"# hello\n");
    let fake = FakeIm::spawn(vec![(400, "{}".to_string())]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "带文件")
            .with(PARAM_FILE, "notes/hello.md")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));
    assert_eq!(fake.connections(), 1, "只发了上传那一次，消息一次都不发");
}

#[test]
fn verifying_said_never_claims_an_attachment_message_for_a_plain_one() {
    // ③ 判错即红：复核不误判。舞台上**只有**一条我发的、正文相同的、**带附件**的消息。
    let world_root = world("verify-pair");
    put(&world_root, "notes/hello.md", b"# hello\n"); // 8 字节，与服务端元数据一致

    let body_only = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_BODY, "带文件的问候");

    // ① 不带文件的复核：那条有附件 ⇒ 不同形 ⇒ 翻到起点都没有 ⇒ not-found。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_attachment("hello.md", 8)),
    ]);
    let mut app = fake.app();
    let outcome = call(&mut app, verify_said(body_only.clone()));
    assert_eq!(
        outcome.failure_class(),
        Some("not-found"),
        "带附件的消息不能被纯正文复核认领（少了附件维度就是误判）"
    );
    assert_eq!(fake.connections(), 2, "恰好两步：先问我是谁，再翻记录");

    // ② 带同一个文件（名字与字节数都对得上）⇒ 找到，且回执**自白**它比的是什么。
    let with_file = body_only.clone().with(PARAM_FILE, "notes/hello.md");
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_attachment("hello.md", 8)),
    ]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(&mut app, verify_said(with_file.clone()));
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(text.contains(&format!("found = {MESSAGE}")), "{text}");
    assert!(
        text.contains("compared = body + attachment file_name + size_bytes"),
        "比的是元数据，必须说出来: {text}"
    );

    // ③ 同名但大小不同 ⇒ 不是那一条 ⇒ not-found。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_attachment("hello.md", 9)),
    ]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(&mut app, verify_said(with_file.clone()));
    assert_eq!(outcome.failure_class(), Some("not-found"));

    // ④ 文件现在读不到 ⇒ 附件这一格**不可比** ⇒ unknown（既不说"找到了"，也不说"没有"）。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        verify_said(body_only.clone().with(PARAM_FILE, "notes/gone.md")),
    );
    assert_eq!(outcome.state(), ResultState::Unknown);
    assert_eq!(fake.connections(), 0, "不可比时一次请求都不发");

    // ⑤ 负对照：一条**没有附件**的同正文消息 ⇒ 纯正文复核**应当**找到它。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_message()),
    ]);
    let mut app = fake.app();
    let outcome = call(&mut app, verify_said(body_only.clone()));
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(
        !text.contains("compared ="),
        "没带文件就不说附件的事: {text}"
    );
}

#[test]
fn an_attachment_message_is_not_claimed_when_the_expected_set_differs() {
    // 复核按**集合**比：消息有 1 个附件、这次预期 1 个但名字不同 ⇒ 不算那一条。
    let world_root = world("verify-name");
    put(&world_root, "notes/other.md", b"# hello\n"); // 同名长度、不同名字
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_attachment("hello.md", 8)),
    ]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        verify_said(
            ActionParams::none()
                .with(PARAM_ROOM, "r-1")
                .with(PARAM_BODY, "带文件的问候")
                .with(PARAM_FILE, "notes/other.md"),
        ),
    );
    assert_eq!(
        outcome.failure_class(),
        Some("not-found"),
        "名字对不上就不是同一条"
    );
}

#[test]
fn the_reading_path_is_the_existing_one_not_a_new_injection() {
    // ④ 判错即红：注入路径**走现状**（与 `im_read_history` 同一条搬运路），本刀不新立行为。
    //   三条机械判据：
    //   - 结果块就是**能力名本身**（与 `im_read_history` 同族的那条搬运路，没有专属注入通道）；
    //   - 第二次读**要再问一次**（App 自己不缓存——缓存就是新立了一层机制）；
    //   - 主体那边**一个字节都不留**（App 不存档、不写盘）。
    let world_root = world("turn-only");
    let fake = FakeIm::spawn_scripted(vec![Scripted::text("一"), Scripted::text("二")]);
    let mut app = fake.app_with_world(&world_root);

    let before = snapshot(&world_root);
    let first = call(&mut app, read_attachment(attachment_params()));
    let second = call(&mut app, read_attachment(attachment_params()));

    assert_eq!(first.state(), ResultState::Success);
    assert_eq!(second.state(), ResultState::Success);
    let labels: Vec<&str> = first.content().iter().map(|block| block.label()).collect();
    assert_eq!(
        labels,
        vec![CAPABILITY_READ_ATTACHMENT],
        "结果块只能走能力名这一条路（与 read_history 同机制）"
    );
    assert_eq!(fake.connections(), 2, "不缓存：第二次要再问一次");
    assert!(text_of(&first).ends_with('一'));
    assert!(text_of(&second).ends_with('二'));
    assert_eq!(
        snapshot(&world_root),
        before,
        "读完不在主体那边留下任何东西（没有存档、没有缓存、没有副本）"
    );
}

#[test]
fn the_attachment_parameter_gate_rejects_before_the_wire() {
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app();

    for bad in ["", "not-a-uuid", "7B2F5A91-2C4D-4F6E-8A10-5D9C3E77BB21"] {
        let outcome = call(
            &mut app,
            read_attachment(
                ActionParams::none()
                    .with(PARAM_ROOM, "r-1")
                    .with(PARAM_ATTACHMENT, bad),
            ),
        );
        assert_eq!(outcome.failure_class(), Some("invalid-argument"), "{bad}");
    }

    // 续读位置：0 / 负数 / 非十进制都非法（一次都不出网）。
    for bad in ["0", "-1", "abc", "1.5"] {
        let outcome = call(
            &mut app,
            read_attachment(attachment_params().with(PARAM_ATTACHMENT_OFFSET, bad)),
        );
        assert_eq!(outcome.failure_class(), Some("invalid-argument"), "{bad}");
    }

    // 带进来的文件名：形状不对（换行会破坏那张按行组织的读数表）⇒ 非法。
    let outcome = call(
        &mut app,
        read_attachment(attachment_params().with(PARAM_FILE_NAME, "带\n换行")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));

    // 正文与文件**都不给** ⇒ 非法（契约：至少一个）。
    let outcome = call(&mut app, say(ActionParams::none().with(PARAM_ROOM, "r-1")));
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));

    // 正文给了但空、又没有文件 ⇒ 同样是非法（空正文不是"没说"）。
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));

    assert_eq!(fake.connections(), 0, "参数非法一次都不出网");
}

#[test]
fn only_the_writes_declare_effects() {
    let fake = FakeIm::spawn(vec![]);
    let app = fake.app();
    let social = fake.social_app();
    let profile = fake.profile_app();
    // 拆包后每个包只声明**自己的**写动词；拆包前那 11 条的并集逐条同序不变，
    // 拆包之后加的那一条（本刀 `im_typing`）单独点名。
    let im_effects = declared_effect_ids(&app);
    let mut declared: Vec<String> = Vec::new();
    declared.extend_from_slice(&im_effects[..4]);
    // ⚠ 关系面那一条**拆包之后加的**（2026-10-13 解除关系）要从这条历史重建里排掉——
    // 它由 `ADDED_AFTER_THE_SPLIT` 单独点名。
    declared.extend(
        declared_effect_ids(&social)
            .into_iter()
            .filter(|id| id != CAPABILITY_END_CONTACT),
    );
    declared.extend(declared_effect_ids(&profile));
    declared.extend_from_slice(&im_effects[4..5]);
    assert_eq!(
        declared,
        strings(&EFFECTS_BEFORE_THE_SPLIT),
        "读附件是读——它不声明写效应（原来这句写的是「不进未决效果门」，门已退役）"
    );
    assert_eq!(
        im_effects[5..],
        [
            CAPABILITY_CREATE_GROUP.to_string(),
            CAPABILITY_INVITE.to_string(),
            CAPABILITY_REMOVE_MEMBER.to_string(),
            CAPABILITY_TRANSFER_OWNER.to_string(),
            CAPABILITY_TYPING.to_string(),
            // 2026-10-20：翻记录是**隐式写**（读完推进读位置）——它不再被"未声明即只读"误读。
            CAPABILITY_READ_HISTORY.to_string(),
        ],
        "拆包之后多了六条声明：房治理四条（2026-10-13）、「正在写」与翻记录的隐式写"
    );
    assert!(!declared.contains(&CAPABILITY_READ_ATTACHMENT.to_string()));
    // 三个只读动词**不**声明写效应（关系账 / 拉黑名单 / 我自己的资料）。
    assert!(!declared.contains(&CAPABILITY_CONTACTS.to_string()));
    assert!(!declared.contains(&CAPABILITY_BLOCKS.to_string()));
    assert!(!declared.contains(&CAPABILITY_MY_PROFILE.to_string()));
}

// ---------------------------------------------------------------------------
// 9. 声明面：骨架 / 详情 / 参数里不许残留**未插值**的格式占位符
// ---------------------------------------------------------------------------

/// 主体看到的是**渲染后的文本**：骨架进动作目录、详情进 `inspect_action` 的卡片、参数进参数表。
/// 声明里若写成普通字符串（而不是 `format!`），`{CONST}` 就**原样**出现在主体眼前。
///
/// **既有缺陷**：`d5fd22e:505/550` 上的 `im_verify_said` / `im_verify_recalled` 骨架就是这样，
/// 一直没人看见（那两行只在目录里露面）；2026-09-21 读目录原文时照出来，顺刀修复。
///
/// 判错即红：把任何一处改回普通字符串、或新写一处带占位符的说明 ⇒ 这条立刻红。
#[test]
fn no_declared_text_leaks_an_uninterpolated_placeholder() {
    let fake = FakeIm::spawn(vec![]);
    // 拆包后三个包各查一遍：声明面是**每个包自己的**事实。
    let packs: Vec<(&str, Box<dyn AppPort>)> = vec![
        (IM_APP_ID, Box::new(fake.app())),
        (IM_SOCIAL_APP_ID, Box::new(fake.social_app())),
        (IM_PROFILE_APP_ID, Box::new(fake.profile_app())),
    ];

    for (app_id, app) in &packs {
        let declared = app.declared_capabilities();
        for capability in declared.items() {
            let mut faces = vec![
                ("骨架", capability.skeleton().to_string()),
                ("详情", capability.details().to_string()),
                ("参数表", capability.params_line()),
            ];
            for spec in capability.params() {
                faces.push(("参数说明", spec.description().to_string()));
            }
            for (face, text) in faces {
                assert!(
                    !text.contains('{') && !text.contains('}'),
                    "`{app_id}` 的 `{}` 的{face}里残留了未插值的占位符——主体看到的是渲染后的文本:\n{text}",
                    capability.id().as_str()
                );
            }
        }
    }
}

/// **上下文里每一条限制都要写明它是谁的限制**（2026-10-11）。
///
/// 真机实测：她把「我这个动作只收文本类」说成了「**IM 这边只让递文本类文件**」——而 IM 契约
/// §8 明文「无 MIME 白名单、不嗅探」，人那条路什么都能传。判据一句话：**这一格的主语是谁？**
/// 混一格的代价是具体的：主体把**自己那只手的形状**读成**世界的规则**，再把它当事实说给人。
///
/// 当天晚些时候主人裁掉了那条类型门（发送侧跟主流 IM 一样：**不限类型，只限通道的大小**），
/// 于是这一格变成了两句话，**各归各的主语**：
/// - **大小**是**通道**的（`IM 通道的上限`，契约 §8）；
/// - **读不动**是**读附件那一侧**的（`我读附件那一侧`，只收文本类）。
///
/// 这条用例钉她实际看得到的**三处**，要求口径一致：参数格与骨架格都说"什么类型都递得出去"
/// （不许留下"只递文本类"那句旧话），详情格把两条限制**各自的主语**写出来。
#[test]
fn the_file_limit_says_whose_limit_it_is() {
    let fake = FakeIm::spawn(vec![]);
    let app = fake.app();
    let declared = app.declared_capabilities();
    let say = declared
        .items()
        .iter()
        .find(|capability| capability.id().as_str() == CAPABILITY_SAY)
        .expect("im_say 必须在声明面里");

    // ① 参数那一格：`pinned-actions` 直接渲染它（她每一段开场都读到这一行）。
    let file = say
        .params()
        .iter()
        .find(|spec| spec.name() == PARAM_FILE)
        .expect("file 参数必须在参数表里");
    assert!(
        file.description().contains("什么类型都递得出去"),
        "参数那一格要说清**类型不受限**: {}",
        file.description()
    );
    assert!(
        !file.description().contains("只递文本类"),
        "那一句旧话必须消失（它正是那次误报的源头）: {}",
        file.description()
    );

    // ② 骨架那一格：动作目录渲染它——同一个口径，不许留下"只递文本类"的第二格。
    assert!(
        say.skeleton().contains("什么类型都递得出去"),
        "骨架那一格要与参数那一格同口径: {}",
        say.skeleton()
    );

    // ③ 详情那一格：两条限制**各归各的主语**——大小是通道的，读不动是读附件那一侧的。
    assert!(
        say.details().contains("IM 通道的上限"),
        "大小那一格要写明是**通道**的上限: {}",
        say.details()
    );
    assert!(
        say.details().contains("我读附件那一侧"),
        "读不动那一格要写明是**她**的边界（不是通道的规则）: {}",
        say.details()
    );
    assert!(
        say.details().contains("不嗅探"),
        "类型怎么定也要如实说（不嗅探）: {}",
        say.details()
    );
}

// ---------------------------------------------------------------------------
// 10. 关系面 / 同意面 / 拉黑面（批次三第三刀 第②步）：搬运、词表与参数网关
// ---------------------------------------------------------------------------
//
// 这一层只证明 App 的搬运（方法 / 路径 / 体 / 键）、失败词表与**参数网关**
// （非法值一次都不出网）。世界侧的语义由 im 服务端的契约测试与独立进程黑盒证明。

fn contacts(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_CONTACTS), params)
}

fn blocks(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_BLOCKS), params)
}

fn contacts_request(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_CONTACTS_REQUEST), params)
}

fn answer_contact(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_ANSWER_CONTACT), params)
}

fn block(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_BLOCK), params)
}

fn unblock(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_UNBLOCK), params)
}

/// 一个形状合法的对端主体身份。
const PEER: &str = "6f1d0c7a-2b34-4d58-9e01-77aa55cc3311";

/// 看关系：一次**裸 GET**，回执把"哪一条在等我回答"算成一行读数。
#[test]
fn the_contacts_call_is_a_bare_get_and_counts_what_waits_for_me() {
    let body = serde_json::json!({
        "contacts": [
            {"peer": {"id": PEER, "kind": "human", "display_name": "甲", "status": "active"},
             "status": "pending", "direction": "incoming", "created_at": "t", "accepted_at": null},
            {"peer": {"id": "other", "kind": "agent", "display_name": "乙", "status": "active"},
             "status": "accepted", "direction": "outgoing", "created_at": "t", "accepted_at": "t"}
        ],
        "request_id": "r"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(200, body)]);
    let mut app = fake.social_app();

    let outcome = call(&mut app, contacts(ActionParams::none()));
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    let text = format!("{outcome:?}");
    assert!(
        text.contains("incoming = 1"),
        "回执必须把「等我回答的条数」算出来: {text}"
    );

    let request = fake.next_request();
    assert!(
        request.starts_with("GET /v1/backend/contacts "),
        "方法必须是 GET、路径必须是关系面: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("idempotency-key"),
        "只读不带键: {request}"
    );
}

/// 看拉黑名单：同样是一次裸 GET。
#[test]
fn the_blocks_call_is_a_bare_get() {
    let fake = FakeIm::spawn(vec![(
        200,
        serde_json::json!({"blocks": [], "request_id": "r"}).to_string(),
    )]);
    let mut app = fake.social_app();

    let outcome = call(&mut app, blocks(ActionParams::none()));
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    assert!(fake.next_request().starts_with("GET /v1/backend/blocks "));
}

/// 回答申请：一次 POST、**无体无键**；`accept` 与 `decline` 走两条不同的路径。
#[test]
fn answering_a_request_posts_the_decision_without_a_key() {
    for (decision, suffix) in [("accept", "/accept"), ("decline", "/decline")] {
        let body = serde_json::json!({
            "contact": {"peer": {"id": PEER, "kind": "human", "display_name": "甲",
                                 "status": "active"},
                        "status": "accepted", "direction": "incoming",
                        "created_at": "t", "accepted_at": "t"},
            "request_id": "r"
        })
        .to_string();
        let fake = FakeIm::spawn(vec![(200, body)]);
        let mut app = fake.social_app();

        let outcome = call(
            &mut app,
            answer_contact(
                ActionParams::none()
                    .with(PARAM_PARTICIPANT, PEER)
                    .with(PARAM_DECISION, decision),
            ),
        );
        assert_eq!(
            outcome.state(),
            ResultState::Success,
            "{decision}: {outcome:?}"
        );

        let request = fake.next_request();
        assert!(
            request.starts_with(&format!("POST /v1/backend/contacts/{PEER}{suffix} ")),
            "{decision}: {request}"
        );
        assert!(
            !request.to_ascii_lowercase().contains("idempotency-key"),
            "回答是一次决定，不带键: {request}"
        );
    }
}

/// **参数网关**：非法决定词与空主体**一次都不出网**（工具面先拒绝）。
#[test]
fn a_bad_decision_or_an_empty_peer_never_leaves_the_process() {
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.social_app();

    for params in [
        ActionParams::none()
            .with(PARAM_PARTICIPANT, PEER)
            .with(PARAM_DECISION, "maybe"),
        ActionParams::none()
            .with(PARAM_PARTICIPANT, PEER)
            .with(PARAM_DECISION, "ACCEPT"),
        ActionParams::none()
            .with(PARAM_PARTICIPANT, "")
            .with(PARAM_DECISION, "accept"),
        ActionParams::none().with(PARAM_PARTICIPANT, PEER),
    ] {
        let outcome = call(&mut app, answer_contact(params));
        assert_eq!(
            outcome.failure_class(),
            Some("invalid-argument"),
            "{outcome:?}"
        );
    }
    for peer in [Some(""), None] {
        let build = || match peer {
            Some(value) => ActionParams::none().with(PARAM_PARTICIPANT, value),
            None => ActionParams::none(),
        };
        assert_eq!(
            call(&mut app, block(build())).failure_class(),
            Some("invalid-argument")
        );
        assert_eq!(
            call(&mut app, unblock(build())).failure_class(),
            Some("invalid-argument")
        );
        assert_eq!(
            call(&mut app, contacts_request(build())).failure_class(),
            Some("invalid-argument")
        );
    }
    assert_eq!(fake.connections(), 0, "参数非法一次都不出网");
}

/// 发起申请：**带幂等键**（同一个 operation 重放不产生第二条关系），体里是对端。
#[test]
fn a_contact_request_carries_an_idempotency_key_and_the_peer() {
    let body = serde_json::json!({
        "contact": {"peer": {"id": PEER, "kind": "human", "display_name": "甲",
                             "status": "active"},
                    "status": "pending", "direction": "outgoing",
                    "created_at": "t", "accepted_at": null},
        "idempotency_replayed": false,
        "request_id": "r"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(201, body)]);
    let mut app = fake.social_app();

    let outcome = call(
        &mut app,
        contacts_request(ActionParams::none().with(PARAM_PARTICIPANT, PEER)),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");

    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/contacts/requests "),
        "{request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("idempotency-key"),
        "发起申请必须带键: {request}"
    );
    assert!(request.contains(PEER), "体里必须有对端: {request}");
}

/// **线路级围栏**：发出去的**体**里，键必须是世界契约的 wire 名（`participant_id`）。
///
/// 上一条用例只看回执里含对端、请求里含键——**不看体里的键名**。于是本动作的参数名
/// （`participant`）曾经被原样拼进体里：世界契约 `CreateContactRequest` 是
/// `deny_unknown_fields`，键不对一律 400 ⇒ 这个动词在真机上永远成功不了，
/// 而上一条用例照绿（假服务按脚本回执，不看体）。这条把**真实发出去的字节**钉死：
/// 体必须恰好是 `{"participant_id": <对端>}`——改回错的键，这里立刻红。
#[test]
fn a_contact_request_body_uses_the_wire_field_name() {
    let body = serde_json::json!({
        "contact": {"peer": {"id": PEER, "kind": "human", "display_name": "甲",
                             "status": "active"},
                    "status": "pending", "direction": "outgoing",
                    "created_at": "t", "accepted_at": null},
        "idempotency_replayed": false,
        "request_id": "r"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(201, body)]);
    let mut app = fake.social_app();

    let outcome = app.execute(
        &agent(),
        &OperationId::new(RunId::new("run-contact"), Turn::FIRST),
        &contacts_request(ActionParams::none().with(PARAM_PARTICIPANT, PEER)),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");

    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/contacts/requests "),
        "{request}"
    );

    // 体是请求原文的**最后一行**（请求行与头逐行在前，体在头之后原样追加）。
    let sent: serde_json::Value =
        serde_json::from_str(request.lines().last().expect("请求原文最后一行是体"))
            .expect("体必须是 JSON");
    assert_eq!(
        sent,
        serde_json::json!({"participant_id": PEER}),
        "体的键必须是 wire 名 `participant_id`、值就是对端（世界契约 deny_unknown_fields：\
         键不对一律 400；本动作的参数名 `participant` 不许出现在体里）: {request}"
    );

    // 幂等键与对端一并钉住（上一条用例只断言「含键」，这里收紧到「键是哪一个」）。
    let key = request
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("idempotency-key:"))
        .map(|line| line.split(':').nth(1).expect("有取值").trim().to_string())
        .expect("发起申请必须带幂等键");
    assert_eq!(
        key, "celira-agent-run-contact-1-0",
        "键由操作身份合成: {key}"
    );
}

/// 拉黑与解除：POST / DELETE 各一次，**204 空体即成功**（幂等写）。
#[test]
fn block_and_unblock_are_bare_writes_and_an_empty_204_is_success() {
    for (action, method) in [
        (block as fn(ActionParams) -> Action, "POST"),
        (unblock, "DELETE"),
    ] {
        let fake = FakeIm::spawn(vec![(204, String::new())]);
        let mut app = fake.social_app();

        let outcome = call(
            &mut app,
            action(ActionParams::none().with(PARAM_PARTICIPANT, PEER)),
        );
        assert_eq!(
            outcome.state(),
            ResultState::Success,
            "{method}: {outcome:?}"
        );

        let request = fake.next_request();
        assert!(
            request.starts_with(&format!("{method} /v1/backend/blocks/{PEER} ")),
            "{method}: {request}"
        );
        assert!(
            !request.to_ascii_lowercase().contains("idempotency-key"),
            "幂等 204 不需要键: {request}"
        );
    }
}

/// 失败词表：目标不可用与"那条不是我的事"各有**自己的**稳定分类——绝不吞成
/// `unknown`（那会把"明确被拒"说成"不知道"），也不分解服务端刻意合成的码。
#[test]
fn the_relation_and_block_failures_keep_their_own_classes() {
    for (status, expected) in [
        (404, "unknown-participant"),
        (400, "invalid-argument"),
        (401, "credential-rejected"),
    ] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.social_app();
        let outcome = call(
            &mut app,
            block(ActionParams::none().with(PARAM_PARTICIPANT, PEER)),
        );
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
    }
    for (status, expected) in [(404, "unknown-contact"), (401, "credential-rejected")] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.social_app();
        let outcome = call(
            &mut app,
            answer_contact(
                ActionParams::none()
                    .with(PARAM_PARTICIPANT, PEER)
                    .with(PARAM_DECISION, "accept"),
            ),
        );
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
    }
    for (status, expected) in [(404, "unknown-participant"), (409, "idempotency-conflict")] {
        let fake = FakeIm::spawn(vec![(status, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.social_app();
        let outcome = call(
            &mut app,
            contacts_request(ActionParams::none().with(PARAM_PARTICIPANT, PEER)),
        );
        assert_eq!(outcome.failure_class(), Some(expected), "HTTP {status}");
    }
}

/// 5xx / 传输层：**未确认**（不是"确定失败"）——三个新写动词一个都不许猜。
#[test]
fn a_broken_reply_on_the_new_writes_is_unknown_not_a_failure() {
    for action in [block as fn(ActionParams) -> Action, unblock, |params| {
        answer_contact(
            params
                .with(PARAM_PARTICIPANT, PEER)
                .with(PARAM_DECISION, "accept"),
        )
    }] {
        let fake = FakeIm::spawn(vec![(500, "{\"error\":\"synthetic\"}".to_string())]);
        let mut app = fake.social_app();
        let outcome = call(
            &mut app,
            action(ActionParams::none().with(PARAM_PARTICIPANT, PEER)),
        );
        assert_eq!(outcome.state(), ResultState::Unknown, "{outcome:?}");
    }
}
// ---------------------------------------------------------------------------
// 表达权（批次四第一刀）：读自己的资料 + 写自己的资料
// ---------------------------------------------------------------------------

fn my_profile(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_MY_PROFILE), params)
}

fn write_profile(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_WRITE_PROFILE), params)
}

/// 读：一次裸 GET（**身份只来自认证**，路径里没有别人的位置）。
#[test]
fn reading_my_own_profile_is_a_bare_get() {
    let body = serde_json::json!({
        "profile": {"profile_version": 3, "short_description": "甲", "public_description": null},
        "request_id": "r"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(200, body)]);
    let mut app = fake.profile_app();

    let outcome = call(&mut app, my_profile(ActionParams::none()));
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    assert!(
        format!("{outcome:?}").contains("version = 3"),
        "回执必须给出我读到的那一版: {outcome:?}"
    );
    let request = fake.next_request();
    // ⚠ **2026-09-29 真机实测修**：这里原先断言的是 `GET /v1/backend/me/profile`
    // —— 那条路径在服务端**只注册了 PUT**，于是这条用例把**错的路径钉死在了测试里**，
    // 实现与测试共享同一个错误假设，两边门禁一起绿，而真机上 `im_my_profile` 恒 405。
    // 契约 `CORE_V1_AGENT_READ_SURFACE.md` §2.2 的「看见自己」是 `/v1/backend/profile`。
    // 别把这条断言改回去：它是"假服务器不再自证"的第一道桩。
    assert!(
        request.starts_with("GET /v1/backend/profile "),
        "读自己的资料必须打契约 §2.2 那条读路径: {request}"
    );
    assert!(
        !request.starts_with("GET /v1/backend/me/profile "),
        "`/v1/backend/me/profile` 是**写**路径（只注册了 PUT），读面不许打它: {request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("idempotency-key"),
        "{request}"
    );
}

/// **读路径与写路径必须是两条路**（2026-09-29 真机实测那条缺陷的结构桩）。
///
/// 背景：这两个动作曾经共用同一个常量 `/v1/backend/me/profile`，而服务端在那条路径上
/// **只注册了 PUT** ⇒ 读恒 405 ⇒ 归入 `unknown` ⇒ 整次活动 `system:indeterminate` ⇒
/// 因为写之前要先读 `base_version`，**Agent 连自己的资料都写不了**。
/// 契约把两者分开写着：读 = `GET /v1/backend/profile`（读取面 §2.2），写 = `PUT /v1/backend/me/profile`。
///
/// 这条用例**不看实现怎么写的**，只看**真发出去的那两个请求**：它们的方法与路径都必须不同。
#[test]
fn reading_and_writing_my_profile_are_two_different_routes() {
    let read_body = serde_json::json!({"profile": null, "request_id": "r"}).to_string();
    let fake = FakeIm::spawn(vec![(200, read_body)]);
    let mut app = fake.profile_app();
    assert_eq!(
        call(&mut app, my_profile(ActionParams::none())).state(),
        ResultState::Success
    );
    let read = fake.next_request();

    let write_body = serde_json::json!({
        "profile": {"profile_version": 1, "short_description": "甲", "public_description": null},
        "idempotency_replayed": false,
        "request_id": "r"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(201, write_body)]);
    let mut app = fake.profile_app();
    assert_eq!(
        call(
            &mut app,
            write_profile(ActionParams::none().with(PARAM_BASE_VERSION, "0"))
        )
        .state(),
        ResultState::Success
    );
    let write = fake.next_request();

    let line = |r: &str| r.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let read_line = line(&read);
    let write_line = line(&write);
    assert_eq!(read_line, "GET /v1/backend/profile", "{read}");
    assert_eq!(write_line, "PUT /v1/backend/me/profile", "{write}");
    assert_ne!(
        read_line, write_line,
        "读与写不许共用同一条路径、同一个方法 —— 那正是真机 405 的形状"
    );
}

/// 写：PUT + 幂等键 + 基准版本；参数非法**一次都不出网**；版本对不上是**确定的拒绝**。
#[test]
fn writing_my_own_profile_carries_the_base_version_and_is_rejected_on_conflict() {
    let body = serde_json::json!({
        "profile": {"profile_version": 4, "short_description": "我是甲", "public_description": "住在这儿"},
        "idempotency_replayed": false,
        "request_id": "r"
    })
    .to_string();
    let fake = FakeIm::spawn(vec![(200, body)]);
    let mut app = fake.profile_app();
    let outcome = call(
        &mut app,
        write_profile(
            ActionParams::none()
                .with(PARAM_BASE_VERSION, "3")
                .with(PARAM_SHORT_DESCRIPTION, "我是甲")
                .with(PARAM_PUBLIC_DESCRIPTION, "住在这儿"),
        ),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    let request = fake.next_request();
    assert!(
        request.starts_with("PUT /v1/backend/me/profile "),
        "{request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("idempotency-key"),
        "{request}"
    );
    assert!(
        request.contains("\"base_version\":3"),
        "体里必须带基准版本: {request}"
    );

    // 参数网关：缺基准版本、非十进制 ⇒ 非法，且一次都不出网。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.profile_app();
    for params in [
        ActionParams::none(),
        ActionParams::none().with(PARAM_BASE_VERSION, "abc"),
        ActionParams::none().with(PARAM_BASE_VERSION, "-1"),
    ] {
        assert_eq!(
            call(&mut app, write_profile(params)).failure_class(),
            Some("invalid-argument")
        );
    }
    assert_eq!(fake.connections(), 0, "参数非法一次都不出网");

    // 版本对不上 ⇒ `version-conflict`（确定的拒绝，零新事实）。
    let fake = FakeIm::spawn(vec![(409, "{\"error\":\"synthetic\"}".to_string())]);
    let mut app = fake.profile_app();
    let outcome = call(
        &mut app,
        write_profile(ActionParams::none().with(PARAM_BASE_VERSION, "0")),
    );
    assert_eq!(
        outcome.failure_class(),
        Some("version-conflict"),
        "{outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// 表达权第二刀：换自己的头像（`im-profile`）——multipart 单步 + 幂等键
// ---------------------------------------------------------------------------

fn write_avatar(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_WRITE_AVATAR), params)
}

/// 换头像：`PUT /v1/backend/me/avatar`，**multipart 单步**（体里 `base_version` 在前、
/// `name="file"` 的部件在后）、**带幂等键**。
///
/// 2026-10-02 拆包后这条动词住在 `im-profile`：它读的仍是**同一个世界观根**（沙盒）、
/// 发出去的仍是**同一份** wire 形状——这条用例就是那次搬运的读数。
#[test]
fn writing_my_avatar_uploads_one_multipart_step_with_the_base_version() {
    let world_root = world("avatar");
    put(
        &world_root,
        "pictures/me.png",
        &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
    );
    let body = serde_json::json!({
        "profile": {"profile_version": 4, "short_description": "甲", "public_description": null},
        "idempotency_replayed": false,
        "request_id": "r"
    })
    .to_string();

    let fake = FakeIm::spawn(vec![(200, body)]);
    let mut app = fake.profile_app_with_world(&world_root);
    let outcome = call(
        &mut app,
        write_avatar(
            ActionParams::none()
                .with(PARAM_BASE_VERSION, "3")
                .with(PARAM_FILE, "pictures/me.png"),
        ),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");

    let request = fake.next_request();
    assert!(
        request.starts_with("PUT /v1/backend/me/avatar "),
        "路径里没有别人的位置（身份只来自认证）: {request}"
    );
    let lowered = request.to_ascii_lowercase();
    assert!(
        lowered.contains("content-type: multipart/form-data; boundary="),
        "{request}"
    );
    assert!(
        lowered.contains("idempotency-key: celira-agent-"),
        "头像写入按契约必带键: {request}"
    );
    assert!(
        request.contains("name=\"base_version\""),
        "并发基准先写（服务端按序读部件）: {request}"
    );
    assert!(
        request.contains("name=\"file\"; filename=\"pictures/me.png\""),
        "{request}"
    );
    assert!(request.contains("Content-Type: image/png"), "{request}");
    assert_eq!(fake.connections(), 1, "multipart 单步 = 恰好一次请求");

    // 参数非法（缺 `base_version`）⇒ 一次都不出网。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.profile_app_with_world(&world_root);
    let outcome = call(
        &mut app,
        write_avatar(ActionParams::none().with(PARAM_FILE, "pictures/me.png")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));
    assert_eq!(fake.connections(), 0, "参数非法一次都不出网");

    // 文件读不到 ⇒ 确定的失败，**不发请求**（与说话那条同一纪律）。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.profile_app_with_world(&world_root);
    let outcome = call(
        &mut app,
        write_avatar(
            ActionParams::none()
                .with(PARAM_BASE_VERSION, "3")
                .with(PARAM_FILE, "pictures/nope.png"),
        ),
    );
    assert_eq!(outcome.failure_class(), Some("file-not-found"));
    assert_eq!(fake.connections(), 0, "读不到文件不发请求");

    // 类型不在白名单里（按扩展名判定）⇒ 同样不出网。
    put(&world_root, "notes/hello.txt", b"# hello\n");
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.profile_app_with_world(&world_root);
    let outcome = call(
        &mut app,
        write_avatar(
            ActionParams::none()
                .with(PARAM_BASE_VERSION, "3")
                .with(PARAM_FILE, "notes/hello.txt"),
        ),
    );
    assert_eq!(outcome.failure_class(), Some("unsupported-file-type"));
    assert_eq!(fake.connections(), 0, "类型不支持不发请求");
}

/// **同一种形状只有一个界**（2026-10-06）：本地文件路径（`file` 参数）在两个包里用**同一个**
/// 上限——`im_channel::MAX_FILE_PARAM_BYTES`（512），**不是**标识那一族的 `MAX_ID_BYTES`（64）。
///
/// 怎么读出来：形状过了才轮到沙盒去读那个文件 ⇒ 一个**超过 64、不到 512** 字节的路径应当是
/// `file-not-found`（**不是** `invalid-argument`）；只有超过**路径**上限才在参数面被拒、零出网。
/// 这条用例存在的理由：这两处曾各有一个答案（通信面递文件 512，表达面换头像借了标识的 64）。
#[test]
fn a_local_file_path_has_one_bound_in_both_packages() {
    let say_path = format!("notes/{}.md", "a".repeat(60));
    let avatar_path = format!("pictures/{}.png", "a".repeat(60));
    for path in [&say_path, &avatar_path] {
        assert!(
            path.len() > MAX_ID_BYTES && path.len() < MAX_FILE_PARAM_BYTES,
            "这条用例要的是「过了标识那一族、没到路径上限」的路径：{} 字节",
            path.len()
        );
    }
    let over_the_path_bound = "a".repeat(MAX_FILE_PARAM_BYTES + 1);

    // 通信面（递文件）：过了形状 ⇒ 沙盒说"没有这个文件"。
    let world_root = world("one-bound-say");
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_FILE, say_path.as_str())),
    );
    assert_eq!(
        outcome.failure_class(),
        Some("file-not-found"),
        "{outcome:?}"
    );
    assert_eq!(fake.connections(), 0, "读不到文件不发请求");

    // 通信面：超过**路径**上限 ⇒ 参数面拒。
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_FILE, over_the_path_bound.as_str())),
    );
    assert_eq!(
        outcome.failure_class(),
        Some("invalid-argument"),
        "{outcome:?}"
    );
    assert_eq!(fake.connections(), 0, "参数非法一次都不出网");

    // 表达面（换头像）：**同一个界**——同一个量级的路径走到沙盒，而不是被参数面拒掉。
    let world_root = world("one-bound-avatar");
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.profile_app_with_world(&world_root);
    let outcome = call(
        &mut app,
        write_avatar(
            ActionParams::none()
                .with(PARAM_BASE_VERSION, "3")
                .with(PARAM_FILE, avatar_path.as_str()),
        ),
    );
    assert_eq!(
        outcome.failure_class(),
        Some("file-not-found"),
        "{outcome:?}"
    );
    assert_eq!(fake.connections(), 0, "读不到文件不发请求");
}

// ---------------------------------------------------------------------------
// 11. 写侧回复引用与成员提及（Core IM v1 封板补漏）：参数面、体、幂等、失败面、复核维度
// ---------------------------------------------------------------------------
//
// 契约依据：`CORE_V1_AGENT_SPEAK.md` §3.1/§3.2（Backend 说话端点接受与人类同一
// `SendMessageRequest` 形状，含可选 `mentions` / `reply_to_message_id`；幂等摘要含 mentions
// 与 reply target）、`CORE_V1_IM_CONTRACT.md` §6.2/§6.3/§6.5、`CORE_V1_MENTION_TARGETS.md`
// §5（两个错误面各自**合成一个码**，执行端不分解）。
// 这一层只证 App 的搬运与判据；世界侧语义由 im 服务端契约测试与黑盒证明。

/// 一个规范的引用目标（小写连字符 UUID 形状）。
const REPLY_TARGET: &str = "3f1c9a02-5b7e-4d61-9a2c-77e0b4d51f38";

/// 另一个引用目标（同一个键换引用对象 = 换幂等摘要）。
const OTHER_TARGET: &str = "b6a1e5c4-0d2f-4a83-9f11-2c8e7d6409aa";

/// 两个可被点名的成员身份。
const MENTION_A: &str = "9d2b7f10-4c58-4e0a-8b31-5a6f9c2e4d07";
const MENTION_B: &str = "c4e8a1b9-7f36-42d5-9c08-1e5b3a7d62f5";

/// 一份带引用 / 提及的说话回执体（`MessageView` 恒含这两格）。
fn say_body_with(id: &str, replayed: bool, reply_to: Option<&str>, mentions: &[&str]) -> String {
    serde_json::json!({
        "message": {
            "id": id,
            "room_id": "r-1",
            "body": "收到，我来办",
            "mentions": mentions,
            "attachments": [],
            "reply_to_message_id": reply_to,
            "created_at": "2026-09-16T11:00:00+00:00",
            "recalled_at": null
        },
        "idempotency_replayed": replayed,
        "request_id": "req-say"
    })
    .to_string()
}

/// 请求里的幂等键（写请求必须带）。
fn idempotency_key_of(request: &str) -> String {
    request
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("idempotency-key:"))
        .map(|line| line.split(':').nth(1).expect("有取值").trim().to_string())
        .expect("写请求必须带幂等键")
}

/// 一条"我发的、正文相同、引用了 REPLY_TARGET"的历史消息。
fn history_with_my_reply() -> String {
    history_with_extra(serde_json::json!({"reply_to_message_id": REPLY_TARGET}))
}

#[test]
fn saying_with_a_reply_target_posts_the_wire_field_and_echoes_the_worlds_answer() {
    let fake = FakeIm::spawn(vec![(
        201,
        say_body_with(MESSAGE, false, Some(REPLY_TARGET), &[]),
    )]);
    let mut app = fake.app();

    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "收到，我来办")
            .with(PARAM_REPLY_TO, REPLY_TARGET)),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");

    let request = fake.next_request();
    assert!(
        request.starts_with("POST /v1/backend/rooms/r-1/messages "),
        "路径必须是后台发消息端点: {request}"
    );
    assert!(
        request.contains(&format!("\"reply_to_message_id\":\"{REPLY_TARGET}\"")),
        "体里必须是契约的 wire 字段名: {request}"
    );
    assert!(
        !request.contains("\"reply_to_id\""),
        "工具参数名不上线（体里只有契约字段）: {request}"
    );
    assert_eq!(fake.connections(), 1, "一次调用恰好一次请求");

    // 回执里那一行取**服务端回执**的那一格（因此能与读侧直接对上）。
    let text = text_of(&outcome);
    assert!(
        text.contains(&format!("reply_to = {REPLY_TARGET}")),
        "回执要说清引用了哪一条: {text}"
    );

    // 负对照：不给引用 ⇒ 体里一个字节都不带，回执里也没有那一行。
    let fake = FakeIm::spawn(vec![(201, say_body(MESSAGE, false))]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "你好")),
    );
    let request = fake.next_request();
    assert!(
        !request.contains("reply_to_message_id"),
        "不给引用就不带那一格: {request}"
    );
    assert!(
        !text_of(&outcome).contains("reply_to ="),
        "没有引用就不说引用的事: {}",
        text_of(&outcome)
    );
}

#[test]
fn the_reply_target_and_the_mention_set_ride_along_in_the_body() {
    let fake = FakeIm::spawn(vec![(
        201,
        say_body_with(MESSAGE, false, Some(REPLY_TARGET), &[MENTION_A, MENTION_B]),
    )]);
    let mut app = fake.app();

    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "收到，我来办")
            .with(PARAM_REPLY_TO, REPLY_TARGET)
            // 逗号分隔、允许空白（工具边界上的列表编码；见 `PARAM_MENTIONS`）。
            .with(PARAM_MENTIONS, format!(" {MENTION_B} ,{MENTION_A} "))),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");

    let request = fake.next_request();
    assert!(
        request.contains(&format!("\"mentions\":[\"{MENTION_B}\",\"{MENTION_A}\"]")),
        "提及集合按数组进体（去空白、保持给出顺序）: {request}"
    );
    assert!(
        request.contains(&format!("\"reply_to_message_id\":\"{REPLY_TARGET}\"")),
        "{request}"
    );

    // 负对照：不给提及 ⇒ 连那一格都不带（空数组与"没给"在服务端摘要里是同一件事，
    // 但体形状要如实反映调用方给了什么）。
    let fake = FakeIm::spawn(vec![(201, say_body(MESSAGE, false))]);
    let mut app = fake.app();
    let _ = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "你好")),
    );
    let request = fake.next_request();
    assert!(!request.contains("\"mentions\""), "{request}");
}

#[test]
fn the_reply_target_is_part_of_the_idempotency_digest() {
    // 同一次操作（同一个键）：同 body + 同 reply target ⇒ 服务端重放（200），回执如实说 replayed。
    // 同一个键**换引用对象** = 换摘要 ⇒ 契约的 409（零新事实）⇒ 一次确定的失败，不重发。
    let fake = FakeIm::spawn(vec![
        (201, say_body_with(MESSAGE, false, Some(REPLY_TARGET), &[])),
        (200, say_body_with(MESSAGE, true, Some(REPLY_TARGET), &[])),
        (409, "{\"error\":\"idempotency_conflict\"}".to_string()),
    ]);
    let mut app = fake.app();
    let operation = OperationId::new(RunId::new("run-reply"), Turn::FIRST);
    let params = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_BODY, "收到，我来办")
        .with(PARAM_REPLY_TO, REPLY_TARGET);

    let first = app.execute(
        &agent(),
        &operation,
        &Action::with_params(ActionId::new(CAPABILITY_SAY), params.clone()),
    );
    assert_eq!(first.state(), ResultState::Success);

    let replay = app.execute(
        &agent(),
        &operation,
        &Action::with_params(ActionId::new(CAPABILITY_SAY), params.clone()),
    );
    assert_eq!(
        text_of(&replay),
        format!("room = r-1, message = {MESSAGE}\nreplayed = yes\nreply_to = {REPLY_TARGET}"),
        "重放要如实说出这是重放，并给出世界回执里的引用格"
    );

    let conflict = app.execute(
        &agent(),
        &operation,
        &Action::with_params(
            ActionId::new(CAPABILITY_SAY),
            params.with(PARAM_REPLY_TO, OTHER_TARGET),
        ),
    );
    assert_eq!(
        conflict.failure_class(),
        Some("idempotency-conflict"),
        "同键异体（换了引用对象）必须是那个稳定的 409 分类"
    );

    let first_request = fake.next_request();
    let replay_request = fake.next_request();
    let conflict_request = fake.next_request();
    assert_eq!(
        idempotency_key_of(&first_request),
        idempotency_key_of(&replay_request),
        "同一次操作必须用同一个键"
    );
    assert_eq!(
        idempotency_key_of(&first_request),
        idempotency_key_of(&conflict_request),
        "换了引用对象也还是同一个键——判「同键异体」是服务端的事"
    );
    assert!(
        conflict_request.contains(&format!("\"reply_to_message_id\":\"{OTHER_TARGET}\"")),
        "冲突那一次带的是**新**引用对象: {conflict_request}"
    );
    assert_eq!(fake.connections(), 3, "每次投递恰好一次请求，冲突不重发");
}

#[test]
fn a_malformed_reply_target_or_mention_set_never_reaches_the_wire() {
    let fake = FakeIm::spawn(vec![]);
    let mut app = fake.app();
    let base = || {
        ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "你好")
    };

    // 引用对象：形状不对（空 / 非 UUID / 大写 / 长度 / 去连字符 / 非十六进制）一律本地拒绝。
    let bad_targets = [
        "",
        "not-a-uuid",
        "7B2F5A91-2C4D-4F6E-8A10-5D9C3E77BB21",
        "3f1c9a02-5b7e-4d61-9a2c-77e0b4d51f3",
        "3f1c9a025b7e4d619a2c77e0b4d51f38",
        "3f1c9a02-5b7e-4d61-9a2c-77e0b4d51f3g",
    ];
    for bad in bad_targets {
        let outcome = call(&mut app, say(base().with(PARAM_REPLY_TO, bad)));
        assert_eq!(outcome.failure_class(), Some("invalid-argument"), "{bad}");
    }

    // 提及集合：空 / 非 UUID / 重复 / 空段 / 超过 64 个 —— 全部本地拒绝。
    let duplicated = format!("{MENTION_A},{MENTION_A}");
    let empty_piece = format!("{MENTION_A},,{MENTION_B}");
    let mut over = String::new();
    for index in 0..=MAX_MENTION_TARGETS {
        if index > 0 {
            over.push(',');
        }
        over.push_str(&format!("9d2b7f10-4c58-4e0a-8b31-{index:012x}"));
    }
    let bad_sets = [
        String::new(),
        "not-a-uuid".to_string(),
        duplicated,
        empty_piece,
        over,
    ];
    for bad in bad_sets {
        let outcome = call(&mut app, say(base().with(PARAM_MENTIONS, bad.clone())));
        assert_eq!(outcome.failure_class(), Some("invalid-argument"), "{bad}");
    }

    // 恰好 64 个（上限之内）⇒ 合法：这里只证它**不**在本地被拒（回执由脚本给）。
    let fake_ok = FakeIm::spawn(vec![(201, say_body(MESSAGE, false))]);
    let mut app_ok = fake_ok.app();
    let mut at_limit: Vec<String> = Vec::new();
    for index in 0..MAX_MENTION_TARGETS {
        at_limit.push(format!("9d2b7f10-4c58-4e0a-8b31-{index:012x}"));
    }
    let outcome = call(
        &mut app_ok,
        say(base().with(PARAM_MENTIONS, at_limit.join(","))),
    );
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    assert_eq!(fake_ok.connections(), 1, "上限之内要真的发出去");

    // 复核与说话**同一套判据**（"参数完全相同"这条承诺）：非法值同样零出网。
    let outcome = call(
        &mut app,
        verify_said(base().with(PARAM_REPLY_TO, "not-a-uuid")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));
    let outcome = call(
        &mut app,
        verify_said(base().with(PARAM_MENTIONS, "not-a-uuid")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));

    assert_eq!(fake.connections(), 0, "参数非法一次都不出网");
}

#[test]
fn a_refused_reply_or_mention_target_is_one_class_without_an_existence_oracle() {
    // 契约 §6.5：撤回 / 未知 / 跨房 / 历史窗口不可见 ⇒ 同一个 `400 invalid_reply_target`，
    // **不泄露是否因撤回**。执行端只给一个分类，且回执里除分类外没有别的字。
    for body in [
        "{\"error\":\"invalid_reply_target\"}",
        "{\"error\":\"synthetic\"}",
    ] {
        let fake = FakeIm::spawn(vec![(400, body.to_string())]);
        let mut app = fake.app();
        let outcome = call(
            &mut app,
            say(ActionParams::none()
                .with(PARAM_ROOM, "r-1")
                .with(PARAM_BODY, "收到，我来办")
                .with(PARAM_REPLY_TO, REPLY_TARGET)),
        );
        assert_eq!(
            outcome.failure_class(),
            Some("not-a-reply-target"),
            "引用面被拒只有一个分类: {body}"
        );
        assert!(
            text_of(&outcome).is_empty(),
            "失败面零新事实：不带正文块，因此也不泄露「为什么」被拒"
        );
        assert_eq!(fake.connections(), 1, "被拒也不重发");
    }

    // 提及面同形（`CORE_V1_MENTION_TARGETS.md` §5：非成员 / 非 active / 重复 / 形状 / 超 64
    // 合成同一个 `400 invalid_mention`）。
    let fake = FakeIm::spawn(vec![(400, "{\"error\":\"invalid_mention\"}".to_string())]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "收到，我来办")
            .with(PARAM_MENTIONS, MENTION_A)),
    );
    assert_eq!(outcome.failure_class(), Some("not-a-mention-target"));

    // 两类**同场**：服务端把两类的拒绝合成同一个状态码 ⇒ 本层不指认是哪一类（不猜、不分解）。
    let fake = FakeIm::spawn(vec![(400, "{\"error\":\"invalid_mention\"}".to_string())]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "收到，我来办")
            .with(PARAM_REPLY_TO, REPLY_TARGET)
            .with(PARAM_MENTIONS, MENTION_A)),
    );
    assert_eq!(
        outcome.failure_class(),
        Some("invalid-argument"),
        "同场 400 无法可靠归因 ⇒ 合并分类，不猜是哪一类"
    );

    // 与 `file` 同场：那时的 400 也可能来自**附件面**（`invalid_attachment`）⇒ 同样不指认。
    let world_root = world("reply-with-file");
    put(&world_root, "notes/hello.md", b"# hello\n");
    let fake = FakeIm::spawn_scripted(vec![
        Scripted::Json(201, upload_body(ATTACHMENT)),
        Scripted::Json(400, "{\"error\":\"invalid_reply_target\"}".to_string()),
    ]);
    let mut app = fake.app_with_world(&world_root);
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "收到，我来办")
            .with(PARAM_REPLY_TO, REPLY_TARGET)
            .with(PARAM_FILE, "notes/hello.md")),
    );
    assert_eq!(
        outcome.failure_class(),
        Some("invalid-argument"),
        "带附件时 400 可能来自附件面 ⇒ 不指认是哪一类"
    );

    // 负对照：不带任何引用时的 400 仍是既有分类（不回归）。
    let fake = FakeIm::spawn(vec![(400, "{\"error\":\"synthetic\"}".to_string())]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        say(ActionParams::none()
            .with(PARAM_ROOM, "r-1")
            .with(PARAM_BODY, "你好")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-argument"));
}

#[test]
fn the_said_verification_judges_the_reply_and_mention_dimensions_too() {
    let body_only = ActionParams::none()
        .with(PARAM_ROOM, "r-1")
        .with(PARAM_BODY, "带文件的问候");

    // ① 舞台上只有一条**引用了 REPLY_TARGET** 的我发的消息：带同一个引用复核 ⇒ 找到，回执自白维度。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_reply()),
    ]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        verify_said(body_only.clone().with(PARAM_REPLY_TO, REPLY_TARGET)),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(text.contains(&format!("found = {MESSAGE}")), "{text}");
    assert!(
        text.contains("compared = body + reply target"),
        "比了引用维度就必须说出来: {text}"
    );

    // ② 不带引用复核 ⇒ 那条是回复 ⇒ 不同形 ⇒ not-found（少了这一维就是误判）。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_reply()),
    ]);
    let mut app = fake.app();
    let outcome = call(&mut app, verify_said(body_only.clone()));
    assert_eq!(
        outcome.failure_class(),
        Some("not-found"),
        "带引用的消息不能被纯正文复核认领"
    );

    // ③ 引用的是**另一条** ⇒ 也不是同一条。
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, history_with_my_reply()),
    ]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        verify_said(body_only.clone().with(PARAM_REPLY_TO, OTHER_TARGET)),
    );
    assert_eq!(outcome.failure_class(), Some("not-found"));

    // ④ 提及维度：舞台那条点名 {A,B}；复核给 {B,A}（**集合语义**：顺序不计）⇒ 找到。
    let with_mentions = history_with_extra(serde_json::json!({"mentions": [MENTION_A, MENTION_B]}));
    let fake = FakeIm::spawn(vec![
        (200, me_body(IM_PARTICIPANT)),
        (200, with_mentions.clone()),
    ]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        verify_said(
            body_only
                .clone()
                .with(PARAM_MENTIONS, format!("{MENTION_B},{MENTION_A}")),
        ),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(text.contains(&format!("found = {MESSAGE}")), "{text}");
    assert!(text.contains("compared = body + mention set"), "{text}");

    // ⑤ 提及集合少一个 ⇒ 不是同一条；不给提及复核那条 ⇒ 也不是同一条。
    for params in [
        body_only.clone().with(PARAM_MENTIONS, MENTION_A),
        body_only.clone(),
    ] {
        let fake = FakeIm::spawn(vec![
            (200, me_body(IM_PARTICIPANT)),
            (200, with_mentions.clone()),
        ]);
        let mut app = fake.app();
        let outcome = call(&mut app, verify_said(params));
        assert_eq!(outcome.failure_class(), Some("not-found"));
    }

    // ⑥ 两维同时比 ⇒ 回执把两维都列出来（逐条列名，不混成一团）。
    let both = history_with_extra(serde_json::json!({
        "reply_to_message_id": REPLY_TARGET,
        "mentions": [MENTION_A]
    }));
    let fake = FakeIm::spawn(vec![(200, me_body(IM_PARTICIPANT)), (200, both)]);
    let mut app = fake.app();
    let outcome = call(
        &mut app,
        verify_said(
            body_only
                .with(PARAM_REPLY_TO, REPLY_TARGET)
                .with(PARAM_MENTIONS, MENTION_A),
        ),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(
        text.contains("compared = body + reply target + mention set"),
        "{text}"
    );
}

#[test]
fn the_say_declaration_carries_the_reply_and_mention_parameters() {
    let fake = FakeIm::spawn(vec![]);
    let app = fake.app();
    let declared = app.declared_capabilities();
    let mut checked = 0;
    for capability in declared.items() {
        let id = capability.id().as_str().to_string();
        if id != CAPABILITY_SAY && id != CAPABILITY_VERIFY_SAID {
            continue;
        }
        let names: Vec<&str> = capability.params().iter().map(|spec| spec.name()).collect();
        assert!(
            names.contains(&PARAM_REPLY_TO),
            "{id} 必须声明 {PARAM_REPLY_TO}: {names:?}"
        );
        assert!(
            names.contains(&PARAM_MENTIONS),
            "{id} 必须声明 {PARAM_MENTIONS}: {names:?}"
        );
        for spec in capability.params() {
            if spec.name() == PARAM_REPLY_TO || spec.name() == PARAM_MENTIONS {
                assert!(
                    !spec.is_required(),
                    "{id} 的 {} 是可选参数（不给就是普通消息）",
                    spec.name()
                );
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 2, "说话与复核两条都要在声明里");
}
