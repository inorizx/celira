# Celira Runtime 终局负责人裁决

> 状态：终局修订前的负责人裁决稿。
>
> 用途：把多路评审收敛为少数必须定案的工程契约，作为后续修改四份终局文档和设计最小验证切片的依据。
>
> 本文不是第五份评审报告，也不逐条转录评审意见；它负责做取舍。

## 1. 总裁决

Celira Runtime 的方向成立，但当前四份终局文档还不能直接作为重构蓝图。

通过的是理念层：模型不是主体，Context 不是记忆本体，Runtime 提供连续性与可靠条件但不替主体判断，多主体共享基础设施不等于共享意识。

未通过的是契约层：主体与世界的切分、耐久事实与运行投影的关系、不可逆副作用的恢复链、Unknown 的结算、上次清醒结局的可见性，以及 Context 装配的可验收边界仍未写死。

因此采取：

```text
宪法方向：通过
终局文档：进入一次定向修订
全面重构：暂缓
第一验证目标：单主体 Effect / Recovery 垂直切片
```

不接受“0 P0”作为当前结论，也不接受“8 个 P0”直接作为重构清单。严重级别只服务于行动优先级，不替代具体失败路径。

## 2. 负责人保留的核心不变量

以下不变量不得因评审压力被削弱：

1. 模型是可替换的认知提供者，不是主体本身。
2. Context 是有限、可重建的视野，不是全部历史，也不是事实唯一来源。
3. Runtime 可以补耐久、隔离、目录、权限、预算、取消、幂等、审计和恢复；不能替主体决定意义、重要性、关系或人生叙事。
4. Subject 的连续性来自稳定身份、耐久事实、主体主动记录和可重建边界；不来自单次模型调用或 KV cache。
5. 外部世界事实、Runtime 观察、主体表达、主体解释、主体决定和执行回执必须能在审计上区分。
6. Unknown 不得被 Runtime 猜成成功或失败。
7. 复杂能力应由主体组合；没有真实消费者和验收路径时，不预先增加高阶器官。

## 3. 状态底座：Subject、World、S、Ledger

### 3.1 最终语义

```text
World
  = Connector 暴露的外部现实与对象域

Subject
  = 稳定身份锚点
  + 私有命名空间
  + 对 World 的私有引用与授权
  + Subject-owned Records
  + Presence Episodes

Fact Ledger
  = 对主体可归属的运行事件的追加式耐久记录

S
  = 从 Ledger 与耐久绑定关系重建出的运行投影

Scope
  = 主体可再次进入的持久工作现场

boundary
  = 当前 Episode 解析出的瞬时观察/行动边界

View
  = 在 boundary 内提供给主体的有限语义投影

Context / x
  = View 的模型输入编码
```

### 3.2 必须修正的表述

删除或改写 `private world`。Subject 不拥有一份与其他主体平行复制的现实；Subject 拥有的是对共同 World 的私有引用、观察资格、行动资格和解释空间。

⚠ **2026-10-06 说清它防的是什么（不是那个词，是那个含义）**：本句防的是「**平行复制的现实**」这个**含义**。项目其余文档用的是中文「**私有世界**」，而它的定义**不是**平行复制的现实——`PRODUCT_VISION.md`「双平面」那一节写的是「Agent 的**身体条件、睡眠与清醒、私有事实、观察、行动与连续性**」，那是**她自己的私有事实**，与「对共同 World 的私有引用」**并不冲突**。⇒ 遇到这个词先问是哪一支：说"**平行现实**"的那一支**废**；说"**她的私有事实**"的那一支**留**（权威顺序 #2 高于本组）。

建议使用：

```text
private references and boundaries
```

逻辑上，Fact Ledger 以 Subject 为隔离单位；物理上可以共享存储引擎。后续实现不必现在决定“一主体一数据库”还是“单库多命名空间”，但必须保证所有读取、写入、恢复和审计都以 Subject namespace 为强制边界。

### 3.3 S 的裁决

`S` 不是一个神秘的大状态对象，也不是第二个事实来源。它是命名状态的运行期投影，至少包含：

