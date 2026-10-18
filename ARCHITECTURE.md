# Celira Architecture

> 状态：唯一 C4 Level 1–2 系统地图。本文同时陈述**当前实现**与**目标架构**，并清楚标注差距；不表示所有目标节点已经实现。当前实现事实以本文与各组件的活文档（`im/server/README.md`、`client/README.md`、`agent/README.md`）和版本化契约为准（`docs/client/LOCAL_CLIENT_HOST.md` 是**退役存档**，不作为当前实现事实的来源）；Native Runtime 的旧线实现已于 2026-09-17 退役（`agent/runtime/` 与 `agent/acp-supervisor/` 已从工作树删除，考古走 Git 历史）；接替者是新线 `agent/`（仍在建设期，未部署）。**Local Client Host 线（`host/` crate 与前端 Host 模块）已于 2026-09-20 随 Host 全托管路径退役并从工作树整体删除**（考古走 Git 历史）——本文 §3、§5、§6 中凡把它写成当前容器/当前连接/当前部署的地方已就地标注为**已退役并删除**；**本机编排现在是根目录 `home.js`（`node home.js up`），浏览器进登录页直连 im 服务端**。产品方向见 `docs/constitution/PRODUCT_VISION.md`。

## 1. 目标与原理

Celira 的权威单元是 **Home**：一个家/账号域的持续私人通信空间（持续账号域，不窄化为单个自然人的生命周期）。**任一时刻，一个 Home 有且只有一个可写的逻辑 im 服务端** 作为公共事实裁决者；一个 Home 由 M 个 Client（呈现平面）与 N 个执行点（执行平面）构成。**不存在双 im 服务端的同步或合并**：逻辑 im 服务端 是单写者，Home 之间不共享、不合并公共事实；托管部署可多租户（多个 Home 共享宿主基础设施），但每个 Home 的公共事实仍只有单权威。备份、冷备与 fencing 待机可以存在，但任一时刻只有一个可写逻辑 im 服务端，从不双写；故障时任何节点都不得伪造公共事实。

**原理声明**：产品角色只有三个——**Client、IM、Agent**。Connector 是连接平面基础设施；Host 是横向部署编排。二者都不是产品角色，也不是 Participant。四个职责面如下：

| 职责面 | 节点 | 责任 |
|---|---|---|
| 呈现 | Client（M 个） | 界面与应用状态；只配置一个 Home，只承认该 Home 逻辑 im 服务端 的公共事实权威 |
| 公共事实 | 逻辑 im 服务端（每 Home 一个） | 公共身份、关系、房间、准入、消息与公共事件投递/路由的唯一裁决 |
| 执行 | Agent + Runtime/执行端（分布于 N 个执行点） | 私有推理、Session、Run、模型、Tool；一处执行点可承载一个或多个彼此隔离的 Agent 执行端，Agent 是其中具有公共 Participant 身份的主体 |
| 连接 | Connector（Gateway Connector） | 协议闭环；Gateway Connector 只承载执行端以公开协议主动接入时的连接侧映射、重试与实际事件上报，不管理它的内部生命周期 |

**Host 横向部署**：Host 是横向部署编排，就近常驻组件部署，只负责**常驻服务级生命周期（启动、健康检查、重启、回收）**，**不属于连接面或任何职责层**；可托管运行 Runtime、Connector 等常驻组件的进程，但不拥有它们的私有事实，也不因此获得公共事实写者地位或官方特权。durable claim、Event 状态、Completion/ACK 桥接（旧线机制，2026-09-24 退役）等是 Runtime/Connector 的执行连接职责，不是 Host 职责。

**im 服务端只做公共事件投递/路由**：普通新消息的公共处理主链是 **公共 Message → Agent Event →（拉取）**（2026-09-24 主人裁决退役、2026-09-28 代码面删除完毕）；撤回通知等其他 Event 按版本化契约定稿。Event 可在无 Binding 时存在；active Binding 只决定哪个 Backend 可观察指定 Agent 的 Event。逻辑 im 服务端 是裁决者与路由者，不是调度 Runtime——不启动进程、不管理 Run、不替主体推理。

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
- Client 是跨 Web/桌面/手机一致体验的前端与接口层，不持有公共事实终局；
- Agent 是执行层中具有公共 Participant 身份、可为 Room 成员的主体；Human 与 Agent 同为 Participant 且都可成为 Room 成员。承载它们的 Runtime/Connector/Host/进程/Session/Gateway 都不是成员、不是产品角色；
- 连接平面（Connector）与横向部署编排（Host）就近执行点部署，不改变各产品角色边界；
- 双平面不新增职责面：公共世界仍由逻辑 im 服务端 单写；Runtime 私有世界不成为第二个公共事实写者。Runtime 内部把 im 服务端视为渠道，不等于 Celira 产品把 im 服务端降格为即时通讯附件。

