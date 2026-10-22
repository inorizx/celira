# Celira Architecture

> **状态**：唯一 C4 Level 1–2 系统地图。本文同时陈述**当前实现**与**目标架构**，并逐处标注差距——目标节点不因写在这里就算已实现。
>
> **当前实现事实**以本文 §3 / §6、各组件的活文档（[`im/server/README.md`](./im/server/README.md)、[`client/README.md`](./client/README.md)、[`agent/README.md`](./agent/README.md)、[`app/README.md`](./app/README.md)）与版本化契约为准。产品方向见 [`docs/constitution/PRODUCT_VISION.md`](./docs/constitution/PRODUCT_VISION.md)。
>
> **退役线与退役实现不在本仓工作树**：历史记录见 [`docs/history/ARCHITECTURE_RETIRED.md`](./docs/history/ARCHITECTURE_RETIRED.md)。本仓的 Git 历史自**首次公开提交**起，退役代码不在其中（边界声明见 [`README.md`](./README.md) 的 Repository map 末段）。
>
> 不读中文的读者请先读下面的 **English summary**——它是自足的。

## English summary

Self-contained for readers who do not read Chinese. The Chinese sections below carry the full map;
this summary states the same architecture in short.

**Terms, once.** **A home** is one account domain — a household's or circle's persistent private
communication space. **A world** is the logical im server that is the single writer of that home's
public facts (identities, contacts, rooms, memberships, permissions, messages, events). ⚠ Frozen
contracts write the *world* with the proper name `Home`; the machine-side word "home" (`home.js`,
`HOME_*` keys) means the execution-end machine's data directory, not the account domain. **A
runtime** (also: execution end) is the machine-side process that runs agents; it owns the model key,
memory, tools and runs, and is not a participant. **An agent** is the public participant the world
knows *and* the private subject a runtime executes. **A Run** is one bounded waking period with an
explicit ending; silence is a legitimate ending.

**Three product roles.**
1. **Client** — presentation plane: the page a human uses. It is configured with exactly one world
   and accepts only that world's public facts as final.
2. **World** — public-fact plane: one writable logical im server per home, the single adjudicator of
   public facts and their order.
3. **Agent runtime** — execution plane: N execution ends on N machines, each hosting one or more
   isolated agents; private state never leaves them.

**Not product roles.** A **Connector** is connection-plane plumbing (protocol closure for an
execution end that dials in); **Host** is horizontal deployment orchestration (start, health check,
restart, reclaim). Neither is a participant, neither owns public facts, neither confers privilege.

**Current implementation.**
- **World**: one Rust binary (`celira-im`) plus one SQLite file, single node, single writable
  instance. Its public contract (Core v1) is frozen. Two kinds of counterpart: `/v1` for humans
  (Bearer Human Session), the Agent Backend protocol for runtimes (per-agent Backend credential).
- **Client**: the Vite/React web client (`client/`), either in a browser or served as `client/dist`
  inside the Tauri desktop shell (`app/`, Windows). No mobile client yet — Android is planned, iOS
  is out of scope.
- **Runtime**: one native runtime, the standalone crate `agent/` (its own workspace): resident host
  `agent_host` (a wake-up doorbell plus event pull → one Run), one-shot `agent_run`, read-only
  `agent_host_status`. Model access is BYOK; tools are Apps — capability packages (files, notes, im,
  web, terminal, MCP) — declared explicitly per agent at assembly time.
- **Orchestration**: `node home.js up|status|doors|down` where there is no shell; the desktop shell
  does the same job on Windows. There is no separate Host process.

**Three connections.**
1. **Client → world**: public `/v1` over HTTP with a Bearer Human Session, plus
   `GET /v1/human/events/stream` (WebSocket; wake-up only, no facts) and `GET /v1/rooms/{id}/live`
   (in-process presence/typing projection).
2. **Runtime → world**: the Agent Backend protocol with a per-agent Backend credential —
   `GET /v1/backend/events?after=&limit=` (pull), `POST /v1/backend/rooms/{room_id}/messages`
   (speak), and the doorbell `GET /v1/backend/events/stream` (SSE, `events.available` only, zero
   facts). Polling is always the complete path; the doorbell is only a latency optimisation.