```text
presence state
active scope binding
pending operations
last terminal outcome
capability / grant bindings
ledger replay position or equivalent projection version
```

恢复时唯一耐久输入可以是 Ledger，但恢复函数必须是确定性的：

```text
rebuild_state(ledger) -> S
```

同一份 Ledger 重放两次，必须得到相同的命名状态。运行期读取状态机投影；投影损坏或缺失时从 Ledger 重建，不允许另造一套无法解释的裁决源。

## 4. 不可逆副作用：必须先建立可靠闭环

所有可能造成不可逆外部变化的 Action，必须遵守：

```text
structured action intent
  -> stable operation_id
  -> durable execution fact
  -> external effect
  -> receipt | unknown
```

### 4.1 operation_id

`operation_id` 是 Runtime 接受一次结构化行动意图时创建并耐久化的稳定身份。

它不能只依赖模型重新生成，也不能只存在于内存。参数指纹可以用于识别疑似重复意图，但不能替代一次明确行动的身份，因为两个相同参数的行动也可能是主体明确要求的两次不同操作。

### 4.2 execution-before-effect

外部 Effect 调用前，必须先持久化 execution fact，至少包含：

```text
operation_id
subject
scope
capability
normalized input reference or fingerprint
target
created_at
status = dispatched | pending
```

若 execution fact 没有成功耐久化，则不得发起外部 Effect。

### 4.3 外部 Connector 契约

Connector 必须声明其副作用能力：

```text
idempotent / deduplicated
reconcilable
retryable
non-retriable
```

Runtime 不能单方面承诺“恢复而不重复”。对于不支持幂等、也不支持查询结算的外部能力，必须显式标记为不可安全重试，恢复后保持不确定，不得假装已经解决。

## 5. Unknown：不是终点，而是状态机

Unknown 的最小生命周期是：

```text
pending
  -> reconciled_success
  -> reconciled_failure
  -> abandoned
```

结算只能来自：

- 外部 Connector 的明确查询结果；
- 主体明确作出的放弃决定；
- 预先定义且可审计的过期策略。

Runtime 不得依据概率、超时直觉或重复调用结果猜测成功/失败。

当前 Scope 的未结算行动必须存在一个机械可达入口，至少呈现：

```text
operation_id / capability / target / created_at / status
```

详细证据由主体显式 Retrieve。相同 operation 不得再次造成 Effect；对同一 target 的同类行动是否阻塞，作为能力契约逐项定义，不由全局隐式猜测。

## 6. Episode 终态与恢复可见性

每次 Presence Episode 必须有可区分的终态或物理中断状态：

```text
主体收场   一档，不带值：结束了就是结束了
系统收场   模型失败 / 行动失败 / 取消 / 超时 / 崩溃 / 结果不确定（六档）
```

⚠ **2026-10-10 收口（主人原话「行吧，可以」）**：本表原列七档（`completed` / `refused` / `deferred` / `cancelled` / `interrupted` / `failed` / `unknown_action_pending`），而 `AGENT_FOUNDATION_CONTRACT.md` §3.11 列五档、`docs/runtime/AGENT_VOCABULARY.md` §9 写「六终态（含沉默）」、代码是主体四档 ＋ 系统六档——**四份形状各不相同**。现收口为**两栏**：

- **主体那一栏不带值**：「为什么结束」归**她自己的记录**（笔记：能列、能读、能钉，开场就摆出来），不归终态——终态**写一次就封，下一次没人读它**。今天那四个值里，`silent` 与 `waiting` 说的是**处境**（"这一条我不回"／"我在等外面"），按裁决应**上屏幕**（每次开场重摆），不进终态；`declined` 今天**零使用**（门铃不派活，没有可拒绝的东西）⇒ 无消费者，先不要。
- **系统那一栏保留原因**：它们的"下一步该动哪一手"不同（换钥匙／等一会儿／重试／去看一眼）——**这是机器的词表，不是她的**。

**「歇着」（`Hold`）与收场并排，是主体那一栏的第二档**（2026-10-10 第二次收口，见下）。
原表里的 `deferred` 因此退役。

