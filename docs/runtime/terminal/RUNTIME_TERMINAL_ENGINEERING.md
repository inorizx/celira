# Runtime 终局工程骨架

> 状态：**终局理念到工程实现的桥梁文档**。
>
> 本文把 `terminal/` 的终局判据翻译成 Rust 工程边界，但不把当前实现、旧 Slice 或某个目录结构升格为终局本体。本文不修改 Core v1 公共契约，不授权立即实现；真正进入代码前，仍需为具体切片单独确认范围、拓扑和验收。

## 1. 原理声明

以领域不变量划分 Runtime，以 Rust 类型和封装使非法状态难以表达，以薄协调器串联外部端口；事实账本只保存可追溯事实，系统不替 Subject 做意义判断。

这套取舍解决的是两个问题：

1. 模型每次调用都可能无状态、算错或中断，但 Subject 的清醒期、行动事实和恢复边界必须可靠；
2. Runtime 需要可扩展，但扩展不能绕过授权、事实记录、隔离和恢复秩序。

## 2. 终局模型到工程对象

| 终局概念 | 工程对象 | 责任 | 明确不负责 |
|---|---|---|---|
| Subject | `Subject` / `SubjectId` | 稳定身份、私有命名空间、单一可写 Presence | 不持有所有 Memory、Tool、Provider 状态 |
| Presence Episode | `Presence` / `PresenceId` | 一次有限清醒期的状态转移和终态收敛 | 不直接写文件、不调用模型 |
| Scope | `Scope` / `ScopeId` | 可再次进入的持久工作现场 | 不替主体判断重点和意义 ⚠ **2026-10-06：这一格未实现**——实现里没有 Scope 实体（`agent/` 零命中）；行动范围今天是 **Run 级绑定 ＋ `navigate` 惰性生效**（宪法判据）。见 `RUNTIME_TERMINAL_FORM.md` 的 Scope 一节 |
| Action Intent | `ActionIntent` / `OperationId` | 结构化行动意图、目标、参数、幂等身份 | 不直接造成外部副作用 |
| Effect | `Effect` / `Receipt` / `Unknown` | 表达外部动作及其可知结果 | 不把未知猜成成功或失败 |
| Fact | `Fact` / `FactLedger` | 追加可追溯事实、恢复和确定性重建 | 不决定什么值得记忆 |
| View | `View` | 当前边界内的机械事实投影 | 不做语义排序、摘要和亲疏判断 |
| Context | `Context` | 把 View 编码为模型输入 | 不成为第二份事实账本 |

这些是边界，不要求每个名词立即对应一个 Rust 文件。对象只有在拥有状态、不变量或真实消费者时才建立。

## 3. 领域层：只放不变量

目标结构：

```text
runtime/src/domain/
├─ subject.rs       # Subject、SubjectId、可写 Presence 资格
├─ presence.rs      # Presence 状态机、Outcome、收敛
├─ scope.rs         # Scope 引用与工作现场边界
├─ action.rs        # ActionIntent、OperationId、动作目标
├─ fact.rs          # Fact、Receipt、Unknown 等事实值
├─ view.rs          # View 的边界和可见事实
└─ context.rs       # Context 与模型输入前的中立表示
```

### 3.1 充血模型只放在真正拥有规则的对象

- `Subject` 守护“同一时刻至多一个可写 Presence”；
- `Presence` 守护接受、运行、等待、拒绝、失败、中断和完成之间的合法转移；
- `ActionIntent` 守护稳定 `OperationId`、目标和参数的结构完整性；
- `Effect`/执行事实守护 Prepared 先于外部 Effect、Receipt/Unknown 不混淆。

`FactLedger` 的核心职责是顺序、原子追加、完整性校验和重建，不应替主体决定事实的意义。不要把所有领域规则塞进一个 `Runtime` 或 `Subject` 上帝对象。

### 3.2 Rust 表达规则的建议

