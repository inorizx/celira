//! **跟世界说话的那几问**（共享层；口径照 `scripts/home/` 现做法）。
//!
//! ⚠ **这一片不属于任何一个功能**（2026-10-03 从 `anjia/world.rs` 提升到这里）：**安家**
//! 与**本机世界**（起世界、首布置、正门）都从这两条线说话。
//!
//! 本模块只有两条线，都是**既有面**、没有新契约：
//!
//! 1. **operator CLI**（`celira-im`）——补一支 Human Session / 铸首位 Human（一行 JSON），
//!    与 `provision.js` 调 `issue-human-session` / `bootstrap-human` 同一手法：**同一份库配置**
//!    （`CELIRA_LISTEN_ADDR` / `CELIRA_DB_PATH` / `CELIRA_MEDIA_ROOT`）经环境注入
//!    子进程（照 `up.js` 的 `imEnv` 处置）；凭据只经环境、不进 argv、不进日志；
//! 2. **HTTP**（loopback、免钥匙）——`/health`（世界在不在，`up.js` 的就绪判据）、
//!    `GET /v1/agents` 核实「这个 id 是我的、且 active」（`adopt.js` 判据 1/2），
//!    `POST …/backend-bindings` + `POST …/credentials`（签后端凭据，照 `provision.js`
//!    的 `obtainBackendCredential` 重入语义）、`GET /v1/backend/me`（问这份凭据代表谁）。
//!
//! ## 三条纪律
//!
//! - **问不到就不动**：连不上 / 回执读不懂 ⇒ 调用方（mod.rs）拒绝，什么都别写
//!   （写下去的是这个家的身份，猜不得）；
//! - **幂等键是壳自己的名份**：`anjia-…` 前缀。**故意不复用** `scripts/home` 的
//!   `home-binding` 等常量键——那些键按「Human + 键」记账，同一台机器上壳与
//!   `home.js` 若共用一把键处理**不同 Agent**，第二方会撞 `409 idempotency_conflict`
//!   （见 `backend_store.rs` 的记账判据）。键里带 agent id ⇒ 同支重跑是重放、
//!   不同支互不相干；
//! - **原文照带**：失败读数带状态码与回执片段（截断到有界长度），不美化、不吞。

use std::path::Path;
use std::process::Command;

use serde_json::json;

// ---------------------------------------------------------------------------
// operator CLI
// ---------------------------------------------------------------------------

/// 一次 CLI 调用的读数（`run_cli` 的直译：exit / stdout / stderr / 起不来）。
#[derive(Debug, Clone)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

/// 跑一次 operator CLI。**环境叠加**（继承 + 注入），不落文件、不进 argv。
pub fn run_cli(exe: &Path, args: &[&str], env: &[(String, String)]) -> CliOutput {
    let mut command = Command::new(exe);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    // **不给它控制台窗口**（2026-10-08）：CLI 是控制台程序、壳没有控制台 ⇒ 少了这一行
    // 就是"每次签会话 / 铸人闪一下黑框"。返回照旧走 `.output()` 的管道。
    crate::console_window::suppress(&mut command);
    match command.output() {
        Ok(output) => CliOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            error: None,
        },
        Err(error) => CliOutput {
            code: None,
            stdout: String::new(),
            stderr: String::new(),
            error: Some(error.to_string()),
        },
    }
}

/// **同一份库配置**（`up.js` 的 `imEnv` 三件）：服务端与它的 CLI 子命令必须看同一个库，
/// 否则子命令会去开默认路径下的另一个库（bash 版实测踩到）。
pub fn im_env(listen: &str, home_dir: &Path) -> Vec<(String, String)> {
    vec![
        ("CELIRA_LISTEN_ADDR".to_string(), listen.to_string()),
        (
            "CELIRA_DB_PATH".to_string(),
            home_dir.join("im").join("core.db").display().to_string(),
        ),
        (
            "CELIRA_MEDIA_ROOT".to_string(),
            home_dir.join("im").join("media").display().to_string(),
        ),
    ]
}

