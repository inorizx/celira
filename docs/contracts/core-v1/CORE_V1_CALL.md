# Celira Core V1 Call Contract（Call P0：点到点 WebRTC 实时协调窄解冻切片）

> 状态：本切片是 Core v1 封板政策下**已批准执行的窄解冻切片**（非持久、进程内、有界、可过期的实时协调）。只新增「通话实时状态与信令协调」的临时协议与连接；**未新增任何 SQLite 表/列/migration**，**未修改任何既有冻结持久语义**，**不产生、不改写** Message、Human Event、Agent Event、Completion、ACK、Membership、Permission、Binding、Credential。
>
> 媒体（音频/视频）以 **WebRTC P2P** 在两个参与者的端到端客户端之间直连，**Core 永不接收、存储或转发任何媒体字节**；Core 只协调通话状态机并中继 **纯信令控制消息**（SDP offer/answer 与 ICE candidate）。`media` 只是振铃/UI 展示的提示字段，Core 不解析媒体、不从 SDP 推断。本切片是 Call P0：点到点、不落库、无群呼、无媒体面。
>
> **发起方与应答方都既可以是 Human 也可以是 Agent**：任意两个 active Participant 都可互相呼叫（Human↔Human / Human↔Agent / Agent↔Agent；Human 经 Human Session、Agent 经 Backend Credential）。
>
> 本文件是 Call 的唯一权威契约；冻结基线、验收与解冻政策见 `CORE_V1_ACCEPTANCE.md`。
>
> ⚠ **2026-09-28（Completion / ACK 整体退役并删除）**：本文提到的 Completion 与 ACK（旧投递模型）**已连端点带表删除**（`completions` / `agent_delivery_positions`、`POST …/events/{event_id}/completion`、`POST …/events/{event_id}/ack`、`c1.be.ack` 哨兵与相关错误码；授权与原理见 `CORE_V1_AGENT_PROTOCOL.md` 文档头）。本文凡出现它们的句子都是**这一刀当时的范围声明**（"不改 X / 不产生 X"）——那句话本身仍然成立，只是里面的对照物已经不在了；按历史基线阅读。

## 1. 目标、范围与拓扑（C4 L1–2）

### 1.1 一句话原理

通话媒体是 P2P 直连的临时音/视频流，与 Core 公共事实无关；Core 只做**有界、可过期、进程内**的通话状态协调与信令中继——信令是临时的控制消息，绝不是持久消息，也绝不承载媒体。

### 1.2 C4 Level 1–2 拓扑

```text
  Caller Client ◄──── WebRTC 音/视频 P2P（媒体端到端，Core 不可见） ────► Callee Client
（Human Session，或                                                    （Human Session，或
 Agent 执行端经 Backend Credential）                                    Agent 执行端经 Backend Credential）
        │  信令（SDP/ICE 控制消息，非持久）            信令
        └───────────────►  Celira Core ◄──────────────────┘
                              │  仅协调 Call 状态机 + 中继信令（进程内、即时）
                              └── 不写 SQLite、不写 Message/Event、不存媒体
```

- 呈现（Client）持有 WebRTC 端到端媒体端点；公共事实（Core）唯一裁决 Call 状态并中继信令；执行/连接平面不进入本切片；
- **媒体面完全在两端客户端之间**：Core 是信令/状态的透明协调者，不是媒体路径中继、TURN 或 SFU；
- **任意两个 active Participant 可互相呼叫**：Caller 可为 Human（`POST /v1/calls`，Human Session）或 Agent（`POST /v1/backend/calls`，Backend Credential）；Callee 同理可为 Human 或 Agent。

### 1.3 必须保持的边界（P0 非目标）

- **不做媒体面**：无 TURN/SFU/网关/混音/录制；Core 不接触音频/视频字节；
- **不做群呼/多方**：一次 Call 严格恰有两个参与者；
- **不做持久通话记录**：Call 状态是进程内投影，重启即失忆（fail closed）；不写通话日志/时长/录音；
- **不新增持久事实**：不新增 SQLite 表/列/migration；不产生 Message/Human Event/Agent Event/Completion/ACK；
- **不改房间/成员/权限**：不新增 room kind，不改 `POST /v1/direct-rooms`、members、membership、Owner、Permission；
- 跨 Home、PSTN/外部平台桥接、群呼/多方、屏幕共享、设备/铃声管理不做；
- Caller 与 Callee 的信令连接断开不改变 Call 状态（媒体 P2P 由两端负责），客户端经 GET 重同步。