## 3. 容器职责与事实归属

| 容器 | 拥有的事实与职责 | 明确不拥有 |
|---|---|---|
| 逻辑 im 服务端（Client 眼中的公共事实权威） | Home Identity、Human 身份/Session、Pairing Grant、可选 Session Metadata、公共身份、Room、Membership、Permission、Message、Event、公共顺序；普通新消息的 Message→Event→（拉取）主链与其他 Event 的版本化定稿（旧线含 Completion、ACK 与其投递裁决，2026-09-24 退役）；Presence/Activity 进程内临时投影（窄冻结例外） | 模型凭据、Agent 私有 Session/Memory/工具授权与 Run、本机能力权限、执行端内部权限与审批、进程启动与 Run 调度；Home Identity 也不承担双副本 fencing/HA |
| Client（Web/桌面/手机） | 统一聊天与管理体验、本地页面状态、用户输入；只配置一个 Home | 公共事实终局、Agent 私有执行事实、节点数据库、权限审批、Control 流量中转 |
| Agent + 执行端（Runtime，分布于执行点） | Agent 私有 Session/Run、模型/BYOK、记忆与能力、执行端私有权限与工具授权；一处执行点可承载多个彼此隔离的 Agent 执行端 | 逻辑 im 服务端 公共事实、其他 Agent 私有状态、Agent 的公共身份与 Room 成员资格（由 im 服务端裁决）、替别的执行端审批 |
| Connector（连接平面） | 协议闭环。**Gateway Connector** 承载执行端以公开协议主动接入时的连接侧映射、重试和实际事件上报 | im 服务端数据库、公共事实、其他 Agent 私有状态、官方特权、代表宿主名下其他 Agent、它所接入执行端的内部生命周期 |
| Host（横向部署编排，不属于任何职责层） | 常驻服务级生命周期：启动、健康检查、重启、回收；托管运行 Runtime/Connector 等常驻组件的进程 | im 服务端公共事实、Room 成员资格、durable claim/Event 状态/Completion-ACK 桥接（属 Runtime/Connector）、以宿主身份代表 Agent、替别的执行端审批、OS 沙箱保证 |

### 当前实现 vs 目标（诚实标注）

| 节点 | 当前实现（存在且已验收） | 目标架构 |
|---|---|---|
| im 服务端 | 单 Linux 主机单进程 + SQLite；启动持久化一个非秘密、不可变 Home Identity，并提供 Home/Pairing Grant/Session 管理 A 包公共契约。单 Active im 服务端 仍是部署/运维单写者不变量：Home Identity 不能阻止同备份双启动。Core v1 冻结，仅含已批准 A 包窄解冻 | 逻辑 im 服务端 概念不变；可多 Client/多执行点接入；不承诺多副本写、迁移或 HA |
| Client | `client/`（浏览器直连 im 服务端 + Mock 演示；**原「Host 同源浏览器入口」已于 2026-09-20 随 Host 线退役并删除，考古走 Git 历史**）；无 Tauri/移动端 | Web / 桌面 / 手机多端一致体验（未实现） |
| Native Runtime | 独立进程执行器：BYOK 聊天、Admin 控制面、私有多 Agent 授权模型——**旧线实现已于 2026-09-17 退役（`agent/runtime/` 已删，考古走 Git 历史）；接替者是 `agent/`（新线，仍在建设期）**；内置 `calculator` 是官方静态内置 Extension Package `celira.calculator` 的 `tool.calculator` Tool Capability Provider——每 Agent 显式 capability binding（既有 `agent_tool_grants`，绝不自动扩散）、Run 建立时钉住 canonical capability snapshot、真实工具调用复检 current active 并在工具审计带 Package/Provider 投影 | 完整自有 Agent 执行器；多逻辑 Agent 执行路由属后续窄协议 |
| Local Client Host | 本机受管 im 服务端/Native Runtime 编排 + 浏览器会话 Broker + 规范同源 web 托管与窄 BFF（`docs/client/LOCAL_CLIENT_HOST.md`）——**该实现已于 2026-09-20 随 Host 全托管路径退役并整体删除（`host/` crate 与前端 Host 模块，考古走 Git 历史）；本机编排现由根目录 `home.js` 承担** | 未来 Client 形态之一；多端 Client 连接入口 |
| 外部 Agent | **无**（不提供第三方 Agent 适配器） | **不设**——2026-09-28 主人裁决：「Hermes 这种，不同 Agent 的适配器这种，不需要」。执行端只有 Native Runtime（`agent/`）；外部常驻 Agent 那一档连同它的 Connector 一并退役（`im/connectors/hermes-celira/` 已删，考古走 Git 历史） |

