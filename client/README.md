# Celira Web

Celira 前端：Vite + React + TypeScript + Tailwind v4 + shadcn/ui。视觉层 + Client 接口层。**启动的第一问是「连哪个服务器 + 你是谁」**：唯一分派点是 `src/api/entry.ts` 的 `decideFrontDoor(prefs)` —— 有可直接复用的直连凭据就直接连，否则**入口路径（`/`、`/splash`）的第一屏是登录页**（`LoginGate`：服务器地址 + token）。（TASK_64：启动探测 `detectHost` 已随 Host 模块一并拔除，不再有 Host 门。）另有 fixtures 驱动的**演示页面**（经右下角 `DemoSwitcher` 出入），它不是一种连接模式。

- **Host 模式（已退役）**：2026-09-20 删除刀 D 已把 Local Client Host 全线删除（`host/` crate ＋ 前端 `host*.ts`/`HostGate`/`CallOverlay` ＋ 启动探测），**本条目整段转为历史能力记录，代码已不存在**——同类能力现由下面的 core 直连承担。**以下原文整段收作历史记录（不再描述当前实现）**：由 Local Client Host 在规范同源（`http://localhost:<端口>`）托管的入口自动探测启用，是本机生产入口；同源 HttpOnly 会话 Cookie + CSRF 是唯一浏览器凭据，身份与房间经 Host 只读 BFF（`/api/v1/local/me`、`/api/v1/local/rooms`、`/api/v1/local/rooms/{id}/members`）读取真实 Core 公开契约视图，**聊天首屏/翻页、纯文本/附件消息发送（含 attachment-only）、显式提及（group Room 可选 `mentions` agent Participant ID 列表，原样透传）、回复（可选 `reply_to_message_id` 原样透传）、附件 multipart 上传与同源授权下载、作者撤回（`DELETE /api/v1/local/rooms/{id}/messages/{mid}` 窄代理 Core）、已读 cursor（`PUT /api/v1/local/rooms/{id}/read-cursor`）经消息 BFF 直读/直写真实 Core Message**；每页 `before`/`next_before` 是 **opaque cursor** 原样透传、绝不解析；发送走「每个逻辑发送一个内存 `Idempotency-Key`、网络/双击/重试复用同一 key」防重复公共消息，同一逻辑发送的 body + 回复目标 + mentions + 有序附件 ID + key 绑定在同一 pending 记录。**刷新与断开/重新上线恢复 = 重新读取 Core 首屏**；实时则经**固定无事实 SSE 唤醒**（`/api/v1/local/events/stream`，`event: events.available` + 空 payload，来源是 Core Human Event WS 的 `events.available`）驱动浏览器重读权威 Core 历史——**无 WebSocket 代理、无轮询、固定 SSE 唤醒不携带任何事件细节/消息正文/cursor 事实，浏览器只经 Host BFF 重读权威 Core 历史获得消息事实、无 Host 持久化消息缓存**。**Host 未读徽标始终来自 Core `rooms.read_state.unread_count`；只有 Host 模式下当前可见房间的权威历史已 `loaded` 且最新权威 Message 已进入一次 React commit 后，才用该 Message ID 推进 read cursor。后台唤醒只重读 history/rooms，hidden 标签不标记已读；成功后再权威重读 rooms，浏览器不本地清零，也不持久化第二份 read state。**撤回成功不伪造本地消息事实，只触发受 epoch 保护的权威首屏重读收敛 tombstone（tombstone/引用占位只来自 Core 权威历史）**。附件只在内存中保留服务端公开元数据与临时预览 URL，切换房间/卸载时回收 URL，不把 Blob、Cookie、CSRF 或幂等 key 写入持久层或 DOM。不接收、不存储任何 Core Bearer token。**Host/Web 不执行、不代理模型/Completion/ACK（无 token 流式）**，但 Runtime 写回 Core 的 **Agent 公共回复随唤醒重读自动出现并展示**；仍不实现 Provider 目录、多 Provider 管理、市场、自动发现与一般 Admin 代理；**BYOK P1 固定配置表单是唯一例外**：它只服务当前 Human 拥有且本机 Runtime 管理的 Agent，走两条固定 Host BFF 路由，绝不伪装为目录或通用代理。**Participant Presentation P0 已闭环**（CORE_V1_PARTICIPANT_PRESENTATION.md）：group Room 参与者面板展示 Agent 成员的群内展示名/角色名（`GET /api/v1/local/rooms/{id}/member-presentations` 摘要）；群主可编辑 active Agent 成员的本群介绍（`GET/PUT /api/v1/local/rooms/{id}/members/{pid}/presentation`）；「我的 Agent」细节页可查看/编辑全局公共资料（`GET/PUT /api/v1/local/agents/{id}/profile`），联系人资料卡对本人拥有的 Agent 作只读展示；PUT 只发 Cookie + CSRF + **浏览器侧每个「逻辑保存」的内存 Idempotency-Key**（双击/重试复用、成功或确定冲突后轮换），`base_version` 原样透传；`*stale` 409 给稳定文案并自动权威重读最新版本、绝不静默覆盖；编辑表单按契约做 UTF-8 字节上限/禁 NUL 前端校验；direct / human_direct 房间恒无 presentation 入口；浏览器/Host 不持久化任何 profile/presentation 副本，刷新即重读 Core。**启动探测完成前只显示诚实加载态（措辞不预设来源）；探测结束后一律按 `decideFrontDoor` 分派：可证明非 Host 的来源走登录页或直连 Core（见下），否则留在 HostGate 显示真实 Host 状态——本来源由 Host 服务时绝不回退 Mock。**（【TASK_64 失效】启动探测与 `HostGate` 已随删除刀 D 删除，这条分派只剩 `decideFrontDoor`。） Host 模式浏览器只拿 Host 会话 Cookie + CSRF + 安全视图；Core Human Session / Backend / Runtime Admin / Provider 凭据永不进入浏览器。**R7a：Agent 运行状态（只读，Host 版已随删除刀 D 移除）**——「我的」页有「Agent 运行状态」入口（`/app/runtime`），Host 版页面曾经 `GET /api/v1/local/runtime/agents` 只读 BFF 展示当前 Human **真实拥有**的每个 Agent 的执行状态投影：是否已被本机 Runtime 管理（`managed`/`unmanaged`/`runtime_unavailable`/`remote_unsupported`）、执行通道（lane）是否运行、凭据是否已配置/被阻挡、Provider 是否就绪、最近一次 Run 的非秘密摘要；Core id/display_name 是唯一显示源，绝不显示 system_prompt/owner_id/config_version/凭据原文/Provider Key 或任何 Runtime 内部配置正文；运行时显示 loading/error（可重试）/empty/未管理/不可用等诚实状态，**绝不用 fixtures 假装真实**。**Agent Create（P3，壳 Agent）已接通**——「新的朋友 → 添加 Agent」**在 core 直连下真实可用**（Host BFF 版 `POST /api/v1/local/agents` 已随删除刀 D 删除，现走 core 直连 `createAgent` → `POST /v1/agents`）：body 只收 `display_name`，浏览器侧每个「逻辑创建」的内存 `Idempotency-Key` 原样透传、成功或确定冲突后轮换；201 创建 / 200 同 key 重放）经 Core 创建公共身份且创建者即 Owner；新 Agent **无执行端为壳**，创建不要求选模型/Provider（属受管 Runtime 面）；创建后权威重读 owned agents 出现在「我的 Agent」；「未接入执行端，不会回复」诚实标注只由既有 R7a `managed` 投影机械推导（R7a 加载失败时保守不渲染，失败≠未接入），与壳 Agent 的 direct room 可开、消息可发（合法公共行为），房间内显示「该 Agent 尚未接入执行端，暂时不会回复」提示条而绝不伪造等待/回复状态；enrollment 泛化/Binding/Provider 接入不在本切片。
- **直连 Core（正门）**：粘贴 `celira-im create-human` 签发的 Bearer token 直连 Core（`LoginGate` 或设置页「连接设置」）。**服务器地址留空 = 同源**（页面自身来源的 `/v1`：Vite 代理或 `scripts/serve_client.mjs` 转发）；也可以填任意 `http(s)://主机:端口` 直连别的服务器——REST、附件上传/下载与两条 WebSocket 都跟着这个地址走。与 Host 模式凭据完全隔离，只存在 `celira.connection.prefs`（Host 模式及其浏览器会话凭据面已随 2026-09-20 删除刀 D 移除）。
- **演示页面**：fixtures + 本地模拟，陈列室 / 无后端演示用（不是连接模式，不经任何 Host/BFF 路径——该路径已删除）。

## 快速开始

```bash
npm install
npm run dev
```

打开后第一屏是**登录页**（`npm run dev` 下没有可复用的直连凭据，`decideFrontDoor` 如实判为登录页；Host 探测已拔除，不再有「非 Host 来源」这种判定）。

- **连真实服务端**：粘 `celira-im create-human` 签发的 token，服务器地址留空（= 同源，经 Vite 代理 `/v1` → `http://127.0.0.1:8080`）→「连接」→ 进值班室；
- **不连，只看界面**：点「不连接，先看演示界面」→ 陈列室；右下角 `DemoSwitcher` 可在「聊天 / 陈列室」间切换；主题切换在「我的」页或桌面图标栏底部。

**Host 模式已删除（2026-09-20 裁决，删除刀 D 执行）**：曾经这里写的是「起真实 `celira-im` /
`celira-runtime` / `celira-local-host` 三件套 + `CELIRA_LOCAL_CLIENT_WEB_DIST=client/dist`，
浏览器经一次性 fragment capability 进 `/splash`」。那条路**整条不存在了**——`host/` crate、
前端 `host*.ts` / `HostGate` / `CallOverlay` 及 14 个 `host-*.test.ts` 已从工作树删除
（考古走 Git 历史），启动探测（`detectHost` / `bootstrapHostSession`）已拔除。裁决与依据见
`docs/client/LOCAL_CLIENT_HOST.md` §0（该文按文档纪律保留为 **BFF 安全模式的历史权威记录**）。

**当前的家路径**：`node home.js up` 起 im + Agent + 生产页，浏览器进登录页**直连**（见根 `README.md`）。

## 数据架构

