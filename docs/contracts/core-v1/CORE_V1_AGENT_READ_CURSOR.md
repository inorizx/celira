# Celira Core V1 Agent Read Cursor（窄解冻切片 M1）

> 状态：**已由项目主人批准解冻**（2026-09-27 派单，依据 `outputs/CORE_ALIGNMENT_DECISION_DRAFT_2026-09-25.md` v5 §1.5（M1）与施工单 `outputs/CORE_ALIGN_KNIFE_02_AGENT_READ_CURSOR_EXECUTION_PROMPT.md`（施工单按「竣工即焚」已随刀删除，考古走 Git））。本切片属 Core v1 封板门禁下的窄解冻：**让 Agent 的 Membership 周期也有一条读游标，并给它一条显式的推进入口**。
>
> 本切片**不修改**既有 Human 侧任何端点的语义与响应形状，不修改 Message / Event / Membership / Permission / Binding / Credential / Presentation / Avatar / Service 的任何语义，**不新增任何持久对象**（无新表、无新列、无新幂等记录、无新事件 kind）。
> 本文冻结本切片的公共语义；实现细节归代码，行为证明归测试。既有 read state 语义见 `CORE_V1_IM_CONTRACT.md` §9——本切片把**同一套**交给 Agent。

## 1. 目标与范围

**"我读到哪儿了"从主体的上下文与宿主的一次 Run 里，搬进世界，成为一条跨清醒期活着的公共事实。**

今天 Agent 的位置住在两个都不跨清醒期的地方：主体的上下文（状态行给一个锚、下次自己递回来）与宿主的一次 Run（通知栏投影的 `anchor_at`）。世界侧的 `since_count` 是纯函数——你喂什么锚它算什么，它不记得你上次喂过什么。Human 的位置则住在世界里（`read_cursors` + 不可变推进事实 + `read.cursor.advanced` 事件）。

**原理声明**：既有游标模型本来就是 kind 无关的（位置 + 不可变推进见证 + 未读是算出来的），它只是被建在 Human 轴上（`human_id` 列 + 两道 `kind='human'` 守卫）。本切片是**把同一套交给 Agent**，不是造第二套——判据：Agent 与 Human 的差异只允许来自**来历**（被创建、可撤销）或**处境**（花钱、会睡、多实例），读游标两者都不是。

## 2. 端点（1 条写，全部 BackendAuth）

| 方法 | 路径 | 语义 |
|---|---|---|
| `PUT` | `/v1/backend/rooms/{room_id}/read-cursor` | Agent 显式声明"我读到这条 Message 了"。请求体 `{message_id}`（`deny_unknown_fields`，上限 1 KiB 与 Human 侧同一常量）；响应 `{read_state: {last_read_message_id, unread_count}, advanced, request_id}` |

**语义逐条同 Human 侧 `PUT /v1/rooms/{room_id}/read-cursor`**（`CORE_V1_IM_CONTRACT.md` §9）：

- 目标必须是**同 Room 且对该 Agent 的历史 Membership 窗口可见**的真实 Message（已撤回 Message 仍是合法目标）；
- 单调：目标不高于当前位置 → 成功 no-op（200 `advanced=false`），不产生任何事实；更高 → 插入不可变 `read_cursor_advancements` 事实，其触发器链在同一事务内单调推进 cursor（**不存在第二条 cursor 写路径**）；
- **无 Idempotency-Key**：单调推进天然幂等；
- 并发收敛到最高合法位置。

### 2.1 与 Human 侧唯一的差别：事件面

**Agent 的推进不产生任何事件。** Human 的推进生成 `read.cursor.advanced` Human Event（按 §10.1 只投递**推进者本人**）；Agent 的推进**不生成 fact、不生成 delivery**，因此也不做 post-commit 唤醒。

理由：`read.cursor.advanced` 的语义是"给 Human 的事件"，它的唯一收件人是推进者本人（多设备/多会话收敛用）。Agent 只有一个执行端（V1：同一 Agent 至多一个 active Binding），而它的位置由本切片 §3 的读面直接读得到——事件对它没有收件人。

**明确非目标**：本切片**不引入"已读回执"**——"对方知道我读了"这件事在今天的公共契约里**不存在**（`read.cursor.advanced` 只投本人），也不在本切片里发明。它是一件独立的产品裁决，Human 与 Agent 一起定。

## 3. 读面：通讯录每行加 `read_state`

`GET /v1/backend/rooms` 的每个房间行新增 `read_state: {last_read_message_id, unread_count}`：

- **与 Human 侧房间列表的 `read_state` 同一投影**（复用同一投影函数）：位置 = 该 Agent **当前 active 周期**的 cursor（`last_read_message_id` 可 null）；未读 = `sequence > cursor`、`author ≠ 该 Agent`、且落在该 Agent 任一真实 Membership 周期窗口内的 Message 数（撤回不回算，tombstone 按原 sequence 占位）；
- 新成员/重入周期的 boundary：`read_sequence = join_after_sequence`、`last_read_message_id = null`（**不把加入前的最后一条消息暴露给新成员**）；
- 周期关闭后 cursor 冻结（不能继续推进），重入 = 新周期 + 新 cursor（离群间隙消息不计未读）；
- 既有 `since` / `since_count` **一字不变**（两个读法并存；它们的口径统一属另一项裁决）。

## 4. 事实层（迁移 `202609270001_agent_read_cursors.sql`）

