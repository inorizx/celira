# Celira Agent Foundation Contract

> 状态：**Agent 地基语义已确认，作为当前架构设计契约**。
>
> 本文冻结 Celira 对 Agent 核心原语、事实归属、执行生命周期、扩展接缝和恢复边界的共同语言。它不是 Runtime 当前实现清单：尚未实现的部分必须明确标注为“后续接入点”，不得被误报为当前能力。
>
> Core v1 公共契约继续冻结。本文不新增 im 服务端端点、公共事件、数据库字段或跨容器协议；若未来实现改变 im 服务端公共语义，必须另行申请解冻并建立版本化契约。
>
> ⚠ **2026-10-03 退役标注（2026-10-11 正文已对齐）**：2026-09-28 主人裁决「不做第三方 Agent 适配器」，执行端只有 Native Runtime 一家——**「外部 Agent」不构成一档执行端**，本文正文里指称它的地方一律读作「执行端」；权威见 `PRODUCT_VISION.md`、`../../ARCHITECTURE.md` §3「外部 Agent」行与本文 §3.1 的 2026-09-28 注记。

## 1. 目标与原理

Celira 的 Agent 地基要解决的不是“现在有多少工具”，而是让未来接入 Memory、Tools、Skills、MCP 或不同 Loop 时，不必重新定义 Agent 身份、会话、权限、运行和恢复边界。

**原理声明：**im 侧 Agent Identity 是用户可认识、加好友、进 Room、授权并跨部署延续的唯一公共 Agent 身份；Runtime 只持有为该身份执行推理所需的私有实例、认知和配置。im 服务端 只裁决公共 IM 事实与公共 I/O，Client 经 Runtime 实际公开的受认证能力管理私有执行；im 服务端 不成为 Runtime 控制面。公共身份、私有执行、Binding/Credential、认知入口、单次运行、能力授权、执行事件和终态事实必须分成稳定原语。

## 2. C4 Level 1–2 拓扑

```text
Human / Client
    │ 公共消息、房间操作、只读观察
    ▼
Celira im 服务端（每个 Home 一个可写公共事实裁决者）
    ├─ im 侧 Agent Identity / Participant 公共投影
    ├─ Room / Membership / Permission
    └─ Message → Agent Event（供拉取）/ 公共顺序
                 ▲                    │
        可信输入 │                    │ 公共输出提议（消息写回）
                 │                    ▼
执行点上的 Runtime Agent
    ├─ 持续私有主体：Profile / Cognitive Conversation / Memory / Task
    ├─ Agent Handle / Lane / Context / Run / Turn
    ├─ Loop Driver / Provider Adapter / Capability Scope / Policy Gate
    ├─ 私有 Agent Event / Audit / Recovery
    └─ im 服务端 Adapter（以 Binding / Credential / im 服务端 Attachment 接入一个公共投影）

Host：只负责常驻组件的启动、健康检查、重启、回收
Connector：只负责协议连接、会话桥接、取消/回收和事件结果转发
```

### 2.1 连接与事实边界

```text
im-bound（当前真实消费者）：
Runtime Agent 私有主体
    → 显式关联 im 侧 Agent 公共投影
    → im 服务端 Binding / Credential + im 服务端 Adapter
    → im 服务端 已裁决的 Agent Event 输入（供拉取）
    → 私有 Cognitive Conversation + Run + Loop / Capability Gate
    → 公共输出提议（普通消息写回，如 `say`）
    → im 服务端 原子写回为公共事实
      （Completion/ACK 已于 2026-09-24 裁决退役、2026-09-28 连端点带表删除，见 §3.6）

im-less / 本地（已确认目标；无公共主链）：
Runtime 本地/世界唤醒原因
    → 私有 Run（有限清醒期）
    → 私有终态（private_completed / 私有失败 / conflict）
    → 不产生 Completion，不 ACK
    → 显式 send_message 且无源 Event：fail closed（Core v1 无 Agent 主动消息端点）
```

- im 服务端 不生成 Agent 内容，不拥有或定义 Agent 私有 Profile、Session、Run、Memory、Task、Tool 或执行轨迹。
- Runtime 不复制或修改 im 服务端 的公共身份、Owner、Room Membership、公共 Message、Event；它只把其中经 im 服务端 授权的事实作为 Adapter 输入。
- Host 不拥有 Runtime 私有事实，不读取 Runtime 私有数据库，不代替主体执行。
- Connector 是连接平面，不是 Agent、Participant、Session 或公共事实写者。
- 执行端的内部 Profile、Session、Run、Memory、Skills、Tools、审批和沙箱留在它自己的系统；Celira 只通过公开协议桥接真实能力。（⚠ **2026-10-03 标注**：「外部 Agent」一档已退役——2026-09-28 主人裁决「不做第三方 Agent 适配器」，执行端只有 Native Runtime 一家；见文首注记与 `../../ARCHITECTURE.md` §3「外部 Agent」行。）

