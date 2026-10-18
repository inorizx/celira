# Runtime 地基与扩展接口设计（修订二稿）

> 2026-09-12 · 二号负责人交付。原理底稿 [RUNTIME_CORE_AND_LAYERING.md](RUNTIME_CORE_AND_LAYERING.md)。
>
> **修订记录**：一稿经首席裁决——保留已有可靠性资产的方向通过；设计暂不进入实施；本稿集中修订：①以共同接入契约为终态、允许阶段性适配；②补齐扩展呈现面及失败、预算、重复调用规则；③区分开场快照与 Run 内更新；④Handoff 从组合设想补成明确交付生命周期；⑤现状证据、设计推演与未覆盖项分层分开。
>
> 本文是设计，不是实现完成声明。以场景推演和代码证据验收；实施授权后按项目门禁独立验证。与宪法、冻结契约、terminal 终局正文冲突时以后者为准。
>
> **注记（2026-09-24；2026-10-03 补注）**：Part A 的「现状事实」按 2026-09-12 旧线快照撰写（`agent/runtime/` 已于 2026-09-17 退役）；Part B/C 中一切 Completion/ACK 环节随 2026-09-24 退役，Part B/C 引用的旧线组织概念（Coordinator / DialogueLane 等）亦随 2026-09-17 旧线退役、不再对应实现——当前实现事实见 `agent/README.md`。

## 0. 原理声明

**Runtime 地基的职责是把「一次有界清醒期」做成确定性事实链：谁醒着、看见了什么、做过什么、结果如何，各归一个权威；扩展只经共同声明契约接入这条链，不另开平行执行路径。**

核心假设：可靠性地基已存在且被证明（Effect 链、两层恢复权威、崩溃矩阵）；真正的缺口是**扩展接入缝**——现状每个 app 需要五刀同落改核心代码，与"扩展不反向定义核心本体"矛盾。本轮设计主刀口：把既有接入模式固化为共同契约（终态），现状类型化路径降格为阶段性适配。

---

# Part A 现状事实（证据基线）

本部分只陈述已存在并经测试证明的事实，不含设计。

## A1 主链与状态归属

| 事实 | 权威 | 代码证据 |
|---|---|---|
| 生产入口 | 唯一：`bin/celira_runtime.rs` → admin → LaneFleet → DialogueWorker → DialogueLane | README §2/§3 |
| Run 管理 | SubjectCoordinator（Lane 引擎），单 Subject × 单活动 Run | `assembly/coordinator.rs` |
| 原始输入内容 | `InputObserved` 同帧 `InputMessageFact`（完整不可变载荷；身份=主体+来源+外部引用，正文不参与） | `assembly/input.rs` |
| 执行/呈现 | `DialogueView` 每次 pump 从账本重建（投影不落盘，非第二裁决者） | `assembly/dialogue_view.rs` |
| 未确认结果 | Effect Ledger `Unknown` 有效，不猜终态；reconcile/abandon 显式结算 | `effect/*` |
| 恢复裁决 | 两层权威：宿主层 Journal（放行/拒开守卫）+ 主体域 Fact Ledger（确定性重建）——主人已批准切分 | README §5 |

既有边界（现状即如此）：in-flight Run 跨进程恢复不提供；Ledger/Outbox/Journal 之间无跨域事务（8 窗口崩溃矩阵逐一证明收敛：不丢、不重、不伪造）。

## A2 可靠性资产清单（保留决定的依据）

- **Effect 链**：Prepared-before-dispatch、operation_id、三态回执、Unknown 不猜、显式结算（`tests/effect_file_ledger_contract.rs` 24 项、`tests/dialogue_effect_chain_contract.rs` 19 项）；
- **两层恢复权威 + 崩溃矩阵**（`tests/crash_matrix_contract.rs` 8 窗口真实 abort）；
- **Outbox 闭环**：首写耐久、单步推进、重放不重复公共消息（29 项）；
- **装配契约**：骨架/尾部拆分 + 字节稳定契约（D 切片）；
- **六终态 + quiescent 静默收敛** + 终态→公共收尾映射（README §4.10 表）。

## A3 扩展接入现状：五刀同落

现状接入一个 app = 同落五处核心代码：

