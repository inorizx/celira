# Celira Runtime 终局执行模型

> 状态：**终局蓝图**——不描述当前实现，不覆盖宪法与冻结契约，也不表示已获实现授权；组内阅读顺序与修订裁决入口见 `README.md`。

## 1. 基本转移

Runtime 的最小执行单元不是“聊天”，而是一次状态转移：

```text
(S, x) → f(x) = y → apply(y, S) = S'
```

- `S` 是从主体 Fact Ledger 与耐久绑定关系重建出的运行投影；
- `x` 是本次模型可见的有限输入；
- `f` 是无状态模型；
- `y` 是模型输出；
- `apply` 只处理明确的结构化意图和可验证结果；
- `S'` 是行动后的新状态。

下一轮输入先构造 orientation view。主体在 orientation view 中产生 `choice`，再通过显式 Retrieve 取得所选事实，最后装配工作视野；Runtime 不预先替主体决定语义相关性。本文约束自动视野装配；主体主动请求认知辅助与自动装配的区别，以 `RUNTIME_TERMINAL_DECISIONS.md` §10.3 为准，不由此改变当前执行策略或新增工具。

```text
S' → orient(S', boundary, budget) → choice
   → Retrieve(choice) → assemble(S', boundary, selected_facts, budget) → x'
```

## 2. 一次清醒过程

```text
trigger
  → establish presence
  → determine boundary
  → enqueue concurrent triggers
  → orient (mechanical orientation view)
  → model choice / Retrieve
  → assemble working view
  → model call
  → classify output
  → execute or observe
  → record fact
  → assemble next view
  → continue or terminate
```

一次清醒过程可以包含许多模型调用和工具调用。正常终止由主体选择；超时、预算、取消、崩溃和基础设施故障是物理边界，不是主体意图。orientation view 必须包含当前时间、距上次 Episode 的时间、当前 Scope 入口、上次 terminal outcome 和 pending Unknown 的机械索引。预算至少区分 Episode 预算、Subject 预算和 Runtime 预算；下层耗尽只影响自身，不得篡改其他主体事实。同一 Subject 默认只有一个可写 Presence Episode；在席期间到达的新 Trigger 先作为耐久队列事实保存，不抢占当前 Episode，也不并行开启第二个可写 Episode，待当前 Episode 结束后再处理。

## 3. 事实与输入的分离

所有已经发生或被观察到的事件进入 Fact Ledger，至少保留：

```text
causal sequence
source
actor / subject
scope
kind
status
  observed_at
  correlation（关联一次请求及其 receipt / Unknown 的 id）
  operation_id（仅用于一次结构化 Action 的耐久身份）
```

模型输入不等于事实账本。每一轮只带入边界允许的有限 View。Runtime 只能按权限、来源、时间、固定配额、主体显式 pin 或命名入口进行机械装配和裁剪，不按语义相关性替主体排序。旧事实通过主体显式读取能力重新进入输入，而不是自动把全部历史拼接进去。

## 4. 上下文代

当 View 接近模型窗口上限时，Runtime 可以结束当前 Context Generation 并重新装配下一代 View：

```text
Generation 1 → durable facts → Generation 2
```

Context Generation 只是一次普通的 View 重装配事件，不是本体对象、记忆层或第二条事实筛选入口。换代不等于离开当前 Scope，不等于结束清醒过程，也不等于删除历史。首次装配与换代装配遵守同一规则。KV cache 只属于某个模型实现和某一段输入的局部优化；语义正确、事实不丢和可恢复性优先于缓存复用。

## 5. 输出处理

模型输出必须区分。`classify` 只接受模型的结构化自报（例如 function call 或结构化输出），Runtime 只校验 schema、权限和执行边界，不从自由文本推断 intent：

```text
expression       主体表达了什么
  observation      Runtime 向主体提供的事实投影
  result fact      Capability 或 Connector 返回的结果记录
  action intent    主体明确请求什么行动
  invalid           结构化输出不符合 schema
  truncated         输出被边界或 Provider 截断
  provider failure 模型服务调用失败
execution        Runtime 实际执行了什么
receipt          行动结果被确认为什么
unknown          结果无法确认
terminal outcome 清醒过程如何收束
```

自然语言判断不自动升级为世界事实。只有结构化、经过授权并能被执行边界接受的意图，才能进入行动链。`invalid`、`truncated` 和 `provider failure` 不进入 Effect 链，必须记为运行事实并进入修复、重试或 interrupted/failed 路径；它们与“行动已发出但结果未知”的 `unknown` 不同。

```text
intent → operation_id → durable execution → authorization / capability → effect → receipt / unknown
```

## 6. 取消、崩溃与恢复

内存只保存正在执行的暂态缓存；它不能是事实唯一来源。重要事件在模型派发前后、能力执行前后和终态收束时进入耐久账本。

恢复规则：

- 每个不可逆 Action 必须先耐久化稳定的 operation_id 与 execution fact，再发起外部 Effect；
- Runtime 不得在 Unknown 状态下自动重新派发同一 operation；只有 Connector 提供幂等或去重契约时，才能保证同一 operation 不产生第二次外部 Effect；
- 已发出但未确认的行动必须保留 Unknown 或显式待结算状态，并通过明确路径结算；
- 未完成清醒过程不得被伪装成正常完成，下一次清醒必须可见 terminal outcome；
- 恢复可以重建 Context，但不能凭空重建模型隐藏思维；
- 事实账本是恢复的唯一耐久输入，S 与状态机是可由账本确定性重建的运行投影。

## 7. 事实保存层次

```text
Fact Ledger：完整运行事实和证据
Subject Record：主体通过 Record 主动留下的跨次内容
View / Context：本次模型实际看到的有限投影及其输入编码
Scope：主体可再次进入的工作现场与机械入口
KV cache：模型调用的暂态加速缓存，不属于事实保存层
```

运行结束、离开 Scope 或重新装配 Context 都不自动删除事实。

## 8. 原理声明

> Context 可以丢失并重建，事实不能静默消失；缓存可以失效，主体的行动边界不能被缓存绑架。
