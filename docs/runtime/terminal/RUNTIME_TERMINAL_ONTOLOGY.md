# Celira Runtime 终局本体

> 状态：**终局蓝图**——不描述当前实现，不覆盖宪法与冻结契约，也不表示已获实现授权；组内阅读顺序与修订裁决入口见 `README.md`。

## 1. 不是房间模型，而是主体—世界模型

空间隐喻只用于帮助理解。终局本体不是一套房间对象，而是：

```text
Subject ↔ World
        ↓
   Presence Episode
        ↓
     Scope + View
```

主体通过清醒过程接触世界；世界通过事实和观察为主体提供连续性。

## 2. 生命周期

```text
Absence
  → Trigger Envelope recorded
  → Trigger Accepted
  → Presence Episode
  → Orient
  → Select / Release Scope
  → Observe / Act / Record
  → Continue / Hold / Refuse / Finish
       Hold   → 在席不动（等下一个 Trigger）→ Observe / Act / Record
       Refuse / Finish → terminal outcome
  → Absence
```

`Hold`（歇着）表示主体**仍在席**、只是此刻无事可做：它**不产生 terminal outcome、不释放当前 Episode 占用**——有事（新的 Trigger）再驱动**同一段**。`Refuse` 是独立可记账终态，不等同于 `Finish` 的正常完成。

⚠ **2026-10-10 收口（主人原话「行吧，可以」）**：本行原写「`Wait` 表示主体主动结束当前在席并通过未来 Trigger 衔接；它**不保持**当前 Episode 占用」，与 `RUNTIME_TERMINAL_DECISIONS.md` §7 登记的「本线走的是第二条（`Wait` 保持在席）」是**两读**。现收口为**保持在席**：**不结束在席**由 `Hold` 承担，「结束在席」由 `Finish` 承担，`Wait` 这个名字随之退役（它与 `SubjectEnding::Waiting` 的"结束"义撞名）。

⚠ **2026-10-10 实现落地（在场那一刀，`agent/README.md` §3.66）**：上面那句原写「**实现欠着**：宿主接住后**立刻再驱动**——不是"等下一个 Trigger"」——**今天不是了**：宿主接住 `Unsettled` 之后**等下一个 Trigger，再驱动同一段**（`Host::run_segment`）。
⚠ **同时收窄一处词义**（`RUNTIME_TERMINAL_DECISIONS.md` §6 第三次收口）：`Hold` **不产生 terminal outcome** ⇒ `subject:held` 线缆上再也产不出来（旧账照读不写），**收场只有一个来源**：主体自己说结束。

Trigger Envelope 是 Fact Ledger 中的一类运行事实，记录来源、到达时间、载荷引用和投递状态；它不等于主体认可的 Subject Fact。触发只创造一次观察和行动机会，不规定主体必须做什么。清醒过程可以停留在无 Scope 的状态，也可以主动选择一个 Scope；结束时不要求先执行仪式性的“回到某处”。同一 Subject 默认只有一个可写 Presence Episode；在席期间的新 Trigger 先入队，不抢占、不并行写入，待当前 Episode 结束后处理。

## 3. Scope 与工作现场

Scope 是主体可以再次进入的持久工作现场，而不是主题标签。一次 Presence Episode 会从 Scope 解析出当前的 boundary。它可以围绕项目、问题、对象、关系或一组行动形成，但系统不按语义自动切分或合并。

一个 Scope 具有：

- 可引用的来源边界；
- 可观察的对象集合；
- 可请求的能力集合；
- 持续的行动轨迹；
- 可再次进入的事实入口。

Scope 是持久工作现场；boundary 是一次 Episode 内由 Scope、授权和主体选择解析出的瞬时边界。

“Desk”只是 Scope 的生活隐喻：主体进入它，表示选择在这个范围内继续行动；离开它，表示解除当前行动绑定，不表示删除事实。

## 4. View 与 Context

View 是当前主体实际能看到的有限世界投影；Context 是把 View 编码为模型输入的执行表示。

构造顺序必须是：

```text
确定现实边界
  → 确定可引用来源
  → 提供当前处境事实
  → 纳入主体主动取用的记录
  → 加入可用能力
  → 受上下文预算约束
```

Context 不是全局记忆池，不是全部历史，不是事实真相，也不是模型隐藏状态。

## 5. Fact、Observation、Interpretation

```text
Fact:
  来源明确且可追溯的事件、声明、观察或执行记录；可追溯不等于内容自动成为外部世界真理

Observation:
  Runtime 在边界内提供给主体的事实投影

result fact:
  Capability 或 Connector 返回的执行结果事实；它不是输入侧 Observation

Interpretation:
  主体对事实的理解和判断

Decision:
  主体基于理解作出的选择
```

Runtime 可以保存 Fact、构造 Observation、执行 Decision 对应的明确 Action；不得把 Observation 伪装成主体的 Interpretation，也不得把 Interpretation 自动写成 Fact。Fact Ledger 保存的是来源明确的事件、声明、观察与回执记录；记录可追溯不等于其内容自动成为外部世界真理。

## 6. 多主体隔离

Runtime 可以承载许多主体，但每个主体在逻辑上拥有独立的 namespace；物理存储可以共享，但每次读取、写入、恢复和审计都必须按 Subject namespace 隔离。每个主体拥有独立的：

```text
identity / self
world references
scope
view
fact ledger
capability authorization
presence episodes
```

共享模型、连接池、工具实现、调度器和存储引擎只是共享基础设施。默认禁止共享主体状态、上下文、私密痕迹、授权、凭据和未授权观察。

跨主体信息只能经显式动作和明确授权传递。引用来源必须保留；“同一 Runtime”不构成“同一现实”。

## 7. 关系不是前置器官

主体与某对象的关系，是多次观察、行动、记录和解释后形成的认知结构。Runtime 不需要先建立一个关系图谱来替主体判断亲疏、信任或重要性。

Runtime 只需提供：

- 交互事实及来源；
- 当前时间、距上次 Episode 的时间和缺席期间可观察的机械变化；
- 可追溯记录；
- 引用边界；
- 主体主动记录的存放与取用能力。

## 8. 事件账本

Fact Ledger 按因果顺序追加运行事实：请求、输出、观察、行动、结果、失败、取消、等待和 Unknown。它是恢复和审计的事实底座，不是模型输入的全量复制，也不是自动生成的人生摘要。

`Unknown` 是一种有效事实状态：已发生请求但结果无法确认时，Runtime 必须保留不确定性，不能猜测成功或失败。执行状态至少区分 `prepared → dispatched → unknown → reconciled_success | reconciled_failure | abandoned`；`abandoned` 只表示不再追查，不推断外部 Effect 未发生。结算必须来自明确查询、主体明确放弃或预先定义且可审计的过期策略。

## 9. 本体判据

1. 删除一个概念后，如果主体只是失去系统替主体做的判断，而非失去能力，它不应成为核心对象。
2. 不同现实可以属于同一个主体，但不能默认进入同一个 View。
3. 持久事实必须可追溯；临时 View 可以重建。
4. 任何由系统自动加入 Context 的内容，都必须能说明来源、边界和加入理由。
5. 生活隐喻必须能翻译成明确的状态或动作，否则只能留在叙事层。