1. `ports.rs` `TurnDecision` 加 enum 变体（现约 30 个变体：Say L132 / FsWrite L181 / TerminalRun L197 / WebSearch L218 …）；
2. `assembly/coordinator.rs` 加 dispatch match 臂（L1686 say / L1774 email_send / L1853 fs_write / L1920 terminal_run / L2017 web_search）；
3. `assembly/dialogue.rs` 加配置字段 + Connector 接线（L156–176 字段、L1182–1364 装配）；
4. 工具白名单追加；
5. Grant 注册。

模式高度一致（每 app 三声明：登录/推送/动作；动作面统一 Effect 链三态 + ToolCall.id 幂等键），但每例手工重复，无声明点。F3 漏改 host wire（R7a bug）即此类多刀同步的教训。

## A4 呈现机制现状盘点

已有两个呈现先例，均为**机械投影、核心装配、app 不自有注入管道**：

- **L0 主屏**：`dialogue_view.rs` L630+——已装 app 一行机械清单（名 + 一句话），im 恒在、其余绑定后出现；
- **高亮点区**：L646+——Agent pin 的记录投影进骨架末段（≤8 条、body ≤512B pin 时校验）；
- 辅助先例：B1 通讯录预览（≤60B 机械截断带标记、计数 99+ 折叠）、最近几条（失败省段不阻塞唤醒）——**呈现段失败 = 省略，不阻塞主体链**的 fail-open 先例已立。

---

# Part B 目标设计（推演与推荐方案）

## B1 共同接入契约 `AppContract`（终态）

### B1.1 契约面

一个 app 经一个对象声明全部接入面。核心认识五面，不开放其他生命周期钩子：

```rust
/// app 接入共同契约（终态）。三声明模式的固化；核心只在既有时机调用。
trait AppContract {
    /// 声明面：工具清单（名/参数形状/门类：Retrieve 免门 | Grant 位/上界）。
    fn manifest(&self) -> AppManifest;
    /// 推送面：变化报告 → 通知栏账本（无推送 app 返回空）。
    fn poll(&mut self) -> Vec<WorldEvent>;
    /// 动作面：执行一个已过门的动作（Effect 链三态回执）。
    fn dispatch(&mut self, op: OperationId, action: ActionRequest) -> Receipt;
    /// 呈现面：声明可进视野的内容候选（核心三闸裁决，app 不自有注入管道）。
    fn presentation(&self) -> PresentationClaim;
    /// 重放声明：同 operation_id 重入行为（声明点归工具作者，系统零猜测）。
    fn replay_policy(&self) -> ReplayPolicy;
}
```

配置面（绑定/解绑/密钥）不进契约——沿用 admin HTTP 既有端点族（email/fs/terminal/web 先例：PUT 即时生效、密文落盘、绝不进模型上下文）。配置变化 → 下次 `manifest()` 调用自然反映（provider revision 在 turn 边界读取同律）。

### B1.2 接口规则表（每面九项）