```
组件/页面（禁止直连 fetch）
   │
   ▼
src/api/store.ts（zustand：状态管理、乐观更新、事件恢复循环）
   │
   ├── src/api/host.ts  —— Host 传输层：同源会话恢复（/api/v1/local/session）
   │                        （me/rooms/members）+ 消息/附件闭环
   │                        （listMessages/loadOlder/sendMessage（含可选回复
<!-- ⚠ 2026-09-28：下面这张拓扑图是 **Host 时代（2026-09-20 已退役）**的历史记录，
     其中的 /api/v1/local/* 与 live.available 都已不在当前实现里——现行主体进行态见
     CORE_V1_AGENT_PROTOCOL.md §6 与 AGENTS.md「主体进行态边界」。 -->
   │                        目标、显式 mentions 与有序 attachment IDs）/
   │                        uploadAttachment/fetchAttachmentBlob/recallMessage（作者撤回
   │                        DELETE，无 body/无幂等 key），opaque 分页 +
   │                        内存幂等 key）+ Participant Presentation 闭环
   │                        （getMemberPresentations 成员群内展示名/角色摘要、
   │                        getRoomPresentation/putRoomPresentation 群主编辑 Agent
   │                        本群介绍、getAgentProfile/putAgentProfile Owner 编辑
   │                        全局资料；PUT 只发 Cookie + CSRF + 浏览器侧内存
   │                        Idempotency-Key 原样透传，base_version 绝不在前端裁决）
   │                        + 按需真实轨迹 Live
   │                        （listRuntimeAgents：/api/v1/local/runtime/agents
│                        Core 归属 + Runtime scoped status 脱敏白名单）
│                        （getRoomLive 只读 /api/v1/local/rooms/{id}/live，
   │                        subscribeHostLiveWakeup 同源 /api/v1/local/live/stream
   │                        只订阅无事实 live.available——仅唤醒、真相 REST 重读）
   │                        + 通讯录读写闭环（listContacts 只读投影
   │                        /api/v1/local/agent-service/contacts；sendContactRequest
   │                        POST /api/v1/local/contacts/requests——Cookie + CSRF +
   │                        浏览器侧按「每个逻辑发送」生成的内存 Idempotency-Key
   │                        原样透传，成功或确定冲突后轮换；acceptContact
   │                        POST /api/v1/local/contacts/{human_id}/accept——无 body、
   │                        无 Idempotency-Key，unknown/非 addressee 统一 404
   │                        contact_not_found 存在性隐藏；写后权威重读联系人列表）
   │                        + 无事实 SSE 唤醒（subscribeHostWakeup：
   │                        events.available 只表达唤醒、驱动既有历史重读）；
   │                        事件事实等其余能力显式不可用；read state 由 Host
   │                        coordinator 经 bounded retry 与 rooms 权威回读闭环；
   │                        不持有任何 token，发送不带 Authorization
   ├── src/api/hostMessages.ts —— Host 消息数据面纯逻辑（in-flight 去重、
   │                        id 去重翻页合并、内存幂等 key 与逻辑发送重试、
   │                        fail-closed 结果应用；hostSessionFence 会话 epoch
   │                        弃旧代写 + WakeupRefetchCoordinator 唤醒去抖/合并/
   │                        等待在途首屏后权威重读 + online 补读）+ 按需真实轨迹
   │                        （HostLiveCoordinator：打开才订阅/单流/去抖合并/
   │                        epoch+exact-api 所有权/关闭清理；projectHostLive
   │                        只投影 phase 与 responding 允许草稿）：无轮询、无缓存
   ├── src/api/hostUi.ts  —— Host UI 策略纯逻辑（纯文本 + group Room 显式提及
   │                        建议源过滤/本切片不生成模型回复/扩展动作阻断；
   │                        稳定错误文案；永不含 key）
   ├── src/api/core.ts  —— 真实 Core HTTP + WS（Bearer + Idempotency-Key，经 Vite proxy 转发 /v1）
   ├── src/api/homeStatus.ts —— 本机只读状态口读取层（`http://127.0.0.1:5190`；**不是 Core API**、
   │                        不经 `getApi()`、不随连接模式切换）。形状认不出即判"读不到"（fail-closed），
   │                        绝不半读半猜；只给运行状态页用
   ├── src/api/homeConfig.ts —— 本机**模型配置门**读写层（`http://127.0.0.1:5191`；TASK_77；刀2b 起管"清单 + 绑定"）。
   │                        与 homeStatus 同一档：**不是 Core API**、不经 `getApi()`、不随连接模式切换、
   │                        形状认不出即判"读不到"；另加两条是"写"带来的——错误码是**封闭表**
   │                        （与门侧逐字对齐，两侧有对表测试）与**凭据不过手**（门永不回 key）；
   │                        刀2b 起读的是**三合一**（清单 + 绑定 + 派生），写有四个动作各走各的路
   │                        （新建 = 保存即启用 / 改一条 = 不动绑定 / 换绑 = `/home-binding` /
   │                        删一条 = 销毁式）；给模型配置页与「模型」房间用
   └── src/api/mock.ts  —— mock 兜底：回复模拟、流式增量、置顶/免打扰/删除会话、ACK 名单、thinking 指示
   │
   ├── src/api/contracts.ts —— Core 契约类型（与 CORE_V1_IM_CONTRACT.md / CORE_V1_AGENT_PROTOCOL.md 对齐）
   ├── src/api/broadcast.ts —— 多标签页脏标记广播（BroadcastChannel，仅 core 模式）
   ├── src/api/idb.ts        —— IndexedDB 消息/房间缓存（token 隔离，仅 core 模式）
   ├── src/hooks/use-notifications.ts —— 浏览器通知授权与聚合
   └── src/lib/agent-days.ts ＋ src/lib/plain-language.ts —— **呈现层纯逻辑**（刀2a / 人话刀）：
                           前者把状态口那份机械读数按「**一次活动**」分组成时光线（缺什么就缺省，
                           绝不拿别的字段顶）；后者把后端给的机械取值换成**用户读得懂的话**
                           （认不出的取值**原样交出来**，不编词）。两者都是纯函数，各有单测
