# Celira Agent Runtime：终局状态与路线导航

> 本文是宏观导航，不是施工单、接口设计、测试报告或局部任务清单。
> 它回答三个问题：Celira 最终要成为什么、当前已经站在哪里、接下来按什么顺序收敛。
> 具体实现以代码和测试为证；产品与架构原则以 `00_AGENT_CONSTITUTION.md`、`02_AGENT_FOUNDATION.md`、`05_AGENT_ENGINEERING.md` 为准。
>
> 产品价值与宪法级判据见 `../constitution/PRODUCT_VISION.md`、`../constitution/AGENT_PHILOSOPHY.md`、`../constitution/AGENT_FOUNDATION_CONTRACT.md`、`../constitution/AGENT_CONTINUITY_CONSTITUTION.md`（冲突时以宪法为准）。
>
> 当前导航基线：TASK_09 完成后的 `ae958bb`。本文不把该提交号当作永久版本号；每次阶段性验收后只更新“当前状态”事实。

## 1. 终局判断

Celira 不是一个把模型包装成聊天机器人的工具集合，也不是一个替模型规划、记忆、恢复和交接的自动化框架。

Celira 的终局是：

> **一个以 Agent 为持续主体、以 Environment 为现实载体、以 Trace 为连续经历、以 OS/App 提供真实能力、以 Provider Adapter 连接可替换模型的 Agent 原生执行系统。**

核心关系：

```text
Agent
  = 持续身份
  + 归属环境
  + 连续经历
  + 主动判断

Model
  = ModelInput → Model → ModelOutput

Celira
  = 事实保存
  + 有限上下文构造
  + 真实能力执行
  + 结果确认
  + 可靠边界
```

系统的价值不是替主体思考，而是保证 Agent 每次面对的现实尽可能诚实：

- 已发生的事实不会被遗忘或伪造；
- 未发生的事情不会被包装成已完成；
- 无法确认的事情保留为 Unknown；
- 模型看到的是当前获准的 View，而不是环境全部内容；
- 工具效果由现实与工具确认，不由模型文本自证；
- 进程退出、模型更换和 Provider 更换不会自动摧毁 Agent 的身份与经历；
- 系统不代替主体规划、摘要、排序、交接或判断业务是否完成。

## 2. 终局对象关系

```text
Agent
 └─ Environment
     ├─ Facts：系统确认的现实事实
     ├─ Trace：Run 内按顺序追加的经历
     ├─ Materials：Agent 可主动读取的材料
     ├─ Capabilities：当前可获得的能力
     └─ Runs
         ├─ Turn：一次模型调用及其处理边界
         ├─ View：本轮获准看见的上下文投影
         ├─ Action：模型提出、系统准入并交给能力端的行动
         └─ Outcome：现实确认的成功、失败或未知
```

这些对象不是同一层的不同名字：

```text
Trace 不是 Memory
View 不是全部 Environment
Run 不是聊天话题
Run 不是进程
Turn 不是 Run
Provider Session 不是 Agent 状态
Action Request 不是 Action Effect
模型正文不是现实事实
```

## 3. 终局系统形状

宏观拓扑保持为：

```text
Human
  ↓
Client
  ↓ 公共协议
IM / 公共世界
  ↕ 明确 Connector / App 边界
Agent Environment / 私有世界
  ├─ Foundation：事实、效果、隔离、耐久边界
  ├─ Core：Run、Turn、View、Trace、唯一执行循环
  ├─ OS：身份、能力入口、准入、资源与机械状态
  ├─ Apps：具体世界与具体能力
  ├─ Provider Adapter：外部模型协议 ↔ Celira 中立语言
  └─ Composition：显式装配与接线
```

依赖方向：

```text
Foundation 不依赖 Core
Core 不依赖 OS、App、Provider 或具体存储格式
App 只依赖 OS 的窄契约
Provider 只通过 Adapter 接入
Composition 负责选择和装配
```

终局不是层数越多越好。任何新抽象都必须有现实消费者，并且明确减少了什么耦合或保护了什么边界。

## 4. 终局行为模型

一次 Agent 活动的最小形状是：