## 4. 关键数据流

### 4.1 Human 明确发言或 `@Agent`

```text
Client -> 逻辑 im 服务端 鉴权并提交公共 Message（幂等）
       -> 逻辑 im 服务端 原子建立必要的 Agent Event（Event 可独立于 Binding 存在）
       -> 由 active Binding 决定的 Backend（Runtime 直连或经 Connector）
          领取 Event 并经执行端私有执行
       -> Backend 提交公共输出提议（普通消息写回）
       -> 逻辑 im 服务端 原子写回为公共事实
       （旧线此处为：Completion → im 服务端 确认 → Backend 提交私有状态并 ACK；
 两条端点与两张表已于 2026-09-28 删除）
```

执行端侧由 Connector 承载协议闭环：Gateway Connector 只管理连接侧映射、重试和实际事件上报，不接管它所接入执行端的内部生命周期。Host 只负责相关常驻服务级生命周期（启动/健康检查/重启/回收）。两者都不是公共事实写者。

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

- Native Runtime 完整展示其真实产生并通过接口公开的可观察执行事件；
- ~~Hermes/AstrBot 等外部常驻 Runtime 需其 Connector 主动上报轨迹~~（**2026-09-28 退役**：不做第三方 Agent 适配器 ⇒ 今天没有这一档执行端）；
- Celira 不虚构执行端没有提供的轨迹，也不把模型未公开的隐藏思维链纳入轨迹承诺；缺失即缺失。

## 5. 允许与禁止的连接

### 允许的连接（当前实现）

```text
Browser（Host 模式）──同源 Cookie+CSRF 守卫 BFF（me/rooms/members 只读、
  消息历史/opaque 分页/纯文本发送/回复/提及/作者撤回、固定无事实 SSE 唤醒）
  ──> Local Client Host
Local Client Host ──in-memory Human Session 克隆──> im 服务端公开 Human 只读/发送路径
  + Human Event WS（仅 events.available 唤醒，不代理事实正文）
client/ 前端（非 Host 模式）──公开 im 服务端协议──> im 服务端（受前端边界约束）
Native Runtime ──Agent Backend Protocol──> im 服务端
Backend ──Backend Credential──> im 服务端 临时状态端点（Presence/Activity）
Human Client ──Human Session──> im 服务端 Room 进行态快照（谁在线 / 谁正在写）
```

> ⚠ **2026-09-20 退役标注**：上列 `Browser（Host 模式）` 与两条 `Local Client Host` 描述的是**已于 2026-09-20 随 Host 全托管路径退役并整体删除**的 Local Client Host 通道（考古走 Git 历史）——当前实现的浏览器路径只有 `client/ 前端 ──公开 im 服务端协议──> im 服务端` 这一条。
>
> ⚠ **2026-09-28 退役标注**：原列的 `External Backend（Hermes connector 等）──Agent Backend Protocol──> im 服务端` **已删除**——不做第三方 Agent 适配器（主人原话：「Hermes 这种，不同 Agent 的适配器这种，不需要」）。今天走这条协议的执行端只有 Native Runtime 一家。

所有连接必须可认证、可授权、可撤销，并遵守相同公共契约。

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
单台 Linux 主机、单用户（当前每个 Home 一个逻辑 im 服务端 = 一个 im 服务端进程）
└── Celira im 服务端（Rust 单二进制 + SQLite，v1 公共契约冻结）
    │  ├── Resource API / Agent Event pull / Human Event REST（旧线另含 Completion / ACK——2026-09-28 已删除）
    │  ├── Home Access A：持久 Home Identity、Pairing Grant、独立 Human Session 管理
    │  ├── Human Event WS 与 Live WS（只发无事实唤醒；Live 为进程内临时投影）
    │  └── SQLite（公共事实；不存 Provider 凭据、Agent 私有执行状态）

Native Runtime（独立进程，含后端控制面）——**以下 L3 描述的是 2026-09-17 退役前的实现**
├── im HTTP Adapter（只走公开 Backend 协议）
├── Control Plane：Admin HTTP API（仅回环 + 独立 Bearer）
├── Event Run Coordinator（Run 钉住 Session/Provider/工具授权/用户授权）
├── Tool Runtime（静态 Catalog = calculator、纯本地确定性执行、私有审计）
├── Runtime 私有 SQLite（配置、加密凭据、Session/Event Run/授权 revision/审计）
└── Rig Agent Adapter + BYOK Provider（用户 Provider 出站，必须支持流式）

