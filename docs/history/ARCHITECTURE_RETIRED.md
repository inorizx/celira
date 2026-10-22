# 退役架构存档（ARCHITECTURE_RETIRED）

> **这是历史存档，不是当前实现。** 本文收的是从 [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md) 移出的退役段落、退役 L3 快照与三次退役标注；**原文事实照抄**，只把"当时是当前实现"的现在时读法标明。当前实现与目标架构只看主文档与各组件的活文档（`im/server/README.md`、`client/README.md`、`agent/README.md`、`app/README.md`）。
>
> ⚠ **本仓的 Git 历史自「首次公开发布」那一次提交起**：下面这些退役物**不在本仓的历史里**，也不在工作树里（边界声明见 [`../../README.md`](../../README.md) 的 Repository map 末段）。所以本文不是"去 Git 里考古"的指路，而是这些事实在本仓里**唯一的落点**。
>
> **读者**：维护者，以及想理解"为什么今天是这样"的人。**不要拿本文当验收依据、实现说明或当前连接清单**——那些都在主文档与活文档里。

## 0. 退役时间线

| 日期 | 退役的东西 | 结果 |
|---|---|---|
| 2026-09-17 | 旧线 Native Runtime（`agent/runtime/` 与 `agent/acp-supervisor/`） | 从工作树删除；接替者是 `agent/`（今天就是当前执行端） |
| 2026-09-20 | Host 全托管路径 / Local Client Host（`host/` crate 与前端 Host 模块） | 从工作树整体删除；本机编排改由根目录 `home.js` 承担 |
| 2026-09-24 → 2026-09-28 | 旧投递协议（Completion / ACK 与其投递裁决） | 2026-09-24 主人裁决退役，2026-09-28 代码面删除完毕（端点与两张表） |
| 2026-09-28 | Hermes connector / 外部常驻 Agent 适配器 | 主人裁决「不同 Agent 的适配器不需要」；`im/connectors/hermes-celira/` 整块删除 |

---

## 1. 原文的退役标注（主文档旧头部）

以下整段是旧 `ARCHITECTURE.md` 的状态声明，逐字保留（它同时是三次退役的原始表述）：

> 状态：唯一 C4 Level 1–2 系统地图。本文同时陈述**当前实现**与**目标架构**，并清楚标注差距；不表示所有目标节点已经实现。当前实现事实以本文与各组件的活文档（`im/server/README.md`、`client/README.md`、`agent/README.md`）和版本化契约为准（`docs/client/LOCAL_CLIENT_HOST.md` 是**退役存档**，不作为当前实现事实的来源）；Native Runtime 的旧线实现已于 2026-09-17 退役（`agent/runtime/` 与 `agent/acp-supervisor/` 已从工作树删除，考古走 Git 历史）；接替者是新线 `agent/`（仍在建设期，未部署）。**Local Client Host 线（`host/` crate 与前端 Host 模块）已于 2026-09-20 随 Host 全托管路径退役并从工作树整体删除**（考古走 Git 历史）——本文 §3、§5、§6 中凡把它写成当前容器/当前连接/当前部署的地方已就地标注为**已退役并删除**；**本机编排现在是根目录 `home.js`（`node home.js up`），浏览器进登录页直连 im 服务端**。产品方向见 `docs/constitution/PRODUCT_VISION.md`。⚠ **本文凡写「考古走 Git 历史」处，指的都是维护者那份仓库的历史**——本仓的 Git 历史自「首次公开发布」那一次提交起，上面那些退役物**不在本仓内**（边界声明见 `README.md`「仓库地图与命名」末段；这个词的登记见 `docs/runtime/AGENT_VOCABULARY.md` §11）。

**今天读这一段要注意两处**：① `agent/` 早已不是"仍在建设期，未部署"——它是当前执行端；② 本仓里没有可供"考古"的历史，退役代码不在其中（见本文开头的边界）。

---

## 2. 旧线 Native Runtime（2026-09-17 退役）

### 2.1 旧 §3「当前实现 vs 目标」里的那一行（逐字）

> | Native Runtime | 独立进程执行器：BYOK 聊天、Admin 控制面、私有多 Agent 授权模型——**旧线实现已于 2026-09-17 退役（`agent/runtime/` 已删，考古走 Git 历史）；接替者是 `agent/`（新线，仍在建设期）**；内置 `calculator` 是官方静态内置 Extension Package `celira.calculator` 的 `tool.calculator` Tool Capability Provider——每 Agent 显式 capability binding（既有 `agent_tool_grants`，绝不自动扩散）、Run 建立时钉住 canonical capability snapshot、真实工具调用复检 current active 并在工具审计带 Package/Provider 投影 | 完整自有 Agent 执行器；多逻辑 Agent 执行路由属后续窄协议 |

