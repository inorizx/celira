/**
 * **路由准入的权威声明表**（2026-09-25 立）。
 *
 * ## 它治的是什么
 *
 * `DESIGN_SYSTEM.md` §导航壳写着一条纪律：「**新增路由时必须先决定它属于哪一侧**」
 * —— 但那是**一句话**，没有任何机械判据。代价已经付过三次：
 *
 * | 路由 | 当时的处境 |
 * |---|---|
 * | `/app/services` | 没人决定过它归哪侧 ⇒ 既不在任何导航分区，也只靠「我的」页一条捷径够得着 |
 * | `/identity` | 决定成"fixture 页"（挂进守卫），但没人问过它「创建身份」这个承诺是否属实 |
 * | `/agent/:participantId` | 同样挂进守卫，但没人问过"真实世界的 4 处入口链到它算不算漏" |
 *
 * 三个都不是"忘了改"，是**没有一处能回答"这个路由归哪一侧"**。
 *
 * ⚠ **2026-09-26 后记**：第三条已结案（重写成世界侧参与者页、搬出守卫，见本表下方那一行）；
 * 第二条已结案（页面与路由整体删除 —— 承诺不实）；
 * **第一条也已结案**（2026-09-28：Agent 服务层整体退役，`/app/services` 随之下线）。
 *
 * ## 判据：每一侧都要能推出「页面自己说不说得出话」
 *
 * | 侧 | 真实世界里的表现 | 谁在保证 |
 * |---|---|---|
 * | `entry` | 首屏（启动页 / 登录页）——**连接之前**就到得了 | `api/entry.ts` 的正门判定 |
 * | `world` | 读真实 core 事实 | 页面自己（数据面是 `CoreApi`） |
 * | `home` | 读**家**（本机或远程那台机器的状态口/配置门） | 页面自己 + `hasRealWorldTransport()` 那道闸 |
 * | `settings` | 关于「我和这台设备」；两种世界都有意义 | 页面自己按能力/凭据判 |
 * | `shared` | 两侧都能开（重定向、错误页这类没有数据面的路由） | 页面自己 |
 * | `demo` | **真实世界被诚实门拦下**（`FixtureRouteGuard`） | 守卫本身 |
 *
 * ⚠ `demo` 是**唯一一个"真实世界进不去"的侧**。所以它是一条纪律的落点：
 * 任何"真实用户能点到、点进去却撞门"的入口都是缺陷 —— `/agent/:participantId`
 * 就是这么被发现的（消息头像 / 右键菜单 / 流式气泡 / 联系人资料卡四处链到它）。
 *
 * ## 判据：**把 im 服务端换成微信/QQ，这一页还成立吗？**（2026-09-26 立）
 *
 * 上面那张表回答"这一侧是什么"，但没回答"**这一页凭什么算这一侧**"。缺这条判据的代价
 * 已经付过：`/app/agents/:participantId` 声明成 `home`，而它**只读 core、不读家条目**
 * —— 声明与事实相反，且它自己的 `what` 里就写着「世界侧」，同一行自相矛盾。
 *
 * 判据来自双平面红线（「对 Runtime 内部本体，im 服务端是**可替换渠道**」），
 * 把它变成一个**可机械套用的页面测试**：
 *
 * - **成立 ⇒ `world`**：换成微信也说得通的东西 —— 消息、会话、联系人、公开资料；
 * - **不成立 ⇒ `home`**：只有"这台机器"才有的东西 —— 在不在跑、醒过几次、用哪条模型、
 *   模型配置门、家钥匙。
 *
 * 两边的"绝不能有"也由此确定（与市面上的正确答案一致：微信不显示"在线"，
 * Modal/Devin 不显示"联系人"）：
 * **IM 侧不能有宿主读数**（可以有 presence 投影 —— 但 2026-09-26 主人已裁：**连投影也不放**，
 * IM 只显示「正在说话」这种进行态，不显示「在不在家」这种身体态）；
 * **执行端侧不能有内容与关系**（可以有"上面跑着谁"的名字标签）。
 *
 * ⚠ **2026-10-14 报告一处冲突，交主人裁**：上面那句"**连投影也不放**"（2026-09-26）与
 * **当前口径**打架 —— `AGENTS.md`「主体进行态边界」写着：IM 侧**只放主体进行态**，而
 * 主体进行态**就是**「**在不在**（由连接派生：Human 的 WS、Agent 的门铃流或"最近被看见"，
 * 不新增任何写者）」与「正在写」，并给了判据原话：「把 im 服务端换成微信/QQ，**"对方在线吗"**
 * 仍成立 ⇒ **该放**；"它在调工具 / 草稿写了什么" ⇒ 不放」。禁的只是**身体态**（宿主在不在跑、
 * 醒过几次）——那归 `/app/runtimes`，本页确实一个字节都不放。
 * ⇒ 本文件按**当前口径**读：**presence（连接在不在）在 IM 面是允许的**；那句旧裁决是否正式
 * 废止、还是"只禁身体态"的另一种说法，**由主人裁**（本行是报告，不是废止）。
 *
 * ⚠ 判据管**页面主体**，不管"有没有引用对方"：机器页引用 Agent 的名字是正当的
 * （"上面跑着谁"是机器的属性），机器页显示它的联系人列表就不正当。
 *
 * ## 判据：归 home 之后，这一页许装什么？（2026-09-30 立）
 *
 * 「换成微信还成立吗」判**归哪一侧**；这一条判**家面的容量**（home 侧与读 / 写家的
 * 设置页）——只许两类东西：
 *
 * - **读数**：机器级事实（在不在跑、醒过几次、用哪条模型、读得到还是读不到）；
 * - **人的动作**：只有人能给、或必须由人做的（放钥匙、绑模型、安家、起停、撤销）。
 *
 * **不立的页**：Agent 自己能决定的（人格 / 记忆 / 工具 / 习惯）——私域归 Agent，
 * **聊天就是 Agent 的配置界面**；不做「遥控器」——管理面板是「工具」的姿势，
 * 「共处」发生在共享面里。
 *
 * **底线：急救入口不许住在被修的东西里**——Agent 不回话时，这一面必须还能独立说得上话：
 * 定位得了「为什么没回话」，救得回来。低频 ≠ 不重要：这类页只在该出事时被打开，
 * 而那正是不可替代的时刻。
 *
 * 记忆点：**Agent 自己能管的，一个像素都不给；Agent 不能自己管的——钱、信任、机器、
 * 以及坏掉之后的后事——一个把手都不能少。**
 *
 * ## 判据：这一页**实际读了哪几个平面**（2026-10-11 立）
 *
 * 上面三条判据都只回答"这一页**归**哪一侧"。而缺这一条的代价，本文件头上已经付过一次：
 * `/app/agents/:participantId` 曾声明成 `home`，而它**只读 core** —— 声明与事实相反。
 * 那次是**人**读出来的（改对了），但**没有任何守卫能回答"这一页到底读了哪一侧"**。
 *
 * 这一条把它变成机械的：每条路由写出 `planes`（读哪几个平面）＋ `page`（页面模块），
 * 由 `tests/route-planes.test.ts` 顺着页面模块的 import 闭包对表。三条断言：
 *
 * - **声明的面 ⊇ 实际 import 的面**：world 页不许读家面权威模块，反之亦然；
 * - **主体那一侧必须在 `planes` 里**（`side` 是 `world` / `home` 时）；
 * - **声明含 `home` 的页必须真的读到家面** —— 防的正是上面那次事故的形状。
 *
 * ⚠ **射程（如实登记）**：它量的是**权威模块的直读**。经**脊柱**（`api/store.ts` 这类
 * 两侧共用的机制）读对面平面的事实**量不到** —— 脊柱按设计同时装两个平面，要机械化得先
 * 拆它，那是另一刀。两张权威表与脊柱名单都在那个测试文件里，每条带理由。
 *
 * ⚠ **`demo` 侧不在射程里**：那一侧读的是 fixtures，不属于任何一个平面（真实世界已被
 * 诚实门拦下）。判据落在下面的 `planeCheckedRoutes`。
 *
 * ## 怎么被验
 *
 * `tests/route-admission.test.ts` 会**读 `src/App.tsx` 的真实路由表**（不 import ——
 * 它带 JSX 依赖），抽出所有路径与它们是否在 `FixtureRouteGuard` 之下，然后与下面
 * `ROUTES` 逐条对表：
 *
 * - 路由表里有、表里没有 ⇒ **新增路由没回答"归哪一侧"**；
 * - 表里有、路由表里没有 ⇒ **陈旧条目**（路由删了，声明要跟着删）；
 * - 声明的侧与"在不在守卫下"对不上 ⇒ **声明与实现矛盾**（`demo` 必须在守卫内，
 *   其余必须在守卫外）。
 *
 * 本模块零运行期依赖，可在 `node --test` 的类型剥离模式下直接读。
 */