## 3. Agent 地基原语

### 3.1 Agent Identity：im 服务端 公共身份与 Runtime 私有执行实例

**定义：**Agent 的公共身份由 im 服务端 持有：可被加好友、进入 Room、被授权、跨部署识别、承载公共 Message 与 Agent Event 的是 **im 侧 Agent Identity**（Completion/ACK 已于 2026-09-24 退役）。Runtime Agent 只是其运行地点中的私有认知与执行实例；是否接入 Celira、以哪个 im 侧 Agent Identity 接入，则由可绑定、可撤销的公共 I/O Attachment 决定。im 服务端 不创建、不定义、不拥有 Agent 的认知主体；Runtime 也不能以私有主体取代公共产品身份。

```text
im 侧 Agent Identity（im 服务端 公共产品身份）
    ├─ 公共显示 / 关系 / Room / 权限 / 消息事实
    └─ im 服务端 Binding / Credential
          → Runtime 私有执行实例（仅作为运行处）
              → Profile / Cognitive Conversation / Memory / Task / Run / Capability
              → im 服务端 Adapter：受理可信输入、提交公共输出
```

**Runtime Agent（私有执行实例）归属：**Runtime（Native Runtime）唯一持有（⚠ 2026-09-28：第三方 Agent 适配器已随「不做第三方 Agent 适配器」裁决退役删除，执行端只有 Native Runtime 一家）。它为 im 侧 Agent Identity 承担当前运行处的私有人格、连续认知、Memory、Task、Run、工具与私有能力；解绑、替换或暂时失去 im 服务端 连接，不得修改或裂变 im 服务端 公共身份与历史。

**im 侧 Agent Identity（公共产品身份）归属：**im 服务端 唯一持有。它是 Human 可加好友、邀请进群、授权、跨部署识别并在公共消息世界中看到的 Agent；它只包含：

- `agent_id`、公共显示名与公共呈现；
- Owner/关系与准入事实；
- Room Membership；
- Agent Event 所需的公共路由身份；
- 该公共投影产生的公共 Message 与历史。

它**不包含**模型凭据、system prompt、Memory、Skills、工具授权、私有认知入口、Session、Task、Run、工具参数、私有轨迹或执行进程状态。

**im 服务端 Adapter 的边界：**从 Runtime Agent 视角，im 服务端 是受认证的公共 I/O 边界——只输入 im 服务端 已裁决、该公共投影有权观察的 Agent Event 与可信公共上下文；只输出 Agent 提议的公共消息写回。公共 Message 由 im 服务端 原子写回；Runtime 不直写 im 服务端数据库、也不把 im 服务端 的公共历史误当作自己的私有认知事实。

**不变量：**im 服务端 在公共世界中仍是身份、成员资格、权限、消息顺序与公开写入的唯一裁决者；“im 服务端 是 Agent 的 I/O 通道”不等于 Runtime 可以绕过其公共准入规则。反过来，Runtime Registry/私有 Agent 配置也不等于 im 服务端 中已经存在公共 Participant，Runtime 不得用私有配置反推或伪造公共身份。

**当前实现状态（0013 认知地基已实现）：**Native Runtime 的私有主体已收敛为 **Runtime Agent**——它可以无 im 服务端 连接独立存在；im 侧 Agent 公共投影以一对一的公共 I/O 关联外引用（实现形状见 `agent/README.md`）；执行 Lane 只属于已建立 Attachment 的 Runtime Agent（未接入主体保持私有存在但不轮询/Presence）。后续接入点（**尚未实现**）仍包括：Memory、Skills、MCP、Subagent、跨 Direct Room 分配之外的多认知入口并发控制等。本节确认的 Agent-first 目标已被该私有模型满足；任何未来的公共身份、Room、Binding 或跨容器方案仍须按根 `../../AGENTS.md` 的「动手前门禁 / Core v1 封板门禁」取得主人确认。

### 3.2 Agent Profile

**定义：**Agent 在执行系统中的私有行为配置；描述 Agent 如何表达和执行，不是公共身份。

**归属：**Runtime（Native Runtime）私有持有。

**语义组成：**

```text
Agent Profile
├─ Persona / System Prompt
├─ Provider / Model Route
├─ 默认 Context Policy
├─ 默认 Capability Policy
└─ 后续 Memory / Skills Policy
```