⚠ 这一行里的 `calculator` / `agent_tool_grants` / "Extension Package" 都是**旧线**词汇：`agent/src/` 里已无 `calculator`，`agent_tool_grants` 在 migration 与 `im/server/src` 里各 0 处。今天"工具授权按 Agent 显式声明、不自动扩散"这条约束仍在，但机制是宿主装配期的 App 声明（见 `agent/README.md` 与主文档 §3）。

### 2.2 旧 §6 的 L3 快照（逐字）

> Native Runtime（独立进程，含后端控制面）——**以下 L3 描述的是 2026-09-17 退役前的实现**
> ├── im HTTP Adapter（只走公开 Backend 协议）
> ├── Control Plane：Admin HTTP API（仅回环 + 独立 Bearer）
> ├── Event Run Coordinator（Run 钉住 Session/Provider/工具授权/用户授权）
> ├── Tool Runtime（静态 Catalog = calculator、纯本地确定性执行、私有审计）
> ├── Runtime 私有 SQLite（配置、加密凭据、Session/Event Run/授权 revision/审计）
> └── Rig Agent Adapter + BYOK Provider（用户 Provider 出站，必须支持流式）

### 2.3 旧线的验收入口

> 验收进程
> ├── Mock Agent Backend（公开协议黑盒：拉取 Event → 以成员身份 say 回话）
> └── Native Runtime 独立进程 E2E（退役前位于 agent/runtime/scripts/e2e_native_runtime.py）

⚠ 其中 **Mock Agent Backend 今天仍在**（`im/server/src/bin/mock_backend.rs`，公开协议黑盒）；退役的只有旧 Runtime 的独立进程 E2E 脚本。

---

## 3. Host 全托管路径 / Local Client Host（2026-09-20 退役）

### 3.1 旧 §3 容器表里的那一行（逐字）

> | Local Client Host | 本机受管 im 服务端/Native Runtime 编排 + 浏览器会话 Broker + 规范同源 web 托管与窄 BFF（`docs/client/LOCAL_CLIENT_HOST.md`）——**该实现已于 2026-09-20 随 Host 全托管路径退役并整体删除（`host/` crate 与前端 Host 模块，考古走 Git 历史）；本机编排现由根目录 `home.js` 承担** | 未来 Client 形态之一；多端 Client 连接入口 |

### 3.2 旧 §5「允许的连接（当前实现）」的原文与退役标注（逐字）

> ```text
> Browser（Host 模式）──同源 Cookie+CSRF 守卫 BFF（me/rooms/members 只读、
>   消息历史/opaque 分页/纯文本发送/回复/提及/作者撤回、固定无事实 SSE 唤醒）
>   ──> Local Client Host
> Local Client Host ──in-memory Human Session 克隆──> im 服务端公开 Human 只读/发送路径
>   + Human Event WS（仅 events.available 唤醒，不代理事实正文）
> client/ 前端（非 Host 模式）──公开 im 服务端协议──> im 服务端（受前端边界约束）
> Native Runtime ──Agent Backend Protocol──> im 服务端
> Backend ──Backend Credential──> im 服务端 临时状态端点（Presence/Activity）
> Human Client ──Human Session──> im 服务端 Room 进行态快照（谁在线 / 谁正在写）
> ```

> ⚠ **2026-09-20 退役标注**：上列 `Browser（Host 模式）` 与两条 `Local Client Host` 描述的是**已于 2026-09-20 随 Host 全托管路径退役并整体删除**的 Local Client Host 通道（考古走 Git 历史）——当前实现的浏览器路径只有 `client/ 前端 ──公开 im 服务端协议──> im 服务端` 这一条。

### 3.3 旧 §6 的 L3 快照（逐字）

> Local Client Host（独立本机编排进程）——**已于 2026-09-20 随 Host 全托管路径退役并从工作树整体删除（`host/` crate + 前端 Host 模块；下列 L3 全部不再存在，考古走 Git 历史）**
> ├── 受管启动与回收 im 服务端 / Native Runtime
> ├── 经公开 HTTP/CLI 幂等 provisioning 默认 Human / Agent / Room / Binding
> ├── 受管本机 fragment capability → bootstrap、HttpOnly Session Cookie 与 CSRF Broker
> ├── 规范同源托管生产 client/dist + 窄 BFF（me/rooms/members、消息/回复/提及/撤回）
> ├── im 服务端 Human Event WS → 固定无事实同源 SSE 唤醒（浏览器重读权威 im 服务端历史）
> └── 无消息/事件/cursor 事实与缓存、非通用反向代理

### 3.4 它当年的安全模式记录