/** 一个路由的归属侧（语义见文件头那张表）。 */
export type RouteSide = 'entry' | 'world' | 'home' | 'settings' | 'shared' | 'demo'

/**
 * **平面**：事实住在哪一侧。边界判据量的就是它（文件头那条「换成微信/QQ 还成立吗」）。
 *
 * ⚠ 它**不是** `RouteSide` 的子集：`side` 回答"这一页在产品地图里是什么"
 * （含 `entry` / `settings` / `shared` / `demo` 这些**不是平面**的取值），
 * `planes` 回答"它读哪几个平面"。两者在 `world` / `home` 上重合是**结论一致**，
 * 不是两份权威 —— 守卫对的是 `planes`。
 */
export type RoutePlane = 'world' | 'home'

export interface RouteDeclaration {
  /** 路由路径，**与 `App.tsx` 里逐字一致**（动态段写作 `:participantId`）。 */
  path: string
  side: RouteSide
  /**
   * **这一页实际读的平面**（顺序 = 主次，**第一个是主体那一侧**；空数组 = 两侧都不读）。
   *
   * 为什么必须逐条写出来（而不是从 `side` 推）：`settings` / `shared` 这类取值是
   * "两侧都可能"的**逃逸口**，而逃逸口正是边界最容易糊掉的地方 —— 写出来才看得见。
   * 它说的是"这一页读的是哪一侧的数据"，**不是产品取舍**：那件事在代码里可核。
   *
   * ⚠ 它由 `tests/route-planes.test.ts` 对表：**声明的面 ⊇ 实际 import 的面**
   * （家面权威 / 世界面权威两张表在那个文件里；脊柱不展开，射程写在那里）。
   */
  planes: readonly RoutePlane[]
  /**
   * 这一页的**页面模块**（相对 `src/` 的路径）。
   *
   * `null` = 这一条没有自己的页面模块：重定向（`/app/runtime`、`/app/experience`）、
   * 壳内联的落点（`/`）。它是守卫的入口，也是"新增页面必须回答它读哪几个平面"的落点。
   */
  page: string | null
  /** 这一页在这个世界上说什么（一句话；新增路由时必须写得出）。 */
  what: string
  /**
   * 在 `FixtureRouteGuard` 之下（= 真实世界渲染诚实门）。
   *
   * 它**必须**与 `side === 'demo'` 一一对应 —— 那条断言就是"声明与实现不许各说各话"。
   * 字段显式写出来（而不是从 side 推），是为了让"这一页被守卫挡着"这件事
   * 在声明表里**看得见**：改 side 的人必须先回答"那它要不要挪出守卫"。
   */
  guarded: boolean
}