- Persona 是 Profile 的行为配置，不另建第二个公共 Agent 身份。
- Profile 配置可以分为多个明确的 revision；不使用无法审计的万能 JSON 作为长期事实源。
- Profile 更新只影响后续 Run；已建立 Run 使用自己的不可变 Snapshot。
- 当前 Runtime 已实现 system prompt、Provider revision、Agent tool grant revision 等分散的配置事实；将来可提供“生效 Profile”只读视图，但不得因此创建第二套配置事实源。

### 3.3 im 服务端 Binding / Execution Attachment

**定义：**Binding 是把一个已存在的 Runtime Agent 的 im 服务端 Adapter，获准接入其显式关联的 im 侧 Agent 公共投影的**公共连接事实**。它不创建 Runtime Agent、不定义其私有身份，也不把 Runtime 变成 im 服务端 的子系统。

```text
Runtime Agent（私有主体）
    → 显式关联 im 侧 Agent Identity（公共投影）
    → active Backend Binding / Credential
    → im 服务端 Adapter 的 `/me` 身份确认
    → 该私有主体的 Agent Lane
```

- Binding 不等于 Runtime Agent，也不等于 im 侧 Agent Identity；它只授权这个公共 I/O 连接。
- **当前映射基数：**一个已接入的 Runtime Agent 同时只关联一个 im 侧 Agent Identity；一个已接入的 im 侧 Agent Identity 同时也只关联一个 Runtime Agent。两端是一对一映射，不允许一个私有主体同时以多个 im 服务端 身份发言，也不允许多个私有主体共同代表同一个 im 服务端 身份。
- Credential 归属于 Binding，只代表指定 im 侧 Agent 公共投影，不代表宿主、Runtime 或 Owner。
- Binding 不产生 Room Membership，不赋予 Runtime 公共数据库权限。
- 当前 Core v1 限制：同一 im 侧 Agent 同时至多一个 active Binding；Runtime 的一对一映射进一步禁止该 Binding 在私有侧被多个 Runtime Agent 共享。
- 未连接或已断开 im 服务端 的 Runtime Agent 仍可以是存在的私有主体，但不能观察 im 侧 Event、也不能提交公共消息写回（Completion/ACK 已于 2026-09-24 退役，见 §3.6）；im 服务端 也不得把它误报为已接入的 Participant。
- 一个 Runtime 进程可以承载多个彼此隔离的 Runtime Agent / Agent Lane；每个 Agent 各自维护这条一对一 im 服务端 Attachment。共享进程级 Provider、Loop、工具基础设施不改变身份、认知、授权、公共输出和恢复的隔离。

### 3.4 Agent Handle / Lane

**Agent Handle** 是进程内操作一个 Agent 执行生命周期的活体句柄；**Lane** 是该 Agent 在一个 Runtime 进程中的实际执行单元。

Handle 负责：

- Agent 执行生命周期；
- 当前 Lane/代数与就绪状态；
- 当前 Active Run 的串行边界；
- `cancel`、`wait_until_idle`、状态观察；取消落进系统终态，只进私有事实与状态读数，无公共写回（实现形状见 `agent/README.md`；旧线取消链路见 Git）。
- 将 Run 交给 Loop Driver。

Lane 负责：

- 使用该 Agent 独立的 im 服务端 认证上下文；
- 独立 Event 投递位置、Cognitive Conversation、Run、授权和恢复边界；
- 独立 Presence/Activity 生命周期；
- 局部失败时不影响其他 Agent Lane。

**不变量：**同一 Agent/Lane 同时至多一个 Active Run；不同 Agent 可以并行，但不得共享 Cognitive Conversation、Context、Memory、Credential、Grant、Run 或私有状态。

### 3.5 私有认知入口（Cognitive Conversation）

**定义：**私有认知入口是一个 Agent 面对同一 Human 或同一群体时持续维护的私有认知连续性。它是 Runtime 的私有事实，**不是** im 服务端 Room、公共 Conversation 或一次 Run。

```text
Human Direct：Agent × Human principal → 恰好一个私有认知入口
Group Room：  Agent × im 服务端 Room        → 恰好一个私有认知入口
```

