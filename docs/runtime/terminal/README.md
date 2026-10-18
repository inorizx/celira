# Celira Runtime 终局图纸

> 状态：终局修订中。理念方向已通过；执行契约以 `RUNTIME_TERMINAL_DECISIONS.md` 为负责人裁决依据。本文档不描述当前实现，不继承当前代码中的术语作为本体约束。
>
> 本组文档回答一个问题：如何让无状态模型在一个持续存在、彼此隔离、可扩展的外部世界中反复成为同一个 Agent 主体。
>
> **术语（2026-09-25 立）**：本组的「主体」= `Subject`（「概念模型」那一层的本体实体，**可数**——「多个主体」「每个主体」在本组里是对的）。它与项目其余文档里的**能力词**「主体」（回答"这是谁的判断/经历/责任"，**不可数、不能有 id、不能当成员**）**不是同一个用法**，两边互不援引。写别处的文档时按 `AGENTS.md`「统一语言」那两条走；在本组内部，`主体` 与 `Subject` 同义，不必统一写法。

## 文档边界

本组文档只讨论 Runtime 自身的终局形态，不讨论任何外部通信产品、公共消息系统或现有实现名称。

### 理念与方法的层次

“终局”指长期方向，不表示所有概念与方法已经定型。本组内容分为四层：

| 层次 | 内容 | 约束方式 |
|---|---|---|
| 根本立场 | 主体性、连续性、事实与解释分离、隔离、系统不替主体作意义判断 | 后续设计必须遵守；修订须由主人确认 |
| 概念模型 | Subject、World、Scope、View、Presence Episode | 表达上述立场的当前模型；可在保留判据的前提下修订，不因名称成为永久本体 |
| 当前策略 | 单一可写 Episode、Trigger 入队不抢占、机械入口与固定配额 | 当前设计约束；不等于主体性的定义，改变须明确修订相应契约，不得由实现自行绕过 |
| 工程契约 | operation_id、execution-before-effect、Ledger 确定性重建 | 规定如何兑现可靠性承诺；不能反向把某一种实现方法定义为主体本身 |

区分层次不降低现有约束，也不宣告当前实现已满足终局。各层仍遵守项目权威顺序；本次理念澄清不解冻公共契约，不新增能力或改变执行语义。

## 阅读顺序

1. `RUNTIME_TERMINAL_FORM.md`：总纲与终局目标。
2. `RUNTIME_TERMINAL_ONTOLOGY.md`：主体、世界、现实范围、清醒过程、视野与事实的正式定义。
3. `RUNTIME_TERMINAL_EXECUTION.md`：从 `S` 到 `f(x)` 再到 `S'` 的执行语言，以及持久化、上下文和恢复。
4. `RUNTIME_TERMINAL_EXTENSIBILITY.md`：最小核心、原子能力与扩展接缝。
5. `RUNTIME_TERMINAL_ENGINEERING.md`：终局理念到 Rust 工程骨架的桥梁，定义领域边界、Ports/Adapters、持久化拓扑与第一实现切片。
6. `RUNTIME_TERMINAL_DECISIONS.md`：多路评审后的负责人裁决，负责指导本轮修订。

历史验证与实施材料已退出工作树，归 Git 历史（考古：`git log -- docs/runtime/history/slices/`）。其中 Slice 05（Effect/Recovery）与 Slice 12（Started/Interrupted）的历史验证证据同此——它们只是当前终局约束的历史证据，不是终局本体或新实现授权。

上述 Slice 材料只表示历史验证或实施证据，不是终局本体的并列定义，也不自动授权继续旧路线。若 Slice 与本组终局裁决、上层契约或当前实现事实冲突，必须报告冲突并以更高权威为准；清理 Slice 前应先承接其中仍有效的唯一约束和未完成状态。

## 术语纪律

生活隐喻可以帮助人理解，但正式设计使用本组文档定义的词。`Bed`、`Desk`、`House` 等词只作为解释性比喻，不作为不可解释的底层实体。

### 跨文档重名：四处「同一个写法、两个所指」（2026-10-06 登记）

**判据**：按 `AGENTS.md`「Runtime 文档纪律」——「**不许/必须**」（规范句）以终局与宪法为准，「**是/有**」（描述句）以实现为准；而**冻结契约的权威高于本组**（权威顺序 #5–#7 ＞ #11）。