```text
现实与事实
  + 当前输入
  + 当前能力
  + 当前限制
  + Agent 主动读取的相关经历
        ↓
      View
        ↓
    ModelInput
        ↓
      Model
        ↓
    ModelOutput
        ↓
  中立解释与准入
        ↓
  真实 Action / Observation / Outcome
        ↓
  追加事实与经历
        ↓
  下一次显式 Turn，或等待/结束
```

其中：

- 模型调用一次就是一个 Turn；
- 多个 Turn 组成一个有限 Run；
- Run 内的经历按顺序追加；
- 纯正文可以结束本次驱动但不自动结束 Run；
- `finish_run` 或系统资源边界可以结束 Run；
- Waiting 不等于 Ended；
- 恢复成功不等于自动继续；
- Action 的意图、准入、派发和结果必须保持区分。

## 5. 工具与现实效果的终局原则

工具不应被 Core 统一强行建模成一种复杂任务对象。工具按自身能证明的事实报告结果。

真正原子的工具可以只表达：

```text
Success | Failure
```

外部效果不可确认、或存在进程崩溃窗口的工具必须能够表达：

```text
Success | Failure | Unknown
```

只有真实长任务需要时，才增加：

```text
InProgress | Progress | Cancelled | JobHandle
```

基本事实边界：

```text
ActionRequested
  ≠ ActionPrepared
  ≠ ActionDispatched
  ≠ ActionSucceeded
```

Core 的最低可靠性职责是：

- 不把准备伪装成派发；
- 不把派发伪装成成功；
- 不把 Unknown 改写成 Failure；
- 不在未决外部效果上无条件 Replay；
- 不替主体判断业务是否完成；
- 不自动重试、补偿、回滚或查询不存在的事实。

## 6. Context、Memory 与 Retrieve 的终局方向

Celira 不把“把全部历史塞进 Prompt”当作连续性，也不把自动摘要当作记忆本身。

终局方向是：

```text
Agent 有一个持续环境
Agent 可以看到有限 View
Agent 可以主动发现材料
Agent 可以主动读取相关经历
Agent 可以主动留下未来可能使用的 Memory
系统只提供机械入口和真实边界
```

因此需要保持以下区分：

```text
Trace = 已发生的活动经历
Fact = 系统确认的现实事实
Memory = Agent 主动留下的可再使用内容
Retrieve = Agent 主动取得相关材料的能力
View = 某一 Turn 获准看到的有限投影
```

未来的 Context/Retrieve 建设目标不是自动替主体选择“最重要的记忆”，而是让 Agent 能够用可理解、可限制、可验证的方式发现和读取它需要的材料。

## 7. 当前状态：TASK_09 之后

当前已形成的最小 Runtime 闭环：

```text
Agent / Run / Turn / Trace / View
Foundation / Core 分层
OS 与 App 窄契约
File App 与 Notes App
Provider Adapter 原生 tool call
中立 ModelOutput
Protocol continuation 的显式载体
纯正文 Waiting
同进程 RunSession 多次驱动
跨进程 Run 恢复最小闭环
恢复判定与显式宿主继续
```

TASK_09 已证明：

- 可以从已确认事实只读推导恢复结论；
- 可以区分可恢复、已结束、阻断、未知和拒绝；
- 新进程可以重建同一个 Run；
- 恢复不会自动调用模型；
- 恢复不会自动 Replay 或补偿；
- 原生请求帧、工具回执和协议续接数据可以按真实关系重建；
- 旧进程的写资格不会被新进程继承。

当前状态仍不是完整产品：

- 真实 Provider 线上行为尚未充分验证；
- 工具效果契约尚未针对长任务形成独立公共形状；
- Retrieve 仍是基础能力，不是完整产品级检索体验；
- 产品宿主和用户可理解的状态呈现尚未完成；
- 后台调度、通知、唤醒和长任务只保留为未来可能能力；
- usage/token、成本与配额仍需真实 Provider 场景补齐。

## 8. 从现在到第一个成品的宏观路线

路线不是按代码目录推进，而是按“用户能否依赖”推进。

### 8.1 Runtime 可靠性收敛

目标：保持当前 Core 语义稳定，并把工具效果边界落到少数真实能力上。