- 同一 Human 与同一 Agent 可以在 im 服务端 中拥有多个 Direct Room；这些 Room 是公共消息的组织、主题和互操作边界，但都进入这对 Agent × Human 的**同一个**私有认知入口，不能因 Room 数量制造多个彼此失忆的会话。
- 一个 Group Room 对每个目标 Agent 只有一个私有认知入口；同一 Group 中的不同 Agent 各自隔离，绝不共享 Cognitive Conversation、Memory、Run、Credential、Grant 或私有上下文。
- 因此，Runtime 不提供任意 `Agent × Room × 多 Session` 的用户可见会话树、fork、切换或平行 active Session。显式重置、归档或恢复如未来出现，必须是该私有认知入口的版本化 Runtime 私有动作，而不是借新建 im 服务端 Room 偷换上下文。
- 私有认知入口只读取该 Agent 已获准读取、且本地已确认的 Turn；每条 Turn 必须保留其来源 im 服务端 Room / Event 引用，以支持撤回更正、审计和按需上下文构建。
- 私有认知入口不等于全量历史常驻模型上下文。Runtime 仍必须通过有界工作集、Handoff、Memory 来源引用和按需检索维持“小、相关、可信、可行动”的当前 Run。
- 正式 Turn 在本地已确认后写入（新线口径：确认 = 本地收场/封存确认，不经 im 侧）；失败、未确认、取消或被撤回的执行不进入正常认知历史。
- **当前实现状态（0013 认知地基已实现）：**Runtime 私有认知入口已收敛为 Cognitive Conversation——Direct 与 Group 分别按主体/Room 维度唯一，来源 Room/Message/Event 引用随 Turn 保留（实现形状见 `agent/README.md`）；旧 `Agent × Room × 多 Session` 已被 0013 破坏式移除，不做兼容/双读。
- 相关后续接入点（**尚未实现**）：显式重置/归档、有界 Context Builder / Handoff、Memory 来源引用与按需检索。

### 3.6 Run

**定义：**Run 是 Runtime 内一次**有限清醒期**：Agent 获得观察处境并决定是否行动的有界私有执行事实。Run 不是 Life，不是 Cognitive Conversation，也不是 im 侧 Event 本身。

```text
Run = 有限清醒期
    → 一个或多个内部 Model Turn
    → 若干 Tool Call（可为零）
    → 一个终态
```

**起源（地基 1:1 全称已解冻）：**一个 Run 必须有且仅有一种起源。`一个 im 侧 Event → 一个 Run` 只约束 **im-bound** 路径，不是全部 Run 的定义。

```text
起源 A · im-bound（当前真实消费者）
    一个 im 侧 Agent Event（供拉取）→ 一个 Run → 私有终态（trace 收敛）

起源 B · im-less / 本地（已确认目标）
    本地或世界唤醒原因 → 一个 Run → 私有终态
    无源 Event，无 Completion，无 ACK
```

唤醒不等于强制认知，也不等于必须回复。沉默、等待、拒绝仍合法。世界变化 / Timer 作为一等来源是已确认目标，不是当前实现事实。

**与公共主链的边界：**

- im-bound 的公共处理主链：普通新消息是 **公共 Message → Agent Event →（拉取）**。这条主链是 im 服务端 公共协议，不是 Runtime Run 起源的全称。Completion/ACK 为旧投递模型的残留条款，2026-09-24 主人裁决退役（零生产流量，见 `outputs/ONLINE_DRILL_ACT1_REPORT_2026-09-24.md`），不再要求；旧线的前缀/ACK 车道规则随之失效，保留为历史基线。
- im-less / 本地 Run 不进入该主链：不产生 Completion，不 ACK。显式 `send_message` 且无源 Event 时 fail closed（Core v1 无 Agent 主动消息端点）。本地 / 世界 Run 只有经显式通信意图才可能进入公共世界；不得把私有终态伪装成公共回复。
- 重放不等于重新生成；im-bound 绝不因重放重复公共消息。

**持久载体与 Snapshot：**

- Run 事实的持久载体是实现形状（随刀更新，见 `agent/README.md`）；不新建平行 Run 表，也不把表名升格为本体。
- Run 建立时原子钉住影响执行的配置与授权事实：Runtime Agent、目标 Cognitive Conversation、Provider revision、System Prompt、授权 revision、Capability Snapshot、调用者上下文，以及 **Driver 身份**（钉住的执行器组合与版本；恢复只认钉住组合，无「当前 Driver」可回退——实现形状见 `agent/README.md`）。
- Profile、Provider、Grant、Memory、Skill 或工具目录之后发生变化，不重写已建立 Run 的 Snapshot。
- Run 不是 Cognitive Conversation。Conversation Turn（新线对应 Agent 的经历/记录）的追加由 Runtime 自身裁决——本地已确认后落盘，不经 im 侧确认；im 侧消息以自己的收据与复核为准（见 `agent/README.md`）。

**取消（旧线实现事实）：**旧线取消实现与含 Completion/ACK 的收敛步骤已随 2026-09-24 退役，历史实现见 Git。

Run 的目标终态至少要区分 `completed`、`private_completed`、`failed`、`interrupted/cancelled`、`conflict`。im-less 取消不走 Completion/ACK。

**新线当前事实（`agent/`，以源码与 `agent/README.md` 为准）：**取消落在系统终局（`SystemEnding::Cancelled`，读数 `system:cancelled`），只进私有事实与状态读数（终态确认 → 封存），无任何公共写回、无 im 侧确认；im-bound 与 im-less 在这一点上同形。