3. **Client → the local runtime's machine door**: loopback HTTP on the home (`/status`, `/healthz`,
   `/home-config`; default ports 5190/5191), guarded by the loopback binding — or by a machine-level
   key when bound beyond loopback — and never by a world identity. Three "logins" exist and none
   implies another: the machine door (carries no identity), the world identity (Human Session /
   Bearer), and a world admin role (does not exist yet).

**Main data flow.** A human speaks or `@`-mentions an agent → the world authenticates, writes the
public message idempotently, and atomically creates the agent event → the runtime bound to that
agent pulls the event (the doorbell only says "something is there") → it runs a Run in private →
it *explicitly* speaks back through the public backend protocol → the world writes that as a public
message. Model output is private by default; speaking is an act, not a by-product.

**Fact ownership.** The world owns public facts and their order. The runtime owns private state:
model key, sessions, memory, tools, runs, audit. **The model key never reaches the world**: the
world's SQLite stores no provider credential and no private execution state. The client owns only
local page state.

**Invariants.**
- **One writable world per home.** At any moment exactly one logical im server may write. Backups,
  cold standbys and fenced standbys may exist; a second writer may never.
- **No dual-write, no sync, no merge.** Worlds never share or merge public facts, and no node may
  forge them. Consistency rests on the single-writer invariant, not on replica agreement.
- **No privileged back doors.** Clients, runtimes and connectors reach the world only through public
  protocols; no component reads the world's database, and the world never reads agent private
  storage or executes models or tools.
- **Facts are separated by kind**: public permission (world) vs private capability grant (runtime);
  public identity (world) vs private memory (runtime).

## 1. 目标与原理

Celira 的权威单元是 **Home**：一个家/账号域的持续私人通信空间（持续账号域，不窄化为单个自然人的生命周期）。**任一时刻，一个 Home 有且只有一个可写的逻辑 im 服务端**（下文亦称 **world**）作为公共事实裁决者；一个 Home 由 M 个 Client（呈现平面）与 N 个执行点（执行平面）构成。**不存在双 im 服务端的同步或合并**：逻辑 im 服务端 是单写者，Home 之间不共享、不合并公共事实；托管部署可多租户（多个 Home 共享宿主基础设施），但每个 Home 的公共事实仍只有单权威。备份、冷备与 fencing 待机可以存在，但任一时刻只有一个可写逻辑 im 服务端，从不双写；故障时任何节点都不得伪造公共事实。

**原理声明**：产品角色只有三个——**Client、IM（逻辑 im 服务端，即 world）、Agent**。Connector 是连接平面基础设施；Host 是横向部署编排。二者都不是产品角色，也不是 Participant。四个职责面如下：

| 职责面 | 节点 | 责任 |
|---|---|---|
| 呈现 | Client（M 个） | 界面与应用状态；只配置一个 Home，只承认该 Home 逻辑 im 服务端 的公共事实权威 |
| 公共事实 | 逻辑 im 服务端（每 Home 一个） | 公共身份、关系、房间、准入、消息与公共事件投递/路由的唯一裁决 |
| 执行 | Agent + Runtime/执行端（分布于 N 个执行点） | 私有推理、Session、Run、模型、Tool；一处执行点可承载一个或多个彼此隔离的 Agent 执行端，Agent 是其中具有公共 Participant 身份的主体 |
| 连接 | Connector（Gateway Connector） | 协议闭环；Gateway Connector 只承载执行端以公开协议主动接入时的连接侧映射、重试与实际事件上报，不管理它的内部生命周期 |

**Host 横向部署**：Host 是横向部署编排，就近常驻组件部署，只负责**常驻服务级生命周期（启动、健康检查、重启、回收）**，**不属于连接面或任何职责层**；可托管运行 Runtime、Connector 等常驻组件的进程，但不拥有它们的私有事实，也不因此获得公共事实写者地位或官方特权。Event 的领取与推进属于执行端／连接平面的执行连接职责，不是 Host 职责。

**im 服务端只做公共事件投递/路由**：普通新消息的公共处理主链是 **公共 Message → Agent Event →（拉取）**；撤回通知等其他 Event 按版本化契约定稿。Event 可在无 Binding 时存在；active Binding 只决定哪个 Backend 可观察指定 Agent 的 Event。逻辑 im 服务端 是裁决者与路由者，不是调度 Runtime——不启动进程、不管理 Run、不替主体推理。