| 面 | 调用时机 | 输入 | 返回与权限 | 组合顺序 | 超时 | 失败 | 重复调用 | 预算 |
|---|---|---|---|---|---|---|---|---|
| `manifest` | Run 开场装配前；配置变化后的下一次 | 无 | 纯声明；派生工具白名单 + L0 行 + 参数机械校验 | 先于一切装配 | 内存调用，即时 | 返回失败 → 该 app 工具面与 L0 行**干净缺席**（与未绑定同形，不冒充） | 幂等（纯函数性质） | 工具数 ≤16/app（现单 app 最多 4，余量充足） |
| `poll` | 宿主调度周期（门铃/轮询节奏） | 无 | `WorldEvent` 列表 → 通知栏账本；**零唤醒权**（响不响铃归唤醒政策按源配置） | 各 app 独立轮询，互不阻塞 | 单 app 轮询周期有界（email 先例 `poll_interval_secs`） | 失败 → 本轮空手而归，下轮重试；**不阻塞其他 app、不阻塞主体链** | 消费幂等由 app 侧事实保证（email UID 指纹先例：账本去重，不用含时间戳的摘要） | 每轮事件数有界（溢出折叠进计数） |
| `dispatch` | 仅在 Prepared 落账之后（Effect 链既有写序） | `OperationId` + 过门后的 `ActionRequest` | `Receipt` 三态（Success/Failure/Unknown） | 单 Run 内串行（单 Lane 唯一执行链既定） | 同步有界（web ≤20s）或异步 execution（terminal 5s 快速窗口 + execution_id 续读）——由 manifest 声明动作模式 | 失败 = 受控 Receipt，注入 Observation 教会 Agent；**绝不伪装 Success** | 同 op 重入按 `replay_policy`（见 B1.3） | 参数上界机械校验在过门前完成（超限拒绝不截断——Agent 的原文律） |
| `presentation` | Run 开场装配（快照段）与 turn 边界（恒摆状态行取数） | 装配时机标识 | `PresentationClaim`：候选内容 + 来源/边界/加入理由自述；**核心三闸裁决装不装、装哪、多大** | 在骨架/尾部固定位置之后（既有装配序不动） | 装配 deadline 内（内存调用为主；外拉数据 = B1 通讯录一次有界读取先例） | 失败 → **该段省略不阻塞唤醒**（A4 fail-open 先例；不拿空内容冒充） | 快照一 Run 一次；状态行每 turn 重摆（见 B2/B3） | 每 app 呈现段机械上界（世界投影**可折叠计数**：预览 ≤60B、计数 99+——web fetch「世界原文给预算」律；**Agent 的原文永不剪**：record/fs 律，两类分界已有先例） |
| `replay_policy` | 同 op 重入时被查阅 | 无 | 枚举声明：`NoopConfirm`（fs 同内容 no-op）/ `IdempotentKeyed`（say/email：同键不再出站，外部幂等兜底）/ `NeverRedispatch`（web：重放即协议失败收敛） | — | — | — | — | — |

**不开放的**：任意生命周期钩子、app 自有注入管道、平行执行链、跨 app 未授权状态共享。注入脸只借核心开场插槽（三闸纪律，CORE_PRINCIPLES §6）。

### B1.3 重放声明制（把散在各处的重放行为收成一个枚举）

现状重放行为已各自正确（fs no-op / web 零出站 / say 幂等键 / email UID），缺的是**声明点的形式化**。`replay_policy` 把散在各 Connector 注释里的行为收进一个枚举，系统在 Effect 链重入点机械执行声明，零猜测。新 app 接入时必须选一种——没有"默认重试"这个隐含选项（不明确声明 = 编译期失败，宁少勿滥）。

## B2 开场快照（Run 边界注入，一 Run 一次）

**内容**（尾部固定序，既有装配序新增一段）：

```text
[通知栏压缩视图] 各 app 未读账本聚合 → 每 app 一行：
    来源 + 计数（99+ 折叠）+ 最新一条预览（≤60B 机械截断带标记）
[自身处境] 当前时间 + 预算余量（dial 实施前只摆时间与事件计数，不虚报预算）
[开场插槽] 授权内容：pin 高亮点（既有）+ 最新未交付 handoff（B4）
```

**机械特性**：

- **快照是状态的全量压缩投影**——幂等、可重建、可重复装配；数据源 = 各 app 侧账本（im = 服务端消息事实 + B1 端点族；email = 未读指纹；terminal = execution 账本），**通知栏本身零新持久面**（队列本体论禁止实体化裁决：一份事实 + 多处投影，永不双写）；
- 触发消息无特权：它只是快照里最新的一条，详情进 app（im_read_history / email_read）；
- 聚合时机 = Run 开场一次性拉取（每 app 一次有界读取，B1 通讯录先例），非 turn 级实时——实时性由 B3 增量机制补，开场不重复付成本；
- 失败语义：单 app 数据源失败 → 该 app 行省略（其余行照常），省略行**计数未知如实显示**，不拿 0 冒充。

## B3 Run 内更新（turn 边界注入，每 turn）

两种机制，**性质不同，规则不同**——这是本节与 B2 分立的原因：