**Trace 身份（后续接缝）：**

Trace 键是实现形状（随刀更新，见 `agent/README.md`）；im-less Trace 键未定稿，本解冻不发明无源 Trace 身份，也不另建平行权威表。

**当前实现对照：**实现形状（公共路径的入口、本地跑法、注入时机）随刀更新，见 `agent/README.md`。

- 公共路径是 im 侧 Event 供拉取 → 一个 Run → 私有终态；本地无源路径只收敛私有终态，不创建无源 Run。
- 世界变化 / Timer 作为一等来源尚未实现。旧实现已于 2026-09-01 删除，判据归档已退出工作树（见 Git 历史 NATIVE_RUNTIME.md）。

### 3.7 Turn / Tool Call

**Turn** 是一次 Loop Driver 对模型的请求轮次；**Tool Call** 是 Turn 内的一次能力调用请求与结果闭环。

```text
Run
  ├─ Turn 1：模型请求 →（可选）Tool Call → 工具结果
  ├─ Turn 2：最终模型请求
  └─ Run terminal outcome
```

- Turn 数量必须有界；当前由宿主配置（`agent_host --max-turns <n>`，两个宿主的内置缺省取 300，支级可在 `<环境根>/agent.conf` 覆盖），预算耗尽如实报成系统终局，不冒充主体完成（当前事实见 `agent/README.md`）。
- **后续工具接入门槛：**若上游明确给出长度截断且该响应包含工具调用，必须拒绝整批调用；不得执行可能不完整的参数。在上游长度截断未被规范化为独立 Runtime 裁决之前，不得把此项写成已验证的当前保证。
- 当前 Tool Call 必须经过参数校验、钉住 Snapshot 上限、current active 授权复检、预算/超时、执行、审计和结果归一化。
- 工具错误是该 Run 的结构化执行数据；不能让后续模型文本洗白已经发生的工具或审计失败。
- 工具定义、工具授权、工具调用上下文和工具审计属于 Runtime 私有事实；Tool 不是 Agent、Participant、Message 或 im 侧 Event。

### 3.8 Loop Driver

**定义：**驱动一个 Run 完成模型与工具循环的**可版本化执行器**；Driver Contract 是 Runtime 私有事实，不进入任何公共契约。

- Driver 的解析、钉住与具体驱动链是**实现形状**，随刀更新，见 `agent/README.md`。
- 恢复与执行只认已建立 Run 钉住的组合：不存在「当前 Driver」可覆盖已建立 Run；未知/缺失/不一致的解析一律 **fail closed**（不调用 Provider、不写任何公共事实，Run 保持可恢复阻塞），绝不把新语义静默套到恢复中的旧 Run。
- 驱动链沿用既有事实与失败分类，不复制第二套（输入透明、失败透明）。
- 未实现：多 Driver、用户选择 Driver、安装/热加载/动态插件、跨 Driver 能力声明。

Loop Driver 不拥有：

- im 服务端 公共事实；
- Agent Registry 的长期配置；
- 全局授权裁决；
- Host 生命周期；
- Client/UI 状态；
- 另一个 Agent 的私有状态。

后续 Driver 接入门槛（未来若实现第二个 Driver，必须先达成）：

- 必须把取消传播与现有模型总时限、工具边界的超时传播保持一致；
- 必须产出 Runtime 私有类型化 Agent Events；
- 不得改变本节的事实归属或绕过 Run Snapshot / Capability Gate；
- 必须登记新的稳定 ID/版本并闭门评审恢复语义——恢复中的旧 Run 绝不被新 Driver 静默接管。

### 3.9 Capability Scope / Policy Gate

能力事实分五层：

```text
Runtime Resource Catalog
    → Agent Capability Binding
    → User × Agent × Room Grant
    → Run Capability Snapshot
    → Runtime Policy Gate
```

- Catalog 只描述可发现的能力定义，不代表任何 Agent 已启用。
- Agent Binding 决定 Agent 默认允许哪些能力；当前工具 Binding 复用 `agent_tool_grants`。
- User Grant 决定具体调用者在 Agent/Room 中可使用的能力；它不是安装事实。
- Run Snapshot 固定本次 Run 的能力上限、版本和来源。
- Policy Gate 在每次真实调用前复检 current active 授权、预算、风险和审计要求；撤销/收窄即时生效，扩权不扩大旧 Run。

未来资源统一遵守：