⚠ **2026-10-10 第二次收口（主人原话「你看吧，合理就行，符合第一性原理，以及模型本身的设计那种」）**：
上面那条把 `Hold` 说成「**在席状态**，不是终态」——**那一半被推翻了**，理由是"先读原文"读出来的三件事：

1. **「宿主不再催她」与「接着这一段干」分不开**：只做前者（下一声铃照旧开新的一段），被搁下的那一段
   就**永远没有收尾事实** ⇒ 下一段的开场纸条会如实报「有一段没有正常收场」，而那是**假话**；
2. **"续同一段"要拿三样东西换**：① 会话**借**环境（`RunSession<'a>` 借 `&mut Environment`）⇒ 会话活着
   = **排他写资格**活着，而家的只读状态口拿**同一把锁**（操作者那一侧整个"歇着"期间被挡）；
   ② 要 Core 开一个"接上既有的 Run"的口；③ 通知栏与处境链都是"开场快照、Run 内不刷新" ⇒ 续段时
   **看不到"有什么新的"**，而那恰恰是这一档的全部意义；
3. **"续同一段"与一条已落地的承诺冲突**：装配在 Run 内钉住，而模型配置的承诺是"**下一次活动**生效"
   （`probe-zero` 真机门禁量过）⇒ 一段横跨多次唤醒 = 配置改动**永远不生效**。

⇒ **她一直在**（身份、家、账、事务、关系都在——`AGENT_PHILOSOPHY.md` §8「无人交互时，Agent 仍然存在」）；
**醒着的只是身体**——Run 是「身体醒着的一段」，不是「她存在的一段」（宪法 §3：**`Run ≠ Life`**；§5：Agent 不等于某次 Run）。
**「歇着」是这一段的一种结束方式**，它与收场**机械上同一件事**（都结束这一段、都封存），**区别只在词**——而那个词进账，
下一段的开场纸条据此说「上一段是你搁下的，不是断了」。⇒ 主体那一栏是**两档**：`ended`（收场）/ `held`（搁下），
**两个都不带值**（她是用**动词**选的，不是用参数）。

⚠ **它与旧值 `waiting` 的关系（如实登记一次自我更正）**：`waiting` 声称"我在等某个外部结果"——
那句话是**假的**（真机上被当成"暂停"用了 9 次）。2026-10-10 第一次收口因此把它删掉，**那一半删错了**：
主体**需要那个词**（那 9 次不是误用，是**她手上最接近的词**）。正解是**改名并去掉假含义**，
不是删掉 ⇒ `held` 回来，含义是「我停在这儿，有事叫我」，而且这次**它有名字**（`hold_run` 在动词面上）。

**实现**：2026-10-10 落地（`agent/README.md` §3.59 / §3.60）。

⚠ **2026-10-10 第三次收口（在场那一刀，`agent/README.md` §3.66）**：上面那条**第二次收口**
把 `Hold` 判成"与收场**机械上同一件事**"——**那一半也被推翻了**。理由不是新意见，是"先读原文"
核出来的两条：

1. 第二次收口列的三条代价里，**两条当天已经不成立**：①「歇着期间操作者的只读状态口被同一把锁
   挡住」——那个口 2026-10-06 已改成**读者打开**（不取写资格）；②「续段看不见『有什么新的』」
   ——`event_cards` 已经在做"**在场期间**的新动静"（宿主跑着时听到铃就拉一次并进细胞）。
   剩下一条（**装配在 Run 内钉住**）**字面仍然成立**：一段在场就是一次活动。
2. 它要躲的那个假话形状（"被搁下的一段永远没有收尾事实" ⇒ 下一段开场报「有一段没有正常收场」）
   **随在场一起消失**：那一段**没有结束**，所以那句纸条根本没机会说假话——它要说的那句
   "有一段没有正常收场"**是真的**（那一段确实没走到终态，因为宿主走了）。