export const ROUTES: readonly RouteDeclaration[] = [
  // ── 正门（连接之前）────────────────────────────────────────────────────────
  // ⚠ **2026-10-11 修陈旧**：这一格从前写「重定向到品牌启动页」—— 那是**半句**：
  //   它走的是**正门判定**（`App.tsx` 的 `EntryLanding`），**没凭据才**落启动页，
  //   连过就直接进应用（`/app/chat`）。`what` 是这一格唯一的说明 ⇒ 说半句就是错的。
  { path: '/', side: 'entry', planes: [], page: null, what: '根路径：走正门判定（没凭据落品牌启动页，连过就进应用）', guarded: false },
  { path: '/splash', side: 'entry', planes: [], page: 'pages/SplashPage.tsx', what: '品牌启动页（首屏定案前的静态加载面）', guarded: false },

  // ── 演示通道（真实世界被诚实门拦下）──────────────────────────────────────
  // ⚠ 这一侧里的每一页都在回答同一个问题："它凭什么只活在演示通道？"
  //   答不出来的就该删或正名 —— `/identity` 就是这么被删掉的（2026-09-25：
  //   标题承诺「创建身份」、实际只 navigate 一下，且真实身份走 CLI 签发 ⇒ 承诺不实）。
  // ⚠ **`planes` 写空**：这一侧读的是 fixtures，**不属于任何一个平面**（`planeCheckedRoutes`
  //   也把它排除在守卫之外）—— 别把"它渲染了消息组件"读成"它读世界"。
  { path: '/gallery', side: 'demo', planes: [], page: 'pages/GalleryPage.tsx', what: '组件陈列室：所有领域组件 × 双主题的正确用法参照', guarded: true },

  // ── 世界侧（im 服务端公共事实）────────────────────────────────────────────
  // `/app` 本身是**布局路由**（`AppShell` 的壳），没有自己的内容面 —— 归 shared。
  { path: '/app', side: 'shared', planes: [], page: 'components/celira/AppShell.tsx', what: '壳层布局路由（AppShell：导航栏 + TabBar + 出口），无自己的内容面', guarded: false },
  { path: '/app/chat', side: 'world', planes: ['world'], page: 'pages/ChatPage.tsx', what: '会话列表 + 当前房间的消息时间线', guarded: false },
  { path: '/app/chat/:roomId', side: 'world', planes: ['world'], page: 'pages/ChatPage.tsx', what: '某个房间的消息时间线', guarded: false },
  { path: '/app/chat/:roomId/history', side: 'world', planes: ['world'], page: 'pages/ChatHistoryPage.tsx', what: '查找聊天记录（关键词/类型筛选 + 定位跳回）', guarded: false },
  { path: '/app/contacts', side: 'world', planes: ['world'], page: 'pages/ContactsPage.tsx', what: '联系人列表（Human 与 Agent 都是居民）', guarded: false },
  { path: '/app/contacts/:participantId', side: 'world', planes: ['world'], page: 'pages/ContactsPage.tsx', what: '某位联系人的资料卡', guarded: false },
  // ⚠ **两格 → 一格（2026-10-20）**：这一页原先的 `planes` 是 `['world', 'home']`，理由很具体
  //   （见 git 历史）：它 import 了 `AgentsListPanel` 里的**一句文案**（`CREATE_WONT_REPLY`），
  //   而那是家面组件 ⇒ 整个家面闭包跟着进来。那一刀把「创建 Agent」从这一页撤走之后
  //   （创建是所有权，归 `/app/agents`；这一页只发关系申请），那句文案也不再需要 ⇒
  //   这一页现在**只读世界面**。声明跟着事实收窄：两格会在 `route-planes.test.ts` 里变成
  //   "声明含 home 而一个家面模块都不读"的相反事实。
  { path: '/app/contacts/new', side: 'world', planes: ['world'], page: 'pages/NewContactPage.tsx', what: '新的朋友：贴上对方的身份 ID，发一段关系申请（Human 与 Agent 同一串 id）', guarded: false },
  {
    path: '/agent/:participantId',
    side: 'world',
    planes: ['world'],
    page: 'pages/AgentPage.tsx',
    // ✅ **2026-09-26 从演示通道搬过来了**（原 `side: 'demo'` + `guarded: true`）。
    //   它原先整页在诚实门之下，而**真实世界有三处入口链到它**（消息头像 / 右键菜单
    //   「查看主页」/ 联系人资料卡；第四处「流式气泡」已随 Agent Live State 撤出 IM 删除）
    //   ⇒ 真实用户点得到、点进去撞门 —— 审计里最重的一条硬伤（H9）。
    //   放出来的前提是那一刀已经把它的假东西拆干净：写死的「在线」徽标、fixtures 的
    //   模型/绑定分区、「发消息」跳演示房号、漏网的内行词。现在它**只读 core**。
    what: '世界里某位参与者的页：公开资料 + 发消息；它是不是你的 Agent / 你的联系人各有其页（互链不合并）',
    guarded: false,
  },
  { path: '/app/discover', side: 'demo', planes: [], page: 'pages/DiscoverPage.tsx', what: '发现：官方 Agent 与外部接入口（Core v1 无 Directory 契约）', guarded: true },

  // ── 执行端（Agent 产品：Agent 住的那台机器）────────────────────────────────
  // ⚠ **2026-09-25 组织轴刀**：这一侧的**主语是机器**（执行端），不是 Agent。
  //    那一区读的全是机器级事实（在不在跑、醒过几次、用哪条模型），轴与事实必须一致。
  //
  // ⚠ **2026-09-26 修一处自相矛盾**：这里原先写「Agent 侧因此**没有独立列表页**」，
  //    而**三行之后就声明了 `/app/agents` 这个列表页** —— 同一份文件里两句话打架。
  //    事实是：**有**一个 Agent 索引的执行侧视图（`/app/agents`，每行拼世界侧所有权 +
  //    家侧三格读数），它**2026-09-29 之前**不在壳层导航的任何分区里（只从「我的」页与
  //    「新的朋友」页进得去）—— 那条"一级内容拿二级待遇"已在本日修掉（见本块末尾）。
  //    但"没有独立列表页"这个说法是错的，已改掉。
  //
  //    归属判据（2026-09-26 立，第一性原理）：**把 im 服务端换成微信，这一页还成立吗？**
  //    成立 ⇒ `world`；不成立 ⇒ `home`。按它量出来：
  //    - `/app/agents`：每行读**家侧读数** ⇒ 不成立 ⇒ `home`（声明正确）；
  //    - `/app/agents/:participantId`：**当时**只读 core（资料 / 接线），**不读家条目** ⇒
  //      成立 ⇒ 该是 `world`（原声明 `home` 是错的，2026-09-26 改正）。
  //      ⚠ "不读家条目"这一条**已被 2026-09-29 修订**（它现在也读家）—— 声明**仍是 `world`**，
  //      依据换成下面那条"判据管页面主体"。见下。
  //
  // ⚠ **2026-09-29**：`/app/agents/:participantId` 现在**也读家了** —— 它补了第二段
  //    「机器」（主语是机器：它住在哪台 ＋ 跳机器页的出口，见 `api/agentMachine.ts`），
  //    因为那一页原先是个**死胡同**（出口只有"返回"与"撤销"，运行状态一个字都不说）。
  //    **声明仍是 `world`，依据是本文件头上那一条**：「判据管**页面主体**，不管有没有引用
  //    对方」—— 这一页的主体还是世界侧的公开资料与接线，末段是**引用**机器那一侧的事实，
  //    且那一段的主语是机器、不复述机器页的运行读数。
  //    ⚠ **原来的已知边界已收**（2026-10-11）：这里曾写「`RouteSide` 是**单选**，表达不了
  //      "一页两段、两段各一侧"」—— 现在由 `planes` 表达（**有序**：第一个是主体那一侧），
  //      本页因此是 `planes: ['world', 'home']`。
  //      ⚠ **仍然没做的一格**：段落在页面里的**位置**（锚点 `#machine`）今天只写在 `what` 那一句里。
  //      要机械化到锚点级，得把 `planes` 升成 `sections: [{ anchor, side }]` —— **今天没有消费者**
  //      （守卫量的是"读了哪几个平面"，不需要锚点），所以不做。
  //
  // ⚠ **2026-09-29 第二处**：`/app/agents` 进了壳层导航的「执行端」组（第二个入口）——
  //    它原先**不在任何分区里**（唯一入口是「我的」页那张卡），一级内容拿二级待遇。
  { path: '/app/runtimes', side: 'home', planes: ['home'], page: 'pages/RuntimesPage.tsx', what: '我的机器：跑 Agent 的机器列表（一台一行；本机那条是预置的）', guarded: false },
  { path: '/app/runtimes/:entryId', side: 'home', planes: ['home'], page: 'pages/RuntimesPage.tsx', what: '一台机器：运行 / 活动 / 模型三个房间 + 上面跑着谁', guarded: false },
  // ★ **2026-10-11 按机器寻址**：这一页管的是**这一台机器**上的钥匙清单（清单归机器、
  //   绑定归住户）⇒ 它挂在机器那条路径下，与「模型」那一间是同一台机器的两个深度
  //   （那一间给读数与换绑，这一页给增删改）。判据一句话：**配置跟着它的对象走。**
  { path: '/app/runtimes/:entryId/config', side: 'home', planes: ['home'], page: 'pages/ModelProvidersPage.tsx', what: '某一台机器的模型源清单（增删改）＋ 供应商预设与模型目录；地址里点名谁就配谁，认不出就如实说', guarded: false },
  // ⚠ **`planes` 是两格，而 `side` 仍是 `home`**（2026-10-11）：每行 = 世界侧所有权 ＋ 家侧读数，
  //   而**主体是家侧**（它回答"我的 Agent 现在怎么样"）。顺序就是主次。
  { path: '/app/agents', side: 'home', planes: ['home', 'world'], page: 'pages/MyAgentsPage.tsx', what: '我的 Agent 列表（**Agent 索引的执行侧视图**：每行拼世界侧所有权 + 家侧三格读数）—— **执行端组的一级入口**（2026-09-29 起在壳层导航里）', guarded: false },
  {
    path: '/app/agents/:participantId',
    side: 'world',
    planes: ['world', 'home'],
    page: 'pages/MyAgentsPage.tsx',
    what: '★ **四段、顺序有判据**（2026-10-20 T4：这一页从"管理台"改成"对象首页"）：①「机器」（`#machine`）**主语是机器** —— 读家侧裁决（`resolveAgentHome`）说"它住在哪台"＋**它现在有没有在跑**（同一句、同源），并给去 `/app/runtimes/:entryId` 的出口（**不复述**运行状态明细，那是机器页三个房间的唯一权威）；②「资料」（`#who`）世界侧的公开资料与归属（只读 core）；③「接线（高级）」（`#wiring`）接入 ＋ 凭据 —— **默认折叠、折叠头带读数、没有生效的接入时默认展开**（2026-10-08 裁决第①类「必须由人同意的」：必须在，但不必在最前）；④「危险操作」（`#danger`）撤销 Agent。首屏两个动作都在页头：**发消息**（判据在 `api/directRoom.ts`，与 `/agent/:id` 同一处）与**在世界里看**（去 `/agent/:id`，那一页才是"它在世界里的样子"的唯一版面）。已撤销的（`revoked`）只渲染页头 ＋ 后果 ＋ 返回（`api/agentRevoked.ts`）',
    guarded: false,
  },
  // ⚠ **2026-10-11 修陈旧**：这一格从前写「…深链接不破，**旧锚点经 `LEGACY_ANCHORS` 映射**」
  //   —— 而这个别名**故意不带锚点**（`App.tsx` 的 `LegacyRuntimeRedirect` 里写着为什么：
  //   那张映射表住在需要 `entryId` 的机器页里）。映射服务的是**直接深链到机器页**的旧书签
  //   （`/app/runtimes/<entryId>#home`），不是这条别名 ⇒ 那句话把两件事说成了一件。
  { path: '/app/runtime', side: 'home', planes: [], page: null, what: '旧「运行状态」地址 ⇒ 重定向到机器列表（**不带锚点**；锚点映射服务的是直接深链到机器页的旧书签）', guarded: false },
  { path: '/app/experience', side: 'home', planes: [], page: null, what: '旧「经历」地址 ⇒ 重定向到机器列表（深链接不破）', guarded: false },

  // ── 设置区（关于我和这台设备）────────────────────────────────────────────
  // ⚠ **`planes` 在这一区最要紧**（2026-10-11）：`side: 'settings'` 是"两侧都可能"的逃逸口，
  //   所以每一页都要自己回答"它读的是哪一侧的数据"。量出来的事实分三档：
  //   - 读**家面**（那台机器的状态口 / 配置门 / 家条目）：`local-service` / `homes`；
  //   - 读**世界面**：`general`（服务器地址与 token 是**那个世界**的东西 ＋ 兑换邀请）；
  //   - **两侧都不读**：其余 —— 它们读的是**这台设备上的客户端状态**（外观 / 通知开关 / 索引），
  //     那不是任何一个平面（`worldIdentity.ts` 的 `celira.machine:` 一族）。
  //     ⚠ 别把"机器级"直接读成"家面"：家面是**读那台机器的门**，而这一族只读浏览器自己的库。
  { path: '/settings', side: 'settings', planes: [], page: 'pages/settings/SettingsLayout.tsx', what: '设置首页：通向下面各分区的索引', guarded: false },
  { path: '/settings/general', side: 'settings', planes: ['world'], page: 'pages/settings/SettingsGeneralPage.tsx', what: '通用：连接设置（服务器地址 + token）与退出登录', guarded: false },
  { path: '/settings/appearance', side: 'settings', planes: [], page: 'pages/settings/SettingsAppearancePage.tsx', what: '外观：主题、配色变体、字号、壁纸', guarded: false },
  { path: '/settings/notifications', side: 'settings', planes: [], page: 'pages/settings/SettingsNotificationsPage.tsx', what: '通知：浏览器通知授权与提及提醒开关', guarded: false },
  // ★ 2026-10-11：这一页**收成一个主语**（名字也从「服务器」改成「本机服务」）——
  //   它读的全是**这台机器**（`planes: ['home']` ✓），而从前的名字与它所在的分组都写着
  //   "这个世界" ✗，页面上还混着两行**世界侧**读数（服务器地址 / 账号）。
  //   那两行的权威在 `general`（地址在那里可编辑）⇒ 这一页不再复述它们。
  //   ⚠ 地址仍是 `/settings/server`（**旧名字留在 URL 上**）：改地址要连带别名与用例，
  //   而这一刀只动"名与组"。⚠ 这条路径字符串**不许**再当"服务器"三个字用（那是 `general` 的词）。
  { path: '/settings/server', side: 'settings', planes: ['home'], page: 'pages/settings/SettingsServerPage.tsx', what: '本机服务：这台机器上的进程与本机记录（只读）', guarded: false },
  // ⚠ **2026-10-12：`/settings/homes` 退役成别名** —— 机器清单**只留一个门**：
  //   清单在 `/app/runtimes`（一级入口），"加一台"跟着清单走（清单页那颗「添加」就地开表单）、
  //   "管某一台"跟着那台机器（机器页的「这台机器的设置」就地展开）。
  //   从前两页列同一批条目、各说一半 ✗ ⇒ 人要记得"加一台去设置、看状态回执行端"。
  { path: '/settings/homes', side: 'settings', planes: [], page: null, what: '旧「家」地址 ⇒ 重定向到机器列表（清单只留一个门；钥匙与移除在那台机器自己那一页）', guarded: false },
  // ⚠ **2026-10-11：模型配置按机器寻址**（`/app/runtimes/:entryId/config`，见上面执行端那一块）。
  //   判据：它管的是**某台机器**的钥匙清单（`<家>/model_sources.json`）⇒ 配置跟着它的对象走。
  //   这一条从此是**别名**（重定向）：带 `?home=<id>` 落那台机器，没有上下文落执行端列表
  //   （**不猜是哪台**，与 `/app/runtime` 同款）。`side` 保留 `settings`（地址还在那个命名空间里）。
  { path: '/settings/providers', side: 'settings', planes: [], page: null, what: '旧「模型配置」地址 ⇒ 按机器寻址的配置页（带 ?home= 落那台；没有上下文落执行端列表，不猜）', guarded: false },
  // 关于页那一行版本号是**壳给的本机读数**（`api/appVersion.ts` → 壳的 `app_version_readout`，
  // 读 exe 旁边的 `VERSION`）—— 它是**这一份 app 自己的身份**，不是世界面也不是家面的事实
  // （判据：把 im 换成微信/QQ，这一页照旧成立）⇒ 声明仍然是空的。
  { path: '/settings/about', side: 'settings', planes: [], page: 'pages/settings/SettingsAboutPage.tsx', what: '关于 Celira（版本号从壳里读：这一份在跑哪一版）', guarded: false },

  // ── 两侧都能开 ───────────────────────────────────────────────────────────
  // `/app/profile` 是「我的」：`side: 'shared'` 说的是**两种世界下都开得出来**（没连世界时也开）。
  // ⚠ **`planes` 是 `world`，不是两侧**（2026-10-11 量出来）：它读的是**世界的身份**（"我是谁"）
  //   ＋ 世界侧所有权（"我接入的 Agent"那几个名字与条数）；那张家侧 Agent 摘要**是一张卡片
  //   （点了跳 `/app/agents`），不是家侧读数**。所以它**不读家面** —— 别把 `shared` 读成"两侧都读"。
  { path: '/app/profile', side: 'shared', planes: ['world'], page: 'pages/ProfilePage.tsx', what: '我的：身份卡 + 我接入的 Agent 摘要 + 外观 / 通知入口', guarded: false },
  { path: '*', side: 'shared', planes: [], page: 'pages/NotFoundPage.tsx', what: '未找到（404 兜底页）', guarded: false },
]

/** 声明里说"真实世界被诚实门拦下"的那些路径。 */
export function demoRoutePaths(routes: readonly RouteDeclaration[] = ROUTES): string[] {
  return routes.filter((r) => r.side === 'demo').map((r) => r.path)
}

/**
 * **真实世界里进不去、但真实用户点得到的入口**该被算成缺陷。
 *
 * 这个函数只回答一个问题：给定一条路由，它在真实世界里是否"页面自己说得出话"。
 * `demo` 是唯一说不出的那一侧（守卫直接换成诚实门）。
 */
export function reachableInRealWorld(route: RouteDeclaration): boolean {
  return route.side !== 'demo'
}

/**
 * **按平面量的那些路由**（`tests/route-planes.test.ts` 的输入）。
 *
 * `demo` 侧**不在射程里**：它读的是 fixtures，不属于任何一个平面（真实世界已被诚实门拦下）。
 * 判据写在这里而不是测试里 —— 它是"哪一页归哪一侧"这条判据的一部分，权威只有一处。
 */
export function planeCheckedRoutes(routes: readonly RouteDeclaration[] = ROUTES): RouteDeclaration[] {
  return routes.filter((route) => route.side !== 'demo')
}