| 资源 | 可以共享 | 不得共享 |
|---|---|---|
| Provider | 连接定义、模型目录 | 凭据使用权、Agent 配置、Run 状态 |
| Tool | 定义、Schema、Provider 实现 | Agent 授权、调用上下文、私有审计 |
| Skill | 包、版本、说明 | Agent 启用关系、注入上下文 |
| MCP | Server 定义、连接描述 | Agent 权限、调用上下文、结果状态 |
| Memory | 后端类型与存储基础设施 | Agent 命名空间、私有内容、检索授权 |
| Cognitive Conversation / Run | 执行内核 | 任何 Agent 间私有事实 |

**原则：共享定义和执行基础设施，不共享授权、上下文和状态。**

### 3.10 Runtime Private Agent Trace

**定义：**Agent Trace 是按单个 Runtime Run 有序追加的、脱敏的私有执行事实链。它解释“一次执行如何走到当前状态或终态”，但不是 im 服务端 公共 Event、普通日志、模型隐藏推理或第二套 Run 状态机。

```text
im 侧 Event ──关联──▶ Run（状态/恢复权威）
                         │
                         ├──▶ Conversation Turn（已确认私有上下文）
                         ├──▶ Tool Audit（实际工具判决权威）
                         └──▶ Agent Trace（有序可解释时间线）
```

#### 3.10.1 事实归属与强制边界

- Run 的状态与恢复权威是 Runtime 私有的单一权威（实现形状见 `agent/README.md`）；Trace 不参与调度、恢复裁决或任何公共事实，也不得另建平行权威表。
- Trace 不复制工具调用的参数、结果摘要或授权原文，只通过稳定引用关联既有审计事实。
- Live `thinking` / `working` / `responding`、delta、流 revision 与进程日志是临时观察或排障材料，**不进入持久 Trace**；它们可合并、丢失或在进程重启后消失。
- Trace 是 Runtime 私有事实：不得成为 im 服务端 Message、im 侧 Event、im 服务端 历史或任何公共事实的替代品；不经 Host BFF、Client、Connector 或任何公共协议暴露。
- Trace 不记录模型隐藏思维、逐 token 内容、原始 Prompt / system prompt、Session 或 Memory 正文、Provider/Admin/im 服务端 凭据、原始上游错误 body、环境变量、工具原始参数/结果或任意自由文本日志。

#### 3.10.2 事实类型与纪律

Trace 只记录当前 Runtime 已实际产生、能以既有状态机与审计事实验证的里程碑；尚未存在的类型不得提前宣称（具体类型清单见 `agent/README.md`）。纪律：

- 模型调用保持 at-least-once 口径，Trace 不构成 exactly-once 承诺；
- 审计或 Trace 失败均不得把工具调用伪装为成功；
- 不同阶段（结果就绪、失败定稿、会话提交、终态确认）是不同事实，不能以任意一个代替另一个。

#### 3.10.3 有序性与幂等

- Trace 的因果顺序由单一 Run 内严格递增的序号给出；墙钟时间只用于展示，绝不用于排序或恢复裁决。
- 重复驱动、幂等重试或进程重启不得重复追加同一状态事实。
- 外部 HTTP 调用与本地提交不能原子成立：Trace 只在本地已确认的事实边界追加，绝不把「请求已发出」伪装成「已确认」。
- Trace 写入失败时不得单独推进相应的本地状态事实；Trace 与状态事实的落库时序是实现契约（见 `agent/README.md`）。
- 恢复只读已确认事实；恢复过程绝不读取 Trace 来决定下一状态，也不允许通过补写 Trace 修复或跳过 conflict。

#### 3.10.4 脱敏投影与读取边界

Trace 的只读投影（形状与字段见 `agent/README.md`）不得返回 system prompt、任何正文、内部索引键、Provider / Grant revision 原值、idempotency key、内部审计 ID、原始工具摘要、Credential、内部错误链或其他 Agent 的记录。读取 Trace 不得改变 Run、Conversation、Live 或任何授权事实。

公共 Message 只由显式公共写回形成；Runtime Trace 永远不自动成为 im 服务端 Message、im 侧 Event 或 im 服务端 历史。

### 3.11 Terminal Outcome

一次执行必须以明确的结构化终态收束：

```text
completed   → 有效结果已准备并可提交
failed      → 执行失败，按契约提交 failure 或保持前缀阻塞
cancelled   → 用户/控制路径取消
interrupted → 进程、连接或运行中断，按恢复策略重驱或定稿
conflict    → 本地事实矛盾，fail closed，不做任何公共写回
```