`docs/client/LOCAL_CLIENT_HOST.md` 按文档纪律保留为 **BFF 安全模式与 Host 威胁模型的历史权威记录**——不是当前实现，也没有可执行入口。

---

## 4. 旧投递协议：Completion / ACK（2026-09-24 裁决，2026-09-28 代码删除）

### 4.1 旧 §4.1 数据流的尾注（逐字）

> （旧线此处为：Completion → im 服务端 确认 → Backend 提交私有状态并 ACK；
> 两条端点与两张表已于 2026-09-28 删除）

### 4.2 旧 §3 容器表里的两处提及（逐字）

> 普通新消息的 Message→Event→（拉取）主链与其他 Event 的版本化定稿（旧线含 Completion、ACK 与其投递裁决，2026-09-24 退役）

> durable claim、Event 状态、Completion/ACK 桥接（旧线机制，2026-09-24 退役）等是 Runtime/Connector 的执行连接职责，不是 Host 职责。

**今天**：普通新消息的公共处理主链只有 **公共 Message → Agent Event →（拉取）**；`POST …/events/{event_id}/completion` 与 `POST …/events/{event_id}/ack` 已连同表与错误码一并退场（见 `docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md` 文档头）。

---

## 5. Hermes connector / 外部常驻 Agent（2026-09-28 退役）

### 5.1 旧 §4.4 轨迹边界里的一条（逐字，原文为删除线）

> ~~Hermes/AstrBot 等外部常驻 Runtime 需其 Connector 主动上报轨迹~~（**2026-09-28 退役**：不做第三方 Agent 适配器 ⇒ 今天没有这一档执行端）；

### 5.2 旧 §5 的退役标注（逐字）

> ⚠ **2026-09-28 退役标注**：原列的 `External Backend（Hermes connector 等）──Agent Backend Protocol──> im 服务端` **已删除**——不做第三方 Agent 适配器（主人原话：「Hermes 这种，不同 Agent 的适配器这种，不需要」）。今天走这条协议的执行端只有 Native Runtime 一家。

### 5.3 旧 §6 的退役标注（逐字）

> ⚠ **2026-09-28 退役标注**：原列的 `Hermes connector（外部接入验证，已完成）` 已随「不做第三方 Agent 适配器」裁决删除。

### 5.4 旧 §3 外部 Agent 行的退役尾巴（逐字）

> **不设**——2026-09-28 主人裁决：「Hermes 这种，不同 Agent 的适配器这种，不需要」。执行端只有 Native Runtime（`agent/`）；外部常驻 Agent 那一档连同它的 Connector 一并退役（`im/connectors/hermes-celira/` 已删，考古走 Git 历史）

**今天**：主文档 §3 保留"外部 Agent ＝ 无（不提供第三方 Agent 适配器）"这一条**当前事实**，只把"哪一年删了哪个目录"留在本文。

---

## 6. 旧目标、迁移与过程材料（已退出工作树）

旧 `ARCHITECTURE.md` §10 曾把下面这些当作并列文档引用；它们**都已不在工作树**，因此从主文档删除：

| 旧引用 | 它曾是什么 | 今天的落点 |
|---|---|---|
| `docs/runtime/target/` | 旧目标架构目录 | 已不存在；目标架构只看主文档与 `docs/constitution/PRODUCT_VISION.md` |
| `handoff/` | 交接材料 | 已不存在 |
| 根层 `RUNTIME_SLICE_*.md` | 旧 Runtime 切片施工记录 | 已不存在 |
| `docs/runtime/history/NATIVE_RUNTIME.md` | 旧线实现判据归档 | 已不存在（本仓历史也不含它） |
| `agent/runtime/README.md` | 旧线实现文档 | 已删除；当前执行端文档是 `agent/README.md` |
| `im/connectors/hermes-celira/README.md` | 外部 Agent 适配器 | 已删除 |

---

## 7. 「考古走 Git 历史」这句话在本仓的射程

旧主文档用这句话指路的地方最多，而它在**本仓**是空指针。词表把这条边界登记在 `docs/runtime/AGENT_VOCABULARY.md` §11：那句话是一句**指路**，不是「资料缺失」的借口；而它在本仓的**射程有限**——本仓的 Git 历史从「首次公开发布」那一次提交开始，那次提交**之前**退役的东西（`agent/runtime/`、`agent/acp-supervisor/`、`host/`、`im/connectors/`）在本仓里查不到，只在维护者那份仓库里。词表自己的结论是：**写前门时不许拿它当承诺**，门后照旧可用。

因此：**前门（`README.md`、`ARCHITECTURE.md` 与其它对外文档）不再用它指路**；退役事实的对外落点就是本文。