**Client 与执行期控制**：Client 只配置一个 Home，只承认该 Home 逻辑 im 服务端 的公共事实权威。这只规定公共事实裁决归属，不等于所有控制流量经 im 服务端进程中转：Run 取消、轨迹等执行期控制可经 **Home 准入授权的独立控制路径或同域路由**承载；它们不成为 im 服务端的耐久公共事实，私有状态仍归执行端。具体机制尚未冻结。公共投递仍走 im 服务端的公开 Agent Backend 契约。

## 2. 目标拓扑（C4 Level 1–2）

```text
参与者：Human（Home 主人）、Agent（执行层 Participant）

┌── 呈现平面（M 个 Client）──────────────────────────┐
│  Web / 桌面 / 手机 Client（同一 Home 的一致体验）    │
└──────────────────────┬─────────────────────────────┘
                       │ 公开客户端契约（公共事实权威）
┌── 公共事实平面（每 Home 一个）─────────────────────┐
│  逻辑 im 服务端（单写者）                                │
│  身份 / 关系 / Room / Membership / Permission /     │
│  Message / Message→Agent Event（供拉取）路由         │
└──────────────────────┬─────────────────────────────┘
                       │ Agent Backend 协议（active Binding 决定
                       │  哪个 Backend 可观察该 Agent 的 Event）
┌── 执行平面（N 个机器/部署位置）────────────────────┐
│  每处可承载一个或多个隔离的 Agent 执行端             │
│  Agent 具有 Participant 身份；Human 与 Agent         │
│  均可成为 Room 成员                                 │
│  Runtime：私有 Session / Run / 模型 / Tool / 凭据   │
└─────────────────────────────────────────────────────┘

连接平面（Connector：Gateway Connector；
          不是产品角色、不是 Participant）
  Gateway Connector —— 执行端以公开协议主动接入时的会话/事件适配层

横向部署编排（不属于任何职责层）
  Host —— 常驻服务级生命周期（启动/健康检查/重启/回收）；
          可托管 Runtime / Connector 等常驻组件进程，
          但不拥有其私有事实
```

要点：

- 逻辑 im 服务端 是每个 Home 公共事实的唯一写者；客户端、执行端与连接平面全部只通过公开协议接入，不因官方身份获得内部特权连接，也没有第二个公共事实写者；
- Client 是跨 Web/桌面/手机一致体验的前端与接口层（今天只有 Web 与 Windows 桌面壳），不持有公共事实终局；
- Agent 是执行层中具有公共 Participant 身份、可为 Room 成员的主体；Human 与 Agent 同为 Participant 且都可成为 Room 成员。承载它们的 Runtime/Connector/Host/进程/Session/Gateway 都不是成员、不是产品角色；
- 连接平面（Connector）与横向部署编排（Host）就近执行点部署，不改变各产品角色边界；
- 双平面不新增职责面：公共世界仍由逻辑 im 服务端 单写；Runtime 私有世界不成为第二个公共事实写者。Runtime 内部把 im 服务端视为渠道，不等于 Celira 产品把 im 服务端降格为即时通讯附件。

> **目标 vs 今天**：Connector 与 Host 是目标架构里的角色，今天**没有独立实现**——执行端直连 world 的 Backend 协议，本机编排由 `home.js`（无壳形态）与桌面壳 `app/`（Windows 形态）承担。它们的边界约束（不是产品角色、不是 Participant、不拥有公共事实）**现在就成立**，与将来是否单独成进程无关。

## 3. 容器职责与事实归属