/// 补一支 Human Session（`issue-human-session --human-id <id>`，一行 JSON ⇒ token）。
///
/// ⚠ 这是**唯一的取 token 路径**（token 只打印一次、壳不存它）：与 `provision.js`
/// 在「Human 已存在」时的做法逐字同款。这里拿到的 token 只活在本次安家调用里。
pub fn issue_human_session(
    im_exe: &Path,
    listen: &str,
    home_dir: &Path,
    human_id: &str,
) -> Result<String, String> {
    let done = run_cli(
        im_exe,
        &["issue-human-session", "--human-id", human_id],
        &im_env(listen, home_dir),
    );
    if done.code != Some(0) {
        let detail = if let Some(error) = &done.error {
            error.clone()
        } else {
            let stderr = done.stderr.trim();
            if stderr.is_empty() {
                format!("退出码 {:?}", done.code)
            } else {
                tail_text(stderr, 3)
            }
        };
        return Err(format!("补发会话失败：{detail}"));
    }
    let token = done
        .stdout
        .lines()
        .find_map(|line| {
            serde_json::from_str::<serde_json::Value>(line.trim())
                .ok()
                .and_then(|value| {
                    value
                        .get("token")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
        })
        .filter(|token| !token.is_empty());
    token.ok_or_else(|| "补发会话的输出读不懂（预期一行 JSON、带 token）".to_string())
}

/// 铸**首位** Human（`bootstrap-human --display-name <NAME>`，一行 JSON ⇒
/// `{ participant_id, token }`）。与 `provision.js` 那一步逐字同款：**token 只在这里出现一次**，
/// 调用方拿到之后必须**立刻**用掉（记 `human_id`、把 token 交给页面），不落盘、不打印。
///
/// ⚠ 这个世界已有别人时它会**失败**（服务端占全局 guard，重复/并发只有一次成功）——
/// 那是**如实失败**，不是"再来一次就好了"。
pub fn bootstrap_human(
    im_exe: &Path,
    listen: &str,
    home_dir: &Path,
    display_name: &str,
) -> Result<(String, String), String> {
    let done = run_cli(
        im_exe,
        &["bootstrap-human", "--display-name", display_name],
        &im_env(listen, home_dir),
    );
    if done.code != Some(0) {
        let detail = if let Some(error) = &done.error {
            error.clone()
        } else {
            let stderr = done.stderr.trim();
            if stderr.is_empty() {
                format!("退出码 {:?}", done.code)
            } else {
                tail_text(stderr, 3)
            }
        };
        return Err(format!("铸首位 Human 失败：{detail}"));
    }
    let opened = done
        .stdout
        .lines()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .ok_or_else(|| "铸首位 Human 的输出读不懂（预期一行 JSON）".to_string())?;
    let human_id = opened
        .get("participant_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "铸首位 Human 的输出里没有 participant_id".to_string())?
        .to_string();
    let token = opened
        .get("token")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "铸首位 Human 的输出里没有 token".to_string())?
        .to_string();
    Ok((human_id, token))
}

/// 末尾几行（读数用；原样、不加工）。
pub fn tail_text(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

// ---------------------------------------------------------------------------
// HTTP（loopback、免钥匙）
// ---------------------------------------------------------------------------

/// 一次 HTTP 问答：拿到了回执（含非 2xx）；`Transport` = 根本没答上。
#[derive(Debug, Clone)]
pub enum HttpAnswer {
    Reply {
        status: u16,
        /// `None` = 回执读不懂（不是 JSON / 没正文）。
        body: Option<serde_json::Value>,
    },
    Transport(String),
}

pub struct WorldHttp {
    client: reqwest::blocking::Client,
    base: String,
}

impl WorldHttp {
    pub fn new(base: &str) -> Result<Self, String> {
        // **no_proxy 是有意的**：本条线只跟 loopback 说话，而机器上可能配了
        // 全局 http_proxy（本机实测就有）——把它带去 loopback 会把本机流量拐出机器。
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|error| format!("HTTP 客户端起不来：{error}"))?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    fn get(&self, path: &str, bearer: Option<&str>) -> HttpAnswer {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        self.finish(request.send())
    }

    /// **主体与主人的那间 `direct` 房**（`adopt.js` 的 `findExistingDirectRoom` 直译）。
    ///
    /// 一次 `GET /v1/rooms` 就够：清单里每条自带 `peer`（对端）——"这个人类与这支 Agent
    /// 之间的那间房"是**问得出来的机械事实**，不用猜、也不用逐间拉成员。
    ///
    /// 两态：`Ok(Some(id))` 找到了 / `Ok(None)` 问到了但没有那间 / `Err(detail)` 问不到。
    pub fn find_direct_room(
        &self,
        human_token: &str,
        agent_id: &str,
    ) -> Result<Option<String>, String> {
        match self.get("/v1/rooms", Some(human_token)) {
            HttpAnswer::Reply { status: 200, body } => {
                let rooms = body
                    .and_then(|value| value.get("rooms").cloned())
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default();
                for entry in rooms {
                    let kind = entry
                        .get("room")
                        .and_then(|room| room.get("kind"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let peer = entry
                        .get("peer")
                        .and_then(|peer| peer.get("id"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    if kind != "direct" || peer != agent_id {
                        continue;
                    }
                    let id = entry
                        .get("room")
                        .and_then(|room| room.get("id"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    return Ok((!id.is_empty()).then(|| id.to_string()));
                }
                Ok(None)
            }
            HttpAnswer::Reply { status, body } => {
                let digest = match body {
                    Some(value) => tail_text(&value.to_string(), 2),
                    None => "（回执读不懂）".to_string(),
                };
                Err(format!("读房间清单被拒（HTTP {status}）：{digest}"))
            }
            HttpAnswer::Transport(detail) => Err(format!("读房间清单时连不上：{detail}")),
        }
    }

    /// **这支凭据代表谁**（`GET /v1/me`，公开协议面）。
    ///
    /// 给"会话拿到之后报一句读数"用：`session` 那条路只回 token 与 id，而
    /// "首位 Human 的显示名到底是什么"是要**看得见**的（它是公共呈现，不是内部字段）。
    /// 读不到 ⇒ `None`（不猜、不编）。
    pub fn me_display_name(base: &str, token: &str) -> Option<String> {
        let http = WorldHttp::new(base).ok()?;
        match http.get("/v1/me", Some(token)) {
            HttpAnswer::Reply { status: 200, body } => body?
                .get("participant")?
                .get("display_name")?
                .as_str()
                .map(|name| name.to_string()),
            _ => None,
        }
    }

    /// **主体在不在线**（`GET /v1/rooms/<房>/live` —— 主体进行态的唯一裁决面）。
    ///
    /// 判据在契约里：**在线由连接派生**（门铃流 / 最近被看见），不是对端上报的身体读数
    /// （`CORE_V1_AGENT_PROTOCOL.md` §6.1）。壳用它做**起住户的安全闸**：账本说本机没有
    /// 主体的宿主、而世界说它在线 ⇒ 多半别处有宿主 ⇒ 不替你起第二个。
    ///
    /// 三态：`Ok(Some(true/false))` 问到了 / `Ok(None)` 问到了但没有这个人 / `Err` 问不到。
    pub fn room_live_online(
        &self,
        human_token: &str,
        room_id: &str,
        participant_id: &str,
    ) -> Result<Option<bool>, String> {
        match self.get(&format!("/v1/rooms/{room_id}/live"), Some(human_token)) {
            HttpAnswer::Reply { status: 200, body } => {
                let members = body
                    .and_then(|value| value.get("members").cloned())
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default();
                for member in members {
                    let id = member
                        .get("participant_id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    if id != participant_id {
                        continue;
                    }
                    return Ok(Some(
                        member
                            .get("online")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                    ));
                }
                Ok(None)
            }
            HttpAnswer::Reply { status, body } => {
                let digest = match body {
                    Some(value) => tail_text(&value.to_string(), 2),
                    None => "（回执读不懂）".to_string(),
                };
                Err(format!("读在场快照被拒（HTTP {status}）：{digest}"))
            }
            HttpAnswer::Transport(detail) => Err(format!("读在场快照时连不上：{detail}")),
        }
    }

    /// **世界在不在**：`GET /health` 回 200（`up.js` 的就绪判据，一字不差）。
    ///
    /// 与 `up.js` 同款的一条诚实：这条只证明"**有人**在地址上答 200"。判"那是不是我们起的
    /// 那个"要另一手（`up.js` 用 pid 账本核）——见 [`crate::local_world`] 的处置。
    pub fn health_ok(&self) -> bool {
        matches!(
            self.get("/health", None),
            HttpAnswer::Reply { status: 200, .. }
        )
    }

    /// 世界答的那个**状态码**（`None` = 连不上 / 回执读不懂）。
    ///
    /// 为什么要有它（而不只是 [`Self::health_ok`]）：状态口要把"没在跑"与"读不到"分开说 ——
    /// 连不上时**不能**给一个数字（`0` 不是 HTTP 状态码，页面上会变成"没在跑（HTTP 0）"，
    /// 那是把"读不到"说成了一个具体读数；`status-server.js` 上真机踩过这一脚）。
    pub fn health_code(&self) -> Option<u16> {
        match self.get("/health", None) {
            HttpAnswer::Reply { status, .. } => Some(status),
            HttpAnswer::Transport(_) => None,
        }
    }

    fn post_json(
        &self,
        path: &str,
        bearer: Option<&str>,
        idempotency_key: Option<&str>,
        body: &serde_json::Value,
    ) -> HttpAnswer {
        let mut request = self.client.post(format!("{}{path}", self.base)).json(body);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        self.finish(request.send())
    }

    fn finish(&self, result: Result<reqwest::blocking::Response, reqwest::Error>) -> HttpAnswer {
        match result {
            Ok(response) => {
                let status = response.status().as_u16();
                let text = response.text().unwrap_or_default();
                let body = serde_json::from_str::<serde_json::Value>(&text).ok();
                HttpAnswer::Reply { status, body }
            }
            Err(error) => HttpAnswer::Transport(format!("连不上：{error}")),
        }
    }
}

/// 回执片段（读数用；有界，不吞）：`HTTP <status>` + 至多 ~200 字符的正文。
pub fn answer_brief(answer: &HttpAnswer) -> String {
    match answer {
        HttpAnswer::Transport(detail) => detail.clone(),
        HttpAnswer::Reply { status, body } => match body {
            Some(value) => {
                let text = value.to_string();
                let mut brief: String = text.chars().take(200).collect();
                if text.chars().count() > 200 {
                    brief.push('…');
                }
                format!("HTTP {status}：{brief}")
            }
            None => format!("HTTP {status}（回执读不懂）"),
        },
    }
}

fn field<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

// ---------------------------------------------------------------------------
// 核实：这个 id 是我的、且 active（adopt.js 判据 1/2 的直译）
// ---------------------------------------------------------------------------

/// `GET /v1/agents` 的裁决。`reason` 的四个取值与 `adopt.js` 同款：
/// `unreachable` / `not-owned` / `not-active` / `ok`。
#[derive(Debug, Clone)]
pub enum OwnedVerdict {
    /// 问到了：是你的、且 active。`name` 是它在这个世界里的名字。
    Ok { name: String },
    /// 问不到（连不上 / 回执读不懂）——**什么都不许动**。
    Unreachable { detail: String },
    /// 世界里没有这个 id，或者它不是你的（`GET /v1/agents` 只列你拥有的）。
    NotOwned { total: usize },
    /// 不是 active 的姿态（撤销过的不许安家）。
    NotActive { status: String },
}

/// 对一份 `GET /v1/agents` 回执做裁决（**纯函数** ⇒ 可机械判错）。
pub fn verdict_of_agents(answer: &HttpAnswer, agent_id: &str) -> OwnedVerdict {
    let (status, body) = match answer {
        HttpAnswer::Transport(detail) => {
            return OwnedVerdict::Unreachable {
                detail: detail.clone(),
            };
        }
        HttpAnswer::Reply { status, body } => (*status, body),
    };
    if status != 200 {
        return OwnedVerdict::Unreachable {
            detail: format!("GET /v1/agents → {status}"),
        };
    }
    let agents = match body.as_ref().and_then(|value| field(value, &["agents"])) {
        Some(serde_json::Value::Array(agents)) => agents,
        _ => {
            return OwnedVerdict::Unreachable {
                detail: "回执读不懂（没有 agents 列表）".to_string(),
            };
        }
    };
    let found = agents.iter().find(|entry| {
        field(entry, &["agent", "id"])
            .and_then(|value| value.as_str())
            .map(|id| id == agent_id)
            .unwrap_or(false)
    });
    let Some(found) = found else {
        return OwnedVerdict::NotOwned {
            total: agents.len(),
        };
    };
    let status_value = field(found, &["agent", "status"])
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if status_value != "active" {
        return OwnedVerdict::NotActive {
            status: if status_value.is_empty() {
                "未知".to_string()
            } else {
                status_value.to_string()
            },
        };
    }
    let name = field(found, &["agent", "display_name"])
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(agent_id);
    OwnedVerdict::Ok {
        name: name.to_string(),
    }
}

/// `GET /v1/backend/me` ⇒ 这份凭据代表的身份（`{id, displayName}`；与 `up.js`
/// 的 `readIdentity` 同一份回执。问不到 ⇒ `Ok(None)`——那是**合法**状态，
/// 起进程时不带「是谁」那两行，不编）。
pub fn identity_of_me(
    answer: &HttpAnswer,
    expected_agent: &str,
) -> Result<Option<(String, String)>, String> {
    match answer {
        HttpAnswer::Reply { status, body } if *status == 200 => {
            let id = body
                .as_ref()
                .and_then(|value| field(value, &["agent", "id"]))
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let name = body
                .as_ref()
                .and_then(|value| field(value, &["agent", "display_name"]))
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let (Some(id), Some(name)) = (id, name) else {
                return Ok(None);
            };
            if id != expected_agent {
                return Err(format!(
                    "这份凭据代表的是别人（me → {id}，本家是 {expected_agent}）"
                ));
            }
            Ok(Some((id, name)))
        }
        HttpAnswer::Reply { status, .. } => Err(format!("me → {status}（这份凭据没通过）")),
        HttpAnswer::Transport(detail) => Err(detail.clone()),
    }
}

// ---------------------------------------------------------------------------
// 签后端凭据（provision.js `obtainBackendCredential` 的直译 + 壳自己的幂等键）
// ---------------------------------------------------------------------------

/// 幂等键：`anjia-` 前缀 + agent id（同支重跑是重放，不同支互不相干）。
pub fn binding_key(agent_id: &str) -> String {
    format!("anjia-binding-{agent_id}")
}

pub fn credential_key(agent_id: &str) -> String {
    format!("anjia-credential-{agent_id}")
}

/// 轮换键：钉在**被替换的那条凭据 id** 上（照 `provision.js`：
/// 同一状态重试会重放，一旦轮换成功状态就变、键也跟着变 ⇒ 不会连转两次）。
pub fn credential_replace_key(replaced_id: &str) -> String {
    format!("anjia-credential-replace-{replaced_id}")
}

fn token_of(answer: &HttpAnswer) -> Option<String> {
    match answer {
        HttpAnswer::Reply { body, .. } => body
            .as_ref()
            .and_then(|value| field(value, &["token"]))
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .filter(|token| !token.is_empty()),
        HttpAnswer::Transport(_) => None,
    }
}

/// 取一支**能用的**后端凭据原文（可重入版，TASK_59 语义的直译）。
///
/// 三条路：
/// - `201`：这一次签发的 ⇒ 直接用 `token`；
/// - `200` / `409 credential_state_conflict`：这条凭据**已经存在**（原文只给一次）
///   ⇒ 读一次现有凭据、拿 id 当 `replaces_credential_id` **换一支**；
/// - 其余：失败。
///
/// ⚠ **绝不在没有原文的情况下返回成功** —— 那会把「响亮失败」变成「静默坏掉」。
pub fn obtain_backend_credential(
    http: &WorldHttp,
    human_token: &str,
    agent_id: &str,
    binding_id: &str,
) -> Result<String, String> {
    let url = format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials");
    let issued = http.post_json(
        &url,
        Some(human_token),
        Some(&credential_key(agent_id)),
        &json!({ "replaces_credential_id": null }),
    );
    if let HttpAnswer::Reply { status, .. } = &issued {
        if *status == 201 {
            return token_of(&issued)
                .ok_or_else(|| format!("签发后端凭据的响应里没有原文：{}", answer_brief(&issued)));
        }
        if *status == 200 || *status == 409 {
            // 走到这里：凭据已存在，「原文只给一次」⇒ 必须轮换一支。
            let current = http.get(&url, Some(human_token));
            let list = match &current {
                HttpAnswer::Reply { status: 200, body } => body
                    .as_ref()
                    .and_then(|value| field(value, &["credentials"]))
                    .and_then(|value| value.as_array())
                    .cloned(),
                _ => None,
            };
            let Some(list) = list else {
                return Err(format!("读现有后端凭据失败：{}", answer_brief(&current)));
            };
            let replaced = list
                .iter()
                .find(|item| item.get("status").and_then(|v| v.as_str()) == Some("active"))
                .or_else(|| list.first());
            let Some(replaced_id) = replaced
                .and_then(|item| item.get("id"))
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
            else {
                return Err(format!(
                    "没有可替换的后端凭据，却也不给签发新的：{}",
                    answer_brief(&issued)
                ));
            };
            let rotated = http.post_json(
                &url,
                Some(human_token),
                Some(&credential_replace_key(replaced_id)),
                &json!({ "replaces_credential_id": replaced_id }),
            );
            if let HttpAnswer::Reply { status, .. } = &rotated {
                if *status == 201 {
                    return token_of(&rotated).ok_or_else(|| {
                        format!("轮换后端凭据的响应里没有原文：{}", answer_brief(&rotated))
                    });
                }
            }
            return Err(format!("轮换后端凭据失败：{}", answer_brief(&rotated)));
        }
    }
    Err(format!("签发后端凭据失败：{}", answer_brief(&issued)))
}

/// 建 Binding 并把 binding id 拿出来（`provision.js` 读了 `body.binding.id`）。
pub fn create_binding_and_id(
    http: &WorldHttp,
    human_token: &str,
    agent_id: &str,
) -> Result<String, String> {
    let answer = http.post_json(
        &format!("/v1/agents/{agent_id}/backend-bindings"),
        Some(human_token),
        Some(&binding_key(agent_id)),
        &json!({ "label": "home" }),
    );
    match &answer {
        HttpAnswer::Reply { status, body } if *status == 201 || *status == 200 => body
            .as_ref()
            .and_then(|value| field(value, &["binding", "id"]))
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "建 Backend Binding 的响应里没有 binding id：{}",
                    answer_brief(&answer)
                )
            }),
        _ => Err(format!(
            "建 Backend Binding 失败：{}",
            answer_brief(&answer)
        )),
    }
}

// ---------------------------------------------------------------------------
// 便利出口（mod.rs 用的三问一签）
// ---------------------------------------------------------------------------

pub struct World {
    pub http: WorldHttp,
    human_token: String,
    agent_id: String,
}

impl World {
    pub fn open(base: &str, human_token: String, agent_id: String) -> Result<Self, String> {
        let http = WorldHttp::new(base)?;
        Ok(Self {
            http,
            human_token,
            agent_id,
        })
    }

    /// 核实「这个 id 是我的、且 active」。
    pub fn verify_owned(&self) -> OwnedVerdict {
        let answer = self.http.get("/v1/agents", Some(&self.human_token));
        verdict_of_agents(&answer, &self.agent_id)
    }

    /// **这支 Agent 与主人的那间 `direct` 房**（住户登记里 `room_id` 那一格的正路）。
    ///
    /// 为什么在客户端上再包一层：Human token 只在它手里（`open` 收走），
    /// 而这一问要的正是"以主人身份看房间清单"。
    pub fn find_own_direct_room(&self) -> Result<Option<String>, String> {
        self.http
            .find_direct_room(&self.human_token, &self.agent_id)
    }

    /// 签后端凭据（建 binding + 签发/轮换）。返回凭据原文。
    pub fn sign_backend_credential(&self) -> Result<String, String> {
        let binding_id = create_binding_and_id(&self.http, &self.human_token, &self.agent_id)?;
        obtain_backend_credential(&self.http, &self.human_token, &self.agent_id, &binding_id)
    }

    /// 这份凭据代表谁（给「是谁」那一行）。
    pub fn identity(&self, backend_token: &str) -> Result<Option<(String, String)>, String> {
        let answer = self.http.get("/v1/backend/me", Some(backend_token));
        identity_of_me(&answer, &self.agent_id)
    }
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(status: u16, body: serde_json::Value) -> HttpAnswer {
        HttpAnswer::Reply {
            status,
            body: Some(body),
        }
    }

    #[test]
    fn verdict_covers_all_four_states() {
        let agent_id = "a-1";
        let list = reply(
            200,
            json!({
                "agents": [
                    { "agent": { "id": "other", "display_name": "别人", "status": "active" } },
                    { "agent": { "id": "a-1", "display_name": "小满", "status": "active" } },
                ]
            }),
        );
        match verdict_of_agents(&list, agent_id) {
            OwnedVerdict::Ok { name } => assert_eq!(name, "小满"),
            other => panic!("应当认领成功：{other:?}"),
        }

        // 撤销过的不许安家。
        let revoked = reply(
            200,
            json!({ "agents": [ { "agent": { "id": "a-1", "display_name": "小满", "status": "revoked" } } ] }),
        );
        assert!(matches!(
            verdict_of_agents(&revoked, agent_id),
            OwnedVerdict::NotActive { .. }
        ));

        // 不在列表里 ⇒ not-owned。
        let absent = reply(
            200,
            json!({ "agents": [ { "agent": { "id": "other", "status": "active" } } ] }),
        );
        assert!(matches!(
            verdict_of_agents(&absent, agent_id),
            OwnedVerdict::NotOwned { total: 1 }
        ));

        // 连不上 / 非 200 / 回执读不懂 ⇒ 一律 unreachable，绝不当成「不是你的」。
        assert!(matches!(
            verdict_of_agents(&HttpAnswer::Transport("连不上：refused".into()), agent_id),
            OwnedVerdict::Unreachable { .. }
        ));
        assert!(matches!(
            verdict_of_agents(&reply(502, json!({})), agent_id),
            OwnedVerdict::Unreachable { .. }
        ));
        assert!(matches!(
            verdict_of_agents(
                &HttpAnswer::Reply {
                    status: 200,
                    body: None
                },
                agent_id
            ),
            OwnedVerdict::Unreachable { .. }
        ));
        assert!(matches!(
            verdict_of_agents(&reply(200, json!({ "nope": [] })), agent_id),
            OwnedVerdict::Unreachable { .. }
        ));
    }

    #[test]
    fn identity_of_me_checks_who_the_credential_represents() {
        let ok = reply(
            200,
            json!({ "agent": { "id": "a-1", "display_name": "小满" } }),
        );
        assert_eq!(
            identity_of_me(&ok, "a-1").unwrap(),
            Some(("a-1".to_string(), "小满".to_string()))
        );
        // 代表别人 ⇒ 明确拒绝（不是「读不到」）。
        let someone_else = reply(
            200,
            json!({ "agent": { "id": "b-2", "display_name": "别的" } }),
        );
        assert!(identity_of_me(&someone_else, "a-1").is_err());
        // 没给 id/名字 ⇒ 合法缺席（不编）。
        let shapeless = reply(200, json!({ "agent": {} }));
        assert_eq!(identity_of_me(&shapeless, "a-1").unwrap(), None);
        // 没通过（401 等）⇒ 失败。
        assert!(identity_of_me(&reply(401, json!({})), "a-1").is_err());
    }

    #[test]
    fn idempotency_keys_are_shell_owned_and_agent_scoped() {
        assert_eq!(binding_key("a-1"), "anjia-binding-a-1");
        assert_eq!(credential_key("a-1"), "anjia-credential-a-1");
        assert_eq!(
            credential_replace_key("cred-9"),
            "anjia-credential-replace-cred-9"
        );
        // 键长在 1–128 可见 ASCII 内（域层判据），且不含逗号。
        for key in [
            binding_key("1c2f8a10-3f5e-4a01-9a6d-2f4d0a1b2c3d"),
            credential_key("1c2f8a10-3f5e-4a01-9a6d-2f4d0a1b2c3d"),
            credential_replace_key("1c2f8a10-3f5e-4a01-9a6d-2f4d0a1b2c3d"),
        ] {
            assert!(key.len() <= 128);
            assert!(!key.contains(','));
            assert!(key.bytes().all(|b| (0x21..=0x7E).contains(&b)));
        }
    }

    #[test]
    fn cli_missing_binary_is_reported_not_panicked() {
        let done = run_cli(Path::new("/definitely/not/here/celira-im"), &["x"], &[]);
        assert!(done.code.is_none());
        assert!(done.error.is_some());
    }
}