| 写法 | 本组指 | 别处指 | 怎么办 |
|---|---|---|---|
| `terminal` | **本目录**（终局图纸） | `terminal_app` —— **主体的手**（一个出厂内置 App，`--terminal` 档位门控） | 说"终局"时写全 `docs/runtime/terminal/`；说那只手时写 `terminal_app` |
| `Session` | `Scope` 的**旧名**（已废） | ① Runtime 原则里的"Run 内的追加式工作轨迹"；② 宪法里的"已被 Cognitive Conversation 取代的旧模型" | **本组不再单独用这个词**；要用就写清是哪一支 |
| 「地基」 | **地基持久层**（四层表的最底层：records/pins ＋ 游标 · Effect 账本 · cursor · 密钥串） | `Foundation` —— **Core 之下的一层**（`docs/agent/00_AGENT_CONSTITUTION.md`） | 说本组那一层时写**全称「地基持久层」**；说那一层时写 `Foundation` |
| 「OS」 | 本组不把 `os` 当系统（`AGENT_VOCABULARY.md` 专门警告过：**它不含循环**） | **OS 骨干** —— 有明确职责的一层（身份/权限接入、能力入口路由、资源与生命周期） | 说那一层时写**全称「OS 骨干」** |

⚠ **本组不因这些重名改自己的本体**——但**遇到同一个写法时，先问一句"这是哪一支"**。

### 层清单差异（两条，2026-10-18 登记）

本组与 `docs/agent/` 的**层清单**有两处不一致。先前判「**粒度差异，只登记不必改**」——
**但一直没有登记在任何地方**。本处补上登记，**不改本体**：

| # | 差异 | 两边各是什么 |
|---|---|---|
| 1 | **本组分层表没有 Adapter / Composition 两层** | 本组：`RUNTIME_CORE_AND_LAYERING.md` §五那张表；`docs/agent/`：`05_AGENT_ENGINEERING.md` 的七模块含 **Adapters** 与 **Composition**（对应物在本组只见于 `RUNTIME_TERMINAL_ENGINEERING.md` 的 ports/adapters 与 host_admin） |
| 2 | **本组工程骨架里没有 OS 层** | 本组：`RUNTIME_TERMINAL_ENGINEERING.md` 的 domain / application / ports / adapters；`docs/agent/`：`05_AGENT_ENGINEERING.md` 的 **OS 骨干**（身份/权限接入、能力入口路由、资源与生命周期）。⚠ 上面那张表已登记「**OS**」这个**写法**的重名，**但没写「本组骨架里没有这一层」**——本条补的正是缺的那半句 |

**判据**：**粒度差异不是本体之争**——两张图切的层数不同，不表示哪一张错。
⇒ 遇到「这一层归谁」的问题时，**先问是哪张图在说话**；要动层清单，先按 `AGENTS.md` 的权威顺序判一次。

### `Run` 与 `Presence Episode`：**一个名字 ＋ 一个解释**（2026-10-06 按权威顺序更正）

`RUNTIME_TERMINAL_DECISIONS.md` 的词表里原来写着「`Run` **不再作为**新的本体词；若保留，**只定义**为 `Presence Episode` 的实现别名」。**按权威顺序这条要反过来**：

- **冻结契约（#5–#7，高于本组 #11）与实现用的都是 `Run`** —— `CORE_V1_IM_CONTRACT.md` 明写「Backend、Runtime、Session、**Run**、Provider 不是 actor」，`CORE_V1_AGENT_SPEAK.md` / `CORE_V1_AGENT_DIRECTORY.md` 通篇是 `Run` / `RunBoundary` / 「无进行中 Run」；而 **`Episode` 在冻结契约里只有三处**（`CORE_V1_AGENT_DIRECTORY.md` 的"上次 Episode"）。
- ⇒ **`Run` 是本体词；`Presence Episode` 是它的解释**，不是第二个名字——这正是 `AGENTS.md`「统一语言」那条：**名字只有一个，解释可以贴着名字给一句人话，但解释不得变成第二个名字**。

## 终局判据

任何新增设计都必须回答：

1. 它补的是 Agent 做不到的能力，还是替主体做判断？
2. 它保存的是事实、可验证投影、Agent 主动留下的内容，还是暂态缓存？
3. 它是否会让不同现实、来源、主体或授权互相污染？
4. 如果删除它，Agent 是失去能力，还是失去系统替主体做的决定？
5. 它是否可以由已有原子能力涌现，而不需要成为新的核心对象？
6. 能否写出初始状态、故障点、不变量和失败判据？写不出时，它只能是原则或假设，不能冒充执行契约。