| | 增量卡片 | 恒摆状态行 |
|---|---|---|
| 本体 | **事件**（历史） | **状态**（当前值） |
| 触发 | 自上 turn 游标以来的新动静 | 每 turn |
| 重复规则 | **一条只播一次**（游标去重——事件是历史，重播即语义错误） | **每 turn 重摆**（状态重摆即刷新，天然幂等，成本有界：app 数有限） |
| 内容 | 事件卡片（来源 + 类型 + 预览） | 当前时间 + 预算余量 + 各 app 未读计数「qq:10 微信:20」式 |
| Agent 处理过的消息 | — | **下 turn 计数归零必须可见**（已读 = 拉过详情即移除投影项） |
| 失败 | 拉取失败 → 游标不动，**下 turn 补播**（增量语义保证不丢，代价是可能迟到一 turn） | 机械行缺省（时间恒有；计数失败显示未知） |
| 预算 | 卡片数有界，满则最旧折叠进计数行 | 单行有界（app 数为自然上界） |

**与追加历史的兼容**：卡片与状态行进 DialogueView 的"观察"区（frames 合并既有机制），不改写历史段。溢出折叠作用于通知栏账面投影，不作用于增量窗（判据 18 折旧只作用于投影，不删事实）。

**去重 ≠ 删除历史**：游标去重只作用于注入视图；账本事实在 app 侧永在，已读移除的是投影项不是事实。

## B4 Handoff 交付生命周期

一稿只给了组合叙事；本节补成明确生命周期。**零新核心对象**：Handoff = record 家族（`kind` 字段标注，A 证据：kind 非空 ≤64B 主体显式输出）+ 交付游标（注入书签，cursor 家族允许的持久事实）+ 开场插槽。

### B4.1 状态机

```text
[写下 Write] Agent 在本 Run 内 record.write（kind=handoff，正文 = 给下次开场的自己）
    │
    ▼
[保存 Persist] Effect 链三态：
    Success  → 事实落 per-scope 账本（跨桌可达）
    Failure  → 当场可见，Agent 可重写（原文不丢：拒绝不截断律）
    Unknown  → pending 可见，reconcile/abandon Agent 裁决（系统不替选）
    │（Success 后进入）
    ▼
[等待 Wait] 无固定时限。交付时机 = 下次 Run 开场，无论谁唤醒
    （门铃/maintenance/timer 均可——arm 闹钟只是"确保会醒"的保险，非交付必要条件）
    │（下次 Run 开场装配时）
    ▼
[交付 Deliver] 开场插槽段：取交付游标之后的最新一条 handoff 记录
    → 注入快照插槽（一次播放，与增量卡片同律：一条只播一次）
    → 交付成功后推进游标
    │
    ▼
[消费 Consume] Agent 看到后的三个自由去向（系统不代管）：
    pin   → 保持续命（骨架常驻，pin 既有机制）
    不管  → 沉入 record 账本（历史可达，永不删除；无主动过期——宁少勿滥）
    再写  → 新 handoff 成为最新（下次开场交付新的）
```

### B4.2 交付游标的权衡（技术细节，已定）

游标推进时机 = **交付成功之后**（游标后行）。崩溃窗口两害相权：

- 游标先行：崩溃 → 游标已过、纸条沉底 → **Agent 永远没看到（丢失）**；
- 游标后行：崩溃 → 游标未过 → 下次开场重播一次 → Agent 多看一遍（轻微噪音，非语义错误）。

**丢失不可接受，重复可容忍**——选游标后行。诚实声明：游标写失败的窄窗口内可能出现一次重播；重播的是"给未来的自己的纸条"，非公共事件重演，无害。

游标物理形态：per-agent 单文件原子写（dialogue-recovery 授权文件同模式），内容 = 最后已交付 handoff record_id。

### B4.3 失败矩阵（任务书问题 6 完整闭合）

| 环节 | 失败 | 后果 | 是否丢失/伪装/重复 |
|---|---|---|---|
| 保存 | Failure | 当场可见可重写 | 不丢（原文在 Agent 手里）、不伪装（Failure 就是 Failure） |
| 保存 | Unknown（回执丢失） | pending 可见，Agent 裁决结算 | 不猜、不自动重发 |
| 接续请求（arm） | 失败 | 当场可见，零误唤醒 | 不伪装成功 |
| 保存✓ + arm✗ | 组合 | 有纸条但没闹钟——下次任何唤醒仍交付 | **不丢失**（arm 非交付必要条件） |
| 保存✗ + arm✓ | 组合 | 闹钟响了、插槽空 | 空跑不伪造（不造空白纸条） |
| 交付 | 装配失败 | 游标不动，下次重试 | 不伪造空、不盲目跳过 |
| 交付 | 游标写失败（交付后崩溃） | 下次重播一次 | 重复容忍 > 丢失（B4.2 权衡） |