Local Client Host（独立本机编排进程）——**已于 2026-09-20 随 Host 全托管路径退役并从工作树整体删除（`host/` crate + 前端 Host 模块；下列 L3 全部不再存在，考古走 Git 历史）**
├── 受管启动与回收 im 服务端 / Native Runtime
├── 经公开 HTTP/CLI 幂等 provisioning 默认 Human / Agent / Room / Binding
├── 受管本机 fragment capability → bootstrap、HttpOnly Session Cookie 与 CSRF Broker
├── 规范同源托管生产 client/dist + 窄 BFF（me/rooms/members、消息/回复/提及/撤回）
├── im 服务端 Human Event WS → 固定无事实同源 SSE 唤醒（浏览器重读权威 im 服务端历史）
└── 无消息/事件/cursor 事实与缓存、非通用反向代理

根目录 home.js（本机编排入口，当前实现，取代上列 Local Client Host）
├── `node home.js up` 起家：im 服务端 + Agent 的执行端 + 生产页（client/dist 经 scripts/serve_client.mjs 托管）
├── `status` / `down` 看读数与停机；凭据经环境交给子进程
└── 只做编排、零第三方依赖（详见 `scripts/HOME_MANUAL.md`）

验收进程
├── Mock Agent Backend（公开协议黑盒：拉取 Event → 以成员身份 say 回话）
└── Native Runtime 独立进程 E2E（退役前位于 agent/runtime/scripts/e2e_native_runtime.py）
```

> ⚠ **2026-09-28 退役标注**：原列的 `Hermes connector（外部接入验证，已完成）` 已随「不做第三方 Agent 适配器」裁决删除。

当前是单 im 服务端、单节点、单一写入裁决者，已满足「一个 Home 任一时刻一个可写逻辑 im 服务端」的架构不变量。当前不实现多副本写入、跨 im 服务端身份、双 im 服务端同步/合并、远程房间投影或冲突解决；备份/冷备/fencing 待机与多租户托管不属于当前实现。

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
- Celira 不建立统一的执行端能力或审批模型；执行期控制只允许经 **Home 准入授权的独立控制路径或同域路由**，不成为 im 服务端的耐久公共事实，私有状态仍归执行端；机制（执行端自己的口 + 家地址 + 家钥匙）与粒度归属见 `docs/constitution/PRODUCT_VISION.md` 的「Client 与执行期控制」，**家口远程档已于 2026-09-24 实现**（家侧 `node home.js issue-home-key`；`home.conf` 的 `status_bind` / `config_bind` 与 `tls_cert` / `tls_key`；客户端「设置 → 家」）；
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

- `docs/constitution/PRODUCT_VISION.md`：产品目标、价值与目标产品形态方向；
- 本文：唯一系统地图——当前实现、目标架构、容器职责、数据流与跨容器演进门禁；系统级未来拓扑只写于 `docs/constitution/PRODUCT_VISION.md` 与本文，Agent 地基语义由 `docs/constitution/AGENT_FOUNDATION_CONTRACT.md` 负责；
- `docs/contracts/core-v1/CORE_V1_IM_CONTRACT.md` / `docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md`：Core v1 冻结公共契约；
- `docs/contracts/core-v1/CORE_V1_ACCEPTANCE.md`：冻结基线、黑盒验收与解冻门禁；
- `docs/constitution/AGENT_FOUNDATION_CONTRACT.md`：Agent 地基原语、事实归属、执行生命周期、恢复语义与未来扩展接缝；
- `docs/runtime/terminal/README.md`：Native Runtime 终局理念、终局本体、执行/扩展边界与负责人裁决入口；不覆盖本文容器边界、上层契约或当前实现事实；
- **活文档（当前边界与验收入口）**：`client/README.md` / `client/DESIGN_SYSTEM.md` / `im/server/README.md` / `agent/README.md`（Native Runtime 新线）/ `docs/constitution/PROJECT_SHAPE.md`（发布单元与版本责任）；`docs/runtime/target/`、`handoff/` 与根层 `RUNTIME_SLICE_*.md` 是旧目标、迁移和过程材料，不是本文的并列架构地图；
- **退役存档（不占权威位置，考古走 Git 历史）**：旧线 `agent/runtime/README.md`（**2026-09-17 退役**；旧实现判据归档已退出工作树，见 Git 历史 `docs/runtime/history/NATIVE_RUNTIME.md`）/ `docs/client/LOCAL_CLIENT_HOST.md`（**2026-09-20 退役**：保留作 BFF 安全模式与 Host 威胁模型的历史记录）/ `im/connectors/hermes-celira/README.md`（**2026-09-28 退役并删除**）；
- 未来节点在进入实现前，必须另立窄设计、验收路径和版本化契约。
