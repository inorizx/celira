# HOME_ACCESS.md — Home 访问与设备绑定

> 状态：**A 包已实现并经独立 Core 进程黑盒验证；A2 包（配对事务可靠提交协议）已实现并经独立 Core 进程黑盒验证；B 包（远程 Client Host 适配）已实现**。`HOME_ACCESS.md` 的 A/A2 设计、`CORE_V1_IM_CONTRACT.md` 的公共契约、`CORE_V1_ACCEPTANCE.md` 的验收记录与 Core 实现现已对齐；B 的最终行为见 [LOCAL_CLIENT_HOST.md §7](../../client/LOCAL_CLIENT_HOST.md#L7)。C（Web/真实双 Client 浏览器验收）未完成，本文不把 C 描述为已交付。
>
> ⚠ **2026-09-20 退役标注**：上句里 **B 包（远程 Client Host 适配）与本文中所有以 Client Host / Local Client Host / Host BFF / 远程 Host / 双 Host 进程 指称的实现，已于 2026-09-20 随 Host 全托管路径退役并整体删除**（`host/` crate 与前端 Host 模块均已从工作树移除，考古走 Git 历史）。受影响的段落是：§1 词汇表「Client Host」行、§3.2 L2 拓扑图的 Client Host 节点、§4.1 不变量第 5 条末句、§4.3「浏览器 BFF C1 管理面」、§4.5「B 包落地」、§7.2 验收矩阵的「真实双 Client 浏览器验收」一列、§8 包 B 行。**这些段落的契约语义（字段表、端点表、错误码表、不变量与验收口径的实质内容）一字未改，仍按冻结契约读**——变的是：它们描述的实现**不再是可执行或可复现的当前实现**。今天的本机路径是 `node home.js up` 起家、浏览器进登录页直连 Core。
>
> 前提：主人已批准窄解冻方向——每个 Home 任一时刻一个且只有一个 Active Core；同一 Home 多个 Client 都只连它；禁止双连、同步和合并；Agent/Runtime/Provider/ACP 不在本切片。
>
> 本文只覆盖 C4 L1–2 与契约层，不写实现细节（数据库守卫、migration 编号、具体 SQL 归实现与测试）。

## 1. 词汇表

| 词 | 定义 | 关键约束 |
|---|---|---|
| **Home** | 持续私人通信空间，一个账号域的权威单元；公共身份、关系、房间、消息都属于某个 Home | 每个 Home 任一时刻恰有一个 Active Core；Home 之间不共享、不合并 |
| **Home Identity** | Core 初始化时生成的持久**非秘密**标识（推荐 UUID v4 + 可选显示名）；是绑定的锚与核验依据 | 不是凭据：知晓它不授予访问；明文存储、明文展示、明文进绑定记录 |
| **Active Core** | 该 Home 当前唯一可写逻辑 Core，公共事实唯一裁决者 | 单写者；禁止双写、同步、合并；部署位置可变（电脑/NAS/VPS/代运维） |
| **Client** | 呈现平面实例：一个 Client Host 部署（及其浏览器入口）是一个 Client；同一 Host 上的多个标签页仍是一个 Client | 同一时刻只允许一个 Active Home Binding；只承认所绑定 Home 的 Active Core |
| **Device** | 物理设备，承载 Client；仅在兑换时登记为非秘密标签，用于会话列表展示 | **不是 Participant、不是 Room 成员、不是事实源**；其凭据可独立撤销 |
| **Human Session** | 既有 v1 事实：≥256 bit Bearer token，Core 只存单向摘要，独立可撤销，证明调用代表指定 Human | 新设备兑换产生**独立** Human Session；撤销一个不影响其他 |
| **Pairing Grant** | 由认证 Human（有效 Human Session）或部署控制面 operator CLI（同一公共 pairing grant 事实/状态机）签发的短期、一次性、可撤销配对凭据；新设备只能兑换为**签发者本人**的 Human Session | 只在兑换端点有效；`兑换所得 Session.human_id == issued_by`；仅「原子建 Session + 标记 consumed」成功事务才消耗；内部失败不消耗、可重试；TTL 内未用自动失效 |
| **Client Host** | 本机编排与连接壳（Local Client Host 演进）：当前受管本地 Core 进程，未来增加远程 Core 连接适配；浏览器会话 Broker 与 loopback BFF | **不是事实源、不是 Participant、不是公共事实写者**；BFF 永远只监听 loopback；远程模式下本机独立 Human Session 持久化于本机受保护 Secret Store（最低要求仅本机用户可读），浏览器绝不持有 |

三条否定必须贯穿契约：**URL 不是 Core 身份**（端点只是可轮换的连接事实）；**Device 不是 Participant**（无成员资格、无消息、无公共身份）；**Client Host 不是事实源**（其绑定记录是本地可销毁镜像，服务端事实终局归 Core）。

## 1.1 邀请一个人进这个世界（2026-09-28 主人批准；方案 a）

> 状态：**已实现并验收**（migration `202609280003` / `202609280004`；契约本节）。
> 授权：主人 2026-09-28「行可以」——三件取舍按建议：任何 active Human 可邀请 /
> 兑换后自动生成一条待答关系申请 / 两阶段配对路径本刀不支持。

**一句话原理**：**邀请是"由已有主体签发给一个新主体"的一次性凭据**——它沿用既有
Pairing Grant 事实与状态机（一次性、可过期、可撤销、原子消耗、不可改写、不可删除），
**只把"兑换出来是谁的会话"从"签发者本人"扩成"也可以是新来的那个人"**。

### 不做匿名自注册（已裁口径，写在这里免得被"普通 IM 都注册"改掉）

一个世界的入口**只有两条**：**建一个世界**（自己部署）或**被请进一个世界**（邀请码）。
理由两条，都在 `PRODUCT_VISION.md` 当前阶段里：

1. **自托管实例是"一个人的家"，不是平台**：core 的消息明文落 SQLite、部署者读得到所有人的
   公共消息——所以这个场景的性质是「朋友之间」，不是「平台与用户」。开一个注册口，等于把
   家门变成公共门（"默认密码从部署完成到首次登录之间是一个**抢占窗口**，任何扫到端口的人
   都能抢先成为这台机器的管理员"——同一份文件的原文）；
2. **注册需要一个全局身份权威**（短信/邮件验证链路 + 滥用兜底），而"人人可自部署"恰恰意味着
   没有那个权威：身份是**局部的**（你在我的世界里叫"阿元"，在别人的世界里可以是另一个人）。
   只有做**联邦**（跨世界通信）才需要一个两边都认的身份权威——那不在 Core v1 里。

**邀请码是"家钥匙"在人身上的对称物**：家钥匙让一台**机器**进来，邀请码让一个**人**进来；
两者同一套纪律（一次性、只在签发那一刻输出、库里只留摘要、可撤销）。

### 1.1.1 协议（与设备配对**同一张表、同一个状态机**）

`POST /v1/home/pairing-grants`（认证 Human）请求体多一个可选字段：

| 字段 | 取值 | 说明 |
|---|---|---|
| `grants` | `self_device`（**缺省**）\| `new_human` | 用途。缺省值保证老客户端一字不改地继续工作 |
| `device_label` | 可选 ≤80 | 仅设备配对用 |

- 响应**纯新增**一个 `grants` 回显（签发响应与列表响应都有）；`token` 仍然**只输出一次**；
- **TTL**：`self_device` 沿用配置的 `grant_ttl_minutes`（默认 15 分钟——签发者就在机器旁）；
  `new_human` 固定 **72 小时**（朋友不一定马上点开；风险由"一次性 + 可撤销 + `issued_by` 留痕"承担）；
- 未知 `grants` 取值 ⇒ **400 `invalid_request`**（封闭词表，不猜、不静默降级）。

`POST /v1/home/pairing-grants/exchange`（**匿名**，Bearer 位置放邀请码）请求体多一个可选字段：

| 用途 | `display_name` | 行为 |
|---|---|---|
| `self_device` | **必须缺席**（带了 ⇒ 400，且**不消费**） | 会话归 `issued_by`（既有语义，逐字未动） |
| `new_human` | **必须**（1..=80、禁 NUL、trim 非空） | 同一事务内：**新建 active Human** → 为它签发独立 Session → 写 Session Metadata → 标记 Grant consumed |

- 名字由**被邀请者自己**给（不由签发者代填）——"自己的名字自己说"；
- 缺名字 ⇒ **400 `invalid_request`**，且**不消费** Grant（补上即可重试）；
- **失败零残留**：任何失败路径都不产生 participant / session / metadata 任何一行；
- 响应形状与设备配对**完全相同**（`session.human_id` 是新人的 id）。

### 1.1.2 错误面（**不新增任何错误码**）

缺凭据 ⇒ 401 `authentication_required`；格式错 / 未知 / 已用 / 过期 / 撤销 ⇒ **统一** 401
`grant_invalid`（**无存在性 oracle**）；用途与请求体对不上 ⇒ 400 `invalid_request`。

⚠ 如实登记的**一处刻意不对称**：邀请码有效但没给名字时返回 400（而不是混进
`grant_invalid`）。理由是"码有效"这件事**只有已经持有有效码的人**才可能看到（码是 ≥256 bit
随机），不构成可利用的 oracle；反过来把有效码谎报成 `grant_invalid`，会让诚实的被邀请者
以为码坏了、白等一张新码。

### 1.1.3 数据库形状（两列，不搬行）

| 列 | 语义 | 守卫 |
|---|---|---|
| `pairing_grants.grants_human` | 0 = 设备配对 / 1 = 邀请（`CHECK IN (0,1)`） | 进"签发事实不可改写"清单 |
| `pairing_grants.invited_human_id` | 这张邀请码**点名了谁** | 一经写入不可改写（`NULL → 有人` 是一次性事实，不允许换人） |

`session_metadata` 的来源守卫**多认一支**：`session.human == grant.issued_by`（设备配对，逐字未动）
**或** `session.human == grant.invited_human_id`（邀请）。

**为什么不是"`grants_human = 1` 就放行"**：那样任何一张邀请码都能被用来给**任意既有 Human**
铸会话，守卫就废了。点名 + 一次性写入让不变量仍然**可证明**：一张邀请码最多给一个人铸会话，
且必须是它在同一事务里新建的那个人。

### 1.1.4 兑换之后：一条待答的关系申请（事后同意）

邀请类兑换成功后，服务端**自动为邀请者向新人建一条 `pending` 关系申请**（邀请者 → 新人）：
新人一进来就能看到"某某想加你"，自己决定接受或拒绝。

- 它是**事后同意**，不是"预先同意"（服务层退役时正是这条理由）；
- 它是 **best-effort**：邀请本身（Human + Session）已经成立，关系申请失败只记脱敏 warn，
  **绝不翻转已经发生的兑换**；幂等键由 Grant id 派生（`invite-<grant_id>`）。

### 1.1.5 明确不做（本切片）

- **不做匿名自注册**（见上）；
- **不做投递**：邀请码怎么送到朋友手里（链接 / 短信 / 口述）不在协议内，客户端只提供"复制"；
- **两阶段配对路径（`prepare` / `commit`）不支持 `new_human`**：那条路径今天**零消费者**
  （客户端与脚本都不调它），遇到邀请类 Grant 一律拒绝（沿用既有统一码 `grant_invalid`），
  **不静默按设备配对给别人铸会话**。要支持它就得给 `pairing_transactions` 也加列——另立一刀；
- **不新增限流**：im 全仓今天没有任何限流机制（如实登记）。这条匿名端点的安全依赖
  **码的熵（≥256 bit）+ TTL + 一次性**；"限流"是独立的一笔账；
- **不改**设备配对的任何既有语义、消息主链、事件面、读游标、撤回、附件、Presentation、
  房治理 / 关系 / 拉黑面。

### 1.1.6 验收证据

- store 级：新 Human 真的被创建（kind/status/display_name）、会话归它自己、metadata 指向本 Grant、
  Grant 变 consumed、**重放统一 `grant_invalid` 且不再造人**、缺名字 / 名字非法 / 撤销 / 未知码
  四条失败路径**零残留**、设备配对带名字被拒且不消费；
- 路由级：认证签发 → 匿名兑换 → **新身份能读 `/v1/me`**、邀请者名下出现 `pending` + `direction: outgoing`
  的关系申请、码一次性、已消费的 Grant 不再出现在签发者列表里；
- 真机：真家副本预演迁移（逐表行数不变）+ 起停 + 一次完整邀请闭环。

## 2. 开源自托管范围

- Core 是用户拥有的软件与数据，可部署在**个人电脑、NAS、VPS**，或未来由**代运维方托管**（Managed Self-host）。代运维只是部署位置与运维契约的变化：不引入平台账号、不引入账号云、不改变单 Home 单 Active Core 与数据自持。
- 明确不在本切片：SaaS 账号体系（无注册、无找回密码、无平台级用户）、联邦（Home 间不互通）、多 Core 同步/合并、云同步服务。
- 操作员路径保持既有形态：首次初始化经本机/SSH 的 operator CLI（`bootstrap-human` 等），不做匿名自注册。补一条**仅本机/SSH operator CLI** 的初始配对路径：在既有 active Human 下签发与 HTTP 同一语义的 Pairing Grant，一次性输出原始 token，用于完成第一台远程 Client 的初始配对；它是部署控制面，不能替代浏览器/远程 API，也不改既有 `bootstrap-human` / `issue-human-session` 语义；普通后续配对仍由已认证 Human 创建自己的 Grant。

## 3. 拓扑与原理

### 3.1 一句原理

Home 身份与连接端点分离：Client 绑定并核验的是**持久非秘密的 Home Identity**，端点只是可轮换的运输事实；绑定、配对与设备会话全部由唯一 Active Core 裁决，Client 端以「一个绑定、单写者、fail closed」约束防止双连、错连与凭据外泄。

### 3.2 C4 Level 1–2

```text
L1（参与者：Human）
  Human ──拥有──> Home（权威单元）
    Home 由以下构成：
      呈现平面：M 个 Client（多设备/多入口，同一 Home 一致体验）
      公共事实平面：恰一个 Active Core（单写者）
      执行平面：N 个执行点（本切片外，仅标位）
    Home 之间不互通、不合并

L2（P0 后）
┌─ 设备 A ──────────────────────────┐
│ 浏览器 ──同源 loopback──> Client Host A│
└──────────────┬───────────────────┘
               │ 绑定：Home Identity + 独立 Human Session
┌─ 设备 B ──────────────────────────┐      （远程 Core 适配：非 loopback 一律 TLS/HTTPS）
│ 浏览器 ──同源 loopback──> Client Host B│
└──────────────┬───────────────────┘
               │ 绑定：Home Identity + 另一独立 Human Session
               ▼
┌─ Active Core（电脑 / NAS / VPS / 代运维）────────────┐
│  ├─ SQLite 公共事实（v1 冻结：Participant/Room/Message/   │
│  │   Membership/Event/Attachment/ReadState）│
│  ├─ P0 新增（解冻切片，见 §4.4）：Home 事实、Pairing Grant、│
│  │   Session Metadata（1:1 引用既有 Session）               │
│  ├─ v1 冻结端点：/v1/me /v1/rooms /v1/sessions/current  │
│  │   /v1/human/events/stream /v1/human/live/stream 等   │
│  └─ P0 新增端点：/v1/home、pairing-grants、sessions 管理  │
└──────────────────────────────────────────────────────┘
```

### 3.3 关键数据流

- **首绑（远程 Core）**：部署 Core → 启动时幂等初始化 Home Identity → 操作员经 CLI 建首位 Human → 初始配对：第一台设备凭**部署控制面 CLI 签发的初始 Pairing Grant**（本机/SSH，一次性输出原始 token）完成绑定；此后的新设备由已绑定设备上的主人签发 Grant → 新设备输入端点 + Grant → 调 exchange：成功响应携带 Home Identity 供首次核验并落绑定，同时得本设备独立 Human Session → 日常连接。
- **日常连接**：每次连接，Core 返回的 Home Identity 必须与 Client 绑定记录一致，不一致即 fail closed（`home_identity_mismatch`），不发送、不展示、不降级。
- **多 Client 并存**：多个 Client 并行连同一个 Active Core 是期望形态；各自持有独立 Session，公共事实由 Core 统一裁决，Client 间无同步。

## 4. 最小 P0

### 4.1 目标与不变量

1. Core 持久化非秘密 Home Identity（初始化一次、不可变）；
2. Client 首次绑定前核验 Home Identity，此后每次连接自动比对；
3. 认证 Human（或仅本机/SSH 部署控制面 CLI，同一事实/状态机）可创建短期/一次性/可撤销 Pairing Grant；
4. 新设备凭 Grant 兑换**独立可撤销** Human Session；
5. 浏览器永不持有 Core Bearer（生产路径；既有 web 的专家/开发 Core 直连模式不能成为生产 Browser 路径，P0 不扩展它）；演进 Local Client Host 做远程 Core 适配：远程模式下 Client Host 将自己的独立 Human Session 持久化于本机受保护 Secret Store（`remote-home` 0700 + `binding.secret.json` 0600 + 原子写 + symlink 防护，见 §4.5），BFF 仍只监听 loopback、浏览器只持 Cookie+CSRF——已由 B 包实现。

### 4.2 新公共事实（契约层）

| 事实 | 字段语义（契约层） | 约束 |
|---|---|---|
| `home` | home_id（UUID v4）、display_name（可选，非秘密）、created_at | 每 Core 恰一条；初始化幂等且只成功一次；不可变 |
| `pairing_grant` | grant_id、token 单向摘要、issued_by（human_id）、issued_at、expires_at、存储状态（active/consumed/revoked；expired 为按 expires_at 派生的展示状态）、device 标签（可选，非秘密） | 一次性：仅成功完成「原子建 Session + 标记 consumed」事务才消耗；内部失败整体回滚、grant 仍可重试；撤销/过期不可兑换；原始 token 只输出一次 |
| `session_metadata`（可选） | 新事实，1:1 引用既有 Session：device 标签、来源 grant（均非秘密） | 不改写 human_sessions 既有语义；session 列表仅返回本 Human 的非秘密元数据，永不返回 token/hash；独立可撤销，不影响其他 Session 与 Human 本体 |

### 4.3 新端点（建议契约，统一错误信封与卫生标准）

- `GET /v1/home` —— 需 Human Session，不能作为匿名发现；认证后返回 home_id/display_name/created_at，供绑定核验与展示；未认证 401。新设备不经此发现：以 endpoint + Pairing Grant 进入 A2 `prepare → pending store → commit`，成功响应携带 Home Identity 供首次核验与落绑定；普通日常使用仍经认证 `GET /v1/home` 复核。
- `POST /v1/home/pairing-grants` —— 认证 Human；可选 device 标签；返回 grant_id/expires_at，token 只输出一次。
- `GET /v1/home/pairing-grants` —— 认证 Human；列出本人未消费 Grant（id/状态/过期时间；expired 为按 expires_at 派生的展示状态，不是持久状态）。
- `DELETE /v1/home/pairing-grants/{id}` —— 撤销未使用 Grant；幂等。
- **浏览器 BFF C1 管理面**：已绑定 Host 仅投影 Home Identity、Grant/Session 非秘密元数据并提供撤销；不注册浏览器 `POST /pairing-grants`，不把一次性 Grant 原文放入浏览器响应、URL、localStorage、日志或错误。原始 Grant 继续由 operator CLI / 可信原生交接交给新 Host。
- `POST /v1/home/pairing-grants/exchange` —— 凭 Grant Bearer（无需 Session）；先校验请求形状与 token 格式；对外所有不可兑换情形（未知、格式错、已消费、过期、撤销）统一 `grant_invalid`，不暴露 Grant 是否存在或曾经有效；一次兑换一个**与 `issued_by` 相同 Human** 的新 Human Session（token 只输出一次），成功响应携带 home_id 供首次核验与落绑定；**仅成功的「原子创建 Session + 标记 grant consumed」事务才消费**——任何持久化/内部失败整体回滚且 grant 仍可重试；成功后重放统一 `grant_invalid`，绝不新建或重发 Session。这比盲目消耗更可靠：内部故障不会把一次可重试的合法配对永久作废，重放也不会产生第二个 Session；若成功响应在传输中丢失，原始 Session token 不可恢复，用户必须重新签发 Grant 并配对。
- **A2 配对事务可靠提交协议**（替代远程 Host 直接调用上述 exchange；既有 exchange 继续保留给受控脚本/兼容客户端，不作为 C 包生产绑定路径）：
  - `POST /v1/home/pairing-grants/prepare` —— 匿名凭 Grant Bearer + Host 生成的 `client_nonce`、`session_token_hash`、`commit_token_hash`（均为固定长度摘要）；校验通过后创建或幂等重用 `pairing_transaction`，**不创建正式 Human Session、不消费 Grant**，返回 transaction_id、Home Identity、issued_by Human 的非秘密身份、reserved session id、expires_at 与 request_id；原始 Session/commit token 从不进入 Core 响应，Core 只存摘要；同一 `client_nonce` + 摘要重试返回同一准备事务，参数不一致统一 `pairing_transaction_invalid`。
  - `GET /v1/home/pairing-transactions/{id}` —— 凭该事务的 commit token Bearer 查询非秘密状态（prepared/committed/expired/aborted）与 Home/Human/Session 元数据；未知、错误 token、其他 Human 或过期事务统一 `pairing_transaction_invalid`，不提供存在性探测；仅供 Host 恢复，不给浏览器直连。
  - `POST /v1/home/pairing-transactions/{id}/commit` —— 凭 commit token Bearer；Core 在一个事务内完成「以 prepare 阶段提交的 session_token_hash 创建正式 Human Session + 写入 session_metadata + 标记 pairing_transaction committed + 消费 Pairing Grant」；首次成功与重复提交返回同一非秘密结果，绝不产生第二 Session；**prepare 阶段的 session token 原地成为正式 Session token**，Host 不需从 Core 再取秘密。
  - `DELETE /v1/home/pairing-transactions/{id}` —— 凭 commit token Bearer 主动放弃 prepared 事务；只允许撤销尚未 committed 的事务，幂等；不影响 Grant 的 active 状态；过期事务由 Core 按时间拒绝并可后台清理，清理不改变 Grant 可重新 prepare 的语义。
  - A2 的秘密流：Host 在任何 Core 请求前本地生成高熵 provisional session token、commit token 与 client_nonce，并先把它们连同 Grant 原文写入受保护 pending store；Core 只接收 SHA-256 摘要；Host 只有在 pending store 原子落盘成功后才调用 prepare，commit 成功后再把 pending 原子提升为 active binding。浏览器永远看不到上述任一秘密。
  - A2 的恢复：prepare 请求/响应丢失时，Host 以同一 client_nonce + 摘要重试；commit 请求/响应丢失时，Host 以同一 commit token 查询/重试；Host 重启后先恢复 pending store，再按事务状态收敛为 active 或清理过期 pending。正常持久化假设下，不产生孤儿正式 Session，也不要求重新签发 Grant；本机 pending store 无法安全读取时必须 fail closed，不得当作未绑定。
- `GET /v1/sessions` / `DELETE /v1/sessions/{id}` —— 认证 Human 列出/撤销自己的 Session；列表仅返回本 Human 的非秘密元数据（device 标签等），永不返回 token/hash；`DELETE /v1/sessions/current` 既有语义保留。

新增稳定错误码：匿名直接兑换仅 `grant_invalid`；A2 匿名 prepare 对不可用 Grant 仍统一 `grant_invalid`，事务恢复/提交入口的不可兑换情形统一 `pairing_transaction_invalid`；认证后的 Session/管理入口可用 `session_not_found`、`home_not_initialized` 等稳定分类。Client 侧另有 `home_identity_mismatch`（非 Core 端点返回）。

### 4.4 解冻影响标注（必读）

Core v1 已冻结。A 包需要：新增 SQLite 表（home、pairing_grant、session_metadata——后者为新事实、1:1 引用既有 Session，不改写 human_sessions 既有语义）、新增 §4.3 端点与错误码。A2 另需新增 `pairing_transactions` 表及 prepare/status/commit/abort 端点；这属于「新产品能力」，按 `CORE_V1_ACCEPTANCE.md` §6 的四类解封理由**不自动成立**，须按 `../../../AGENTS.md` 的「动手前门禁 / Core v1 封板门禁」由主人显式批准为**窄解冻切片**（主人已批准 A2，理由是消除 Core Session 与 Client Host 本地持久化之间的不可逆提交窗口；先例：Agent Live State）。

切片边界承诺：只新增事实与端点；**不修改任何既有冻结端点语义、不修改既有表、不改变 Human/Agent 身份、Room/Membership/Message/Event/Completion/ACK 语义**；既有 operator CLI（bootstrap-human / issue-human-session）语义不变；新增仅本机/SSH 的初始配对 CLI，与 HTTP grant 使用同一公共 pairing grant 事实/状态机，仅多出「一次性输出原始 token」的部署控制面用途，不新增其他语义。

### 4.5 B 包落地（Client Host 最终行为）

> ⚠ **本节描述的实现已于 2026-09-20 随 Host 线退役并整体删除**（`host/` crate、前端 Host 模块、下列 `/api/v1/local/home/binding` 控制面与 Secret Store 均不再存在；考古走 Git 历史）。以下内容是**那套实现的契约层记录**，不再是当前行为；契约语义本身未改。

远程 Client Host 适配已实现，权威实现与端点清单见 [LOCAL_CLIENT_HOST.md §7](../../client/LOCAL_CLIENT_HOST.md#L7)。契约层事实：

- **远程模式不做本地执行面工作**：不启动/注册/编排本地 Core、Runtime、launcher 或 Provisioner，不执行 Core CLI，不读取本地 `provisioning.json`，也不解析或保留本地 Provider 配置、Runtime Admin client/token；绑定/恢复/解绑全部经 operator-only 控制面 `GET/POST/DELETE /api/v1/local/home/binding`。B 包只承诺远程 BFF REST；远程 SSE/Live WebSocket（含 HTTPS/WSS）不在本包验收或兼容承诺内。
- **bind**：operator Bearer + 严格 Headers（无 query、无 Cookie、无 Origin、无任何 `sec-fetch-*` Fetch Metadata、无 CSRF）+ 端点为非 loopback http 即拒绝；pairing grant 兑换独立 Human Session 后逐字段核验 Home 身份与 Human 身份，核验通过才原子写盘并激活。
- **restore（启动自动）**：重新核验后才 ready；端点不可达/会话失效/身份不符 → 保留持久 bound=true/ready=false 且不阻塞 Host 启动；Store 损坏/不安全 → fail closed（稳定 `remote_binding_store_unavailable`，绝不静默 bound=false）。
- **unbind**：销毁本地 Store 与 active 绑定、清理 BrowserAuth、共享 Core 切到不可达占位、允许重新 bind；不外撤远端 Session。
- **Secret Store**：`remote-home/` 0700、`binding.secret.json` 0600、普通文件、属主为运行用户、原子写（临时文件 + fsync + rename + 父目录 fsync）、O_NOFOLLOW、schema 白名单 fail closed。**浏览器永不持有 Core Bearer**：浏览器只持本 Host Cookie + CSRF，Host 以 in-memory Session 克隆读写远程 Core；公开 DTO/Debug/日志不含 token。
- **威胁边界（如实声明）**：0700/0600 与路径校验不构成 OS 沙箱，**不能防御** root、同 UID 恶意进程或已攻陷操作系统。

## 5. 默认建议

1. **配对凭据形态**：opaque 随机 token，与 Human Session 同标准（≥256 bit CSPRNG、Core 只存 SHA-256 摘要、只输出一次）；单一用途（仅兑换本 Home 的 Human Session）；不嵌入 URL、不带可读语义、不做二维码。
2. **熵**：≥256 bit；摘要存储；校验用固定长度常量时间比较（复用既有 Session 卫生）。
3. **TTL 与 Session 生命周期**：Grant 默认 15 分钟（1–60 分钟可配置）；过期/撤销/已消费即不可兑换。P0 新签发的设备 Session 沿用既有完整 Human Session 语义，不自动过期、仅支持独立撤销；设备标签不是降权或隔离边界。自动轮换/短会话是后续切片。
4. **消费与重放**：先校验请求形状与 token 格式；仅成功的「原子创建 Session + 标记 consumed」事务才消耗。任何持久化/内部失败整体回滚且 Grant 仍可重试；匿名兑换对未知、格式错、已消费、过期和撤销一律返回 `grant_invalid`，不形成状态探测；成功后的重放绝不新建或重发 Session。若成功响应在传输中丢失，原始 token 不可恢复，必须重新签发 Grant 并配对。
5. **撤销**：主人可在任一已绑定 Client 撤销未使用 Grant 或任一设备 Session；后续 REST 请求立即按无效 Session fail closed（401）。既有长连接按现有 Session 撤销契约在下一次唤醒前关闭；本切片不承诺强杀设备或瞬时断线。
6. **URL、TLS 与部署入口**：URL/端口只是连接端点，不是身份；绑定事实是 Home Identity，每次连接核验——两者缺一不可：TLS 保障传输与端点认证，Home Identity 防止错连错误 Home。**任何非 loopback 的 Core 链接必须是 HTTPS**：B 包使用默认系统受信链，绝不关闭证书验证；证书/公钥指纹固定（TLS pinning）不在本包实现或验收范围。P0 由用户部署的反向代理 / Ingress 在 Core 外终止 TLS，Core 保持其既有 HTTP 公共协议与单写职责；只有进程间 loopback 可用 http。证书或端点变更必须由用户明确确认，不得静默接受。Client Host 对端点做显式 allowlist + fail-closed 配置；**localhost BFF 永远只监听 loopback，绝不改公网**。
7. **单绑定**：一个 Client（一个 Client Host 部署）同时只允许一个 Active Home Binding；换绑 = 显式操作（销毁本端绑定与本地凭据 → 绑定新 Home），不维持第二连接。
8. **Core 迁移不在 P0**：换机器 = 停机 + 备份恢复 + 重部署，Home Identity 随库迁移，Client 只改端点、身份不变；在线迁移、零停机、多副本不在 P0。
9. **P0 边界（如实声明）**：Home Identity + 单 Client Binding **无法在技术上阻止**操作员从同一备份启动两个带相同 Home Identity 的可写 Core；它只让每个 Client 同时绑定一个 Home+端点并检测错误 Home。「一 Home 一 Active Core」仍是部署/运维单写者不变量，不由 Home Identity 保证。迁移、fencing、clone/双启动检测、HA 均不在 P0，绝不宣称已由 Home Identity 保证（见 §6 不做项）。

## 6. 不做项与禁止连接

### 不做

- SaaS 账号体系：注册、找回密码、平台级用户；联邦；多 Core 同步/合并；云同步服务；
- 在线 Core 迁移、零停机迁移、多副本写；fencing、clone/双启动检测、HA（Home Identity 不保证，见 §5）；
- 自动发现（无 mDNS/扫描/局域网广播）：端点与 Identity 一律显式配置；
- Device 级管理协议（远程锁定/擦除/设备指纹）：只做 Session 级独立撤销；
- 匿名访问 Home：一切 Home 数据需认证；
- Pairing Grant 作为长期凭据或多用途凭据；
- 把 `HOME_ACCESS.md` 写入 `../../../web/node_modules/zwitch/readme.md` 地图或任何权威文档（待确认后另行安排）；
- 把既有 web 的专家/开发 Core 直连模式扩展为生产 Browser 路径：不删除、不禁止该开发模式，但它不能成为生产 Browser 路径，P0 不扩展它；

### 禁止连接

```text
Client ──X──> 同一时刻连接第二个 Core / 第二个端点（双连）
同 Home ──X──> 两个可写 Active Core（双写/同步/合并）
Client ──X──> 直读 Core 数据库
浏览器 ──X──> 直连远程 Core 端口（生产 Browser 路径：浏览器只信本机 Client Host 的 loopback BFF；既有专家/开发直连模式不扩展为生产路径）
Client Host BFF ──X──> 监听非 loopback / 暴露公网
Core ──X──> 反向连接设备 / 主动推送
以 URL ──X──> 代替 Home Identity 作为绑定事实
Device / Pairing Grant ──X──> 作为 Participant 或 Room 成员
Client Host ──X──> 向浏览器交付 Core Bearer / 代理凭据
```

## 7. 用户故事与验收矩阵

### 7.1 用户故事

- **US1 首次自托管**：用户在电脑/NAS/VPS 部署 Core → 初始化 Home Identity 与首位 Human → 桌面浏览器经 Client 绑定并开始使用。
- **US2 第二设备**：已绑定设备上签发 Pairing Grant → 新设备输入端点 + Grant → 兑换独立 Human Session → 两设备并行可用，公共事实一致。
- **US3 重放/超时/撤销**：同一 Grant 二次兑换被拒且不产生新 Session；过期 Grant 被拒；撤销 Grant 后兑换被拒；撤销某设备 Session 后该设备立即 401，其他设备不受影响。
- **US4 错误 Core/Home**：绑定 Home A 的 Client 被指向错误 Core（另一 Home 或旧桩）→ Home Identity 核验失败，fail closed，不发送、不展示、不泄露 Home A 数据。
- **US5 改绑**：显式解除 Home A 绑定（本端凭据销毁）→ 绑定 Home B → 旧端点不再被本 Client 使用；Home A 既有公共事实不受影响。
- **US6 断线恢复**：断网/服务重启后重连 → 用既有 Session 恢复，不产生重复公共消息；Core 重启后 Home Identity 不变，绑定仍有效。
- **US7 传输安全**：非 loopback 端点未启用 TLS 或证书不受默认系统信任链信任 → 连接失败，不发送任何数据；TLS 验证通过但 Home Identity 不符 → `home_identity_mismatch` fail closed（与传输失败区分）。TLS pinning 不在 B 包。
- **US8 初始配对（第一台远程设备）**：尚无已绑定设备时，操作员经本机/SSH CLI 在既有 active Human 下签发初始 Pairing Grant（一次性输出原始 token）→ 第一台远程 Client Host 凭 endpoint + Grant 完成绑定；此后新设备仍由已认证 Human 签发 Grant。

### 7.2 验收矩阵

| 故事 | 协议级验收（独立进程黑盒，公开协议） | 真实双 Client 浏览器验收 |
|---|---|---|
| US1 | 全新 Core 启动后 `GET /v1/home` 返回稳定 home_id；CLI 建首位 Human 后即可签发 Grant | 两个真实 Host 进程 + 各自浏览器连同一真实 Core；首绑流程完整走通 |
| US2 | 签发 → 兑换返回新 Session token 且只一次；新 Session 可用；两个 Session 并行互不影响 | 两浏览器并行收发消息，双方均只见权威 Core 事实（SSE 唤醒 + 重读），无重复消息 |
| US3 | 重放、过期、撤销或未知 Grant 兑换均为 `grant_invalid` 且 Session 数不变；`DELETE /v1/sessions/{id}` 后该 Session 的后续 REST 请求 401，另一 Session 仍 200；既有长连接按当前撤销契约在下一次唤醒前关闭 | 撤销浏览器 A 的设备 Session → A 后续 BFF 请求 fail closed，B 不受影响；A 的网络与界面无降级回退 |
| US4 | 协议层断言绑定记录比对失败路径：端点返回另一 home_id → 客户端停止并报 `home_identity_mismatch`，不发出任何写请求 | 把 A 的 Client Host 配置改为 Core B 的端点 → 界面诚实报错，A 既有消息与绑定不受污染 |
| US5 | 销毁本端绑定后旧 Session 不再被使用；服务端旧 Session 由主人撤销后 401 | 换绑后浏览器只展示新 Home 的权威事实 |
| US6 | Core 重启后 `GET /v1/home` 的 home_id 不变；既有 Session 恢复可用；断线窗口的重试与幂等按既有契约收敛，无重复消息 | 断网 → 恢复 → 浏览器补读权威首屏，pending 发送以原幂等 key 收敛，无重复公共消息 |
| US7 | 非 TLS 远程端点必须失败（断言失败发生在任何业务请求之前）；证书不受默认系统信任链信任必须失败；TLS 通过但 home_id 不符 → `home_identity_mismatch` 且不发出任何写请求；TLS pinning 不在 B 包 | 浏览器路径不直连传输层；Client Host 配置错误时 UI 诚实报错，无降级回退 |
| US8 | 本机/SSH CLI 在既有 active Human 下签发的初始 Grant 能完成第一台远程 Client Host 绑定；exchange 成功响应携带 home_id 供首次核验与落绑定；后续新设备仍走已认证 Human 签发路径 | 第一台远程设备经 CLI 初始配对后，浏览器经 BFF 正常收发；后续设备走 US2 浏览器路径 |
| US3 补充 | 兑换遇 Core 内部故障（持久化失败等）→ 整体回滚、grant 不消耗、可重试；重试成功只产生一个 Session | — |
| 双启动（如实标注） | 同一备份副本启动两个相同 Home Identity 的可写 Core：**不属于本 P0 可证明事项**，不做检测/保证声明；「一 Home 一 Active Core」由部署/运维单写者不变量承担（§5 第 9 条） | 同左，不列入真实浏览器验收 |

约束：行为证明必须来自公开协议/真实进程黑盒，不得直读数据库冒充成功证据；真实双 Client 验收以两个独立 Host 进程（各自浏览器）连同一真实 Core 为准，浏览器网络只见本机同源 BFF/SSE，0 个 Core Bearer，0 控制台错误，拆除后无残留进程/端口。

## 8. 分三包实施

| 包 | 内容 | 依赖 | 边界 |
|---|---|---|---|
| **A：Core Home + Pairing 契约**（解冻切片，**已实现**） | Home 事实初始化与持久化；`/v1/home`、pairing-grants 签发/列表/撤销/兑换、sessions 按 id 列表/撤销；摘要存储、一次性消费、稳定错误码；`tests/home_access_blackbox.rs` 独立进程协议验收 | 无（复用既有 Human Session 语义） | 只新增事实与端点；不改既有冻结端点/表语义 |
| **A2：Pairing Transaction 可靠提交**（窄解冻，**已实现并验收**） | Host 预生成秘密并先落 pending；Core `prepare → status/commit/abort`；事务幂等、Grant 延后消费、Session token 原地激活、崩溃恢复与独立黑盒验收 | A | 只新增 `pairing_transactions` 事实与端点；不改既有 exchange、Human Session、Grant、Membership、Message 语义；不做通用 2PC 框架 |
| **B：Local Client Host 远程 Core 适配** | Host 配置扩展为「受管本地 Core」与「远程 Core 端点 + Home Identity」两种模式（远程模式不做进程编排，只做 Client 连接）；非 loopback 端点强制 HTTPS + 默认系统受信链（TLS pinning 不在 B 包）；绑定核验（mismatch fail closed）；A2 `prepare → pending store → commit` 可靠绑定与重启恢复；会话/换绑操作路径；远程模式将本机独立 Human Session 持久化于本机受保护 Secret Store（最低要求仅本机用户可读，浏览器绝不持有）**——该实现已于 2026-09-20 随 Host 全托管路径退役并整体删除（考古走 Git 历史）：本行的实现状态按历史读，B 包不再有可复现入口** | A2 + B 已接入并经真实 Core/Host 黑盒验证 | BFF 永远 loopback；不产生第二事实源；不改 Core |
| **C：Web 与真实验收** | Web 首绑/换绑/设备与 Session 管理 UI（只经 BFF 与 `src/api/` 契约层）；§7.2 真实双 Client 浏览器验收 | 包 A + B | 遵守前端边界：api 层先行、组件不直连 fetch、`npm run verify` 全绿 |

顺序：A → A2 → B（接入 A2）→ C，每包独立验收后再进下一包；任何包都不得反推修改冻结 Core 契约，A2 是主人批准的唯一窄解冻例外。

## 9. 已确认的 A 包默认

| # | 已确认项 | 采用的默认 |
|---|---|---|
| 1 | 是否批准「Home Identity + Pairing Grant + Session Metadata」为**窄解冻切片** | 批准；边界与 Agent Live State 先例同级：只新增事实与端点，不改任何冻结语义 |
| 2 | Home Identity 形态 | UUID v4 + 可选显示名；非秘密；初始化一次不可变 |
| 3 | 远程 Core 场景的首位 Human 与首台设备路径 | 首位 Human 仍经既有 operator CLI（SSH/本机）；第一台远程设备经仅本机/SSH 的初始配对 CLI 签发 Pairing Grant（与 HTTP 同一公共 pairing grant 事实/状态机，一次性输出原始 token）；普通后续配对仍由已认证 Human 签发；不做匿名自注册 |
| 4 | Grant TTL 默认值 | 15 分钟（1–60 可配置） |
| 5 | 换绑的服务端语义 | Client 端显式销毁本端绑定；服务端旧 Session 由主人手动撤销，不做级联自动撤销 |
| 6 | 远程端点传输要求 | 任何非 loopback 的 Core 链接必须 HTTPS + 默认系统受信链；仅进程间 loopback 可用 http；TLS pinning 不在 B 包；证书/端点变更须用户明确确认、不静默接受；Host 端点 allowlist + fail closed |
| 7 | 设备标签 | 兑换时可选提交设备名（非秘密，仅展示于 sessions 列表） |
| 8 | 包顺序与验收门 | A → B → C，每包独立验收后再进下一包 |