**幂等键各环节独立**：record = ToolCall.id；arm = (run_id, label)；交付 = record_id 游标。无一处"重试 = 重做"。

**移除 Handoff**：删掉 kind=handoff 的读取与插槽段，基本循环、异常恢复、record 家族零影响（它本来就是消费侧组合，无专属核心分支）——验收点 1 成立。

## B5 状态归属终态增补表

在 A1 表基础上新增三行（全部为投影或书签，无第二裁决者）：

| 事实 | 权威 | 形态 |
|---|---|---|
| 通知栏/状态栏 | 各 app 侧账本（唯一事实）+ 投影（可重建，零持久面） | 内存装配 |
| 交付游标 | per-agent cursor 文件（注入书签） | 原子写小文件 |
| handoff 记录本体 | record per-scope 账本（Effect 链事实） | ndjson 追加 |

## B6 三能力场景推演（正常 + 失败）

| 能力 | 正常路径 | 失败路径 |
|---|---|---|
| 笔记（record） | RequestAction → Grant → Prepared → dispatch → Receipt Success → per-scope 账本 | 回执丢失 → Unknown（pending 可见，reconcile/abandon Agent 裁决）；写前失败零调用 |
| Handoff | 写下→保存→等待→下次开场交付→Agent pin/不管/再写 | 见 B4.3 矩阵：各环节与组合失败均不丢、不伪装、不盲目重复 |
| 通信（im） | 事件 → 门铃/轮询 → InputObserved → Run → say/reply（写回即公共事实；旧稿尾部「Outbox → Completion+ACK」已随 2026-09-24 退役） | 401 → AuthBlocked 停驱不消费；确认丢失 → 重放幂等零重复公共消息；撤回 → EventSkipped |

三能力零专属核心分支。

---

# Part C 阶段性适配（现状 → 终态路径）

## C1 五 app 的阶段地位

终态 = **全部 app 经 `AppContract` 共同契约接入**，`TurnDecision` 不再新增 per-app 变体。现状五 app 的类型化路径 = **阶段性适配**（被允许保留，不是终态）。

回迁策略（技术细节，已定）：

- **接触即回迁**：某 app 下次因任何原因需要改其 dispatch 臂/工具变体时，顺路迁移到注册表，不专门立项；
- **终局检查点**：Agent 真实入住前统一盘点——届时仍存类型化路径的 app 列成短清单，由首席裁决一次性收编或明示豁免理由；
- 回迁期间 dispatch 路径双轨并存，但**新增能力只走注册表**——双轨是收敛的过渡态，不是稳定态（一稿"长期双轨"表述作废，按首席终态指令修正）。

## C2 实施范围建议（仅划界，不进实施）

设计暂不进入实施（首席指令）。范围建议供后续裁决参考：

1. **第一刀：`AppContract` 注册表骨架**——manifest 派生白名单/L0/校验三刀收敛，纯内部、零行为变化，第一个真实消费者 = 下一个新 app（触发器名单里的 QQ/微信 bridge 候选）；
2. **第二刀：通知栏快照 + 增量卡片 + 恒摆状态行**——随 Peripheral 切片（B1 数据源已就位），含交付游标与 handoff 插槽段；
3. **第三刀：`replay_policy` 形式化**——可并入第一刀（同文件族）。

三刀互不依赖，可独立裁决独立验收。

---

# Part D 不做 / 未覆盖 / 裁决记录

## D1 明确不做（减法清单，对齐"拒绝要写成文档"）

- 五 app 专门回迁立项（接触即回迁，不值得扰动全绿测试）；
- 统一 Ledger 收束 / 跨域原子事务（既有裁决不做，两层恢复权威已批准）；
- in-flight Run 跨进程恢复（既有边界）；
- 任意生命周期钩子 / app 自有注入管道 / 插件市场 / 万能框架；
- RAG（宪法已否）、自动摘要/排序/分诊（防镀金，等真实痛点）；
- handoff 主动过期/消费标记（沉底即可，pin 是 Agent 的续命选择，系统不代管）。

## D2 未覆盖路径（明示）