```

> ⚠ **树中三个 Host 节点已删除（2026-09-20 删除刀 D）**：`src/api/host.ts`（Host 传输层）、
> `src/api/hostMessages.ts`（Host 消息数据面纯逻辑）、`src/api/hostUi.ts`（Host UI 策略）连同
> 14 个 `host-*.test.ts` 已随 Local Client Host 全线移除；上面树里保留的条目文字是**历史记录**，
> 权威正文见 `docs/client/LOCAL_CLIENT_HOST.md`。真实世界从此只有 **core 直连**一条路径。

能力边界：**Core 契约未实现的功能一律不进 mock 之外的真实路径**——流式回复、Markdown 渲染是显示层能力；`body` 保持纯文本契约，MD 只在渲染时生效；真实 Core 的消息写回是原子的（无流式；旧线的 Completion 写回已退役）。**Host 模式（已退役：2026-09-20 删除刀 D 已删 `/api/v1/local/*` BFF 与前端 Host 模块，下列整段为该路径的历史能力记录；同类能力现由 core 直连的 `src/api/core.ts` 承担）**：身份/房间只读 + **消息/附件闭环**（首屏真实 Core 历史、显式「加载更早」（仅当 `next_before` 存在；并发防抖；滚动锚定）、纯文本或 attachment-only 发送 pending/sending/failed + 以原幂等 key 重试、multipart 上传返回服务端公开 metadata、附件图片经授权 Blob 预览/文件经同源授权 Blob 下载且 object URL 回收、**显式提及**（group Room 输入区 `@` 建议源 = `GET /api/v1/local/rooms/{id}/members` 真实成员投影，仅列 agent；选择后以结构化 `mentions` 随消息原样透传，渲染层按 `mentions` 高亮 `@名字`；direct / human_direct 不识别 `@`）、**回复**（可选 `reply_to_message_id` 原样透传，`ReplyQuote` 从已加载权威 timeline 解析目标；目标未加载时按 opaque `before` 分页补早；目标已撤回只显示 `[该消息已撤回]`，绝不泄露撤回前正文）、**作者撤回**（`store.recall` 成功后不伪造本地消息事实，触发受 epoch 保护的权威首屏重读收敛 tombstone；撤回后的 tombstone/引用占位只来自 Core 权威历史））+ **固定无事实 SSE 唤醒**（收到 `events.available` 后重读已进入房间的权威 Core 首屏、含断线/重新上线补读；固定 SSE 唤醒不携带任何事件细节/消息正文/cursor 事实，浏览器只经 Host BFF 重读权威 Core 历史获得消息事实、无轮询、无持久化缓存）；**read state 已闭环**：Core `rooms.read_state.unread_count` 是唯一未读权威，只有 visible + loaded + rendered 的当前房间最新权威 Message 才触发 `PUT`，PUT 与成功后的 rooms 权威回读作为一个整体，生产最多首次加两次 bounded retry；回读成功后才算 advanced，delivered 且 recalled 的 Core tombstone 也可推进 cursor；浏览器不本地清零、不持久化第二份 read state。Runtime 写回 Core 的 Agent 公共回复会自动出现；**BYOK P1 已接入**：设置 →「为我的本机 Agent 配置模型」只展示当前 Human 拥有且由受管本机 Runtime 管理的 Agent，允许一次性保存 OpenAI-compatible 地址、模型 ID 与 API Key，并真实检查连接。Key 只存在于 React 输入和一次 Cookie+CSRF 保护的同源 Host BFF 请求中，settle 后即清空；Host 不持久化 Key、不调用模型，Runtime 加密建立新的 Provider revision，且只影响之后新建立的 Run。**Host 房间管理（P2）已接入**：经 Host BFF 白名单端点真实闭环——建群（`POST /api/v1/local/groups`，群名 1..64、浏览器按每个逻辑建群生成内存 Idempotency-Key 并复用/轮换，写后权威重读 rooms）、群主邀请已接受联系人/自有 Agent 进群（`POST .../members` 带 key；候选源只来自 contacts accepted + owned agents 投影，绝不回退 fixtures）、群主移除成员（`DELETE .../members/{pid}` 无 key）、成员退群`POST .../leave`、群主转让`POST .../owner-transfer`（带 key）；成员/权限/幂等裁决全部在 Core，UI 只按 members 投影的 role 控制入口可见性，冲突时如实呈现 Core 稳定错误文案；group 房在 Host 模式同样可编辑 Agent 成员群内介绍（P0 Presentation 链路闭环）；direct/human_direct 房不显示邀请/转让入口。**Host 服务面（P2，已随 Host 删除而移除）**：`/app/services` 曾离开 fixture 守卫（`FixtureRouteGuard`，TASK_56 前叫 HostRouteGuard），publication/accesses/开服务会话真实可用；`/app/discover` 在真实世界由 `FixtureRouteGuard` 诚实拦住——Core v1 无 Directory/发现契约（该守卫判的是「是不是真实世界」，**不是**「是不是 Host」；TASK_56 之前判反过，core 直连的真实用户曾在「发现」看到假居民）。测试不写 Core Message、Run、Session、Completion 或 ACK。仍不实现 Provider 目录/市场/自动发现、多 Provider 管理、图片生成或一般 Admin 代理；fixtures 绝不回填 Host 模式的身份/房间/消息。Host 模式对消息与 key 不写 localStorage/sessionStorage/IndexedDB。**Host 按需真实轨迹（Live，已随 Host 删除而移除）**：含真实 Agent 的房间提供「运行轨迹」入口，**用户不打开时不建 Live EventSource、不轮询、不主动读取 Live**；打开时才 `GET /api/v1/local/rooms/{id}/live` 读一次权威快照并订阅同源 `GET /api/v1/local/live/stream`（固定无事实 `live.available`，收到后 REST 重读当前房），关闭/换房/断开/会话 epoch 换代立即停流并清理旧房投影；同一时刻至多一条 Host live EventSource，重复/慢唤醒去抖合并；面板只显示「当前执行」卡片（Agent 名 + phase + responding 允许正文草稿），activity 清除后显示「当前没有运行中的轨迹」，绝不生成/展示 thinking 正文、tool 名称/参数/结果、system prompt 或 provider 信息，也绝不复用/渲染 mock 虚构 steps；最终回答仍只由 Core 消息流出现且恰一次。**Host 通话（Call，已随 Host 删除而移除）**（CORE_V1_CALL.md 窄解冻切片）：2 人房间（有 peer）头部语音/视频按钮经 `POST /api/v1/local/calls` 发起，动作（`GET/POST /api/v1/local/calls/{id}[/accept|decline|hangup|signal]`）走 Host BFF 白名单代理；实时信令经同源 `GET /api/v1/local/call/stream`（**带事实** SSE：`call.ring`/`call.signal`/`call.state` 含 SDP/ICE）订阅；**媒体由两端浏览器 WebRTC P2P 直连，Host/Core 不接触媒体字节**（不采集/不转发/不存储）；未开通场景（群聊/非 host 模式）按钮诚实标注「尚未开放」，绝不伪装支持。（⚠ 文案已从「即将开放」改为「尚未开放」——本仓有措辞纪律「**不承诺"即将提供"**」（见 `FixturePageUnavailable`）："尚未"是当下的事实、"即将"是承诺。组件在 `ChatHeader.tsx` 的 `showCallHint`。）

## 页面地图

> ★ **runtime 平面只做三件事**（2026-10-08 主人裁决：*"runtime agent 很多时候只是一次性配置那种，
> 很少使用，常用的是 im 的部分……一些不是很重要的配置，我完全可以让 agent 帮我配置"*）：
> ① **必须由人同意**的（首次跑世界、模型 key、凭据 / 撤销）；② **看得见**（在不在跑、几支、出没出事）；
> ③ **诊断出口**（日志、活动、手动重试）。
> **其余旋钮一律不做 UI**：要么交给 agent（它手上有 `manual.md` ＋ 终端那只手，文件级配置它自己能改），
> 要么留在 CLI。判据一句话：**这一格是"只有人能决定的事"，还是"agent 自己能改的文件"？** 后者不做 UI。
> ⇒ 由此**明确不做**的：机器页按住户展开、住户选择器、"多支细分"那一类形态（本页与
> `/app/runtimes` 保持"一台机器一行"）；多支只在**读数如实**这一条上处理（见下一段与
> `/app/agents/:participantId` 那一行）。

> ★ **界面语言里的产品词是「机器」**（2026-10-20 主人裁决，T5 命名统一）：屏上只说**机器**
> （这台机器 / 我的机器）与**Agent**；`执行端` 是**平面名**（`docs/runtime/AGENT_VOCABULARY.md`：
> "本节的名字全部属于执行端平面"），**只在机器清单页那句定义里出现一次**（"每台机器上跑着
> Agent 的执行端"）。`执行点` 是它的架构词（`AGENTS.md`：承载执行端的机器或部署位置），不上屏。
> 同理 **`家` 退出界面语言**：表单里是**机器地址 / 机器钥匙**（架构与 CLI 仍叫
> `home` / `issue-home-key` —— 屏幕词与它各归一处，映射只在那条帮助句里）。
> 撤销那一族也拆成三个词：**解除接入 / 作废凭证 / 撤销 Agent**。

| 路由 | 页面 | 说明 |
|------|------|------|
| `/` | —（正门） | 根路径 ⇒ **正门**：有可复用凭据就直接连并落 `/app/chat`；没有就落品牌启动页 `/splash`（而入口路径上的第一屏由 `RootLayout` 换成登录页）。⚠ **2026-09-29 真机实测修**：这里原先是**无条件** `<Navigate to="/splash" replace />`，与 `resolveFirstScreen` 的 `navigate('/app/chat')` 争同一个落点 ⇒ 登录成功后**永远停在品牌页**，而那颗「进入」正是 `navigate('/')` ⇒ 点回去又被送回 `/splash`（闭环）；只有手敲 `/app/chat` 才进得去 |
| `/splash` | SplashPage | 品牌启动页（「进入」回正门 `/`） |
| `/app` | —（布局） | 壳层布局路由（`AppShell`：导航栏 + TabBar + 出口），无自己的内容面 |
| `/app/chat`、`/app/chat/:roomId` | ChatPage | 消息模块（壳内，房间可深链；虚拟化消息列表） |
| `/app/chat/:roomId/history` | ChatHistoryPage | 查找聊天记录（关键词/类型筛选 + 定位跳回） |
| `/app/contacts`、`/app/contacts/:participantId` | ContactsPage | 联系人（分组列表 + 资料卡） |
| `/app/contacts/new` | NewContactPage | 新的朋友：**一个输入框**——贴对方的身份 ID（`participant_id`，Human 与 Agent **同一串**；契约 `CORE_V1_CONTACT_REVERSIBILITY.md` §5.2），发一段关系申请。落定后按**回执**分四种处境说（刚发出 / 之前发过 / **对方先发过、该我点头** / 早就是联系人，判据 `api/uiPolicy.ts` 的 `classifyContactRequest`），并对 pending 给「撤回」。⚠ **这一页不创建 Agent**：创建是**所有权**、不是关系，唯一入口是 `/app/agents` 的「添加」 |
| ~~`/app/services`~~ | ~~MyServicesPage~~ | **已删除（2026-09-28，Agent 服务层退役）**：那一页是「我的服务」（已获服务访问与开服务会话，`src/api/service.ts`）。服务层（Publication / Service Access / 服务房）整体退役后，页面、`ServiceApi` 与协议面一并下线；路由级"不许回来"由 `tests/route-admission.test.ts` 钉住 |
| `/app/discover` | DiscoverPage | 发现（官方 Agent + 外部接入口；真实世界由 `FixtureRouteGuard` 诚实拦住——Core 无 Directory 契约） |
| `/app/agents` | MyAgentsPage（`AgentsListPanel`） | **我的 Agent——列表**（`side: 'home'`：**Agent 索引的执行侧视图**，每行拼世界侧所有权 + 家侧**那一句 ＋ 三格读数**）。⚠ **2026-10-06 起那一句在最前**：宿主进程在不在跑 ＋ 上次怎么样（判据与文案在 `api/agentHealth.ts`，纯函数 ＋ 离线用例）。⚠ 它与 IM 里的「在线」**不是一件事**（那一个是连接派生的易失投影，这一个读的是机器侧的宿主进程）——两个平面不许互相冒充，因此**分开摆**。点一个 Agent 进 Agent 的主页。⚠ **2026-09-29 起它是壳层「执行端」组的第二个一级入口**——在那之前它**不在任何分区里**（只从「我的」页与「新的朋友」页进得去），一级内容拿二级待遇。★ **2026-10-20（T1「创建 → 它回第一句话」主路）**：空态与创建面板**顶部**各摆一份**三步清单**（模型源 / 建一支 / 接上一台机器），每步一个 ✓/✗ ＋ 一个能走的出口；判据全在 `api/firstRun.ts`（纯函数、有 `tests/first-run.test.ts`），页面只渲染 —— **零新请求、零新持久面**（状态只取 `useHomeEntries` 的配置读数 × `resolveAgentHome` × `anjiaByAgent`）。缺模型源那一步**在创建之前**就说，且自带出口「去配模型源」（与 `missing-model-credential` 那句拒绝同一个去处）；没有壳时「接上」那一步给**深链** `/app/agents/<id>#machine`（那一段本来就能走远程家，**不复制**接上的写入口），一条机器都没有时才说前提（要一台装着 Celira 的机器 ＋ 家地址/家钥匙，出口「我的执行端」）。★ **同日起那一行的话**：住址那一格只说住址（「在这台机器上 / 在<机器名>上 / 不在这台机器上 / 不知道在哪台机器上」—— 不再借「运行」两个字），**一行只有一个状态点**且只跟 `agentHealth` 的色档走；一份读数都没读回来时**四格同因收成一句**（原因 ＋ 出口「去我的执行端」，判据 `rowSameCauseLine`） |
| `/app/agents/:participantId` | MyAgentsPage（`AgentHomePage`） | **两段**（前者为主体，`side: 'world'`）：①「**资料**」（所有权 / 头像 / 公开资料 / 接入 / 凭据 / 撤销，数据源 core 直连），锚点 `#who`；②「**机器**」（`#machine`）—— **主语是机器**：读家侧裁决（`resolveAgentHome`）说"**它住在哪台**"并给去 `/app/runtimes/:entryId` 的出口，四态（正在读 / 说得出住址 / 说不准 / 能读到的执行端都没报它 / 读不到）在 `api/agentMachine.ts`（纯逻辑，有测试）。★ **2026-10-08 加住户清单那一条判据**：一个家可以住**多支**（`PROJECT_SHAPE.md` §8.0），而口子把**第一支**抬到顶层（`agent` / `lastWake` / `host`）⇒ 只比顶层那一格的话，**第 2、3 支会被写成「还没有机器在跑它」——假话**（它们就在这台机器上跑着，只是各住一个环境根）。⚠ **那句话的措辞已在 2026-10-11 改成带作用域的**「能读到的执行端都没有报它在跑。」（`api/agentMachine.ts` 文件头：不带作用域就是把"我没看见"说成"没有"——边界是**两个可替换的平面**）。现在 `claimEntry` **两个判据、顺序有意**：先比顶层那一格（单支的家与从前一字不变），再比**住户清单** `residents[]`；并且裁决多出一格 **`ownReadings`** —— 认领靠清单时**住址对、读数不是它的**（顶层那几格是第一支的）⇒ 账本 / 最近活动 / 经历一律**不给**，宿主那一格读**它自己那一项**（`claimedHostOf`，逐支判：一支一个进程）。⚠ **同刀补的一个欠账**：顶层 `host` 那一格**两个门从前都没给过**（它只在每支那一份里）⇒ `agentHealthReadout` 的「机器在跑 / 机器没在跑」在真机上**永远只会说"读不到"**（没有生产者）；这一刀在两个门（`scripts/home/status-server.js` ＋ `app/src-tauri/src/home_doors.rs` 的 `host_view`）都补上，词表必须同形（`running` / `running-orphan` / `stopped` / `unknown`）。⚠ **2026-10-06 加第二句**：裁决过"这台机器跑的就是它"（`claimed`）时，那一段多一句**"机器在不在跑 ＋ 上次怎么样"**（`api/agentHealth.ts` 的 `agentHealthReadout`——**与「我的 Agent」列表那一行是同一个函数、逐字同一句话**，色档映射共用 `components/celira/status-tone.ts` 的 `READOUT_TONE_TEXT`）。⚠ **归属判据**：`unknown-id` / `elsewhere` / `unreachable` 三档**不给**这一句——那台的账本读数未必是这一支的，摆上来就是张冠李戴（`verify-ui.mjs` 有正反两条断言）。⚠ **2026-09-29 补第二段的理由**：在此之前这一页是个**死胡同**（出口只有"返回"与"撤销"），而列表行上摆着三格读数、点进去一个字都不说——要问"它现在在跑吗"得自己想起来去「我的执行端」从机器那头找。⚠ 这一页**不复述**运行状态**明细**：运行 / 活动 / 模型三间的唯一权威是机器页（**逐次活动与系统分类只有那一页有**；本页那一句是汇总，与列表同一句）。地址里点名了哪个 Agent、页面就说哪个 Agent（认不出时如实说「没有这个 Agent」，**不许回落成列表里第一个**）。⚠ 原先的「家 / 日子 / 脑子」三间读的是**机器级事实**，已搬到 `/app/runtimes/:entryId`（轴与事实必须一致）；「**服务**」那一间（`AgentServicePanel`）已随 Agent 服务层退役删除（2026-09-28）★ **2026-10-20（T3）**：已撤销的（`agent.revoked`）**只渲染页头 ＋ 那一句后果 ＋ 返回** —— 接入 / 凭证 / 撤销 / 接上 / 模型**一件都不渲染**（它们一件也做不成：软撤销之后世界那一侧不再接受它；判据 `api/agentRevoked.ts`，与列表那一行**同一个常量**）。★ 同日起「模型」那一格「还没有配好的钥匙」**带出口**（去那台机器的「模型」那一间 `#model`，按钮文案与列表行同一句「去配模型源」）★ **2026-10-20（T4：这一页从"管理台"改成"对象首页"）**：四段、顺序有判据 —— ①「机器」（`#machine`，首屏那句 health 的**唯一出处**）②「资料」（`#who`，所有权 ＋ 公开资料只读）③「接线（高级）」（`#wiring`：接入 ＋ 凭据，**默认折叠、折叠头带读数、没有生效的接入时默认展开** —— 2026-10-08 裁决第①类"必须由人同意的"必须在，但不必在最前）④「危险操作」（`#danger`：撤销 Agent，不可恢复 ⇒ 页尾）。首屏两个动作都在页头：**发消息**（判据在 `api/directRoom.ts`，与 `/agent/:id` 那颗"发消息"同一处；写入口仍只有 store 的 `createDirectRoom`）与**在世界里看**（去 `/agent/:id` —— 那一页才是"它在世界里的样子"的唯一版面，这一页不重画）。撤销那一族拆成三个词：**解除接入 / 作废凭证 / 撤销 Agent**（一个动词一件事） |
| `/app/runtimes` | RuntimesPage | **我的机器——机器列表**（2026-09-25 组织轴刀）：一台机器一行（名字 + 地址 + **上面跑着谁** + 那一格的处境）。处境**四态不压成两态**：正在读 / 读不到（固定人话）/ 它没报上上面是谁 / 上面那支不在这个世界里。本机那条是**预置**的（钥匙为空、走同源代转），排第一条、删不得 |
| `/app/runtimes/:entryId` | RuntimesPage（`RuntimeDetailPage`） | **一台机器那一页**：机器卡（这台是什么 + 上面跑着谁，点名可跳 Agent 页）＋ 三个房间 —— 「**运行**」（本机只读状态口：在不在跑 / 能不能开始新活动 / 醒过几次 / 最近一次活动 / 接下来；⚠ **"这台机器上还没有 Agent"** 与"读不到（缺件）"是**两态**，2026-10-05 起分开）、「**活动**」（同一份读数按**一次清醒（Run）**重排成时光线，裸行降为每次可展开的详情 —— 只给动作与结果状态，**不给正文与参数值**。⚠ **app 形态**里壳不端 `experience` 那一格 ⇒ 这一间只排到"每一次怎么收场"，明细那半**如实说"这台机器没给"**而不是编成"没有动手"，见 `app/README.md` §4.2.2）、「**模型**」（那台机器那一格模型配置的**读数 + 真动作**：正在用哪一条（从清单与绑定算出来）、这台机器上还有哪几条、**换成这条** / **解除指定**；只显示"配没配"，**任何密钥内容都不回浏览器**）。锚点 `#run` / `#activity` / `#model`。★ **2026-10-11 第一次那条路**：这台机器上**还没有模型源**时，三个房间**前面**多一张卡（「先给这台机器配一条模型源」＋ 出口「去配置模型」与「我的 Agent」）—— 在场判据是 `modelSourceAbsent`（**读不到不摆**），配好自然消失、零新状态；依据一句话：**动作排在读数之前**（三个房间回答"现在怎么样"，而第一次来的人要的是"我该做什么"）。⚠ 同刀把"锚点没生效"那条疑案**量清了**：真机上容器**每次都滚到上限**（171/171、348/348、768/768），「模型」是**最后一间**、顶不到最上面是物理事实 ⇒ 正解不是把滚动做花，而是把动作搬上去。⚠ ~~壳层「执行端」组只有一个入口~~（**2026-09-29 已改**：该组现在是**两个**入口 —— 我的执行端 · 我的 Agent）；看一个 Agent 的运行/模型有**两条路**：先有机器（机器列表 → 那台机器），或者从 Agent 那一页的「机器」那一段跳过来（`/app/agents/:id#machine`） |
| `/app/runtime` | —（重定向） | 旧「运行状态」页 ⇒ 2026-09-25 起**重定向**到 `/app/runtimes`（机器列表；**不猜是哪台**）。**保留路由**：深链接/书签不破、不 404。⚠ 2026-09-26 修：重定向**不带 hash** —— 列表页没有房间可滚；旧锚点 `#home` 只在**带 entryId 的地址**上有效（`/app/runtimes/<entryId>#home`，由 `RuntimeDetail` 映射到 `#run`） |
| `/app/experience` | —（重定向） | 旧「Agent 的经历」页 ⇒ 2026-09-25 起**重定向**到 `/app/runtimes`。同上：保留路由、不 404、不带 hash；旧锚点 `#days` 在带 entryId 的地址上映射到 `#activity` |
| `/app/profile` | ProfilePage | 我的（身份卡 + Agent 配置 + 外观） |
| `/settings` | SettingsPage | 设置首页：通向下面各分区的索引（index 重定向到 `/settings/general`） |
| `/settings/general` | SettingsGeneralPage | 通用：连接设置（服务器地址 + token）与退出登录 |
| `/settings/appearance` | SettingsAppearancePage | 外观：主题、配色变体、字号、壁纸 |
**通知与角标（2026-10-08 登记，含一条"走不通"的精确理由）**：

- **系统通知**已做（壳里走原生 toast，浏览器走 Web 通知）。两处已知边界：壳里**点通知不跳房**、
  也不做"替换上一条"（聚合标题里带「（N 条新消息）」代替）。⚠ **"点通知跳房"在桌面端走不通，
  不是没做**：`tauri-plugin-notification` 的 `desktop.rs` 明文写着桌面端**只用 title/body/icon/sound**，
  `scheduling / grouping / action` 相关选项**一律忽略**（2.5.0 源码原文）⇒ 通知带不出"点了要跳哪"。
  要真做得自己造 toast（WinRT/各平台）＋ 深链协议，属另立项，别顺手加。
- **未读角标**已做：页面把 `rooms[].unreadCount` **求和成一个数**报给壳（`api/unreadBadge.ts` →
  壳命令 `set_unread_total`），壳画在**窗口外** —— 托盘悬停那句（`Celira —— … · N 条未读`，
  `99+` 封顶）＋ Windows 任务栏小红点／macOS·Linux 系统角标（`app/src-tauri/src/unread.rs`，
  红点在代码里画、不引资源）。⚠ 页面报的只是**一个总数**，壳不解释也不自己算（read state 仍是唯一权威）；
  命令**返回拼好的那句悬停文案** ⇒ 真机探针断言它，而不是去截屏看任务栏。

| `/settings/notifications` | SettingsNotificationsPage | 通知：浏览器通知授权与提及提醒开关 |
| `/settings/server` | SettingsServerPage | 本机服务：这台机器上的进程与本机记录（只读）；⚠ 地址仍是旧名，**这一页不再是"服务器"**（世界侧那两样在「通用」） |
| `/settings/homes` | SettingsHomesPage | 旧「家」地址 ⇒ 重定向到「我的机器」（这台浏览器记得的机器：本机 + 远程条目 + 那把钥匙） |
| `/settings/about` | SettingsAboutPage | 关于 Celira |
| `/app/runtimes/:entryId/config` | ModelProvidersPage | **某一台机器的模型配置**（TASK_77 重写；刀2b 升成**清单管理**；★ **2026-10-11 按机器寻址**）：读那台机器的**模型配置门**（`node home.js doors` 的写端面 `/home-config`）的三合一（清单 / 绑定 / 派生），管的是**这台机器上的钥匙**（端点 / 密钥 / 模型名 / 上下文长度 / **这个模型能不能看图** + 可选接口类型）——**「这一个 Agent 用哪一条」是另一件事**（在机器页「模型」房间里换；原理：**钥匙串归房子，用哪把钥匙归住户**）。★ **「这个模型能不能看图」是这台机器的事实**（2026-10-17）：它写进清单第七格 `vision`，**不勾就是不支持**（fail-closed），同一台机器上的住户共用这一格 ⇒ 不勾的后果**写在勾旁边**（读附件时图片会被拒 `unsupported-content-type`，能读的类型清单里也不会列出那四种媒体类型）——本仓纪律「人不许比系统知道得更少」。⚠ 它是**三态**：**没说**（老页面）⇒ 门从旧条目**原样带过来**；`true` / `false` ⇒ 以它为准 —— 压成两态会让"明确关掉"那一态**消失且没有任何报错**（判据在 `api/homeConfig.ts` 的 `declarationBody`，门与页面两侧各有一条用例）。⚠ **它随模型走**：换型号那一刻不许沿用旧值 —— 走**模型目录**换（点一个型号）那条路**不走草稿**，由 `modelChangeDraft` 显式把它拨回"不支持"并请人重新勾（有离线用例），写完之后型号 / 窗口 / 这一格一起**拨回表单**（界面 = 落盘）；**手填型号名**那条路所见即所存，补的是一句条件提示（型号名跟已存的不一样、且这一格正说"能"时才出现）。⚠ 门落盘时总把这一格规范化成**显式布尔**（`writeList`）⇒「没说」只是**线上**的一态（它的语义是"从旧条目带过来"），文件里从来没有过第三态。加一条 = 进清单**并指定给 Agent**；改一条 = 只动那一条；删一条 = **销毁式**（那条里的密钥只有本机这一份，正被指定的删不掉），与**解除指定**（撤销式）分开说。保存后门替我们连一次（**试连失败不拦保存**，读数如实显示），并把它顺手读回的**模型目录**摆出来 —— 点一个就换那个模型名（上下文长度会重问）。它**不再**是 fixtures 驱动的供应商目录（那两个组件 `ProviderListItem` / `ProviderModelsSheet` 已随刀删除）⇒ 因此也**不再挂在 `FixtureRouteGuard` 之下**：非真实世界由页面自身按 `isRealWorldSession` 处理（读显示照实、**写动作关着**）。门没开时如实说读不到，不画成一份空配置。★ **为什么地址里有机器**：一个"配某台机器"的页面住在没有机器上下文的地址里，就只能自己长一行"配哪台机器"的选择器（**同一个问题两个答案**）⇒ 现在**地址就是那台机器**，那一行随之退场；**认不出那台机器就如实说"没有这台执行端"，绝不回落本机**（旧实现是 `?? localHomeEntry()`，会默默去配本机）。**旧地址 `/settings/providers` 保留成别名**：带 `?home=<id>` 落那台、没有上下文落执行端列表（**不猜**） |
| ~~`/settings/providers`~~ | —（重定向） | 旧「模型配置」地址 ⇒ 2026-10-11 起是**别名**：带 `?home=<id>` ⇒ `/app/runtimes/<id>/config`；没有上下文 ⇒ `/app/runtimes`（**不猜是哪台**，与 `/app/runtime` 同款）。设置目录里**不再有它**（入口归机器页的「模型」间） |
| `/agent/:participantId` | AgentPage | **世界侧的参与者页**（`side: 'world'`）：公开资料 + 发消息。它是不是你的 Agent / 你的联系人**各有其页**（`/app/agents/:id` 管接入与凭据、`/app/contacts/:id` 是资料卡），三页**互链不合并**。⚠ 2026-09-26 前它整页在 `FixtureRouteGuard` 之下，而真实世界有三处入口链到它（消息头像 / 右键「查看主页」/ 联系人资料卡）⇒ 真实用户点得到、点进去撞门；那一刀把它重写成只读 core 的真页并搬出守卫 |
| `/gallery` | GalleryPage | 组件陈列室（参照标准） |
| `*` | NotFoundPage | 未找到 |

## 壳层导航分区（上组=世界 / 下组=执行端）

导航是**分区**而不是一排平级 tab：**连线归世界，生活归执行端**。

| 分区 | 语义 | 入口 |
|------|------|------|
| 上组「世界」 | Celira IM = 通讯产品 | 消息 · 联系人 · 发现（⚠ 真实世界下**不摆**，见下） |
| 下组「机器」 | Agent 产品 = 用户侧系统 | 我的机器 · 我的 Agent（**两个入口**，2026-09-29 起） |

- **2026-10-03 死入口刀**：真实世界里导航**不摆"点进去必然撞门"的入口** ——
  `navGroupsFor(realWorld)` 按路由表自己的判据（`routeRegistry.reachableInRealWorld`；
  `demo` 是唯一"真实世界进不去"的侧）把那些入口摘掉，当下就是「发现」
  （`/app/discover`，`side: 'demo'`：Core 无 Directory 契约 ⇒ 点进去只有"尚未提供"）。
  依据是 routeRegistry 自己写的那条纪律：**任何"真实用户点得到、点进去却撞门"的入口都是缺陷**。
  ⚠ 这是**派生不是第二份清单**：契约落地后那条路由的 `side` 一改，入口自动回来；
  `client/scripts/verify-ui.mjs` 的 `EXPECTED_NAV`（它跑在真实世界）与
  `tests/nav-groups.test.ts` 那把锁必须一起动（锁会红着提醒）。
- **2026-10-11 平面刀：归属判据从"这一页**归**哪一侧"补到"这一页**实际读**哪几个平面"**。
  声明表每条路由多两格：`planes`（读哪几个平面，**有序**，第一个是主体那一侧）＋ `page`
  （页面模块）。判据与射程写在 `src/api/routeRegistry.ts` 文件头；守卫是
  `tests/route-planes.test.ts`（**声明的面 ⊇ 实际 import 的面** ＋ 主体那一侧必须在
  `planes` 里 ＋ 声明含 `home` 就必须真的读到家面）。量出来的三处事实：
  `/app/agents` 与 `/app/contacts/new` 是**两格**（`side` 一个都没改）、
  `/settings/providers` 的 `planes` 是 `home`（**地址在 `/settings` 下不改变它读的是家面**
  —— 这一格正是下面「配置跟着对象走」那一刀的依据：量出来它读家面，却住在设置区）、
  `/app/profile` 只读世界面（那张"我接入的 Agent"卡是**跳 `/app/agents` 的卡片，不是家侧读数**）。
- **2026-10-11 配置归位刀：模型配置按机器寻址**（`/app/runtimes/:entryId/config`）。
  判据一句话：**配置跟着它的对象走，不跟着"设置"走** —— 它管的是**某台机器**的钥匙清单，
  所以地址里点名那台机器（上面那条 `planes` 量出来的 `home` 就是依据）。三件一起动：
  ① 设置目录**去掉**「模型配置」（入口归机器页的「模型」间，那里本来就有"去配置模型"）；
  ② 那一页里"配哪台机器"的选择器**退场**（**地址就是那台机器** —— 同一个问题不许两个答案）；
  ③ 旧地址 `/settings/providers` 变**别名**（带 `?home=<id>` 落那台、没有上下文落执行端列表）。
  ⚠ 同刀修掉一处**会配错机器**的旧实现：认不出地址里那台时它 `?? localHomeEntry()`
  **默默去配本机** ⇒ 现在如实说「没有这台执行端」，一个字节都不写。
- **2026-10-11 第一次那条路那一刀：动作搬到读数前面**（机器页那张「先给这台机器配一条模型源」的卡）。
  判据一句话：**第一次的落点必须一眼看到"下一步做什么"** —— 而 `/app/runtimes/:entryId` 原先
  只有读数（运行 / 本机服务 / 活动），要你做的那件事（配一条模型源）在**折叠线以下**，
  靠落点带的 `#model` 那次滚动去够（**把动作藏起来再用手势拽出来**，形状本身就是错的）。
  卡的三条纪律：在场判据只有一处（`modelSourceAbsent`）· **读不到不摆** · 零新状态（配好自己消失）；
  两个出口都写出来（「去配置模型」＋「我的 Agent（在那里建一支）」—— 后一步在**世界侧**，
  不写出来这条路走到一半就断）。
  ⚠ 同刀量清了一条**我先前判错的**事（记账）：曾判"落点锚点没生效 = 真缺陷"，真机三档量下来
  容器**每次都滚到上限**（171/171、348/348、768/768），**注入旧实现读数一模一样** ⇒
  那个缺陷**没有被量出来**；「模型」是最后一间、顶不到最上面是物理事实。
  ⇒ 所以带锚点的滚动只收成一份共用实现（`src/lib/anchorScroll.ts`：一份判据 ＋ 有界地等它出现，
  **不盯、不抢用户的滚动**），由 `tests/first-run-landing.test.ts` 全仓扫 `scrollIntoView` 守着。
- **2026-09-25 组织轴刀**：下组的标签与入口从「家 / 我的 Agent」改成
  **「机器 / 我的机器」**。理由不是审美，是**轴错了**：那一区读的全是**机器级事实**
  （Agent 在不在跑、醒过几次、用哪条模型），而它当时以 **Agent** 为轴 —— 页面必须先"认领"
  （遍历机器找哪台报的是这个 Agent）才说得出话。现在机器是主语：
  `/app/runtimes` 一台一行、`/app/runtimes/:entryId` 一台一页（运行 / 活动 / 模型三间），
  Agent 主页瘦成**资料 + 服务**两间（世界侧）。**路由本体一个都没删**：`/app/runtime`、
  `/app/experience` 仍直达（现在是重定向到执行端列表，**不带 hash** —— 列表页没有房间可滚；
  旧锚点 `#home`/`#days`/`#mind` 只在**带 entryId 的地址**上由 `RuntimeDetail` 映射到
  `#run`/`#activity`/`#model`），`/settings/providers` 那时也没搬
  （从「模型」房间深链进去）—— 入口搬家，深链接不破；
  ⚠ **2026-10-11 补**：`/settings/providers` 那一格**后来搬了**（按机器寻址，见上一条），
  它现在只是别名 —— 上面这句是 2026-09-25 当时的记录。
- **2026-09-22 刀2a 收敛（历史，已被上一条取代）**：下组原本是四个平级入口（我的 Agent ·
  模型配置 · 运行状态 · Agent 的经历），而它们切的是**数据来源**（世界侧 / home-status /
  trace / config）—— 于是「运行状态代表谁」「经历是谁的」页面自己答不出来（机器第一、人第二）。
  刀2a 把它们收进一个 Agent 主页的四个房间；2026-09-25 那一刀再按**轴**重排（见上）。
- 唯一事实源 = `src/components/celira/navGroups.ts`（`NAV_GROUPS` 是入口集合的权威；
  **真实世界那份派生**是 `navGroupsFor(true)`，`flattenNavItems` 是唯一摊平派生）；
  桌面图标栏与移动 TabBar **共用同一份分组数据**，不建平行机制、不建第二套导航范式；
- 桌面在两组之间画分隔线并给每组 `role="group"` + 名称，移动 TabBar 按同序排出并在组间留一条
  竖分隔线（窄屏标签允许截断，绝不撑破底栏）；
- **执行端那一区的主语是机器，不是 Agent**（2026-09-25 组织轴刀）：Agent 在不在跑、醒过几次、
  用哪条模型，都在**那一台机器那一页**（`/app/runtimes/:entryId`）说；Agent 自己的公开资料与
  服务在世界侧（`/app/agents/:participantId`）。⚠ 旧说法「主语是 Agent」「连同机器口径一并去掉」
  已于 2026-09-26 作废——它与同文件上面那条组织轴刀**方向相反**，是本文件曾经的内部矛盾；
- 入口存在 ≠ 页面可用：**真实世界**（core 直连——判据只有一处：
  `src/api/prefs.ts` 的 `isRealWorldSession`）里 fixture 驱动页由 `FixtureRouteGuard` 渲染诚实的
  「本页面尚未提供真实数据」，分区不给任何页面造假通行证；**演示通道**（未连接，
  经 `DemoSwitcher` 出入）才让 fixtures 露面。TASK_56 之前的守卫判的是 `mode === 'host'`，
  于是 core 直连的真实用户在「发现」看到假居民、点进不存在的房间；
- ~~「我的服务」（`/app/services`）~~ **已删除（2026-09-28，Agent 服务层退役）**：它原先归属世界侧
  （`routeRegistry.ts` 的 `side: 'world'`：说的是"谁向我开了服务"，**人类侧**的公共事实——与 Agent 主页
  「服务」房间的主角不同）。页面、路由声明与导航注释一并退场，那桩"归属已裁、入口未预建"的悬案随之结案；

## 已知缺口

**客户端建出来的 Agent 会不会回复，分两种运行环境。**

- **在浏览器里**（没有壳）：仍是这条事实 ——「创建 Agent」只发一条 `POST /v1/agents`
  （`src/api/core.ts` 的 `createAgent`）：只建世界侧身份，**不建身体**（不写环境根、不发后端
  凭据、不启动任何东西）。**浏览器里没有任何路径**补上机器侧那一半；界面照实说
  （`AgentsListPanel.tsx` 的 `CREATE_WONT_REPLY`：创建面板的预期句与创建成功横幅共用它）。
- **在 app 里**（2026-09-30 首刀步骤 2）：创建成功 ⇒ **自动**把它接到这台机器上 —— 壳的命令
  `anjia_start`（前端接线 `src/api/anjia.ts`；六步与三条判据见 `app/src-tauri/src/anjia/`），
  四态如实显示（进行中 / 成功 / 失败 / 读不到）。分流的**唯一判据是"有没有壳"**
  （`hasAnjiaBridge` ← `shell.ts` 的 `isTauriRuntime`）：不判平台名、不判构建变量，所以
  同一份产物在浏览器里安静走旧路。
- **在 app 里，而那一支已经存在**（2026-10-09 主人裁 **B** —— **本缺口的前半段已关闭**）：
  那段关系（身份 ↔ 执行端）从此**可以随时建立**，不再只是"创建时顺手试一次" ——
  **本机没认领它**时，`AgentsListPanel` 那一行给一个出口「**接到这台机器上**」⇒ store 的
  `attachAgent` ⇒ **同一个** `anjia_start`（壳那六步一字未改）。哪几态给出口由纯函数
  `src/api/agentMachine.ts` 的 `shouldOfferAttach` 裁决：**`unknown-id`（机器没报上面跑的是谁
  —— 最常见的样子就是一台还没有住户的机器）与 `elsewhere`（报的都是别人的 id）都给**；
  `claimed`（已经有家）／`unreachable`（一份都没读回来）／还没读回来 —— 都**不给**
  （读不到的时候给一个会动手的按钮，等于让用户替我们赌一把）。
  ⚠ **`unknown-id` 也给**这一条是**实测改正的**：第一版只认 `elsewhere`，于是"机器上还没有
  住户"那一格反而没有出口 —— 而它恰恰是用户第一次会撞上的那一格（`agent.state = no-resident`）。
  给错的代价有界：若那台机器其实已经跑着它（老门不报 id 那种），壳会说 `already-homed`
  （「已经有它的家——没有重复动」）。
  ⚠ **浏览器里仍然没有这条路**（没有壳 ⇒ 没有机器侧那一半），那一档照旧如实说
  （`CREATE_WONT_REPLY`）。
  ⚠ 屏上措辞只许用现成的界面语言「接到这台机器上」：设计词「安家」照旧**不上屏**
  （`scripts/verify-ui.mjs` 的 `BANNED_FALSE_PATH` 拦的是**那个词**，与这件事本身无关）。

机器侧另有一条窄路仍在：`node home.js up --agent <id>`（只通「这个家还没有住户」的场合；
手册有专节；2026-09-27 落）。本机那条家的归属仍是 `up` 在**家里还没有 `agent_id` 时**自建一个
（`scripts/home/provision.js`）—— **不是**上面那条路。

**发出去的关系申请，只有发的那一刻看得见。**

`GET /v1/contacts` 的客户端投影只把 `direction === 'incoming'` 的 pending 列进「待确认」
（`src/api/core.ts` 的 `listContacts`；判据是"接受/拒绝归 addressee"，批次三第三刀补的
`direction` 才让这条判据可用）。**我自己发出去、还没被答复的那一条，离开「新的朋友」之后
在界面上就不存在了。** 两处兜底：① 同一页落定后给「撤回这条申请」（`DELETE
/v1/contacts/{participant_id}` 对 pending 段成立，段以 `ended` 闭合）；② 对方接受之后它自然
出现在「联系人」里。**没做**的是"我发出的"那份列表 —— 那要改 `listContacts` 的投影，是另
一件事，不在这一页的射程里。

**2026-10-03（完整 app 那一刀）之后的缺口**：

1. **本机那条家在壳里已经接通**（原缺口 ① 收了）：壳把本机的 loopback 地址（世界 + 家的门两个口）
   解析出来交给页面（`api/shellLocal.ts`，`app/src-tauri/src/local_world.rs`），页面用
   `homes.ts` 的 `setLocalHomeBase` 把它装成"本机条目的地址" ⇒ **家面三格在 app 里读得到了**
   （§8.4 推论 2：本机地址由 app 解析）。⚠ **远程条目仍是"用户填的绝对地址 + 家钥匙"**，
   与以前一字不差。**四态读数就在创建的那一刻说**（2026-10-20 收窄：创建入口只剩
   「我的 Agent」页右上角那颗「添加」，它是**就地**开一块面板、不跳走 ⇒ 读数落在**那块面板**里，
   不再有"从「新的朋友」创建完跳过来、读数要认领到某一行"那条路；无壳时那句仍是
   `CREATE_WONT_REPLY`）。`refused` 另带**封闭子词**
   （缺模型凭据 / 缺可执行件 / 仓根找不到 / 家没布置 / id 形状 / 绑定读不懂 / 凭据文件读不懂 / 空 id），
   页面按子词给人话与下一步，未知子词回退通用句（`api/anjia.ts` 的 `REFUSED_SUBS`）；
2. ~~**没有进程生命周期**：崩了不自动重启、不做 PID 认领、不做托盘与退出语义（承 §8.2-8.3/§8.6-8.7）~~
   —— **2026-10-05 对账：这一条已经过期，按 app 侧的读数更正**（承 `app/README.md` §5 行 7 与
   §8.2-8.3/§8.6-8.7）：**托盘与退出语义已落地**（× ⇒ 收起、托盘图标看得出"还有本地服务在跑"、
   菜单「退出（停止本机服务）」、机器级 `quitStopsServices`、面板上的总开关）；**PID 认领也已落地**
   （账本 `logs/agents/<支>/host.pid` ＋ `process_facts::ownership`：动手前先验身份，陈旧账本／
   pid 撞车一律**绝不动它**）。**仍然为真的一条**：**崩了不会当场自动重启** —— 壳在**每次启动**时
   把住户拉起来（`ensure_residents`），运行中崩掉要等下一次启动。
   **页面这一侧**照旧不该假装有：壳起的那些进程不归页面管，页面只读"在不在跑"。

**附件预览的内存缓存**（2026-10-08，`lib/previewCache.ts` ＋ `lib/attachmentSource.ts`）：图片/语音/视频
取回来的 object URL 进一块**有上限的会话内缓存**（条数 ＋ 字节数两头卡、LRU 逐出时撤销、同键去重）⇒
滚回去不重下、也不闪；命中是**同步**的。**卸载时不再 revoke**（那是从前的写法，等于每次滚回来重下
一遍），撤销只发生在逐出/清空；清空点在**撤销会话**与**换世界**两处。

- **不是第二份权威**：同一批字节、随时可整个丢掉、**不落盘**、刷新即空。
- ⚠ **落盘的附件缓存有意不做**：家里已经存着字节，再往磁盘存一份就要配**配额、逐出、以及"那条消息
  被撤回了怎么办"的失效**三套策略，而浏览器形态本来还有 HTTP 缓存兜着。真要做得单独立项，
  别顺手加。
- ⚠ **已知的机制重复（不合并）**：`hooks/use-member-avatar.ts` 里有同款"Map ＋ in-flight"缓存。
  两条策略不同：头像那条要**负缓存**（"确定没有头像"必须记住，否则每次渲染都打一次 404）且数量受
  成员数约束；附件这条要**按字节卡的 LRU**（16 MiB 的视频能撑爆标签页）。合并会把两边的判据搅在一起。

**拖到整窗发送已做**（2026-10-08）：拖进窗口任意处都能把文件加进这条消息，提示写「松开就加到这条
消息里」（加进待发条，不是立刻发出）。判据只有一处（拖放全挂 `window`，容器上那三个 handler 已删，
否则同一次拖放接两遍、文件进两条），三条纪律见 `DESIGN_SYSTEM.md`。

**2026-10-08 登记：「删除 · 仅为我」（本机隐藏）以及它的三条代价。** 消息菜单里除了「撤回」
（公共动作，归 Core）多了一项「**删除 · 仅为我**」：它**不是删除**，只是**这台机器上这个人的
视图** —— 名单存本机（`lib/hiddenMessages.ts` 判据 ＋ `api/hiddenMessages.ts` 存取，
**世界作用域 ＋ 房间级**），渲染时滤掉。三条代价如实登记：

1. **只在你这边隐藏**：对面照旧看得见（世界里那条消息一动没动）；
2. **不是删除**：换台机器、清了本地存储、重装之后，它照样回来（所以隐藏之后**给了撤销**，
   存不住时如实说「没能保存（刷新之后它还会回来）」）；
3. **不产生任何公共事件**：别的端不会跟着变，因此也没有"同步到我的其他设备"这回事。

⚠ **多选/批量已做**（2026-10-08）：菜单里「**选择**」进多选态，点气泡勾选/取消；输入区被多选条
换掉，三个动作 **转发 / 复制 / 删除 · 仅为我** ＋取消。批量动作的顺序**一律按时间线**（判据只在
`src/lib/messageSelection.ts` —— 按点击先后发出去，对面读到的就是一段被打乱、且没人当场发现的对话）；
批量转发逐条转，**半途失败要说清"前 N 条已经转过去了"**。

**2026-10-08 登记：通话（`CORE_V1_CALL` 窄解冻切片）契约有、客户端没接。**
**2026-10-09 更新：刀 ①（信令面 ＋ 媒体面）与刀 ②（页面入口）都落地了。**
**主人 2026-10-08 决定先不做**（原话「通话的话先不做也行。」）、**2026-10-09 主人点头接**
（「可以」，三刀：信令接线 → 通话中 UI → 真机探针）。契约那一刀已批过
（记录在 `CORE_V1_ACCEPTANCE.md` §6.1 的 Call P0 条），接入**不需要再解封**。

**刀 ① 落地了什么（2026-10-09）**：

- **数据面** `api/core.ts` 实现 `CallApi`（第五个事实域，**故意不并进 `WorldApi`** ——
  演示通道没有通话，见 `api/types.ts` 的 `CallApi` 那段）：六个方法
  （`initiateCall` / `getCall` / `acceptCall` / `declineCall` / `hangupCall` / `signalCall`）
  ＋ **第二条 WS** `GET /v1/call/stream`（独立连接、独立退避；每次接上都叫一次 `onResync`
  让调用方用 `getCall` 重同步 —— 契约 §4.1 明写这条流**不重放**）。
- **帧解析** `api/callFrames.ts`（纯函数、fail closed）：三种帧逐字段过形状，**读不懂的一律丢掉**
  （不认识的 `type` 不许当成 `call.ring` —— 否则界面会显示一通不存在的电话）。
- **媒体面** `lib/callMedia.ts`：WebRTC P2P，复用 `lib/voice.ts` 的**有界麦克风**；
  服务端的 SDP/ICE 边界（`1..=16384` / `≤32 条` / 每条 `≤2048` bytes、禁 NUL）在这里**分批与筛除**，
  越界宁可少发也不发一个会被 400 拒掉的包；**callee 先建对象后采集**（振铃期间收到的 offer/ICE 缓冲，
  接听时按序应用 —— 否则就是"接听了但一直连不通、两边都没错误可看"）。
- **接线** `api/store.ts` 的 call slice：`call` / `callRole` / `callMediaState` / `callNotice`
  ＋ 四个动作；三个帧的唯一处理点在 `handleCallFrame`。**断开/登出连媒体一起收**
  （状态清掉而麦克风轨道还开着是最不该留的东西）。
- **人话**：`contractErrors.ts` 补了服务端那五个稳定码（`call_party_busy` 等）；
  `lib/callText.ts` 管通话域自己的话（`ended_reason` 四种**从我的角度**说、媒体面三种失败）。
- **证据**：`tests/call-frames.test.ts`（6）＋ `tests/call-media.test.ts`（10）＋
  `tests/call-text.test.ts`（6）＋ `tests/reconnect-compensation.test.ts` 那条新用例
  （真 store ＋ 真 `CoreApi` ＋ 假 WebSocket：**两条流各走各的**，通话帧不驱动唤醒链）；
  三处断点**证伪过**（摘缓冲 / fail-open / 把每批上限改成 64 ⇒ 各自当场红）。
- ⚠ **视频面今天不开**：壳只放行麦克风（`app/src-tauri/src/lib.rs` 那一处三条边界），
  `Camera` 一律 Default = 拒 ⇒ 视频通话要**先有真实消费者 ＋ 一次新的裁决**。

**刀 ② 落地了什么（2026-10-09）：页面入口 —— 但"电话只人跟人"**

**主人当次判断**（原话：「电话只人跟人，人和 ai agent 的应该不一样吧？」）。这不是风格偏好，
落成判据是三条**今天做得到什么**（契约本身允许任意两个 active participant 互呼 —— 本刀**不改契约**，
只管界面给不给入口）：

1. **"在不在"不一样**：人的"在不在电话旁"由连接派生（Human 的 WS）；Agent 的进行态是
   "门铃流 / 最近被看见"（`CORE_V1_AGENT_PROTOCOL.md` §6）——它不是坐在电话旁等响铃的东西，
   "响铃 45 秒没人接"对一支 Agent 没有意义；
2. **"拿什么说话"不一样**：实时语音要求**两端各有采集/播放**那一面。人这一端有（麦克风，
   2026-10-09 已在壳里放行）；**Agent 那一端今天不存在** —— 执行端只会读事件、写文本消息
   （`agent/` 里没有任何音频采集/播放）⇒ 人↔Agent 的"实时语音"今天**做不出来**，做了就是假入口；
3. **"过程往哪放"不一样**：人跟人打电话中间没有"它在想"这件事；跟 Agent 通话，它说话前后有
   "在想 / 在调工具"，而那条红线说那些**不进 IM**，实时通话这条渠道里也没有位置放它们
   ⇒ 用户只会看到"它半天不出声"，而系统知道得比用户多（「人不许比系统知道得更少」）。

⇒ **判据落在 `api/uiPolicy.ts` 的 `callEntryFor(room, selfId)`（唯一一处）**：

| 房 | 语音 | 视频 | 为什么 |
|---|---|---|---|
| 对端是**人**的 1:1（`human_direct`） | **真入口** | 只是提示 | 人的响铃/接听/挂断是成立的动作；视频要摄像头，壳今天只放行麦克风 |
| 对端是 **Agent**（`direct`） | **一个都不给** | 一个都不给 | 上面第 2、3 条：执行端还没有那一面 |
| **群房** | 一个都不给 | 一个都不给 | Call P0 明确不做多方（契约 §6）；"打给群里某个人"要另设计 |

- **入口**：`ChatHeader` 两个按钮都按 `callEntry` 门控（不给就**根本不渲染** ——
  不给一个"按下去对面永远没人接"的假入口）；语音按钮走 store 的 `startCall`（页面不许自己拼协议）。
- **那一层** `components/celira/CallOverlay.tsx` 挂在**外壳**上（`App.tsx` 里与 `RouterProvider` 同层）：
  **来电不看你正停在哪一页** —— 挂进 `ChatPage` 就等于"只有打开那间房才收得到电话"（静默丢来电）。
  三种脸：来电（接听/拒接）、正在呼叫（取消）、通话中（媒体态 ＋ 挂断）；`callNotice`
  （"对方拒接了"这类话）单独一条 —— `call` 在 `ended` 之后就清空了，而那句话得留在屏幕上。
- ⚠ **数据层保持通用**：`handleCallFrame` 不按对端 kind 过滤（契约允许任意两个 participant；
  今天**没有任何执行端能发起**通话）⇒ 若将来真有 Agent 来电，那是**那时**要定形状的事，
  现在不预先设计、也不假装它不存在。
- **证据**：`tests/call-entry.test.ts`（8：三格判据 ＋ "`none` 必须说得出理由" ＋
  "视频今天永远不是真入口" ＋ 两条接线判据）；两处断点**证伪过**（把 Agent 房放行 / 摘掉按钮门控
  ⇒ 各自当场红）。

**刀 ③ 落地了什么（2026-10-09）：真机探针 —— 而它当场就抓出两个别的门禁都看不见的缺陷**

探针 `node scripts/home/probe-call.js`（从仓根跑，临时家里**两个浏览器上下文 = 两个人**真互呼；
判据、退出码与触发条件见 `AGENTS.md` 那一条）。它第一次跑就红在**第一格**，第二次跑又红了
**媒体那一格** —— 两个缺陷都不是"文档没写"，是**真的坏了**：

| # | 缺陷 | 为什么别的门禁看不见 | 修法 |
|---|---|---|---|
| ① | `POST /v1/calls` 吃 **400 `invalid_request`**（界面只有一句「请求不合法」） | 根因是**请求体被二次编码**：`fetchJson` 自己会 `JSON.stringify`，而 `core.ts` 里我传的是 `JSON.stringify(body)` ⇒ 服务端读到一个 JSON **字符串**。纯逻辑用例碰不到出网层、边界守卫只对**路径与方法**、两条 UI 门禁都不点通话按钮 | 传对象（`initiateCall` / `signalCall` 两处），并补 **`tests/call-api.test.ts`**：截住 `fetch`、读**真实请求体**，钉住"发出去的是一个 JSON 对象"＋三个动作的路径与方法＋错误信封 |
| ② | **ICE 候选一条都发不出去** ⇒ 连通性靠运气（第一次 9/9 是 SDP 里恰好带了候选） | 候选几乎总在 `setLocalDescription` **之后**才由 `onicecandidate` 攒出来，而那时 `start()`/`accept()` 里那次 `flushIce()` 已经过去 ⇒ 只按"攒够 32 条才发"的话永远不发。**假 peer 的用例**不会自己攒候选，所以它量不到 | 加**去抖发送**（`ICE_FLUSH_DELAY_MS = 50`：同一轮并成一条信令，满 32 条立刻发），补一条用例"候选不达一批也要发出去" |

顺带在复查时改了第三处（同一次真机复查发现的**顺序**隐患）：对端的 ICE 可能**早于** offer/answer
到达，而 `addIceCandidate` 在 `setRemoteDescription` 之前会抛 ⇒ 从前"试一下、失败就算了"会
**丢掉对方唯一的可达地址**。现在**先排队、描述落地后一条不少地补上**（用例"ICE 早于 offer ⇒
先排队"钉着；三处都**证伪过**：各自注入断点 ⇒ 当场红）。

**读数**：修完之后连跑 **三次都是 9/9**、退出码 0、跑完临时家已删；判据逐条摆出来的样子见
`AGENTS.md` 那一条。⚠ **探针自己也被证伪过**：把"发 offer"那条腿摘掉 ⇒ **只有**
「★ 两端媒体都真的连上」那一格红（8/9），其余七格照旧绿 —— 说明那一格真的在量媒体，不是跟着状态机一起绿。

**2026-10-09 收口：壳里"麦克风那一格"（语音消息原先在 app 形态里假可用）。**

- **症状**：app 形态里输入区的麦克风按钮**可见可点**（`isVoiceRecordingSupported()` 只问"API 在不在"，
  壳里两个都在 ⇒ 报支持），按下去**界面什么都不发生** —— 没有录音态、没有错误、没有出口。
- **根因（真机实测，两次实验只改浏览器参数）**：壳里那份 WebView2 的权限处理器由 wry 装，
  **只放行剪贴板读**（`wry-0.57.0/src/webview2/mod.rs` 的 `PermissionRequested`）⇒
  `getUserMedia({audio:true})` **永远不答**（实测 10 秒无应答）；而**只加**
  `--use-fake-ui-for-media-stream --use-fake-device-for-media-stream` 立刻 `granted`（1 条音轨）
  ⇒ **管道没问题，堵点就是权限那一格**。判据不是"被拒"而是"没反应"——最坏的一种。
- **修法两半**：① **客户端**（`lib/voice.ts`）给 `getUserMedia` 一条**有界等待**（`VOICE_PERMISSION_TIMEOUT_MS`）
  ＋ **稳定分类**（`unsupported` / `no_answer` / `denied` / `no_device` / `failed`，逐格一句人话
  `VOICE_START_FAILURE_TEXT`，`no_answer` 那句给出出口："更新一次应用就好；现在可以先打字"）；
  **迟到的流必须放掉**（用户在我们不等了之后才点"允许"时，麦克风不许没人录还开着）；
  ② **壳侧**（`app/src-tauri/src/lib.rs` 的 `on_permission_request`，**2026-10-09 主人裁：放行**）
  只在用户按下录音时放行 `Microphone`，**`Camera` 一律 Default（= 拒）**——今天只有语音消息这一个
  消费者，将来接通话的视频面时**在那时**加一行。三条边界写在那一处代码里（只放行麦克风 /
  页面只在用户动作时请求 / 拿得到它的只有我们自己的页面）。
- **验收**：`client/tests/voice.test.ts`（8 条：分类、人话表逐格有出口、有界、迟到流放掉、
  **接线判据**——`getUserMedia` 全文件只有一处调用点，退回无界 `await` 当场红）＋ 真机探针
  （`_probe-shell-mic.mjs` 那两次实验：默认参数 vs 只改浏览器参数）。

**2026-10-08 登记：转发的附件语义（以及它的三条代价）。** 转发从"仅正文"改成"**附件一起转**"，
做法是**取原字节 → 在目标房重新上传一份**（跨房引用做不到：附件绑定绑死在"上传者本人 ＋
同 Room ＋ 24h 窗口"，`CORE_V1_IM_CONTRACT.md` §8）。**三条代价如实登记，不藏**：

1. **字节在家里存两份**（家是单机磁盘；上限 16 MiB/个、≤8 个/条 ⇒ 最坏 128 MiB/条）；
2. **"转发自谁"没有事实可依**：契约没有转发语义，而正文是作者写的 ⇒ **客户端不伪造**
   （所以转出去的那条不显示来源）；
3. **撤回联动断开**：源消息撤回后，转出去的那份仍在（它是一条新消息的附件）——与主流一致
   （转发是副本）。

编排在 `src/lib/forwardMessage.ts`（纯逻辑，判据是**全有或全无**）；配方见 `DESIGN_SYSTEM.md`。
✅ **真机验过了**（2026-10-08）：`node scripts/home/probe-forward.js` —— 在**临时家**里真的驱动页面
（登录 → 进房 → 选文件 → 发送 → 右键 → 转发 → 选目标房），再**只看世界里的公共事实**判三条：
源房那条消息带上了附件／目标房那条新消息文件名一致／**目标房那份附件的字节与原件逐字节相同**
（第三句是硬证据：只比 id 或名字都能被"占了个位"骗过去）。读数 **3/3**。

**2026-10-08 登记的外观缺口**（判据见 `AGENTS.md` §前端边界的「扩展接缝」）：

- ✅ **当日收口：用户主题的运行时装载已落地（刀 1）**，**"装之前先看"的小样也已落地（刀 2）**。
  装一份**带作用域的 CSS** 即生效、刷新还在、删掉回默认，**不用重编**（app 形态连壳都不用重编）；
  小样把同一份主题的作用域换成**保留 id** `user-preview`，靠**嵌套 `data-theme`** 关在卡片内，
  **不改全局、不落任何持久面**（真机量过：`<html>` 无 `data-theme`、全应用配色一毫不变、localStorage 为空）。
  权威位置：格式 `src/lib/themeFile.ts`、体检 `src/lib/themeAudit.ts`、本机存储与导入导出
  `src/lib/userTheme.ts`、官方令牌表读取 `src/lib/themeCatalog.ts`、样张 `ThemeScopePreview`、
  装载口与预览 `src/components/celira/UserThemeSection.tsx`；口径见 `DESIGN_SYSTEM.md` 与
  `THEME_GUIDE.md`「用户主题」节。
- ✅ **官方那九套覆盖层主题已搬成「包」（刀 3）**：`src/styles/themes/*.css`，**一个包 = 一个文件 = 一个作用域**
  （文件名即作用域名），判据是「一条规则属于主题 X ⟺ 它的每一个选择器分支都带 `[data-theme='X']`」。
  于是**官方实例与用户主题共用同一个格式**（贡献路径 = 自定义路径）；**默认主题 `celira` 不是包**，
  它是基底（`:root`/`.dark`，连它的配色变体也锚在 `:root` 上）。包**仍随产物编译**（`?raw` 急加载，
  启动时在 `createRoot` **之前**注入 `#celira-theme-packages`）⇒ 挂载前就位、零首帧、零网络面；
  **代价如实认下**：加一套主题**仍要走一次构建**（"不用重编"那一格没拿到，升级路径在
  `src/lib/themeCatalog.ts` 文件头）。纯搬迁可证：规则集合逐条相等（9 个主题 87 条 ＋ 基底 17 条）。
- ⚠ **令牌守卫的射程（刀 3 已扩，但仍有一条在外）**：`scripts/check-tokens.mjs` 白名单 =
  `src/styles/tokens.css`（基底）**∪** `src/styles/themes/*.css`（包）⇒ 规则重述为
  **"色值只许出现在主题文件里"**。**用户主题仍是运行时文本、在守卫射程外**，那道校验必须由
  装载路径自己扛：格式三条硬要求**拦人**，体检（对比度／五态可辨／agent 色可区分）**照实报但不拦人**。

**切片状态**：「**创建 → 安家**」在 app 里的正路**已落地**（2026-09-30 首刀步骤 2：壳的命令
＋ 前端接线 ＋ 四态），**「打开就在自己的世界里」也已落地**（2026-10-03 刀：壳起世界 + 首次布置
＋ 正门直进；前端接线 `api/shellLocal.ts`、正门第三格 `api/entry.ts` 的 `local-ask`、本机家条目走
loopback 绝对地址 `api/homes.ts` 的 `setLocalHomeBase`）。形状承 `docs/constitution/PROJECT_SHAPE.md`
§8.4 / §8.6-1 / §9.1 / §9.3；**剩下的**是上面那两条，以及 §9.3 第 3 步尾巴
「**存储换成文件实现**」——**壳那一半已落地**（2026-10-05：`app/src-tauri/src/local_store.rs`
的文件后端 ＋ 四个命令 ＋ 客户端 `api/storage.ts` 的转发门面与 `api/shellStorage.ts` 的装配；
契约见 `AGENTS.md` 边界守卫那条"第二条接缝"）。

✅ **2026-10-08 补：两段式启动已做**，连接偏好那一半**搬完了**。原先 `store.ts` 在**模块求值期**
同步读 `localStorage`（那是"文件存储接不进来"的唯一原因）⇒ 壳里那份状态（**含 Human token**）
一直住在 **webview 的 `localStorage`**：不在家里、不跟着家走、清一次站点数据就没了。现在：

1. **模块求值期只有缺省值**（`let prefs = DEFAULT_PREFS` / `EMPTY_ROOM_PREFS`）；
2. `hydrateLocalState()`（幂等）在 **`createRoot` 之前**经过窄口把真值读进来，并**推回 store 的
   状态**（⚠ 少了那一推，页面会按演示通道渲染 —— 实测把 `verify:ui` 从 91/91 打到 67/80）；
3. 读写都走**同一个窄口**（壳里 = 家里的文件存储）；
4. **读穿式迁移**：窄口还空着的那一次，把老位置（`localStorage`）的值捞出来写进窄口 ——
   判据**只有一处**：`api/storage.ts` 的 `preferStoredRaw`（纯函数）＋ `readThrough`（唯一读老位置
   的地方）。**用户不会因为换存储而被登出**；
5. 顺带修掉一个真 bug：**登出原先清的是 `localStorage` 那一族**，于是壳里**家里那一族世界级
   数据（cursor／系统消息／房间缓存）根本没清**。现在按前缀清**窄口**，老位置只顺手清残留。

真机读数（`verify:ui:app` 18/18）：跑门禁前 `local_store.json` 里**没有** `celira.connection.prefs`、
跑完**有了** ⇒ 迁移真的落了。

✅ **2026-10-08 第二刀：这条依赖链的下一环也做完了**（外观／草稿／房间偏好，四族同上一条判据）。
搬完的家族与各自的持久面（**键名一字不改** —— 改了就等于把用户已经选好的东西丢掉）：

| 家族 | 键 | 持久面（键 / 宽容解析 / 首帧快照 / 补水 / 写） | 呈现面读快照的地方 |
|---|---|---|---|
| 置顶 / 免打扰 | `celira.world:<世界>:room-prefs` | `api/roomPrefs.ts` | `store.ts` 的 `refreshRooms` 投影 |
| 明暗 / 预设 / 配色变体 / 字号 | `celira-theme` 等四格 | `lib/themePrefs.ts`（新） | `hooks/use-theme.tsx` 的 `useState` |
| 用户主题（列表 / 当前生效） | `celira-user-themes` / `celira-theme-user` | `lib/userTheme.ts` | 同上 |
| 壁纸偏好 / 上次所在会话 | `celira-wallpaper` / `celira-last-room` | `lib/wallpaper.ts` | `hooks/use-wallpaper.tsx`、`ChatWallpaper` |
| 会话草稿 | `celira_drafts` | `lib/messageDrafts.ts`（新） | `MessageInput` 的 `useState` |

- **判据同款**：模块求值期只有缺省值 → `main.tsx` 在 `createRoot` **之前** await 五族补水
  → Provider/组件用**同步快照**做初始值 ⇒ **第一帧就是用户自己的东西**（不闪默认主题/壁纸、
  不丢草稿）。每一族都幂等、失败各自降级并留痕（**白屏比"没读到偏好"糟得多**）。
- **搬家读只有一处**：`api/storage.ts` 的 `readThrough`（窄口优先；窄口空而老位置有 ⇒ 用老的
  并搬进窄口）。**老位置（webview 的 `localStorage`）从此只被它读** —— 机械守卫
  `tests/local-state.test.ts` 钉着三条：`src/` 里 `localStorage` 这个标识符**只许出现在
  `api/storage.ts`**、搬家读的调用点在白名单里（新增一族必须同时在 `main.tsx` 点名）、
  五族补水都排在 `createRoot` 之前。
- **写是异步的**（窄口就是异步的）：界面不等落盘，失败只记一笔日志；草稿那一族把同键的写
  **排成一条链**（打字是每敲一下一次写，后发先到会让盘上留着上一版）。
- **不再同步读存储**：全仓已无同步读持久面的调用点（`store.ts` 里那句"这两条还是同步的"
  陈旧注释一并更正）。

真机读数（`verify:ui:app` 18/18）：跑门禁前壳的 `local-store.json` 里只有 `celira.connection.prefs`
＋ 世界级那几格；跑完**多出** `celira-theme`（值是 `light`，**不是缺省的 `dark`** ⇒ 确实是从老位置
搬来的）、`celira-theme-preset`、`celira-theme-variant`（非空映射）、`celira-font-size`、
`celira-last-room`、`celira_drafts` ⇒ 四族真的落进了**家里的文件存储**（不再是 webview 的库）。

⚠ **如实登记的边界**（都不是本轮的事）：

- **IndexedDB 那两份仍在 webview 自己的库里**（`api/idb.ts` 的房间/消息缓存、自定义壁纸图片
  Blob）。它们是缓存与二进制，不是"状态层的键"；要收它得另说（`PROJECT_SHAPE.md` §9.3 第 3 步）。
- **草稿与壁纸的按会话覆盖没有世界作用域**（只按房间 id 分；房间 id 是服务端铸的 `uuid v4`
  ⇒ 两个世界撞同一个 id 的概率可忽略）。真要收，得先定"该按世界分还是按机器分"，且键名一改
  用户手上那份就没了 ⇒ 另作一刀（登记在 `lib/messageDrafts.ts` 文件头）。

## 动手前必读（按顺序）

1. `DESIGN_SYSTEM.md` — 硬规则与组装配方；
2. `src/styles/tokens.css` — 所有颜色的唯一事实源；
3. 运行 `npm run dev` 看「陈列室」页 — 所有组件的正确用法参照。

## 常用命令

| 命令 | 作用 |
|------|------|
| `npm run dev` | 开发 |
| `npm run lint` | oxlint + 令牌检查（禁 hex / 裸色阶 / arbitrary 颜色 / 内联色） |
| `npm run build` | 类型检查 + 构建 |
| `npm run test` | 纯逻辑/契约测试（front-door、ui-policy、contract-errors、real-world 等，node --test，无需新框架） |
| `npm run shots` | **不适用**：视觉回归的 Host 通道要求 8088 真 Host，而 Host 全托管路径**已删除**（见 `docs/client/LOCAL_CLIENT_HOST.md` §0）。脚本保留，本机跑不通也不该假装能跑 |
| `npm run shots:catalog` | **陈列室渲染健全性检查**（自起 `vite preview` 走 legacy 通道）：只查"陈列室真的渲染出来了、展示节有面积"，**不比对像素** |
| `npm run verify:ui` | **页面级结构验证**（真机，core 直连）：分组、入口集合、撤销按钮、头像区按能力渲染。需先起家并提供 `CELIRA_UI_BASE`/`CELIRA_UI_TOKEN`（或 `--core <base> --token <tk>`）。⚠ **现成的一条路（2026-10-08 建）**：`node scripts/home/probe-ui.js` —— 起**临时家**、铸一支只属于它的 token、按本文件头那份清单喂饱世界、端构建产物（`scripts/serve_client.mjs`）、跑本门禁、**删掉整个临时家**；全程不碰真家，token 只经 argv 交子进程、不打印。实测读数：**91/91 通过 · 3 条不适用 · 0 失败**（那 3 条不适用都带理由：这轮没有活动记录 / 那个房间本来没消息 / 没有等我回答的申请） |
| `npm run verify:ui:app` | **产品形态（打包形态）的真机页面级门禁**（`scripts/verify-ui-app.mjs`，2026-10-05 加）：跑的是 `tauri build` 那一形态（页面走**内嵌 `client/dist`**、来源 `http://tauri.localhost`、本机两个口由**壳自己端**），量 `verify:ui` 结构性看不见的那一面 —— 形态判据、**壳里嵌的产物是不是最新的**（页面入口脚本 == 磁盘上 `client/dist/index.html` 那个；Vite 产物名带内容哈希 ⇒ 名字不一致就是"嵌的是旧的"，2026-10-05 补且负例验过会红）、落点、壳专属读数、机器页三间、**两个门的来源判据 ＋ 安全反例**、全路由渲染冒烟。**要打包产物 ＋ 一个家**（前置与触发条件见脚本文件头与 `AGENTS.md`）；缺产物或那一颗是 dev 形态 ⇒ 报 **N/A** 并给重编命令。⚠ **2026-10-08 第二刀（门禁自洁）**：① 收尾原先只扫**世界**、不扫壳带起来的**住户**（`agent_host.exe`）⇒ 孤儿留在盘上，**下一次冷启动**世界与门起不来，门禁自己读成 13/18（看着像代码坏了）—— 现在住户也按**差集 ＋ 种类 ＋ 家事实**收掉；② 面板那三条断言原先"面板一出现就判"，而读数还在「正在读本机服务…」那一帧时**有两条会假绿**（"本机服务 / 世界"在说明文字里就有、「退出时：读不到」加载态也成立）⇒ 现在**读数落定才算数**。两处判据钉在 `tests/verify-ui-app.test.ts`（源码级，随 `npm test` 跑）；实测连跑两次都 18/18、跑完零残留 |
| `npm run probe` | **页面探针**（`scripts/probe.mjs`）：`routes` 列路径 / `sweep` 全路由冒烟 / `open <path>` 单页可点可填 / `selfcheck` 正对照。**什么都不用先起**（自己起 dev server，默认演示通道）。判据只三条非像素的：不是白屏、不是渲染降级页、零未捕获异常 —— 详见该脚本文件头 |
| `node scripts/probe-shell-mic.mjs` | **壳里"麦克风那一格"真机探针**（2026-10-09 立，从**仓根**跑）：跑两次、**只改浏览器参数**（A 默认参数 = 真实用户那条路 / B 自动放行＋假设备 = 辅助读数）⇒ A 档 `granted` = 壳里那一处放行在位；A 档**没落定**（8 秒无应答）= 放行被摘掉了。退出码 **0/1/2**（见文件头）。**触发条件**：改壳的权限处理器、改 `lib/voice.ts` 那条采集路、或接通话（Call P0）时跑它。⚠ 它会把这个家的世界起起来，跑完**只收自己起的**（世界按差集＋身份、住户按基线差集） |
| `npm run verify` | lint + build + 陈列室渲染检查，交付前必须全绿 |

> **2026-10-03 补**：`probe` / `verify:ui` 的「未捕获异常」判据改从**页内记录器**读回
> （本机唯一可用的引擎不发 CDP 异常事件 ⇒ `page.on('pageerror')` 恒收不到）；同一根因对
> `console` 类消息仍成立 —— `npm run probe -- selfcheck` 是这条判据的正对照，
> 详见 `scripts/probe.mjs` 文件头。

> **2026-09-25 裁决：像素对齐拆掉了。** 主人原话「不要走这种截图对齐了，这种没啥意义」。
> 理由：像素 diff 只能证明"变了"、永远证明不了"变好还是变坏"，于是每次都要人肉裁决——
> 成本落在人身上；而"人看过这一眼"本就不可靠（同一轮里我自己拿压缩到 63KB 的截图去分辨
> 40px 头像上的青与紫，**看错了还当事实说了出来**）。**防"AI 乱写样式"的是固定组件 + 语义
> 令牌**（颜色由 `check-tokens.mjs` 机械守住），视觉回归从来不是那条防线。
> 现在 `shots:catalog` 只守**灾难**（dist 过期 / 静默变成门禁页 / 展示节没渲染或没面积），
> 三条判据都不依赖像素，所以**不会因改样式而误报**。基线目录 `design-baseline-catalog/`
> 与 `pixelmatch` / `pngjs` 依赖已一并删除。**不要再加回来**——要看"这一眼"就开 dev server 看，
> 或写一次性截图脚本（看完即删）。

## 交付门禁

改动后 `npm run verify` 必须全绿（lint + build + 陈列室渲染检查）。**样式改动不需要更新任何基线**——渲染检查不看像素（见上）。新接口必须先落在 `src/api/` 契约层再接入页面，禁止组件直连 fetch 或绕过 api 层直接消费 fixtures。

**Host 模式已删除（2026-09-20 裁决，删除刀 D 执行完毕）**：Local Client Host 的全托管路径
**已整体移除**（裁决依据见 `docs/client/LOCAL_CLIENT_HOST.md` §0，该文保留为 BFF 安全模式的
历史权威记录）。因此——

- **`/api/v1/local/*` 与 `mode === 'host'` 分支都已不存在**：`host/` crate、前端 `host*.ts`、
  `HostGate`、`CallOverlay`、启动探测与 14 个 `host-*.test.ts` 全部删除；`ConnectionMode` 与
  世界侧传输层的 `mode`（`RoomApi`，经 `WorldApi` 可达）只剩 `'core'`，`contracts.ts` 里的 `Host*` 类型已清零；
- 客户端**只有 core 直连**一条真实路径。需要给真实用户加能力时，走 core 直连
  （那是当前唯一有真实消费者的路径）；
- 发现某件事"只有 Host 模式能做"，先回到 §0 的裁决：那条路径没有真实消费者，
  它不是需求，是待删除的遗留。