1. **身份列改名**：`read_cursors.human_id` 与 `read_cursor_advancements.human_id` → **`participant_id`**（`ALTER TABLE … RENAME COLUMN`：数据保留、既有行不动，引用这两张表的其它触发器与索引由 SQLite 自动改写）。名字是事实的一部分——Agent 的游标不该住在一个叫 `human_id` 的列里。
2. **两道守卫放开**：`read_cursors_insert_guard`（去掉 `p.kind='human'`，保留"cursor 身份 = 周期的 participant 且同房"）与 `read_cursors_membership_insert`（去掉 WHEN 的 kind 限定，Agent 周期同样得到一条 cursor）。
3. **事件面加闸**：`human_event_facts_read_cursor_advanced` 加 WHEN，只对 **Human** 的 advancement 造 fact。

**其余判据一字不动**：成员窗口、`active_marker = 1`、单调、精确见证、转让继承、无 DELETE/REPLACE/UPDATE 路径。

## 5. 错误面

| 情况 | 响应 |
|---|---|
| 缺 Authorization | `401 authentication_required` |
| 无效凭据 / Human Session token / 未知 token / revoked | `401 invalid_credential`（同一文案，不形成存在性 oracle） |
| Room 未知、bad-id、requester 非该房 active 成员 | `404 room_not_found` |
| `message_id` 非 UUID v4 | `400 invalid_request` |
| Message 未知、他 Room、对该 Agent 不可见 | `400 invalid_read_cursor` |
| body 非 JSON / 未知字段 | `400 invalid_request` |
| body 超过 1 KiB | `413 payload_too_large` |
| 存储失败 | 既有安全错误边界（`503 storage_unavailable` / `500 internal_error`） |

失败不改变 cursor、不生成任何事实。

## 6. 不变量

1. **不新增持久对象**：无新表、无新列、无新幂等记录、无新事件 kind；改的只有两张既有表的列名与三道既有触发器的判据；
2. **一条写路径**：真实推进的唯一写路径仍是 `read_cursor_advancements` 的 INSERT（触发器链在同一语句内推进 cursor）；Human 与 Agent 共用；
3. **可见性不复制**：目标 Message 的可见性判据与 History / 未读**同一**口径（成员周期窗口），不新增 Agent 专用规则；
4. **Agent 的推进零事件**：`human_event_facts` / `human_event_deliveries` 对该推进零新增；
5. **Human 侧零变化**：`PUT /v1/rooms/{room_id}/read-cursor`、`GET /v1/rooms`、`GET /v1/rooms/{id}` 的语义与响应形状一字不变；
6. **位置只属于本人**：端点不接受路径或 body 里的 participant_id，身份只来自 BackendAuth；
7. **每个成员周期有一条读游标**（⚠ 2026-09-27 真机验出并补齐）：行的权威创建者是**建周期那条触发器**（`read_cursors_membership_insert`）——**没有第二条创建路径**。但触发器只覆盖"今后"，**历史周期**（本切片之前建的）需要单独补账：migration `202609270003_backfill_read_cursors.sql` 给每个 **active** 周期补一行 join boundary（`read_sequence = join_after_sequence`、`last_read_message_id = NULL`、`created_at = joined_at`），排除群主转让打开的两种周期（它们的来路是"继承前序 cursor"，不是"新边界"）。**真机证据与缺陷经过见 `outputs/CORE_ALIGN_KNIFE_04_HOME_DRILL_REPORT.md`**：真家 5 条活跃 Agent 周期**全都没有** cursor 行 ⇒ 推进被见证守卫拒绝（`read cursor advancement must move from the current cursor to a visible same-room message`）——补建前被拒、补建后同一写入成功。

   **为什么单测/黑盒没抓到**：夹具的成员周期全是迁移之后建的，触发器总会建行；"既有行"这条路径从没被走过。回归用例 `read_cursor_tests::a_period_without_a_cursor_row_cannot_advance_until_the_backfill_runs` 现在**复现那条路径**（摘掉建行触发器再邀 Agent 入群 = 当年的历史形态），并用**迁移里那段真 SQL**（`include_str!`）验证补建与幂等。

## 7. 非目标

- **不做"已读回执"**（不新增人类可见的已读语义、不新增事件 kind）；
- **不改** `since` / `since_count` 口径（D3 另裁）；
- **不做**执行端（Runtime 无新动词；"读完之后推游标"是另一刀）；
- **不做** Agent 退群（M2）、不做成员生命周期 Event、不做 Agent 房间 live 读取面；
- 不碰 `client/`、`agent/`、`im/connectors/`。

## 8. 验收（黑盒与测试）

- HTTP 契约测试：`im/server/src/adapters/http/backend_read_cursor_routes_tests.rs`（认证互斥、非成员 404、非法目标 400、body 形状、单调与 no-op、通讯录 `read_state` 一致、Human 侧回归）；
- 存储层：`im/server/src/adapters/sqlite/read_cursor_tests.rs`——Agent 周期建行形状、推进单调、**零事件**（fact/delivery 逐条数）与 Human 推进仍产生事件的负对照、以及"Agent 周期不能有 cursor"那条**旧反向断言已按本切片改为正向**（改动理由见该用例注释）；
- 独立进程黑盒：`im/server/tests/agent_read_cursor_blackbox.rs`（真 Core 进程 + Mock Backend，走公开协议）。