- dial 三件套（max_turns/max_tokens/deadline）与预算余量的真实计量：待 dial 切片；状态行先摆时间 + 事件计数；
- Peripheral 顶栏具体项与溢出折叠阈值：待 Peripheral 施工单；
- 多 app 并存的注入总量字节配比（骨架/快照/卡片）：待真实长任务撞 ObservationFull 后裁（B2 先例）；
- 呈现段的 401/凭据失效细节（呈现读取与动作 dispatch 的凭据边界是否同律）：待第一刀实施时定；
- 会话 Scope 语义化、多 Provider 热切换：既有待办，本轮不动。

## D3 本轮裁决记录（在案）

- 保留已有可靠性资产方向：首席通过（2026-09-12）；
- 共同接入契约为终态 + 阶段性适配：首席指令（2026-09-12），取代一稿"长期双轨"推荐；
- 通知栏聚合时机 = 开场一次性拉取：随"区分开场快照与 Run 内更新"指令落地为 B2/B3 两机制；
- 设计暂不进入实施：首席指令；实施范围建议见 C2，待裁决。

---

## 验收对照（任务书 §7 五问）

1. **移除 Handoff，循环与恢复仍成立**：✅ B4——它是 record+游标+插槽的消费侧组合，无核心分支；
2. **换呈现内容来源不改执行语义**：✅ B2/B3——投影与事实分离；来源换 = 账本换，装配通道与执行链不动；来源/权限/容量仍受 B1.2 预算列约束；
3. **多扩展/超量通知/失败钩子不绕过隔离、预算、唯一执行秩序**：✅ B1.2——无平行执行链、单 Lane 串行、每面预算与失败规则齐备、Unknown 不猜；
4. **崩溃恢复不把未知当失败、不把已读当完成**：✅ Unknown 有效 + 已读只移投影项 + NoReply 可带未读收工 + B4.3 矩阵各窗口闭合；
5. **换模型适配器不迫使上层理解供应商格式**：✅ 现状已成立（AgentMessage 中立模型 + provider/wire 单一转换边界），本轮不动。

---

# Part E Core 最终地基（方向稿）

> 本部分定义 Runtime Core 的最终边界与最小执行模型。它不是当前代码完成声明，也不授权立即迁移既有 App 路径。后续实现必须先对齐本部分，再从底层向上重建；如果现状代码与本部分冲突，以本部分描述的最终职责为目标，不能反过来让旧代码结构定义 Core。

## E1 原理声明

**Core 只负责一次 Run 内的清醒循环、工作轨迹、两个注入接缝、模型调用、行动承接、收场与恢复；它不拥有任何具体 App 的语义，外部能力只能通过中立接缝进入。**

## E2 Core 的最小闭环

Core 在没有 IM、Clock、Email、Notes、Handoff 或任何其他具体 App 时，仍必须能够完成以下闭环：

```text
Core Input
  → accept
  → begin Run
  → establish Session Trace
  → establish Run-level Slot
  → repeat Turn
      → assemble Turn-level Slot
      → call Model
      → classify Model Output
      → accept neutral Action or Terminal Decision
      → execute Effect or record Observation
      → append result to Session Trace
      → Agent chooses continue or terminal outcome
  → persist Outcome
  → close Run
  → rebuild from durable facts when needed
```

这里的“没有 App”不是要求生产宿主永远不装 App，而是 Core 的结构测试：任何 App 都不能成为建立 Run、调用模型、结束 Run 或恢复状态的必要条件。

## E3 Core 的最小对象

### E3.1 Core Input

`Core Input` 是被 Core 正式接纳、并成为一次 Run 起点的中立输入。它只描述：

- 输入身份或来源引用；
- 输入到达的时间；
- 必要的去重身份；
- 可选的原始载荷引用；
- 本次 Run 的关联边界。

它不描述某个 App 的业务对象，不携带“应该打开哪个 App”或“应该执行什么动作”的判断。

```text
外部来源 / 中层调度
  → 中立 Core Input
  → Core acceptance
  → durable InputObserved
  → Run
```

`InputObserved` 是 Core 对输入接纳的事实见证。外部事件、通知、唤醒请求和 Core Input 不得混为同一对象。

### E3.2 Run

`Run` 是一次有限清醒期，也是 Core 唯一的执行循环边界。它负责：