- `SubjectId`、`PresenceId`、`ScopeId`、`OperationId` 使用不同 Newtype，避免跨域误传；
- 状态使用带数据的 `enum`，避免多个布尔字段表达非法组合；
- 关键字段私有，通过领域方法改变状态；
- 失败使用 `Result`，不可知结果使用显式 `Unknown`，不以 `Option` 偷渡语义；
- Typestate 只用于短生命周期、编译期收益明确的边界；跨重启状态仍由持久事实恢复，不强行类型化；
- `trait` 只放在真正可替换的外部边界，领域对象之间不为“抽象”而抽象。

## 4. Application 层：薄协调，不做裁决

目标结构：

```text
runtime/src/application/
├─ wake.rs       # Trigger 接纳与 Presence 创建
├─ orient.rs     # Scope、授权和 View 装配
├─ drive.rs      # 模型回合驱动
├─ execute.rs    # ActionIntent 的授权与执行编排
├─ persist.rs    # 事实提交顺序
└─ finish.rs     # 调用 Presence 收敛并完成外部提交
```

Application Coordinator 可以：

- 按正确顺序调用 Subject、Presence、Ledger、Scope 和外部 Port；
- 处理重试、取消和适配层错误的编排；
- 把外部结果转换为领域可理解的事实。

它不可以：

- 直接构造“看起来合理”的 Outcome；
- 把模型自由文本暗中解释成行动；
- 把 Unknown 改写成成功或失败；
- 同时写两个声称拥有同一事实的裁决源；
- 吸收所有领域规则成为新的超级 `RunService`。

## 5. Ports / Adapters：隔离外部世界

```text
runtime/src/ports/
├─ model.rs       # 模型调用
├─ world.rs       # 外部观察
├─ effect.rs      # 外部副作用、回执、对账
├─ ledger.rs      # 事实追加与重建
├─ scope.rs       # Scope 持久化
└─ clock.rs       # 时间来源

runtime/src/adapters/
├─ core/          # im 服务端 REST/SSE 与通信适配
├─ provider/      # OpenAI-compatible 等模型适配
├─ file/          # 文件账本、Scope 和 Artifact
└─ host_admin/    # 进程装配与受管控制面
```

每个能力或外部适配器在进入 Runtime 前必须声明：目标范围、授权/Grant、幂等性、取消语义、回执语义、Unknown 处理和恢复方式。Connector 不得成为第二个事实写者；Provider 不拥有 Subject 状态；存储后端不决定“记住什么”。

## 6. 持久化拓扑与唯一写者

终局方向是按事实归属分面，而不是把所有状态塞入一个万能 Store：

```text
Subject Fact Ledger
  ├─ Presence / Action / Effect / Receipt / Unknown facts
  └─ deterministic rebuild → runtime projection

Scope Store
  └─ 可再次进入的工作现场、正式消息和 Scope 事实

Trace / Journal
  └─ 观察轨迹，不作为恢复权威
      ⚠ 2026-09-07 P2 收敛修订：经主人批准的两层权威切分，持久化 Journal 已立为
      **宿主层**恢复权威（主体域权威仍为 Fact Ledger）——旧线实现事实曾见
      `agent/runtime/README.md` §5 与 §6（该线已于 2026-09-17 退役，考古走 Git
      历史；本行保留为 P2 之前的目标态判断）
```

旧线实现与终局方向之间存在明确过渡：`agent/runtime/README.md`（已于 2026-09-17 退役，考古走 Git 历史）曾记录 AgentStore、SessionStore、Effect Ledger 和宿主状态的实际写者关系；当前实现事实见 `agent/README.md`。当前不能宣称统一 Ledger 或跨 Store 整体事务已经完成。

持久化必须维持以下不变量：

```text
structured ActionIntent
  → stable operation_id
  → durable execution fact
  → authorization / capability check
  → external effect
  → receipt | unknown
```

同一个 `operation_id` 不得在恢复或重试中造成重复外部 Effect。无法确认结果时必须保留 Unknown，直到显式查询、主体放弃或经审计的过期策略结算。

## 7. 第一实现切片

第一切片只证明一个最小、可重放、可故障注入的闭环：

```text
单 Subject
  → 单 Scope
  → View / Context
  → Fake Model
  → 一个 Fake Connector
  → Fact Ledger
  → 崩溃注入
  → rebuild_state
```