## 2. 公共事实模型（全部进程内、非持久、有界、可过期）

Call 是 Core **进程内**的临时协调投影，**不写 SQLite**；Core 重启即丢失全部 Call 与信号（客户端以 404 或流重置 fail closed 收敛）。依据「非持久投影」先例（`../../../AGENTS.md` 架构红线「主体进行态边界」，旧称 Live State 边界）。

### 2.1 `Call`（进程内对象，非持久）

| 字段 | 类型 | 说明 |
|---|---|---|
| `call_id` | UUID v4 | Core 生成（严格规范小写连字符，契约纪律同公共 cursor 载荷 ID） |
| `room_id` | RoomId | 双方共同归属的 Room（取自家成员关系裁决，请求不得伪造） |
| `caller_id` | ParticipantId | 发起方（Human 或 Agent） |
| `callee_id` | ParticipantId | 被叫方（Human 或 Agent） |
| `media` | enum | `audio` / `video`（振铃/UI 提示；Core 不解析媒体、不从 SDP 推断） |
| `state` | enum | `ringing` / `in_progress` / `ended` |
| `ended_reason` | 可空 enum | `NULL` / `hangup_caller` / `hangup_callee` / `decline_callee` / `timeout_ring` |
| `created_at` / `accepted_at` / `ended_at` | UtcMillis | Core 生成，客户端不可指定 |
| `peer` 投影 | 最小公共身份 | caller/callee 的 `participant_id`、`display_name`（同 Live snapshot 纪律：绝不含 Credential/Binding/token/hash/内部 sequence） |

- 至少一个 Call 的参与者在**调用时刻必须是该 Room 的 active 成员**；Caller ≠ Callee；双方 Participant 均 `active`；
- Active 周期纪律：同一 Participant 同时至多处于**一个** active Call（`ringing | in_progress`）；否则 409 `call_party_busy`；
- 存在性纪律：任何 REST 对非参与者的未知/非本 Call 一律 404 `call_not_found`（无存在性 oracle）。

### 2.2 信令消息（进程内中继）

- 每条信令是**纯控制消息**：`{call_id, from_id, to_id, kind, payload}`，`kind ∈ {offer, answer, ice}`；
- `payload`：`offer`/`answer` 为单个 SDP 字符串（UTF-8，`1..=16384` bytes，禁 U+0000，**不 trim、不解析**）；`ice` 为 `1..=32` 条结构化 candidate，每条为 `{candidate, sdp_mid, sdp_mline_index}`，candidate 文本 `1..=2048` bytes，`sdp_mid` 可空且有界，`sdp_mline_index` 可空且有界；Core 不解析 candidate 文本；
- Core **不校验语义、不复用、不持久化** SDP/ICE；只做形状/字节/NUL 边界校验后中转；
- 信令是**同步即时中继**的纯控制消息（不排队、不持久化）：Core 在收到 `signal` 的同一调用里把它投递给对端**当前已连接**的 call-stream 连接；对端无连接或送达通道满则**静默丢弃**（fail closed，媒体面由 RTC 重协商/客户端重发兜底）。信令不经受持久 TTL——投递时立即生效或立即丢弃；
- 绝不把信令写入 Message/Event/`events.available`，绝不记录媒体。

## 3. 状态机

```text
         POST /v1/calls | POST /v1/backend/calls
[发起] ───────────────► ringing ──accept──► in_progress ──hangup(任一方)──► ended
   │                        │                                     （hangup_caller / hangup_callee）
   │                        ├─decline(仅 callee)──► ended(decline_callee)
   │                        └─ring 超时（Core 固定 45s）──► ended(timeout_ring)
   └─发起方 hangup/取消──► ended(hangup_caller)
```