- 模型错误、工具错误、超时、取消和基础设施故障不能只靠异常文案区分。
- 外部副作用前后的本地事实必须能判断“未发生、已发生未结算、已结算”；公共写回使用既有幂等键与状态机收敛。
- 进程重启不得把已确认的结果重新调用模型；`result_ready` 之后只恢复本地 Turn/经历 commit。
- 在飞模型 HTTP 请求无法与本地存储原子提交，执行保持 at-least-once；不得对模型生成过程宣称 exactly-once。
- **当前强制保证与本表区分**：`completed`/`failed`/`cancelled`/`interrupted`/`conflict` 的 Runtime 私有状态收敛由当前切片强制执行；统一 Typed Agent Event、Memory/Skills/MCP Context Snapshot 仍属后续接入门槛——尚未实现的能力不得写成已支持（当前实现事实见 `agent/README.md`）。

## 4. Context Snapshot 规则

未来 Memory、Skills、MCP 接入前，必须保留以下可追溯关系：

```text
Context Snapshot
├─ System Prompt / Profile revision
├─ Cognitive Conversation history boundary
├─ Provider / Model revision
├─ Capability Snapshot
├─ Memory source references（后续）
├─ Skill source references（后续）
└─ Room / caller context
```

核心规则：

1. **模型可见内容必须有 Runtime 私有来源。** 不能让模型看见无法解释来源的隐式数据。
2. **Run 使用不可变 Context Snapshot。** 后续 Memory、Skill、Tool 目录变化不重写旧 Run。
3. **来源引用与正文归属分离。** 不要求每次把所有外部正文复制到每条 Run，但必须记录来源、版本、边界和是否成功注入。
4. **不同 Agent 的 Context/Memory 命名空间默认隔离。** 跨 Agent 信息只能经明确的公共 Message 或未来显式授权投影传递。
5. **当前实现状态：** system prompt、Conversation Turn、Provider revision、Capability Snapshot 已有对应钉住语义；Memory/Skills/MCP Context Provider 尚未实现，不得提前宣称支持。

## 5. 生命周期与强制顺序

```text
im-bound（公共主链，当前真实消费者）：
im 服务端 Message
  → im 侧 Agent Event
  → Runtime 建立 Run
      └─ 原子钉住 Profile/Provider/CognitiveConversation/Capability/Context 与 Driver 身份
  → Driver 按 Run 钉住组合解析并执行有界 Turn（未知/缺失/不一致 fail closed）
      ├─ 当前 Live/内部流映射（可丢、非公共）
      ├─ Policy Gate / Tool Call
      └─ 超时或失败分类
  → 持久 result_ready 或 failure
  → Runtime Conversation Turn commit（本地已确认）
  → Run terminal / Lane 继续

im-less / 本地（已确认目标；无公共主链）：
本地或世界唤醒原因
  → Runtime 建立 Run（同一 Driver / Snapshot / Gate 边界）
  → 有界 Turn / 可选 Tool Call
  → 私有终态（private_completed / 私有失败 / conflict）
  → 不产生 Completion，不 ACK
  → 显式 send_message 且无源 Event：fail closed
```

- 未来统一 Typed Agent Events、用户控制入口和更强的执行中止能力接入时，仍不得把私有轨迹变成 im 服务端 公共事实。
- im-less 路径不走 Completion/ACK；不得把 `private_completed` 伪装成公共回复。

强制顺序：

> Completion/ACK 强序已随机制退役（2026-09-24），历史原文见 Git。

- （im-bound）只有当前 Event 的 Run 进入可恢复状态后，才允许继续处理该 Event；
- Live/delta 不能替代终态结果；
- Agent A 的失败、阻塞或取消不能改变 Agent B 的私有事实或授权；
- 取消是协作式、尽力而为的执行控制，不承诺强杀已经发出的上游 HTTP 请求；终态必须最终收敛或明确进入 conflict。

## 6. 恢复语义

> Completion/ACK 相关步骤已随机制退役（2026-09-24），历史原文见 Git；当前恢复事实见 `agent/README.md` §3.10。

- **起源分流：**恢复只在 Runtime 私有侧收敛（Turn/经历 commit 后收口），不欠 im 侧任何确认；im-bound 与 im-less 的差别只在有无源 Event。
- **未开始 Run：**可以重新读取 current 配置并建立新的完整 Run。
- **已建立但未执行：**只能使用该 Run 已钉住的 Snapshot（含 Driver 身份）；缺失或损坏时 fail closed（确认/更正 Run 除外——那类 Run 不执行模型），不用 current 配置或「当前 Driver」替代。
- **generating/在飞执行：**模型调用可能因外部 HTTP 与本地存储非原子而重试一次；必须保留 at-least-once 口径，不能重复公共写回。
- **result_ready：**禁止再次调用模型，只恢复本地 Turn/经历 commit。
- **结果已确认：**只恢复本地 Turn/经历 commit。
- **Turn commit 后：**无后续公共步骤。
- **取消/中断恢复：**历史实现见 Git（2026-09-24 剥离）。
- **本地事实冲突：**保持 conflict，停止该执行前缀，绝不通过猜测或跳过恢复步骤自行修复。
- **未来若引入持久 Turn Log：**孤儿 Turn 必须被明确归类为 interrupted；不得截断历史或伪造 completed。