### 必须通过的验收

1. 同一 Subject 同时不能接受两个可写 Presence；
2. Presence 只能由明确 Outcome 收敛；
3. Prepared fact 必须先于外部 Effect；
4. Effect 后 Receipt 丢失只能得到 Unknown；
5. 重启后保留同一个 `operation_id`；
6. 恢复不会重复外部 Effect；
7. Unknown 不会被自动猜成成功或失败；
8. 同一个 Ledger 重放两次得到相同的状态投影；
9. `Waiting`、`Refused`、`Failed`、`Interrupted` 与 Unknown 不互相冒充。

### 第一切片不做

- im 服务端 公共协议或 SQLite migration；
- 多 Agent 调度和复杂控制面；
- 插件市场、Sub-agent 和 MCP；
- 语义 Memory、自动摘要、自动排序和重要性判断；
- 全局 `Agent → Subject`、`Run → Presence`、`Session → Scope` 重命名；
- 旧 AgentStore 的兼容迁移；
- 第二个公共事实写者；
- 为了目录完整而预建没有真实消费者的模块。

## 8. 当前代码的渐进处置

> ⚠ **历史快照（2026-09-07 P3 收敛之前）**：本节对 `store.rs` / `lifecycle/service/` /
> `host_admin/` 等的处置建议，写于 Legacy 执行链尚存之时。P3 后 `RunService` /
> `RuntimeHost` / `ImPump` / `store.rs`（AgentStore/FileStore）/ Session 私有持久面
> **已物理删除**；当前实现事实**曾**以 `agent/runtime/README.md` 为准（**该线已于 2026-09-17 退役，考古走 Git 历史**），本节只作考古。

| 当前代码 | 第一阶段处置 |
|---|---|
| `domain/agent.rs` | 保留并逐步收敛 Subject 语义 |
| `domain/run.rs` | 保留已证明的状态转移，逐步映射 Presence |
| `domain/outcome.rs` | 保留明确终态和值对象 |
| `effect/` | 保留可靠性零件，重新确认 Fact/Effect 归属 |
| `provider/` | 作为 Adapter 保留，不让 Provider 持有主体状态 |
| `extensions/session/` | 作为 Scope 候选，不立即全局改名 |
| `lifecycle/service/` | 暂作为组合根，按真实边界逐片抽取 |
| `store.rs` | 作为当前事实来源，不直接认定为终局聚合 |
| `host_admin/` | 保持在外部装配层 |
| `observability/` | 保持 Trace，不混入恢复账本 |

## 9. 第一切片代码盘点与处置

> ⚠ **历史快照（第一切片立项时）**：本节所说的「三条并存路径」中的**生产路径
> （`host_admin → RuntimeHost → RunService → ForLoop`，以 AgentStore 为 Run outcome
> 写侧裁决面）已随 P3 物理删除**，不再是当前代码事实；旧线的唯一执行链（admin →
> LaneFleet → DialogueWorker → DialogueLane，引擎 = `SubjectCoordinator`）也已于
> 2026-09-17 随旧线退役退出工作树（当前实现事实见 `agent/README.md`）。本节其余
> 条目（保留 / 隔离 / 暂不碰）同为当时判断，只作考古，不授权继续旧路线。

当前代码不是一条已经统一的终局路径，而是三条并存的实现/验证路径：

1. **生产路径**：`host_admin → RuntimeHost → RunService → ForLoop`。受管宿主默认仍以 AgentStore 作为 Run outcome 的写侧裁决面。
2. **Effect 可靠性路径**：`ActionGate → FileEffectLedgerStore → rebuild_state`。这里已经有 Prepared-before-effect、Unknown、显式 reconcile/abandon 和进程级崩溃测试。
3. **迁移验证路径**：测试显式注入 `FileRunBoundaryWriter`、`EffectOrgan`、`FileRunOutcomeReader`，把 RunService 与 Ledger 接起来；它不是默认二进制装配。

### 9.1 保留