- 建立一次可写清醒期；
- 承接一个合法起点；
- 约束本次 Session Trace；
- 驱动有限的 Turn；
- 接受 Agent 的继续或终态选择；
- 形成唯一 Outcome；
- 结束后释放执行权。

Core 不为不同来源建立不同 Run 类型：

```text
IM Run / Email Run / Clock Run 不是 Core 概念。
它们都只是不同来源进入同一个 Run 的输入形状。
```

### E3.3 Session Trace

`Session` 在 Core 中只表示**当前 Run 内追加式的工作轨迹**。它可以包含：

- 模型消息；
- 当前输入；
- 模型动作请求；
- 工具或外部动作结果；
- Observation；
- Receipt / Unknown；
- 失败、取消与继续信息；
- 当前 Run 的终态前轨迹。

它不是：

- Agent 的长期人格；
- 跨 Run 无限增长的消息容器；
- 系统自动摘要后的记忆；
- Handoff、Notes 或其他 App 的业务对象；
- Core 替主体选择的“应该保留内容”。

Session Trace 允许在 Run 内追加，但必须受模型上下文和 Run 预算约束。接近上限时：

```text
真实余量可见
  → Agent 自己决定继续或收场
  → Agent 可主动调用外部记录能力
  → Run 结束
```

Core 不自动生成 Handoff，不自动摘要，也不把上下文撞墙伪装成正常完成。

### E3.4 Run-level Slot

Run-level Slot 是一次 Run 建立时形成的初始处境接缝。它只在 Run 开场阶段装配一次，或者由 Core 按明确的 Run 边界规则保持稳定。

它可以承载：

- 本次 Run 的起点事实；
- 当前主体的基础处境；
- 当前预算与窗口等机械状态；
- 外部世界提供的可见入口；
- 允许在本次 Run 开场出现的事实或引用。

它不可以承载：

- 某个 App 的专属核心分支；
- 系统替主体总结的上次经历；
- 系统替主体判断的优先级；
- 自动打开 App 的命令；
- “你应该先做什么”的建议。

### E3.5 Turn-level Slot

Turn-level Slot 是 Run 内每一轮的当前处境接缝。每轮模型调用前，Core 可以重新装配它。

它可以承载：

- 新事实；
- 新观察；
- 工具结果；
- Effect Receipt；
- Unknown；
- 状态变化；
- 本轮新增且尚未进入轨迹的事件事实。

它不可以承载：

- 自动生成的任务判断；
- 自动摘要；
- 重要性排序；
- 人格结论；
- 未经 Core 接纳的 User 消息伪装；
- 自动替主体打开的 App 内容。

Run-level Slot 与 Turn-level Slot 的区别不是“一个长期、一个短期”，而是：

```text
Run-level Slot  = 这次清醒开始时的处境
Turn-level Slot = 这次清醒进行中的变化
```

两个插槽当前可以继续由装配函数和缓存策略实现；是否类型化，必须等真实的多个接入者证明有必要后再决定。

### E3.6 Model Boundary

Core 只认识中立的 `TurnModel` 输入与输出边界。Provider、供应商 wire 格式、具体 App 工具 schema 不属于 Core 本体。

模型输出必须先经过结构化分类，才能成为：

```text
neutral Action Request
Observation request
Terminal Decision
Invalid output
```

模型输出本身不等于：

- Agent 对外发言；
- 已执行动作；
- 公共消息；
- 已送达；
- 现实事实。

### E3.7 Action / Effect Boundary

Core 负责承接中立 Action，不负责理解具体 App 的业务意义。所有具有外部副作用的动作必须经过：

```text
Action Request
  → authorization
  → operation identity
  → Prepared fact
  → dispatch
  → Receipt / Unknown
  → durable Effect fact
```

Core 必须保证：

- 跨边界写效应不能绕过授权；
- 同一操作可以判定重复；
- 未知结果不能猜成成功或失败；
- Effect 结果可以回到下一轮 Session Trace；
- 失败和取消不能伪装成正常完成。

具体由哪个 App 执行动作，属于外部接入层；Core 只保证动作边界和结果事实。

### E3.8 Outcome

`Outcome` 是一次 Run 的唯一收场结果。它只表达本次清醒期如何结束，例如：

```text
finish
no_reply
wait
continue
refuse
failed / cancelled（系统或执行失败，不冒充主体选择）
```