## 7. 后续扩展接缝

以下是地基允许的扩展，不代表当前已实现：

### Memory

挂在 Context Builder/Context Snapshot 之前；Memory 必须拥有明确命名空间、来源、版本、检索授权和大小预算。Memory 不能直接改写 im 服务端 Message 或绕过 Run Snapshot。

### Tools

挂在 Capability Catalog + Policy Gate + Loop Driver 的 Tool Call 边界；工具必须提供 schema、超时、取消、安全摘要和错误分类。动态工具发现不能绕过 Agent Binding、User Grant、Run Snapshot 或 current recheck。

### Skills

Skill 是可版本化的 Agent 工作方法/提示资源，不是公共身份、工具授权或隐式 Memory。Skill 是否对 Agent 可见必须经过明确启用关系，并在 Run Context Snapshot 中留下来源引用。

### MCP

MCP 只能作为 Runtime 私有 Tool Provider 来源；Server 连接、凭据、工具选择、调用上下文和审计都留在执行端，不进入 im 服务端。MCP 的动态能力不能静默改变已建立 Run 的工具上限。

### Subagent

Subagent 默认是 Runtime 私有的临时或持久执行委托，不自动创建 im 服务端 Participant、Room Member、公共 Event 或公共 Message。

- 创建前显式校验 depth、能力范围和父子关系；
- 子 Agent 使用独立 Conversation/Run/Context 命名空间；
- 子 Agent 只能看到创建时授予的能力，不继承父 Agent 的全部权限后再靠黑名单剥除；
- 子结果与 Runtime 的 settled/aborted 账目分开标记来源；
- 跨 Agent 公共协作只能经明确的 im 服务端 Message 或未来版本化协议。

## 8. 不应进入地基的复杂度

当前明确不把以下设计作为 Agent 地基前置条件：

- 把助手（Assistant）与 Agent 拆成两个公共实体的做法；
- 动态插件中心、热加载与万能 Plugin Context；
- “Everything is a Plugin”式的同进程热组合；
- 完整 Session fork/branch/Lane 树与动态加工具；
- 把 Workspace、Shell、文件编辑、Git、Plan Mode 与 coding-agent 审批搬进地基；
- 同进程任意代码 Plugin、默认宿主高权限与多平台 Gateway 细节；
- im 服务端 公共事件化 Runtime 内部工具调用、Memory、Skill、MCP 或隐藏推理；
- 没有真实消费者和验收路径的通用工作流、市场、安装器、沙箱、联邦或多写者路由。

这些能力未来如有真实价值，必须先定义独立事实归属、信任边界、版本化契约和恢复验收，不得借“地基”名义提前引入。

## 9. 当前实现对照

当前实现事实的唯一权威是 `agent/README.md`（新线账本）；本节历史对照表见 Git（2026-09-24 剥离）。

## 10. 变更门禁与事实归属

- 本文拥有 Agent 地基原语的定义、关系、边界和不变量。
- `../../ARCHITECTURE.md` 拥有 C4 Level 1–2 系统地图和容器关系；不重复维护本人的完整术语定义。
- `agent/README.md` 拥有 Native Runtime 当前实现事实（旧线 `runtime/README.md` 已退役、死引用；旧实现判据归档已退出工作树，见 Git 历史 NATIVE_RUNTIME.md）。实现与本文不一致时必须报告，不得静默改写其中一方。
- `../runtime/terminal/README.md` 及其 `RUNTIME_TERMINAL_DECISIONS.md` 拥有 Native Runtime 当前终局理念、终局本体与负责人裁决入口；它们不覆盖 Core v1 冻结契约或 `agent/README.md` 的当前实现事实。旧目标、迁移、证据与考古材料（原 `docs/runtime/history/` 与根层 `RUNTIME_SLICE_*.md`）已退出工作树、归 Git 历史，不能作为新设计的并列权威。
- `../contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md` 拥有 im 服务端 公共 Agent Backend 协议；本文不得改变其冻结语义。
- `PRODUCT_VISION.md` 拥有产品价值和长期方向；本文不得以执行便利反推产品价值。
- 未来实现 Memory、Skills、MCP、Subagent 或新的 Loop Driver 前，必须先补充相应的窄设计、事实归属、权限模型、Snapshot 语义和真实验收路径。
- 任何改变公共身份、Room、Binding、Event 或跨容器连接的方案，都必须依照根 `../../AGENTS.md` 的解冻门禁另行取得主人确认。