- `runtime/src/domain/agent.rs`：`Subject` 的单活动 Run、停止拒绝和精确记账守卫；新路径需补齐 Wakeup 归属及恢复序号边界。
- `runtime/src/domain/run.rs`：`Run::converge` 及明确终态。
- `runtime/src/ports.rs`：`ModelPort`、`TurnDecision`、`CancelSignal`，第一切片可兼用。
- `runtime/src/effect/intent.rs`：结构化意图解析与参数规范化。
- `runtime/src/effect/gate.rs`：授权、Prepared-before-dispatch、Unknown、reconcile/abandon。
- `runtime/src/effect/file.rs`、`store.rs`、`rebuild.rs`：文件账本、OWNER、追加和确定性重建的基础，但需补身份、生命周期和写入不确定性校验。
- `runtime/src/domain/message.rs`：来源、信任和工具角色边界。
- 现有 Effect/Recovery 测试：作为可靠性回归资产，不把测试装配当成生产接线证明。

### 9.2 重写或隔离

- `lifecycle/service/run.rs`（P3 已物理删除，历史路径）：`process` 同时编排 Subject、Session、Context、ForLoop、持久化、通信、Memory、Slot、Alarm 和 Journal，不作为第一切片的中心协调器。
- `lifecycle/service/persistence.rs`：当前是 AgentStore-first，再写 Ledger terminal mirror；不直接带入“Ledger 唯一事实源”路径。
- `lifecycle/run_boundary.rs`：可选 writer、默认 no-op 和吞掉读取错误的形状，不作为新权威恢复接口。
- `effect/driver.rs`：`SliceDriver` 是测试工具，不能直接转正；它没有真实 Subject/Scope，且会折叠不同终态。
- `extensions/effect_organ.rs`：旧关联在 dispatch 后 fail-soft 写入；新路径不依赖事后关联作为唯一归属。
- `extensions/session/manager.rs`：只提取必要的 Scope 归属和有序追加规则，不整体搬成终局 Scope。

### 9.3 第一切片暂不碰

- `host_admin/`、`lifecycle/host.rs`、`lifecycle/supervisor.rs`：保持旧生产可运行性和多 Agent 编排。
- `provider/transport.rs`、`wire.rs`、`config.rs`：保持真实 Provider、网络和凭据边界。
- `store.rs`、`extensions/session/file.rs`：不为第一切片扩大旧存储迁移面。
- `extensions/im.rs`、`im_pump.rs`、`exec.rs`、`net.rs`：不在第一切片接入真实外部世界。
- `lifecycle/consolidation.rs` 和跨 Session/Scope 的整理逻辑。

### 9.4 第一切片最小代码触点

推荐新增一条独立的、测试先行的组合路径，而不是改线旧生产入口：

```text
assembly/coordinator.rs       # 单 Subject、单 Scope、单活动 Presence/Run
新建最小 Scope 边界              # 不搬多桌 fork/rotate/archive
现有 ModelPort/Fake Model         # 只作为可替换模型端口
现有 Effect Gate/File Ledger      # 加固后复用可靠性零件
first_slice_contract.rs           # 正常、失败、崩溃、重放、Unknown 验收
```

第一切片的真实恢复验收必须包含“重启后再次装配并调用 Fake Model”，并确认模型能观察到 pending/Interrupted，而不只是测试端直接读取 Ledger。还必须验证：输入重复不会无意创建第二次行动，独立意图即使参数相同仍获得不同操作身份，terminal 写失败不能报告 durable Completed，错 Subject/Scope、矛盾终态和损坏账本均拒绝。

## 10. 进入实现前的门禁

任何第一切片实现都必须先补齐：

- C4 Level 1–2 拓扑及一句原理声明（本文件的拓扑只描述目标工程形状，具体切片仍需对照当前生产装配）；
- 领域对象、不变量和唯一写者表；
- 初始状态、每个故障点、恢复状态和失败判据；
- Fake 与真实 Adapter 的边界；
- 不修改 Core v1 公共契约的证明；
- 真实消费者或明确的测试消费者；
- 单独的实现授权和独立提交。

本文是工程方向，不是实现授权。若具体实现需要新增公共事实、跨容器连接、持久化语义或安全边界，必须暂停并重新请负责人裁决。
