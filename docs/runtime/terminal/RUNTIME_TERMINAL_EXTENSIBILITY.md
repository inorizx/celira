# Celira Runtime 终局扩展模型

> 状态：**终局蓝图**——不描述当前实现，不覆盖宪法与冻结契约，也不表示已获实现授权；组内阅读顺序与修订裁决入口见 `README.md`。

## 1. 最小核心

终局 Runtime 的核心只承担不能由 Agent 自己完成、且必须可靠的能力：

```text
persist facts
preserve order
enforce isolation
check authorization
execute bounded actions
track receipts and reconcile Unknown
enforce budget and cancellation
recover without guessing
```

Runtime 不预先实现完整的关系网、任务树、知识库、工作流或人生摘要。

## 2. 原子能力

能力按世界变化划分，而不是按产品名词堆叠：

```text
Observe      观察现实
Retrieve     取回已有事实
Record       留下主体主动选择的痕迹
Act          对明确对象产生副作用
Communicate  向明确对象表达
Schedule     安排未来触发
Navigate     改变当前行动范围
Finish       结束当前清醒过程
```

这些是主体可见的基础动作类别，不宣称已经是理论上的最小闭合集合或完全正交的原语；复杂行为由主体组合能力产生。每项能力仍必须单独定义授权、结果、Unknown、取消和验收路径。

每个能力必须有；其中不可逆 Action 的 operation_id 与 execution-before-effect 是强制前置条件：

```text
input schema
authorization boundary
resource / target scope
stable operation_id and idempotency rule
execution-before-effect rule
timeout / cancellation rule
result and Unknown semantics
auditable fact
```

最小授权记录为：

```text
grant_id / subject / capability / target_scope / credential_ref
issuer / expiry / revocable
```

Action 的授权判定、grant_id 和适用版本必须随 execution fact 一起耐久化；授权撤销是否在派发前再次检查，由具体 Capability 契约声明。

## 3. 扩展接缝

可扩展部分都挂在稳定核心的边界上：

```text
Model Provider
  只提供认知，不拥有主体状态

Capability Provider
  提供可调用能力，不拥有 Agent 授权

World Connector
  提供观察和行动边界，不成为第二事实裁决者；声明 idempotent / deduplicated / reconcilable / retryable / non-retriable 能力

Memory Backend
  提供存储基础设施，不决定主体记住什么

Skill / Method
  提供可版本化工作方法，不隐式修改授权和上下文

  Sub-agent
  是隔离的执行主体或委托；第一验证切片不包含它，不默认共享上下文和权限
```

共享定义、连接池和执行基础设施可以；共享主体状态、授权、凭据、Context、私密痕迹和结果不可以。

## 4. 渐进式演进

每个新增能力必须先证明：

1. 存在真实的 Agent 能力缺口；
2. 不能由已有原子组合得到；
3. 新事实归属、边界和恢复语义清楚；
4. 不替主体做判断；
5. 有真实消费者和可验证验收路径；
6. 失败、取消、重试和 Unknown 已定义。

没有真实消费者时，保留扩展接缝，不提前增加复杂实现。

## 5. 多主体扩展

Runtime 可以承载多个主体。扩展必须按主体隔离：

```text
shared infrastructure
  ≠ shared consciousness
```

同一个 Provider、Tool、Memory Backend 或调度器可以服务多个主体，但每次执行必须钉住：

```text
subject
scope
sources
capabilities
credentials
run facts
```

任何跨主体协作都必须显式发生并留下可追溯事实。接收侧必须保留 origin / provenance，不得把发送方的 Interpretation 记成接收方的 Observation；共享 Connector 必须按主体过滤对象可见性。

同一 Subject 默认只有一个可写 Presence Episode；在席期间到达的 Trigger 进入该 Subject 的耐久队列，不抢占当前 Episode，不并行开启第二个可写 Episode。

## 6. 关系与高阶结构的涌现

关系、习惯、计划、工作现场和自我理解不是核心系统预先发放的对象。它们可以从：

```text
观察 → 行动 → 反馈 → 主体主动记录 → 重复选择
```

中逐渐形成。

系统提供事实、来源、可达性和边界；当前默认可达性通过时间、来源、kind、主体命名入口、最近条目、pin 和计数等机械结构建立，不依靠 Runtime 自动进行语义相关性排序；主体决定事实意味着什么。主体主动请求认知辅助与系统自动装配的区别见 `RUNTIME_TERMINAL_DECISIONS.md` §10.3；此处不新增扩展能力或改变当前装配规则。

## 7. 原理声明

> 核心只提供主体无法自举的可靠条件；其余复杂性留给原子能力的组合和主体自身的成长。本节不是关系、计划或自我结构已实现的功能承诺。