⇒ **今天生效的样子**：`Hold` ＝ **仍在席**——主体点名 `hold_run` ⇒ `DriveOutcome::Unsettled`
（**不写终态、不释放活动槽**），宿主**等下一次 Trigger 再驱动同一段**；**收场只有一个来源**：
主体自己说结束（`finish_run`）。⚠ `subject:held` **线缆上再也产不出来**，旧账**照读不写**
（`store` 那两张封闭表一个字没动）。⚠ 同时如实登记：`core::hold_activity` 与 `RunSession::hold`
**已删除**——它们的消费者没了，存在的理由也随在场一起消失。

**预算、超时、取消、崩溃不是主体选择，但也不能被伪装成正常完成。**

下一次清醒的 orientation view 必须机械提供：

- 上次 Episode 是否正常结束；
- 若未正常结束，物理原因是什么；
- 最后一个未结算 operation 是什么；
- 是否存在需要主体决定的 pending Unknown。

不得自动摘要主体没有记录的隐藏推理。未被主体主动记录的暂态思维可以消失；消失本身不能被伪装成正常完成。

## 7. Trigger、Refuse、Wait

Trigger 只创造一次观察和行动机会，不自动构成任务，也不自动升级为主体认可的世界事实。

建议把外部输入分为两层：

```text
Trigger Envelope
  source / received_at / payload locator / delivery status

Subject Fact
  只有经过主体观察、Record 或明确接受后才产生
```

Runtime 可以执行主体显式设置的 source 级屏蔽规则；不能自行判断某个触发“值不值得”唤醒主体。Refuse 必须是可表达、可记账的终态。Wait 的语义必须明确：若它表示结束在席并等待未来事件，则等价于带未来衔接的 Finish；若仍保持在席，则必须定义预算和墙钟上限。

⚠ **2026-10-06 按实现登记（要求有效、实现欠着）**：本线走的是**第二条**（`Wait` 保持在席：`Decision::Yield` ⇒ `Waiting` ⇒ 活动槽仍属于它、还能再驱动），而**只满足了一半**——有**轮次上限**（`Limits{max_turns}`），**没有墙钟上限**。按判据「**不许/必须**看终局」，这一条**不因未实现而失效**，它是**欠着的**。

⚠ **2026-10-10 补记（在场那一刀）**：上面这句里"**实现欠着**"的那一半**已经落地**——
宿主接住 `Unsettled` 之后**不再立刻再驱动**，而是**等下一个 Trigger 再驱动同一段**
（`Host::run_segment` 里那一个循环，见 `agent/README.md` §3.66）。
**"没有墙钟上限"那一半照旧欠着**（理由见下面那句，一字未改）。

**为什么不补（写明理由，不悬着）**：轮次上限**已经是硬上界**（`max_turns` 用尽即以系统收场结束，不会把宿主挂住），而墙钟会引入一个新概念（"时间预算"）——按「没有真实消费者和验收路径时，不预先增加高阶器官」，它今天**没有消费者**。⇒ 这一格如实登记在 `agent/README.md` §4，**不假装已符合**。

⚠ **2026-10-10 收口（主人原话「行吧，可以」）**：上面那句「两读」已收口——**终局本体也改成"保持在席"**（见 `RUNTIME_TERMINAL_ONTOLOGY.md` §2 的 2026-10-10 注），`Wait` 这个名字退役，改叫 `Hold`（歇着）。

**墙钟仍然不补**，但理由变了，不许照抄上一段：`max_turns` 是硬上界，且**歇着不计轮**（预算量的是"她想了多少次"，不是"过了多久"）——裁的是"如实登记"那一档。⚠ 而**"歇着"给了墙钟第一个真实消费者**（一段可以挂到天荒地老）⇒ 这一格从"**没有**消费者"变成"**有**消费者、但选择不补"，两条理由不同，`agent/README.md` §4 要按后者写。

## 8. Context 装配：允许呈现选择，不允许意义裁决

`assemble` 不删除。有限 Context 必然需要装配和裁剪，但必须区分两类判断。

### 8.1 Runtime 可以做