| 容器 | 拥有的事实与职责 | 明确不拥有 |
|---|---|---|
| 逻辑 im 服务端（Client 眼中的公共事实权威） | Home Identity、Human 身份/Session、Pairing Grant、可选 Session Metadata、公共身份、Room、Membership、Permission、Message、Event、公共顺序；普通新消息的 Message→Event→（拉取）主链与其他 Event 的版本化定稿；Presence/Typing 进程内临时投影（在线由连接给出；窄冻结例外） | 模型凭据、Agent 私有 Session/Memory/工具授权与 Run、本机能力权限、执行端内部权限与审批、进程启动与 Run 调度；Home Identity 也不承担双副本 fencing/HA |
| Client（Web + 桌面壳；今天无手机端） | 统一聊天与管理体验、本地页面状态、用户输入；只配置一个 Home | 公共事实终局、Agent 私有执行事实、节点数据库、权限审批、Control 流量中转 |
| Agent + 执行端（Runtime，分布于执行点） | Agent 私有 Session/Run、模型/BYOK、记忆与能力、执行端私有权限与工具授权；一处执行点可承载多个彼此隔离的 Agent 执行端 | 逻辑 im 服务端 公共事实、其他 Agent 私有状态、Agent 的公共身份与 Room 成员资格（由 im 服务端裁决）、替别的执行端审批 |
| Connector（连接平面，目标角色） | 协议闭环。**Gateway Connector** 承载执行端以公开协议主动接入时的连接侧映射、重试和实际事件上报 | im 服务端数据库、公共事实、其他 Agent 私有状态、官方特权、代表宿主名下其他 Agent、它所接入执行端的内部生命周期 |
| Host（横向部署编排，目标角色，不属于任何职责层） | 常驻服务级生命周期：启动、健康检查、重启、回收；托管运行 Runtime/Connector 等常驻组件的进程 | im 服务端公共事实、Room 成员资格、Event 的领取与推进（属 Runtime/Connector）、以宿主身份代表 Agent、替别的执行端审批、OS 沙箱保证 |

### 当前实现 vs 目标（诚实标注）

| 节点 | 当前实现（存在且已验收） | 目标架构 |
|---|---|---|
| 逻辑 im 服务端（world） | `im/server`：单 Linux / Windows 主机单进程 + SQLite（二进制 `celira-im`）；启动持久化一个非秘密、不可变 Home Identity，并提供 Home / Pairing Grant / Human Session 管理（A 包公共契约）。单 Active im 服务端 仍是部署/运维单写者不变量：Home Identity 不能阻止同备份双启动。Core v1 冻结，仅含已批准的 A 包窄解冻 | 逻辑 im 服务端 概念不变；可多 Client/多执行点接入；不承诺多副本写、迁移或 HA |
| Client | `client/`（Vite + React：浏览器直连 world 的 `/v1`；也可作为 `client/dist` 被桌面壳端出）+ `app/`（Tauri 桌面壳，Windows；同一份构建里也承担本机 world 与本机执行端的编排）。**无手机端**：Android 纯壳未开始，iOS 不做 | Web / 桌面 / 手机多端一致体验（手机端未实现） |
| 执行端（Native Runtime，`agent/`） | 独立 crate、独立 workspace：常驻宿主 `agent_host`（门铃 + 拉取 Event → 一次 Run）、一次性 `agent_run`、只读读数 `agent_host_status`；模型 BYOK（机器级清单 `<家>/model_sources.json` + 支级绑定 `<环境根>/model_binding.json`）；工具面是 App（files / notes / im / web / terminal / MCP）；终端的档位与 MCP 来源由宿主在装配期**显式声明**，不自动扩散到别的 Agent；私有状态落环境根 `<家>/agents/<id>/`（身份、trace / facts 追加存储、环境写锁） | 完整自有 Agent 执行器；多逻辑 Agent 执行路由属后续窄协议 |
| 外部 Agent | **无**（不提供第三方 Agent 适配器），执行端只有 `agent/` | **不设**：不同 Agent 的适配器不需要（主人裁决） |
| 本机编排 | `node home.js up / status / doors / down`（无壳形态：CLI / Linux）与桌面壳 `app/`（Windows 形态）承担：起 world、起本机住户、端两个家口、停机；**没有独立的 Host 进程** | Host 作为横向部署编排的概念保留（见 §1），不成为产品角色 |

## 4. 关键数据流

### 4.1 Human 明确发言或 `@Agent`

```text
Client -> 逻辑 im 服务端 鉴权并提交公共 Message（幂等）
       -> 逻辑 im 服务端 原子建立必要的 Agent Event（Event 可独立于 Binding 存在）
       -> 由 active Binding 决定的 Backend（今天由执行端直连）
          领取 Event 并经执行端私有执行
       -> Backend 提交公共输出提议（普通消息写回）
       -> 逻辑 im 服务端 原子写回为公共事实
```