Core 不把某个 App 的业务动作名称写进 Outcome 语义。某个动作是否已经产生公共或外部足迹，应由 Effect / Receipt 等中立事实表达，而不是由 Outcome 猜测。

### E3.9 Recovery

Recovery 不是重新生成，也不是自动替主体做决定。它只根据耐久事实恢复 Core 的可验证状态：

- 当前是否存在活动 Run；
- 哪些 Input 已接纳；
- 哪些 Effect 已准备、已执行、未知或已结算；
- 哪个 Outcome 已经落账；
- 哪些操作可以继续、重concile 或停止。

Recovery 必须保持：

```text
不丢事实
不伪造成功
不把 Unknown 猜成 Failure
不因恢复重复公共副作用
```

## E4 Core 的依赖方向

最终依赖方向只有一条：

```text
具体 App / Connector / Provider Adapter
  → Core neutral ports and contracts
  → Core Run / Turn / Effect / Outcome / Recovery
```

更准确地说，Core 提供中立协议，外部接入层实现协议：

```text
Core domain
  ← Core ports / neutral contracts
  ← Runtime OS / Connector / App adapter
```

Core 不得直接依赖：

```text
具体 App 名称
具体 App 数据结构
具体 App 工具参数
具体 App 业务错误
具体 App 的打开和导航逻辑
具体 App 的通知逻辑
具体 App 的轮询实现
具体 App 的展示文本
```

当前代码中仍存在的 typed App 端口、typed TurnDecision 和 Dialogue 外层调度，属于迁移过程中的阶段适配；它们不能继续被当作最终 Core 形态，也不能在没有迁移设计的情况下直接删除。

## E5 Core 与 Runtime OS 中层的边界

手机 OS 中层不是 Core，也不是具体 App。它负责把外部世界变化组织成 Core 可接纳的输入和插槽内容：

```text
App / 外部世界
  → Event
  → 中层接纳与策略
  → 可选 Notification / Wakeup
  → Core Input
  → Run
```

中层可以拥有：

- 输入接纳与调度；
- 唤醒政策；
- 通知和状态投影；
- App Open / Navigation；
- 外部连接生命周期；
- 来源级去重和读取策略。

Core 不拥有：

- Notification；
- Notification Ledger；
- App 页面；
- 通知已读与保留策略；
- 某个来源是否应该响铃的业务政策；
- 自动打开哪个 App。

通知不等于唤醒，唤醒不等于打开 App，打开 App 不等于采取行动。

## E6 Core 的自检问题

任何新增核心机制都必须回答：

1. 没有任何具体 App，它是否仍然必要？
2. 它表达的是主体如何运行，还是某个世界如何工作？
3. 它能否用中立事实、动作、回执和接缝表达？
4. 它是否替主体做了理解、排序、摘要、取舍或决定？
5. 删除它后，Run、Session Trace、Turn、Effect、Outcome、Recovery 是否仍能成立？

只有第一问答案为“必须”，并且后四问不越界，才有资格进入 Core。

## E7 从当前实现到最终 Core 的重建顺序

后续不是继续在旧结构上叠加适配，而是按依赖方向自底向上重建：

```text
1. Core domain / neutral facts
2. Input acceptance 与 Run 边界
3. Session Trace 与两个插槽
4. TurnModel 与中立输出分类
5. Action / Effect / Receipt / Unknown
6. Outcome 与 Recovery
7. Runtime OS 中层接入
8. App / Connector 逐个接入
9. 删除旧的 typed 路径与重复装配
```

当前明确不提前做：

- 不为 App 新增核心特权；
- 不先实现 Notification；
- 不先做 Handoff；
- 不先做 Session / Run 全局改名；
- 不先把两个插槽类型化；
- 不先抽不存在真实消费者的事件总线；
- 不因为旧代码能运行，就把旧的 Core/App 混合结构视为最终地基。

## E8 本部分的验收边界

本部分完成的不是代码迁移，而是最终职责边界。后续每个实现任务必须明确属于以下之一：

```text
Core 本体
Core 中立接缝
Runtime OS 中层
App / Connector
过渡兼容
```

任何任务如果无法归类，先暂停，不进入实现。

本部分不改变当前公共协议，不修改 App 行为，也不把目标设计写成当前已实现事实。