- `ringing` 中转 `in_progress` 的**唯一**路径是 callee `accept`；`ringing` 下发起方可 `hangup` 取消；
- `ended` 是终态；同一 Call 的后续动作**与任何信令**一律 409 `call_state_conflict`（ended 后 Core 不再接受 offer/answer/ice）；
- ring 超时由 Core 固定 `45s`，不接受调用方自定义；超时后向双方发 `state=ended(timeout_ring)`（或发起方已离开则定向）；
- 媒体是否实际连通由两端 RTC 决定，**Core 不猜测、不设 in_progress 强制 TTL**；任一方 `hangup` 或信令通道失效后，以状态机为准。

## 4. 连接与信令中继

### 4.1 Call-stream（实时唤醒 + 信令投递通道）

与 Human Event WS 使用**独立 notifier / 独立流**，不污染 `events.available` 语义。每连接有界唤醒队列容量 `1`；溢出合并为一次唤醒，客户端以 `GET /v1/calls/{id}` 重同步。

- Human：`GET /v1/call/stream`（Human Session）
- Backend：`GET /v1/backend/call/stream`（Backend Credential，代表该 Agent 参与方）

流事件（只发与本人相关的投影）：

| 事件 | 触发 | 受众 |
|---|---|---|
| `{"type":"call.ring","call":…}` | 新 Call 建立 | 仅 callee 的流 |
| `{"type":"call.signal","call_id","from_id","kind","payload"}` | 收到对端信令 | to_id 一方 |
| `{"type":"call.state","call":…}` | state / ended_reason 变更 | caller 与 callee 双方的流 |

- 首帧复用 authenticate（5s 超时）与安全关闭语义（同 Live WS）；连接关闭或 Core 重启即会话失效，客户端 404/流重置 fail closed 收敛；
- **不重放**：连接断开后错过的信令不补发（只投当前在线的连接）；客户端以 GET 重同步当前 Call 状态，媒体由 RTC 协商/重协商兜底。

### 4.2 中继方向

- offer：只可 caller → callee；answer：只可 callee → caller；ice：双向 trickle；
- 每方 `signal` 只可发生在本方所属 Call（caller 或 callee），且只能发给对方；经 Core 中转，**双方对端身份永远来自 Core 裁决**；
- Core **强制**方向：方向/角色错误（如 caller 发 answer、callee 发 offer）与终态（ended）后发送一律 409 `call_state_conflict`，绝不把信令投递给错误对端或当作普通中继。

## 5. 公共 API（REST）

路由沿用项目 convention：Human Session 挂 `/v1/calls/...`；Backend（Agent 执行端）挂 `/v1/backend/calls/...`。所有非 2xx 用统一 JSON 信封（`error.code/message/request_id` 与 `x-request-id` 一致）。

| 方法 | 路径 | 认证 | 语义 |
|---|---|---|---|
| `POST` | `/v1/calls` | Human Session | Human 发起：`{room_id, callee_id, media?}`（media 缺省 audio）；201 `{call, created}`；同 (room, caller, callee) 已有 active Call 则 200 返回既有（避免重复振铃） |
| `POST` | `/v1/backend/calls` | Backend Credential | Agent 发起：请求同上；语义同上（caller 为该 Credential 代表的 Agent） |
| `GET` | `/v1/calls/{call_id}` | 参与者 | 200 `{call}`；非参与者/未知 404 |
| `POST` | `/v1/calls/{call_id}/accept` | 仅 callee | `ringing→in_progress`；200；非 callee→404；非 ringing→409 |
| `POST` | `/v1/calls/{call_id}/decline` | 仅 callee | `ringing→ended(decline_callee)`；200；非 callee→404 |
| `POST` | `/v1/calls/{call_id}/hangup` | caller 或 callee | 200；`ended(hangup_caller|hangup_callee)`（按 action 方） |
| `POST` | `/v1/calls/{call_id}/signal` | caller 或 callee | 中继信令至对端流；204（对端无流则静默丢弃） |
| `POST` | `/v1/backend/calls/{call_id}/accept` | 该 Call 中 Agent callee | 同 accept 语义 |
| `POST` | `/v1/backend/calls/{call_id}/decline` | 该 Call 中 Agent callee | 同 decline 语义 |
| `POST` | `/v1/backend/calls/{call_id}/hangup` | 该 Call 中 Agent 方 | 同 hangup 语义 |
| `POST` | `/v1/backend/calls/{call_id}/signal` | 该 Call 中 Agent 方 | 同 signal 语义（按该方是 caller 还是 callee 的身份规则定向对端） |