协议闭环由连接平面承载（Gateway Connector；今天执行端直连，Connector 是目标角色）：它只管理连接侧映射、重试和实际事件上报，不接管它所接入执行端的内部生命周期。Host 只负责相关常驻服务级生命周期（启动/健康检查/重启/回收）。两者都不是公共事实写者。

**发言是显式动作**：执行端在私有 Run 里产生的内容不自动成为公共消息；只有它经公开协议提交的输出提议才由 im 服务端写回为公共事实。沉默是合法的收场。

### 4.2 执行期控制路径

```text
Client / 执行点 <-经 Home 准入授权的独立控制路径或同域路由-> Run 取消 / 轨迹增量
```

规则：控制路径需经 Home 准入授权，只承载执行期控制；其事件不成为 im 服务端的耐久公共事实，执行私有状态仍归执行端，也不能绕开公开 Agent Backend 契约产生公共 Message 或 Agent Event。具体机制尚未冻结，Client 的公共事实权威仍归 Home 逻辑 im 服务端。

### 4.3 故障与恢复

```text
正常：一个可写逻辑 im 服务端（单写者）
故障：可备份 / 冷备 / fencing 待机接替，任一时刻只有一个可写逻辑 im 服务端
禁止：双写、双逻辑 im 服务端 同步 / 合并、备用写者伪造公共事实
```

逻辑 im 服务端 恢复与换桩不得造成公共 Message 重复、乱序或身份漂移；重试与幂等按既有版本化契约终局。Home 的公共事实一致性依赖单写者不变量，不依赖多副本协商。

### 4.4 轨迹边界

轨迹是执行端实际提供的可观察执行事件，Celira 只转发与展示执行端真实提供的内容：

- 执行端（`agent/`）完整展示其真实产生并通过接口公开的可观察执行事件；
- Celira 不虚构执行端没有提供的轨迹，也不把模型未公开的隐藏思维链纳入轨迹承诺；缺失即缺失。

## 5. 允许与禁止的连接

### 允许的连接（当前实现）

```text
页面（浏览器 / 桌面壳内）──公开 `/v1`（Bearer Human Session）──> world
  ├─ REST：身份 / 房间 / 成员 / 消息 / 附件 / 读位置
  ├─ WS `/v1/human/events/stream`：只发无事实唤醒（`events.available`）
  └─ GET `/v1/rooms/{id}/live`：进程内进行态投影（谁在线 / 谁正在写）
执行端（`agent/`）──Agent Backend 协议（BackendAuth）──> world
  ├─ GET `/v1/backend/events?after=&limit=`：拉取（完整的事实获取路径）
  ├─ POST `/v1/backend/rooms/{room_id}/messages`：以参与者身份发言
  └─ GET `/v1/backend/events/stream`：门铃（SSE，只发 `events.available`，零事实）
页面 / 操作者 ──家口（loopback HTTP；绑非 loopback 时凭家钥匙）──> 本机执行端的门
  ├─ 只读状态口 `/status`、`/healthz`（默认 5190）
  └─ 模型配置门 `/home-config`、`/home-binding`、`/home-config/probe`（默认 5191）
```

所有连接必须可认证、可授权、可撤销，并遵守相同公共契约。门铃只是及时性优化：轮询永远是完整的事实获取路径，门铃丢了不会丢事实。

### 禁止的连接（当前与未来一致）

```text
Client ──X──> im 服务端数据库
Runtime ──X──> im 服务端数据库
Backend ──X──> im 服务端数据库
im 服务端 ──X──> Agent 私有数据库
im 服务端 ──X──> 模型或 Tool 执行
官方 Runtime ──X──> im 服务端 内部特权 API
im 服务端 ──X──> 把 Presence/Activity 写入 SQLite / Human Event 表 / Cursor / 未读
```

组合发行、同机部署或由同一团队维护，不改变这些边界。

## 6. 当前部署事实（实现快照）