- 按 Subject、Scope、授权和来源做边界过滤；
- 注入当前触发的机械事实；
- 注入上次 terminal outcome；
- 注入未结算 Unknown 的机械索引；
- 注入主体显式 pin、命名入口和目录；
- 按固定来源配额、时间顺序、最大条数或硬截断执行机械裁剪；
- 添加截断标记和可追溯来源。

### 8.2 Runtime 不可以做

- 按语义相关性排序；
- 判断哪条记忆更重要；
- 自动总结人生、关系或工作意义；
- 将多个事实合成为主体没有表达过的结论；
- 把主体的自然语言解释升级为世界事实。

每一条自动加入 Context 的内容都必须能回答：来源是什么、边界是什么、加入理由是什么；且不能反写 Ledger 成为主体的 Interpretation 或 Decision。

因此：

```text
默认呈现规则归系统，主动取用与意义判断归主体。
系统只能按公开可解释的规则机械装配，不擅自做语义策展。
```

## 9. 术语收敛

后续正文只使用以下主术语：

| 术语 | 唯一含义 |
|---|---|
| Subject | 跨 Episode 持续的身份与主体边界，不是状态容器 |
| World | 外部现实与 Connector 对象域 |
| Scope | 持久、可再次进入的工作现场 |
| boundary | 当前 Episode 的瞬时边界 |
| View | 语义层有限投影 |
| Context / x | View 的模型输入编码 |
| Fact Ledger | 追加式运行事实记录 |
| Record | Subject 主动留下的内容 |
| Trace | Run 内按发生顺序只追加的经历（一次活动的权威有序序列）——⚠ **2026-10-06 按实现更正**：原写「Ledger 中的因果链或引用维度，不是额外内容容器」，而实现里**它就是**那个容器（`agent/src/store.rs` 的 `trace.ndjson`） |
| Presence Episode | 一次从触发到终止的清醒过程 |
| Run | **一次有限清醒期**——它是**本体词**。⚠ **2026-10-06 按权威顺序更正**：本行原写「不再作为新的本体词；若保留，只定义为 Presence Episode 的实现别名」，而**冻结契约（权威顺序 #5–#7，高于本组 #11）与实现用的都是 `Run`**，`Episode` 在冻结契约里只有三处 ⇒ **`Presence Episode` 是它的解释，不是它的名字**（见 `README.md` 的「术语纪律」） |
| Observation | Runtime 向 Subject 提供的事实投影 |
| result fact | 能力或 Connector 返回的执行结果事实 |
| operation_id | 一次结构化 Action 的耐久身份 |
| Unknown | 已发生请求但结果尚未确认的事实状态 |

`Context View`、`private world`、`Working Scope` 作为正式本体词删除或降级为解释性旧称。`Memory/Self` 不作为 Runtime 预设存储层；Subject 主动 Record 的内容可以带用途 kind，但不能由 Runtime 预建意义。

⚠ `private world` 那一支的说明见 §3.2 的注：**废的是"平行复制的现实"那个含义，不是中文「私有世界」**（后者是 `PRODUCT_VISION.md`「双平面」的定义，权威顺序 #2）。

## 10. 暂不采纳的评审建议

### 10.1 不现在引入完整授权治理系统

授权是必要契约，但当前只补最小 Grant：

```text
grant_id
subject
capability
target_scope
credential_ref
issuer
expiry
revocable
```

不现在设计完整 Principal 树、组织授权、跨 Runtime 联盟和复杂委托图。Sub-agent 在没有真实消费者前不进入第一验证切片。

### 10.2 不把所有能力强行收敛为四个抽象原语

Observe / Retrieve、Record / Act、Communicate / Act 的正交性问题需要修，但不凭理论直接得到一个万能 `Effect` 或 `Control` 接口。

保留能力名称作为主体可见的动作类别；逐项声明它的边界、授权、回执、Unknown 和验收路径。没有真实消费者的能力只保留接缝，不提前实现。

### 10.3 不因可达性焦虑引入语义检索和自动排序

可达性先通过时间、来源、kind、主体命名入口、最近 N 条、pin、计数和未结算索引建立。Agent 能否主动找回事实是待验证假设，不是通过 Runtime 预装判断来解决的缺陷。