需要证明：

- 成功、失败、未知不会互相伪装；
- 保存顺序和外部效果边界可解释；
- 真实工具失败不会破坏下一次判断；
- 恢复不会产生重复外部效果；
- 不用新增大平台解决局部问题。

### 8.2 Context 与 Retrieve 可用化

目标：Agent 能主动找到并读取自己需要的材料，而不是依赖全量历史或系统摘要。

需要形成：

- 可发现的历史与材料入口；
- 有界读取与分页；
- 当前 Run 与旧 Run 的清晰关系；
- 事实、经历、Memory、临时注入的展示边界；
- Provider 协议续接数据不误呈为普通记忆。

### 8.3 至少一个真实 Provider 闭环

目标：至少一个真实模型在真实网络条件下稳定完成：

```text
正文
→ 原生 Action
→ 真实结果
→ 下一次请求
→ 等待或明确结束
→ 失败/超时/截断的真实处理
```

Provider 数量不是目标。先验证一个主 Provider，再验证一个字段形态明显不同的 Provider，足以证明 Adapter 边界没有被单一供应商偷渡定义。

### 8.4 至少一个真实端到端场景

选择一个小而完整的场景，例如：

```text
Agent 读取资料
→ 主动查阅相关经历
→ 创建或修改一个明确材料
→ 看到真实结果
→ 进程退出
→ 新进程恢复
→ 继续或明确结束
```

这个场景必须走生产路径，而不是只在测试替身里成立。

### 8.5 产品宿主

目标：普通用户可以理解并使用 Runtime，而无需知道内部模块名称。

至少需要能看见：

- Agent 身份与归属环境；
- 当前 Run 是否等待、结束或阻断；
- 最近发生的事实与 Action 结果；
- Unknown 为什么未知；
- 何时需要用户显式继续；
- Provider 与模型配置；
- 基本的成本、配额和失败反馈。

UI 包住 Runtime，不重新发明 Run、Trace、权限或恢复语义。

### 8.6 长任务与后台能力

只有真实产品场景证明需要，才逐项增加：

```text
JobHandle
Progress
Cancel
Query
Notification
Wakeup
Schedule
```

它们不是终局理念的前置条件，也不应在没有真实消费者时提前建设。

## 9. 第一个成品的判据

Celira 可以称为“第一个可依赖的 Agent Runtime 产品”，不需要实现所有未来能力，但必须满足：

1. Agent、Run、Turn、Trace、View 的含义稳定，不再频繁改名换义；
2. 至少一个真实 Provider 能完成正文、原生行动、结果回传、继续和结束；
3. 至少两个真实 App 能通过统一边界被 Agent 使用；
4. 一个真实端到端场景能跨 Run 或跨进程保持诚实连续；
5. Action 的成功、失败和未知对用户可解释；
6. 用户能看懂 Agent 当前状态，并知道何时需要显式动作；
7. 关键失败路径不会自动伪装为成功；
8. 文档、实现、测试和未完成项的状态互相一致。

这定义的是第一个真实可用成品，不是宣称所有平台能力已完成。

## 10. 明确不作为终局前置条件的能力

以下能力不是当前终局成立的前提：

- 全自动规划器；
- 自动摘要全部历史；
- 无限上下文；
- Provider 私有 Session 依赖；
- 通用插件市场；
- 通用事件总线；
- 多 Agent 并行调度；
- 通用 Action Replay；
- 自动补偿和回滚平台；
- 对所有工具统一建模 Progress；
- 先建设万能权限平台；
- 为想象中的未来提前增加持久种类。

## 11. 导航规则

未来每个较大阶段只需要回答：

```text
它是否让 Agent 更能使用真实环境？
它是否让事实更可靠？
它是否让上下文更诚实？
它是否让用户更能理解当前状态？
它是否引入了不必要的新权威、新循环或新平台？
```

如果一个提案不能改善以上至少一项，或者只能通过增加一套平行状态、自动判断和隐藏流程来实现，应暂缓。

本文不授权任何具体施工。具体代码任务必须另行说明范围、消费者、禁止修改区、验收证据与未覆盖项。