```text
默认单机部署（Linux 或 Windows；一个 world = 一个 celira-im 进程 + 一个 SQLite 文件）；也支持
world 在另一台主机、本机只跑执行端（`node home.js up --im-endpoint ...`），以及跨机照看（家钥匙 + TLS）。
├── world：celira-im（Rust 单二进制 + SQLite，Core v1 公共契约冻结）
│   ├── `/v1` Human 面：身份 / 房间 / 消息 / 事件 / 附件 / 读位置
│   ├── Agent Backend 面：拉取 / 发言 / 门铃 SSE
│   ├── Human Event WS `/v1/human/events/stream`（只发无事实唤醒，不代理事实正文）
│   ├── `GET /v1/rooms/{id}/live` ＋ `POST /v1/rooms/{id}/typing`：进行态读写面（进程内临时投影，无推送线）
│   └── SQLite（公共事实；不存模型凭据、不存 Agent 私有执行状态）
│
├── 执行端：agent/（独立 crate、独立 workspace）
│   ├── agent_host：常驻宿主（门铃 + 拉取 Event → 一次 Run）
│   ├── agent_run：一次性命令（init / run）
│   ├── agent_host_status：只读读数（谁 / 什么状态 / 该不该出手）
│   └── 环境根 `<家>/agents/<id>/`：身份、trace / facts 追加存储、笔记、模型绑定
│
├── 页面：client/dist（浏览器直连；或由桌面壳端出）
├── 桌面壳：celira-app（Tauri，Windows；一体包 = 壳 + world + 执行端）
└── 本机编排：node home.js up|status|doors|down（无壳形态）；桌面壳内同一职责（Windows）
```

当前是单 world、单节点、单一写入裁决者，已满足「一个 Home 任一时刻一个可写逻辑 im 服务端」的架构不变量。当前不实现多副本写入、跨 im 服务端身份、双 im 服务端同步/合并、远程房间投影或冲突解决；备份/冷备/fencing 待机与多租户托管不属于当前实现。

## 7. 授权、成员与生命周期

- **Credential** 归属于 Binding，可轮换、可撤销，只代表指定 Agent，不代表承载它的宿主；若某个执行实例需要独立撤销，它应有独立的 Binding 与所属 Credential，不发明「一个 Binding 对应多执行实例凭据」的语义；Credential 不等于身份、成员资格或房间权限；
- **同一 Agent 同时至多一个 active Binding 是 V1 当前限制，不是永久架构承诺**；active Binding 只决定哪个 Backend 可观察该 Agent 的 Event，不代表 Backend 成为 Participant，也不自动产生 Room Membership；Credential 轮换与 Binding 变更不重置投递位置；
- **Human 与 Agent 都是 Participant 且都可成为 Room 成员**：Runtime、Connector、Host、进程、Session、Gateway 都不是 Room 成员，不进入公共成员事实；
- **Connector 负责协议闭环**：Gateway Connector 只管理执行端以公开协议主动接入后的连接侧映射、重试与实际事件上报，不管理它的内部生命周期；**Host 只负责常驻服务级生命周期**（启动/健康检查/重启/回收）；
- **自动发现只产生候选，主人确认后才建立 Binding**：任何「发现连接」都不得在不经主人确认的情况下建立 Agent Binding 或成员资格；
- **平台公共 Permission（im 服务端裁决）与执行端私有 Capability 授权（Runtime/宿主内部）是两类不同事实**：后者不进入 Core v1 公共契约，也不赋予跨宿主能力；
- **三种「登录」是三种不同的东西，互不共享、互不认**：① **执行端的门**——防公网随便进，凭机器级秘密进入，**不携带身份**，不回答「你是谁」；② **im 服务端的身份**——Human Session / Bearer，回答「你是这个世界的谁」；③ **im 服务端上的管理角色**——**同一身份上的权限**，不是第二套账号（当前不存在，方向见 `docs/constitution/PRODUCT_VISION.md` 的「未来形态」）。你在 im 服务端是什么身份，与能否进入某台执行端**无关**。

## 8. 扩展与演进纪律

- 每增加一层抽象，都必须证明它降低当前复杂度；
- 新能力必须有可验证的验收路径，不能只为想象中的未来建设；
- 不同宿主与连接组件（Runtime、Connector）各自持有私有状态与私有能力边界，不共享数据库，不产生跨节点隐式特权；
- 组合发行、同机部署或由同一团队维护，不改变边界；
- MCP 只能作为执行端 Tool Provider 来源（**已实现**：宿主 `--mcp` 显式声明、默认档 `unverifiable-write`、可声明 `read-only`；第一版**冻结、不再扩**，见 `agent/README.md` §3.29 与 §6）；
- Celira 不建立统一的执行端能力或审批模型；执行期控制只允许经 **Home 准入授权的独立控制路径或同域路由**，不成为 im 服务端的耐久公共事实，私有状态仍归执行端；机制（执行端自己的口 + 家地址 + 家钥匙）与粒度归属见 `docs/constitution/PRODUCT_VISION.md` 的「Client 与执行期控制」，**家口远程档已实现**（家侧 `node home.js issue-home-key`；`home.conf` 的 `status_bind` / `config_bind` 与 `tls_cert` / `tls_key`；客户端「设置 → 家」）；
- 所有扩展不得引入第二个公共事实写者，不得建立双 im 服务端同步或合并，不得把连接平面（Connector）基础设施或横向部署编排（Host）升格为产品角色或 Participant。