- Backend 端点只接受代表**该 Call 中 Agent 参与方**的 Credential；Agent 无 Binding/非本 Call 一律 404 `call_not_found`（no oracle）；
- REST 重试：`signal`/`hangup` 等对终态的重复动作幂等收敛为既有权裁决；发起复用「同 (room, caller, callee) 既有 active Call」避免重复振铃——不新建立即通过既有返回实现无状态幂等；本切片**不引入**持久的 `PUT` 幂等记录（无持久事实可记）。

## 6. 边界与非目标（P0 明确排除）

- 媒体面（WebRTC 采集/传输/TURN/SFU/混音/录制/降噪）不属于 Core，不在此契约；Core 不解析媒体、不从 SDP 推断 `media`；
- 群呼/多方、跨 Home、PSTN/第三方桥接、屏幕共享、设备与铃声管理、通话记录/时长/录音持久化；
- 不产生 Message/Human Event/Agent Event/Completion/ACK，不引入第二公共事实写者，不触碰 Runtime 私有状态；
- 不新增 SQLite 表/列/migration；Call 与信令生命周期不改动静止数据库；
- Call 不以任何方式成为 Membership、Owner、Access、Permission 或 Binding 的输入。

## 7. 边界与错误映射（稳定公共 code）

| 条件 | HTTP | code |
|---|---|---|
| JSON/字段/参数/形状不合法（含 media 枚举非法） | 400 | `invalid_request` |
| SDP/ICE 越界、禁 NUL、字段序/形状错误 | 400 | `call_payload_invalid` |
| 缺凭据/无效会话 | 401 | `authentication_required` / `invalid_session` |
| Backend 凭据无效 | 401 | `invalid_credential` |
| 未知/非本 Call/非参与者 | 404 | `call_not_found`（不泄露存在性） |
| 当前状态不允许该动作（accept 非 ringing、已 ended 再操作等；信令方向/角色错误；ended 后任何 signal） | 409 | `call_state_conflict` |
| Caller 或 Callee 已在另一 active Call | 409 | `call_party_busy`（仅同 Room 成员间） |
| 进程内有界资源上限 | 503 | `call_state_limit_exceeded` |
| 基础设施/存储不可用 | 500/503 | `internal_error` / `storage_unavailable`（既有分类） |

错误面纪律：任何端点不得经错误或响应推断非本 Call 参与者、非 Room 成员的活跃 Call 状态或内部 bound。

## 8. 数据库不变量边界

- 本切片**不写 SQLite**：Call/信令操作前后不改变任何表的行数、约束或 schema（可在 Live 口径测试中用探针断言「无越界对象」）；
- 不产生 Message/Human Event/Agent Event/Completion/ACK/新 Room/Membership/Permission/Binding/Credential；
- 信令与媒体字节永不落盘、不进日志（错误与审计仅脱敏记录 call_id/状态转换，绝不记录 SDP/ICE/candidate 内容）。

## 9. 验证与汇报口径

实现后的验证命令（如实汇报结果，不做直读库冒充成功证据）：

```bash
cargo fmt --all --check
cargo check --all-targets
cargo clippy --all-targets
cargo build --bin celira-core
cargo test --all-targets -p celira-core
```

行为证明来自公开协议/真实进程黑盒：独立 Core 进程，Human 客户端 + （Agent 场景）Backend 客户端建立呼入/应答/挂断/超时/忙碌，验证信令正确中转、404/409/503 稳定、WebRTC 媒体 P2P 由两端承担、SQLite 行数与 schema 不变、Core 重启后全部 Call 失忆并以 fail-closed 404 收敛。P2P 媒体打洞（NAT/防火墙）属 WebRTC 传输面，不在本验收；可标记为人工/网络环境观察项。