这是当前取用策略，不是“主体永远不能借助语义辅助”的理念禁令。必须区分：

- **Runtime 自动装配**：仍遵守 §8 的机械规则；不得擅自检索、排序、总结或决定重要性，再把结果冒充主体自己的选择。
- **主体主动请求的认知辅助**：理念上可以由主体选择明确的检索、整理或比较能力；辅助输出必须标明来源与方法，保留其候选、排序或解释性质，不自动升级为外部真理、主体认可的结论或 Subject Record。是否采纳与记录仍由主体决定。

显式委托不解除来源、Scope、授权、预算与审计边界，也不授权能力暗中改变后续 Context 装配。本段只澄清主体性边界，不批准新增语义检索、自动排序或具体工具；实际引入仍须经能力契约、真实消费者与项目权威门禁确认。

### 10.4 Context Generation 降级，不删除执行语义

Context Generation 不是本体对象、不是记忆层、不是第二条事实筛选入口；它只是一次普通的 View 重装配事件。首次装配与换代装配遵守同一规则。

## 11. 第一验证切片

历史验证材料归 Git 历史（RUNTIME_SLICE_05_EFFECT_RECOVERY.md）；终局契约以本文本身为准。

不做双主体、Sub-agent、复杂授权、Memory Backend 或关系网。

```text
单 Subject
单 Scope
Retrieve
Act
Finish
Fake External Connector
可注入崩溃
```

验证流程：

```text
structured intent
  -> operation_id durable
  -> execution fact durable
  -> external effect succeeds
  -> receipt intentionally lost
  -> process crash
  -> restart
  -> terminal outcome + pending Unknown visible
  -> reconciliation query
  -> success / failure / abandoned
```

强制验收：

1. 没有 execution fact 就不能发起外部 Effect；
2. Effect 已发生但 receipt 丢失时，恢复结果只能是 Unknown；
3. 重启不会生成新的 operation_id；
4. 同一 operation 不会产生第二次 Effect；
5. Unknown 能通过明确路径结算；
6. 上次 Episode 的中断状态对 Subject 可见；
7. 同一 Ledger 重放两次得到逐字节一致的 `S`；
8. `Finish`、`interrupted`、`unknown_action_pending` 不混为同一状态。

## 12. 第二阶段入口

第一切片通过后，才进入：

```text
双主体隔离
  -> 跨主体 Communicate
  -> provenance / 拒收
  -> Connector per-subject view filter
  -> Grant / Credential
  -> Sub-agent
```

每一步都必须有真实消费者、失败场景和验收不变量。

## 13. 负责人原理声明

> Runtime 不替主体活着，也不替主体理解世界。它只把主体无法可靠完成的物理条件——耐久、边界、目录、授权、幂等、审计与恢复——做成可见、可验证、可重建的环境；意义、选择和关系仍然属于主体。

## 14. C4 L1–L2 拓扑

```text
[Subject]
    |
    | structured intent / explicit Record / Retrieve choice
    v
[Presence Episode]
    |
    +--> [Boundary Resolver] -- subject + scope + grants --> [World Connector]
    |                                                        |
    |                                                        +--> observation / result fact
    |
    +--> [Context Assembler] -- mechanical filtering only --> [View / Context]
    |                                                        |
    |                                                        +--> [Model Provider]
    |
    +--> [Effect Gate]
    |       |
    |       +--> durable execution fact --same commit boundary--> [Fact Ledger]
    |       |
    |       +--> external effect / receipt / unknown -----------> [World Connector]
    |
    +--> [State Rebuilder]
            |
            +--> replay Fact Ledger --> [S projection]
            +--> presence / scope / pending operation / outcome

[Memory Backend] and [Scheduler]
    = shared infrastructure only
    != Subject state / judgment / authorization owner
```

L1 只有一条耐久裁决链：`Fact Ledger -> deterministic State Rebuilder -> S`。L2 的 Provider、Connector、Assembler、Effect Gate 都必须通过 Subject namespace 和显式 boundary 工作，不能绕过这条链建立第二个事实来源。