## 9. 明确不做

- 为尚无真实消费者的自动参与机制建立默认拓扑或公共协议；
- 双 im 服务端同步/合并、备用写者或任何形式的公共事实双写；
- 一个跨 im 服务端、执行端、Host 与本地宿主的万能插件 Context；
- im 服务端普通第三方行为插件或插件直写数据库；
- im 服务端或官方组件替执行端管理权限、审批或沙箱；
- 把 QQ、飞书、Discord 等外部社交平台映射进 Celira 房间模型的 Bridge；
- 用聊天命令承载 Provider、工具、Memory 和权限配置；
- 第三方自由扩展公共持久事件；
- 虚构或承诺执行端没有提供的轨迹；
- 把 Runtime/Connector/Host/进程/Session/Gateway 写成 Room 成员或产品角色。

## 10. 文档关系

- [`docs/constitution/PRODUCT_VISION.md`](./docs/constitution/PRODUCT_VISION.md)：产品目标、价值与目标产品形态方向；
- 本文：唯一系统地图——当前实现、目标架构、容器职责、数据流与跨容器演进门禁；系统级未来拓扑只写于 `docs/constitution/PRODUCT_VISION.md` 与本文，Agent 地基语义由 [`docs/constitution/AGENT_FOUNDATION_CONTRACT.md`](./docs/constitution/AGENT_FOUNDATION_CONTRACT.md) 负责；
- [`docs/contracts/core-v1/CORE_V1_IM_CONTRACT.md`](./docs/contracts/core-v1/CORE_V1_IM_CONTRACT.md) / [`CORE_V1_AGENT_PROTOCOL.md`](./docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md)：Core v1 冻结公共契约；
- [`docs/contracts/core-v1/CORE_V1_ACCEPTANCE.md`](./docs/contracts/core-v1/CORE_V1_ACCEPTANCE.md)：冻结基线、黑盒验收与解冻门禁；
- [`docs/constitution/AGENT_FOUNDATION_CONTRACT.md`](./docs/constitution/AGENT_FOUNDATION_CONTRACT.md)：Agent 地基原语、事实归属、执行生命周期、恢复语义与未来扩展接缝；
- [`docs/runtime/terminal/README.md`](./docs/runtime/terminal/README.md)：Native Runtime 终局理念、终局本体、执行/扩展边界与负责人裁决入口；不覆盖本文容器边界、上层契约或当前实现事实；
- [`docs/runtime/AGENT_VOCABULARY.md`](./docs/runtime/AGENT_VOCABULARY.md)：名字的权威位置——本文用到的词先在这里查；
- **活文档（当前边界与验收入口）**：[`client/README.md`](./client/README.md) / [`client/DESIGN_SYSTEM.md`](./client/DESIGN_SYSTEM.md) / [`im/server/README.md`](./im/server/README.md) / [`agent/README.md`](./agent/README.md)（执行端）/ [`app/README.md`](./app/README.md)（桌面壳）/ [`docs/constitution/PROJECT_SHAPE.md`](./docs/constitution/PROJECT_SHAPE.md)（发布单元与版本责任）/ [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md)（家的命令与故障处置）；
- **退役存档（不是当前实现事实的来源）**：[`docs/history/ARCHITECTURE_RETIRED.md`](./docs/history/ARCHITECTURE_RETIRED.md)（本文移出的退役段与三次退役标注）与 [`docs/client/LOCAL_CLIENT_HOST.md`](./docs/client/LOCAL_CLIENT_HOST.md)（Host BFF 安全模式的历史记录）；
- 未来节点在进入实现前，必须另立窄设计、验收路径和版本化契约。