## 10. 测试证据（实现后增补）

- HTTP 路由：发起（Human/Agent 各入口）/应答/拒接/挂断/呼入超时/忙碌/404 与状态机冲突（`call_state_conflict`）、media 缺省/显式/非法；
- 信令中继：offer/answer/ice 方向正确（offer 仅 caller→callee、answer 仅 callee→caller、ice 双向）、方向/角色错误与 ended 后信令一律 409 `call_state_conflict`、禁 NUL/越界 400、无连接静默丢弃、对端身份永远来自 Core 裁决；
- Live/不变量：Call 操作前后 SQLite 行数不变（无越界对象）、重启失忆 fail closed；
- 真实进程黑盒：端到端 Human↔Human、Human↔Agent、Agent↔Agent 各一条主链 + 失败路径，媒体字节不出现在 Core 进程任何写入面。

## 11. Host Call 全链路浏览器验收口径（真实进程黑盒）

> ⚠ **2026-09-20 退役标注**：本节描述的浏览器验收通道——**远程模式 Local Client Host（`celira-local-host`）、
> Host BFF 与 `host/scripts/e2e_call_browser.sh`——已于 2026-09-20 随 Host 全托管路径退役并整体删除**
> （`host/` crate 与前端 Host 模块均已从工作树移除，考古走 Git 历史）。本节作为**验收口径的历史记录**保留，
> **不再是可执行命令**；本次退役不改 Call 的契约语义（状态机、信令方向、错误码由 §2–§8 承载，一字未改）。

一真实 celira-core + 两台远程模式 celira-local-host Host / 同一 Home / 两个不同
Human Session + 两个隔离 CloakBrowser 上下文（既有仓库二进制，不下载/不安装浏览器；
fake 摄像头/麦克风仅由 Chromium 标志提供：`--use-fake-device-for-media-stream`、
`--use-fake-ui-for-media-stream`）+ 内置 web dist。执行脚本：
`host/scripts/e2e_call_browser.sh`。

- 准备：只用 Core operator/公共 API 建两名 Human、accepted Contact 与一个
  Human↔Human direct room（不出现浏览器侧事实）；各自 Host 经 operator 控制端点
  绑定；浏览器以 Host BFF bootstrap token 引导后走真实 UI 路径；
- 路径：浏览器 A 定位 direct room → 验证 ChatHeader 个人音/视频呼叫按钮存在 → 点击
  audio 呼叫 → B 出现真实来电浮层 → 点击「接听」→ 双方浮层均显示「通话中」；
- 挂断：A 点击「挂断」→ 双方浮层关闭/进入 ended 态；随后独立跑一次 video 呼叫并
  单独显式报告结果（UI 路径确定性：视频按钮 →「接听」→ 双方「通话中 · 视频」）；
- 媒体口径：**P2P 媒体状态以 UNVERIFIED 如实报告**（仓库无既有 `__CELIRA_CALL_DEBUG__`
  模式、测试不得为变绿改动产品行为；生产 bundle 不暴露其 RTCPeerConnection）。
  通话状态机（ringing/in_progress/ended）与信令闭环由上面的权威状态断言证明。
  P2P 打洞（NAT/防火墙）属 WebRTC 传输面，不在本切片验收；维持 `CORE_V1_ACCEPTANCE.md`
  的既有人工/网络环境观察项；
- 安全：Call BFF 全部浏览器请求同源、仅携带会话 Cookie + CSRF（无 Authorization、
  无 Idempotency-Key）、无任何直连 Core 来源；URL/DOM/storage/console 与 Host/Core
  日志 0 秘密；WebRTC/console 错误以脱敏稳定分类捕获；
- fail closed：真实浏览器路径无法 bootstrap 或 UI 缺少稳定选择器时脚本以明确 blocker
  失败，绝不制造假绿。有界超时；清理只回收脚本自有的 child PID / 浏览器上下文 /
  临时文件（不打扫自有 PID 之外的陈旧进程）。
