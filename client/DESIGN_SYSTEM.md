# Celira Web 设计系统硬规则

本目录（`client/`）是 Celira 前端。首要目标是**视觉一致与可组装**：后续任何人（包括 AI）只组合组件与页面，不自由发挥样式。颜色、字号、圆角只来自语义令牌；页面只做组件组合与状态流转。

> 本文只保留当前设计与交互规则中无法由 lint/视觉约束完全推导的部分。本机路径现在是 `node home.js up` 起家、浏览器进登录页**直连 core**（`../docs/client/LOCAL_CLIENT_HOST.md` 所述 Host 窄 BFF 已随 Host 线退役并删除）；Mock 模式只作无后端演示兜底，绝不用于真实连接。
>
> ⚠ **2026-09-20 退役标注（Host 模式规则一律按历史读）**：下文凡以「Host 模式」为主语的规则（§外观个性化边界、§导航壳的 Host 遮蔽、§核心交互配方的 Host 分支、§Host 模式能力边界整节、§视觉回归的原前置），以及凡引用 `host.ts` / `hostMessages.ts` / `hostCall.ts` / `hostUi.ts`、`HostGate`、`CallOverlay`、启动探测、`/api/v1/local/*` 窄 BFF 的行，描述的都是**已随 Host 线整体删除**的那套界面行为（考古走 Git 历史）。**这些规则作为历史留着**——它们记录的是那套窄 BFF 的界面行为约定，**新规则不得据此外推**。（`HostRouteGuard` 自 TASK_56 起已改名 `FixtureRouteGuard`，判据从 `mode === 'host'` 换成「是不是演示通道」。）

## 分层

| 层 | 位置 | 规则 |
|----|------|------|
| 令牌 | `src/styles/tokens.css` | 所有颜色、圆角、字号标尺的唯一事实源。新增颜色必须先在这里定义语义变量（双主题各一组） |
| 原子件 | `src/components/ui/` | shadcn/ui 生成件，不手改样式，需要变体走 cva |
| 领域组件 | `src/components/celira/` | 消费令牌与原子件，暴露 props。业务视觉在这里封装 |
| Mock | `src/fixtures/` | 类型化假数据，字段对齐 Core 领域术语 |
| 页面 | `src/pages/` | 只做组件组合 + 状态流转，禁止出现颜色类名 |

## 禁止事项（Lint 强制）

`npm run lint` 会拦截以下内容，CI 级报错：

1. **禁止 hex 颜色**：字面量色值只允许出现在 `tokens.css`；
2. **禁止 Tailwind arbitrary 颜色值**：如 `bg-[#xxx]`、`text-[oklch(...)]`、`from-[hsl(...)]`；
3. **禁止裸色类与裸色阶**：`bg-white`、`text-black`、`bg-black/50`、`text-gray-400`、`bg-purple-500` 等一律禁用（扫描范围含 `index.html`）；
4. **禁止内联 style 颜色**（例外见文末登记表）；
5. 颜色一律使用语义类：`bg-background`、`text-muted-foreground`、`bg-primary`、`text-agent`、`bg-presence-thinking` 等。

## 语义令牌速查

- 品牌：`brand`（Logo 紫）、`primary`（主行动）
- 表面：`background` / `card` / `popover` / `muted` / `accent` / `border` / `input` / `ring`
- Agent 身份：`agent`（青色文字/描边）、`agent-surface`（Agent 头像与徽章底色）
- 气泡：`bubble-self-from/foreground`（自己，**纯色**；2026-10-20 起正文字段不再用渐变）、`bubble-self-to`（只给头像与预设小样的渐变用）、`bubble-peer/foreground`（对方）
- 状态（回复、沉默、超时、失败必须语义分离，禁止混用）：`presence-online` / `presence-thinking` / `presence-silent` / `presence-timeout` / `presence-failed`

## 其他硬规则

- **图标**：统一 `lucide-react`，禁止用 emoji 当图标；
- **主题**：深色（默认主题·深空紫调）为默认，浅色（晨雾浅紫）通过 `ThemeToggle` 切换；入口唯一化：桌面端 = 导航栏左下角，移动端 = 我的/设置页，聊天头部等场景页禁止再放切换按钮（设计陈列室除外）；新组件必须双主题目测；
- **主题预设**：明暗模式之外，官方风格以「预设」组合（目录 `src/lib/themes.ts`；**样式是「包」**——`src/styles/themes/<id>.css`，**一个包 = 一个文件 = 一个作用域**，文件名即作用域名，判据：**一条规则属于主题 X ⟺ 它的每一个选择器分支都带 `[data-theme='X']`**，令牌与质感/形态覆盖——透明度、模糊、辉光、切角、字族、纹理——都写在包里；**默认主题（`celira`）不是包**，它是基底 `:root`/`.dark`，留在 `tokens.css`）——组件代码永不感知主题；设置页经 `ThemePresetCard` 真实预览切换（嵌套 data-theme 作用域把预览限制在卡片内），偏好只存**本机持久面**（`api/storage.ts` 窄口：浏览器里是 `localStorage`，壳里落 `<app_config_dir>/local-store.json`；键 `celira-theme` 明暗 / `celira-theme-preset` 预设；第一帧的值由 `main.tsx` 在挂载**之前**补水，见 `client/README.md`「两段式启动」），不进 Core；新预设必须在陈列室「主题预设」节补双模式预览；官方与社区二创新预设的提交规范见 `THEME_GUIDE.md`；
- **用户主题**（2026-10-08 刀 1）：外观这一层是前端**唯一**的合法装载口（判据见 `AGENTS.md` §前端边界的「扩展接缝」）。用户在设置页「我的主题」导入一份**带作用域的 CSS**，落本机、注入 `#celira-user-themes`（**本机装的全部**，不只是生效那份）、写 `data-theme='user-…'`；**组件零改动**（它只消费语义令牌，永不感知主题）。分界是刻意的——**格式三条硬要求拦人**（每条规则必须带作用域／日与夜两档齐全／零外部引用，权威 `src/lib/themeFile.ts`），**体检不拦人**（对比度／五态两两可辨／Agent 色与主行动色可区分，权威 `src/lib/themeAudit.ts`，且 `unknown` 与 `ok` 严格分开报：**"我没检查"≠"没问题"**）。用户主题生效期间配色变体让位（变体是官方预设那一套里的东西）。**字体只能用本机已有的**——不能随主题带字体文件（字体必须随仓库分发）；要从成品起步就用这一区的「导出」拿一套官方主题当底稿。格式与验收口径的权威在 `THEME_GUIDE.md`「用户主题」节。**装之前先看（2026-10-08 刀 2）**：同一份主题、作用域换成**保留 id**（`themeFile.ts` 的 `PREVIEW_SCOPE_ID`）就是一张小样，靠**嵌套 `data-theme`** 关在卡片里（`ThemeScopePreview` —— 官方预设的预览、用户主题的小样、**已装列表里的真样张**都是它，视觉只此一份）。**装完之后也看得到（刀 4）**：已装那一列每条都带真样张，**切之前就能比**——它成立的前提是注入的是**全部**已装主题（没生效的那些在文档里但永不命中：作用域只带各自的 id）。**三个 `<style>` 各有唯一写者**：官方包归启动那一处（`#celira-theme-packages`，全量、一次性），本机装的全部用户主题归 `use-theme.tsx`（`#celira-user-themes`，持久），小样归设置页那一区（`#celira-theme-preview`，**瞬时**，闸门一关整个移除）。小样**只按当前明暗**渲染、**不落任何持久面**；保留 id 不许被主题文件占用（否则小样会漏到全应用）。
- **配色变体**：每套主题预设自带若干**手工策展**的配色变体（目录在各 preset 的 `variants` 字段，如默认主题：原色/星蓝/蔷薇），组件样式与布局完全不变、只换「色彩声音」——tokens.css 文末配色变体层按 `[data-theme='…'][data-variant='…']` 只覆盖 `primary` / `brand` / `ring` / `bubble-self-*` / `accent` 系令牌，保留原令牌明度档位故对比度不劣化，禁止触碰 agent 系、presence 五态与 destructive/code 语义色；数值由 `scripts/variant-lab.mjs gen` 生成（oklch 保 l/c 拧策展色相、色域内收敛）；'standard' 不写 data-variant、完全跟随主题原色；每个主题的记忆各自独立，持久化 key `celira-theme-variant`（JSON 映射 `{presetId: variantId}`）；
- **字号**：三档（目录 `FONT_SIZES`，小 / 标准 / 大）——只覆盖语义文本令牌 `--text-display/title/body/caption/micro`（`html[data-font-size=…]`），是纯阅读/无障碍偏好，**绝不缩放根字号、`--spacing`、按钮 padding、图标、布局或触控几何**；小档不把 caption 压到 0.75rem 以下、micro 压到 0.625rem 以下；'standard' 不写属性、用默认字阶；持久化 key `celira-font-size`；
- **内嵌字体**：只允许随仓库分发的本地字体，`@font-face` 在 `tokens.css` 按主题声明、`font-display: swap`，从 `/fonts/` 加载，**禁止外网字体 CDN**；paper 用思源宋体子集、pixel 用缝合像素子集（均 OFL，许可证文本已随 `public/fonts/` 提交，详见 `THEME_GUIDE.md`「内嵌本地字体」节）；
- **聊天壁纸**：官方策展 SVG（`public/wallpapers/`，目录 `OFFICIAL_WALLPAPERS`）+ 本地上传（图片 Blob 存 IndexedDB `celira-appearance`，**不进键值窄口** —— 它体积可能大）；偏好结构 = 全局 + 按会话覆盖（`rooms[roomId]` 优先，含显式 'none'），蒙版弱/中/强三档保可读；壁纸层只在聊天页消息区（`ChatWallpaper`，absolute + pointer-events 穿透 + 语义蒙版），持久化 key `celira-wallpaper` / `celira-last-room`（经同一个窄口，`lib/wallpaper.ts` 出首帧快照）；
- **参照**：`GalleryPage`（陈列室）是所有组件的正确用法示例，写新组件前先对齐它，并在其中补一节；
- **Human 与 Agent 视觉分离**：Agent 的名字、头像、徽章、活动指示永远使用 `agent` 系令牌，不得用 primary 紫；
- **可交互元素**：必须有 `cursor-pointer` 与 hover 反馈；过渡 150–300ms；
- **文字对比**：正文用 `foreground`，次要文字最低 `muted-foreground`，禁止更浅的自定义灰。

## 外观个性化边界

外观呈现是**纯本机浏览器状态**，与 Core 事实、Host BFF、Runtime 完全隔离，不得跨任何协议传输：

- **存储只落本机**：偏好（明暗/预设/配色变体/字号/壁纸选择/房间置顶免打扰/草稿）走**本机存储窄口**（`api/storage.ts`：浏览器里是 `localStorage`，壳里是家里的文件存储），自定义壁纸图片 Blob 存 IndexedDB（库 `celira-appearance`，独立于 core 消息缓存的 `celira-cache`）；主题切换经 `use-theme.tsx` 只写 `html[data-theme|data-variant|data-font-size]` 与 `dark` class 同源属性；
- **渲染不触数据面**：`ChatWallpaper` 只是消息区底下 `pointer-events-none` 的装饰图 + 语义蒙版，绝不影响消息列表的虚拟化/滚动/取数/已读/撤回/附件行为；默认壁纸为 `none`，未选择时零像素渲染差异；
- **会话覆盖是 UI 偏好**：`saveLastRoom` 只记录最后停留房间 id/name 供设置页「仅此会话」作用范围使用，不改动任何房间事实；
- **禁止上抛**：不允许把外观偏好写入 Core/Host message/event/read cursor 或任何持久公共事实；不允许第二份表征源（除 `src/lib/{themes,wallpaper}.ts` 与 tokens.css 外不得另存主题事实）。

## 新增一个组件的流程

1. 需要新颜色？→ 先加进 `tokens.css`（双主题各一组）；
2. 在 `components/celira/` 建组件，只用令牌类名与 ui 原子件；
3. 在 `GalleryPage` 补展示节；
4. `npm run verify` 全绿才算完成。

## 新增一个页面的配方

页面 = 布局容器 + 领域组件 + 数据（经 `src/api/`）。禁止在页面里出现颜色类名与像素魔法数（间距用 Tailwind 标准档）。根容器用 `flex h-full min-h-0 flex-col`，**不要自己写 `h-screen`/`min-h-screen`**：窗口高度只有一个来源 —— 路由根的窗口框 `components/celira/AppFrame.tsx`（标题栏 + 提示条 + 内容，`tests/shell-titlebar.test.ts` 机械钉着「只有它写 `h-screen`」）；新页面必须在 App.tsx 注册并走路由级代码分割（`lazy()` + `Suspense`，首屏只保留 SplashPage 静态加载，fallback 用统一 `RouteFallback`）。

## 排版

字号只允许使用 5 档类型标尺（定义在 `tokens.css` 的 `@theme` 中），禁止散装 `text-sm/xs/[10px]`：

| 类 | 用途 |
|----|------|
| `text-display` | 页面标题 |
| `text-title` | 组件标题、房间名、参与者名 |
| `text-body` | 正文、消息气泡 |
| `text-caption` | 辅助说明、时间戳 |
| `text-micro` | 徽章、分组标签（配 `tracking-wider uppercase`） |

唯一例外：头像 initials 的字号跟随头像尺寸（`ParticipantAvatar` 内部已封装，不得在其他地方模仿）。

## 响应式

- 三档断点：移动（`<md`，单栏堆栈）、平板（`md`~`xl`，两栏）、桌面（`≥xl`，两栏为主；**聊天页右侧参与者栏不常驻**——抽屉「参与者」tab 是唯一成员面板，头部「参与者」图标按钮为其入口，禁止再引入常驻第三栏）；
- 新页面必须在三档下都自洽：禁止固定像素宽度（如 `w-[300px]`），侧栏一律 `w-full md:w-72` 式写法；
- **桌面端禁止居中窄列偷懒**：内容区容器上限 `max-w-5xl`（聊天消息列、参与者网格）；阅读线由内容元素自身控制（如气泡 `max-w-[70%]`），不靠容器收窄；设置/资料类表单容器例外，上限 `max-w-3xl`；
- 列表型页面在桌面端优先用卡片网格（`md:grid md:grid-cols-2 xl:grid-cols-3`）；
- 移动端固定浮层不得遮挡输入区；
- Radix `ScrollArea` 内容需横向收缩时，必须加 `[&_[data-radix-scroll-area-viewport]>div]:!block`（长文本截断依赖它）。

## 导航壳

- **窗口框只有一层**（2026-10-20 立）：路由根的 `components/celira/AppFrame.tsx` 画**标题栏**（品牌 + 最小化/最大化/关闭），再按 **栏 → 提示条 → 内容** 排。判据两条：① **窗口最上面那一条只归窗口** —— 换世界/断线这类"这一趟旅程的状态"属于内容，长在栏**下面**（2026-10-20 主人真机："这个提示的位置说得过去吗。居然能在关闭窗口的上面"）；② **高度只由这一层给**（框 `h-screen`、内容那格 `min-h-0 flex-1`，页面根 `h-full`）—— 于是任何一条提示出现时是**页面缩**，不是整页多一根滚动条。别在某一屏里另画一条栏，也别让页面自己撑窗口；`tests/shell-titlebar.test.ts` 把这两条都钉在源码上。

- 路由即产品地图（react-router v7，`src/App.tsx`）：`/`（重定向到 `/splash`）、`/splash`、`/app`（index→chat / chat/:roomId / chat/:roomId/history / contacts / contacts/:participantId / contacts/new / discover / runtime / experience / **runtimes / runtimes/:entryId / runtimes/:entryId/config** / agents / agents/:participantId / profile）、`/settings`（index / general / appearance / notifications / server / homes / about）、`/settings/providers`（**别名** ⇒ 按机器寻址的配置页）、`/agent/:participantId`、`/gallery`、`*`；
  ⚠ 这份清单**必须与 `App.tsx` 同步**：2026-09-26 审计发现它当时缺 `/app/runtimes` 与 `/app/runtimes/:entryId`（最新、最重的两条），并多列了已删除的 `/settings/devices`。**权威是 `routeRegistry.ts` 的 `ROUTES`**（`route-admission.test.ts` 读 `App.tsx` 逐条对表），这一段只是给人看的副本——**改了路由记得改这里**；
  **每条路由的归属侧由 `src/api/routeRegistry.ts` 声明，`tests/route-admission.test.ts` 会读 `App.tsx` 的真实路由表逐条对表**（新增路由没声明、声明陈旧、或"demo ⇔ 在守卫下"对不上都红）；**同一条声明还写着这一页读哪几个平面**（`planes`），由 `tests/route-planes.test.ts` 顺着页面模块的 import 闭包对表——别再靠"记得决定"。
  ⚠ **2026-10-11 加同一条纪律的第二处**：**带锚点的滚动只许有一份实现**（`src/lib/anchorScroll.ts`），由 `tests/first-run-landing.test.ts` 全仓扫 `scrollIntoView` 守着（各页自己写一份 = "滚一次就收工"，而真机上量过它到不了位）；同一份用例还钉着**第一次那条路的顺序** —— 机器页那张「先给这台机器配一条模型源」的卡**必须排在三个房间之前**（判据：**动作排在读数之前**，2026-10-11 主人裁）。
- **真实世界遮蔽（`FixtureRouteGuard`；TASK_56 从 `HostRouteGuard` 改名）**：`/gallery` 与 `/app/discover` 在守卫内——**真实世界**里渲染诚实「尚未提供」（它们是 fixture 管理面；判据是"手里有没有真实凭据"，见 `isRealWorldSession`）。`/app/runtime`、`/app/runtimes`、**`/app/runtimes/:entryId/config`**（2026-10-11 起模型配置按机器寻址）与 **`/agent/:participantId`** **刻意在守卫之外**：它们不是 fixture 页，页面自身显示诚实状态（机器读不到就说读不到；参与者目录读不到就说读不到）。**新增路由时必须先决定它属于哪一侧**（真实连接下能不能开）——现在这条由 `routeRegistry.ts` + `route-admission.test.ts` 强制；
  ⚠ **2026-09-26 把 `/agent/:participantId` 搬出守卫**：它原先整页读 fixtures（假模型 / 假绑定 / 写死的「在线」），而**真实世界有三处入口链到它**（消息头像 / 右键「查看主页」/ 联系人资料卡）⇒ 真实用户点得到、点进去撞门（审计 H9）。重写成**世界侧参与者页**（只读 core：参与者目录 + `getAgentProfile` 能力探测）后搬出，`side` 同步由 `demo` 改成 `world`。**"搬出守卫"与"改声明"必须同一刀完成**，否则 `route-admission.test.ts` 会红；
  ⚠ **2026-09-25 删掉 `/identity`**：标题「你是谁？先创建你的 Human 身份」、按钮「创建身份并进入」，实际两个按钮都只是 `navigate('/app/chat')`，**一个身份都不创建**；而真实身份是 CLI 签发的（`celira-im create-human`）。承诺不实的路由不留。`SplashPage` 的「进入」随之改为回正门 `/`；
- 所有主页面必须挂在 `/app` 布局路由（`src/components/celira/AppShell.tsx`）下，禁止脱离壳的独立页面（进入旅程 splash/identity、设置二级页、agent 主页、陈列室除外）；
- 壳提供：移动端底部 TabBar + 桌面左侧图标栏，两者共享**同一份分组数据**（`src/components/celira/navGroups.ts` 的 `NAV_GROUPS`，**真实世界那份派生**是 `navGroupsFor(true)`，唯一摊平派生 `flattenNavItems`；NavLink 驱动），新增入口只往对应分区加一项并配好路由；
- **真实世界里不摆"点进去必然撞门"的入口**（2026-10-03 刀）：`navGroupsFor(realWorld)` 按路由表自己的判据（`routeRegistry.reachableInRealWorld` —— `demo` 是唯一"真实世界进不去"的侧）把那些入口摘掉；当下就是「发现」（`/app/discover`）。依据：**任何"真实用户点得到、点进去却撞门"的入口都是缺陷**（`routeRegistry.ts` 原话）。这是**派生**：契约落地 ⇒ 那条路由的 `side` 一改，入口自动回来（`tests/nav-groups.test.ts` 钉着这条"跟着路由表走，不跟人走"）；
- **导航是真分区，不是一排平级 tab**：上组「世界」（IM：消息 · 联系人 · 发现）+ 下组「**执行端**」（Agent 产品：**我的执行端 · 我的 Agent**）——**连线归世界，生活归执行端**。桌面在两组之间画分隔线并给每组 `role="group"` + 名称，移动 TabBar 按同序排出、组间一条竖分隔线（两端只是呈现差异，**入口集合只能来自那一份数组**）。分区规则：入口的归属按「这件事属于世界还是属于执行端」判，**页面本体不因归组而搬家**；⚠ **2026-09-29 执行端组补第二个入口**：下组原写「**我的执行端** 一个入口」，本次补上「**我的 Agent**」（`/app/agents`）。补的依据不是审美，是两件实测事实——① 那一页**原先不在任何分区里**（唯一入口是「我的」页里那张「我接入的 AGENT」卡），一级内容拿二级待遇；② 2026-09-25 去掉它的理由（"Agent 优先的页读机器读数 ⇒ 运行状态代表谁答不出来"）**针对的是那一页当时的形状**，而它 2026-09-26 起已是一张明说自己拼两件事的清单（`routeRegistry.ts` 里 `side: 'home'`：每行 = 世界侧所有权 + 家侧三格读数），不再冒充运行状态。**轴没有变**（两个入口都在执行端组、都读家）；⚠ **2026-09-25 组织轴刀**：那一区的**主语仍然是机器**，`/app/runtimes` 一台一行、`/app/runtimes/:entryId` 一台一页（**运行 / 活动 / 模型**三间）；而 Agent 主页（`/app/agents/:participantId`）2026-09-29 起是**两段**——「资料」（世界侧，主体）＋「机器」（**主语是机器**：它住在哪台 ＋ 跳机器页的出口，`#machine`）⇒ 两个平面**双向都通**（机器页「上面跑的是 X」也可点回 Agent 页）。⚠ **2026-09-22 刀2a 收敛**（历史）：下组曾按**数据来源**铺四个平级入口，那次把它们收进一个 Agent 主页的四个房间（是谁 / 家 / 日子 / 脑子）——**那组比喻房间名已随本次清掉**（它们不是产品名）。**路由本体全留**（`/app/runtime`、`/app/experience` 变重定向到 `/app/runtimes`，**不带 hash** —— 列表页没有房间可滚；旧锚点 `#home`/`#days`/`#mind` 只在**带 entryId 的地址**上由 `RuntimeDetail` 映射，`/settings/providers` 没搬）；**入口存在 ≠ 页面可用**——真实世界里未接通的页面由 `FixtureRouteGuard` 渲染诚实「尚未提供」（⚠ `/settings/providers` 自 TASK_77 起**已在守卫之外**：它读的是本机模型门、写本机私有配置，不是 fixtures 页，非真实世界由页面自身处理），分区不给任何页面造假通行证；设置区语义收敛为「关于我和这台设备」。
- **用户可见字符串说用户的语言（人话刀，2026-09-22 主人裁决）**：**设计词汇给设计者，界面语言给用户——两套词典不许串门**。用户可见面（`title` / `label` / `description` / `children` 文本、`aria-label`、错误人话、按钮名）上**禁用**本项目的内部设计词典：`清醒`／`唤醒`／`准入`／`收场`／`读数`／`管家`／`门铃`／`答话`／`本机这份家`／`在岗`／`标称窗口`／`线协议`／`摘除`，以及同族的**内部组件名** `Core`／`Runtime`。⚠ **代码注释里照旧合法**（那是给开发者的）。
  - **词表落点**：`src/lib/plain-language.ts`（`resultLabel` / `stateLabel` / `nextLabel`）—— 只做「机械取值 → 人话」，**认不出的取值原样交出来**（编词就等于告诉下一刀"这一格已经翻译过了"）；要新增映射就加在那里，别在页面里就地写 if。
  - **它是机械判据**：`scripts/verify-ui.mjs` 把上面那张词表当**反向断言**过一遍渲染后的 `innerText`（「Agent 主页里没有内行词 —— 一个都没有 ✓」），外加一条正向锚点（「待命/已暂停在，而不是『在岗（准入 …）』」）⇒ **新写文案时它会拦住你**。
  - **别越界**：陈列室（`/gallery`，给设计者的参照页）、fixtures 演示正文、活动明细里的**机械标识**（折叠区内）不属于"用户可见面"，照旧可以用设计词汇／原始取值。
  - **⚠ 人话刀砍的是"实现词"，不是"难词"（2026-10-03 主人裁决补的第三格）**：*"要符合用户直觉的那种……语义化，也不是小白化，因为假设用户懂点 AI"*。所以判据是**词的取舍不看难易，看它是不是"用户要操作的那件事"的名字**：
    - **概念词**（`世界` / `执行端` / `Agent` / `模型`）＝ 用户要操作的对象 ⇒ **保留产品词**，不换成大白话（换成"我的机器"这类临时口语反而丢了 `PRODUCT_VISION` 的「一词一主」）；
    - **实现词**（上面那份禁用表）＝ 我们内部怎么做的 ⇒ 不上屏；
    - **门槛必须能被跨过**（可验收）：一个懂点 AI 的人第一次看到那个词，**同一屏**里能不能拿到"它是什么"的一句？能 ⇒ 合格的门槛；不能 ⇒ 黑话。范例：`/app/runtimes` 的「这里列的是跑 Agent 的机器（执行端）」、`/app/agents` 的「这份清单拼两件事：世界里你拥有的 Agent，＋ 它们在哪台机器（执行端）上、在不在跑（机器上的数据）」、机器页三间各自的 `subject`。
    - **机械判据**：`tests/page-subject-lines.test.ts` 钉着"同一屏那句"不许消失（钉事实不钉整句）；`scripts/check-voice.mjs` 继续管禁用词与人称代词（⚠ 产品面注释里也不许出现"她/他"—— 那会同时断言性别与单一连续人格，2026-10-03 被它拦过一次）。
- **NavLink 函数式 className 禁止放进 `TooltipTrigger asChild`（Radix Slot 会把函数字符串化）**：需要 Tooltip 的导航项用 `useLocation` 算好字符串 className 再传入；
- **品牌位与"我的/设置"入口唯一**：品牌移动端在 ConversationList 头部、桌面端在图标栏顶部 Logo；"我的"桌面端在图标栏底部头像、移动端在 TabBar；页面组件内禁止重复放置 Logo、"我的"或设置入口。

## 可访问性

- 焦点环：全局 base 层统一（`index.css`，`outline: 2px solid var(--ring)`），组件不得自行关闭 outline；
- 图标-only 按钮必须有 `aria-label`；列表行按钮带上下文（如 ConversationItem 报房间名 + 未读数）；
- 状态不得只用颜色表达：presence/连接状态/发送状态都必须伴随文字或图标；
- 骨架屏容器加 `aria-hidden`，不干扰读屏。

## 核心交互配方

- **消息正文**一律经 `MarkdownText` 渲染（react-markdown + remark-gfm + remark-math + rehype-highlight，lowlight common 语言集）；`body` 契约保持纯文本，MD 只是显示层；提及高亮用 `components.text` 注入 `MentionText`，禁止 rehype-raw；代码高亮颜色只来自 tokens.css 的 `--code-*` 语义令牌与 `.hljs-*` 映射（禁止引入 hljs 主题 CSS）；行内 code/pre 背景：self 气泡用 `bg-bubble-self-foreground/20`，peer 用 `bg-muted`；代码块 `overflow-x-auto` 防溢出；
- **Mermaid 块**经 `MermaidBlock` 渲染：动态 import（不进主包）、按「内容 + 主题」双 key 缓存 SVG、`startOnLoad: false` + `securityLevel: 'strict'`、render 串行化、先 parse() 校验、IntersectionObserver 延迟渲染；**解析失败必须降级为代码原文 + 「图表解析失败」提示行**；流式中一律显示代码原文不渲染；`$..$`/`$$..$$` 经 `FormulaBlock` 动态 katex（`throwOnError: false`；remark-math 默认只认 `$$`，禁止开启 `singleDollarTextMath`）；mermaid 主题色只从 tokens.css 语义令牌映射；块级代码带语言标签与复制按钮；
- **撤回是终态 tombstone**：撤回后消息投影为居中系统行「你撤回了一条消息 / X 撤回了一条消息」（`SystemMessage` 的 Undo2 变体），**无气泡、无头像、无操作菜单、不可回复**（recalled 消息不会进 `MessageContextMenu`）；引用目标已撤回 → 引用条与 `ReplyQuote` 一律显示「[该消息已撤回]」占位并降透明度，禁止展示撤回前正文；Host 模式撤回成功后不伪造本地消息事实，只触发受 epoch 保护的权威首屏重读收敛 tombstone（tombstone/引用占位只来自 Core 权威历史）；
- **回复闭环唯一路径**：`MessageContextMenu` 回复 → `MessageInput` 上方引用条（可取消）→ 发送后气泡带 `ReplyQuote`（点击跳转原消息）；回复目标从已加载权威 timeline 解析；未加载时按 opaque `before` 分页补早（不猜 cursor、不无限循环）；`ReplyQuote` 由调用方传入当前已加载的真实目标消息/参与者 resolver，组件不查 fixtures；
- **@提及只有 `MentionPopover` 一种形态**（输入 `@` 触发，Agent 优先排序，键盘可选）：建议源 = Core 成员契约（core 模式）的真实成员投影，**判据是 id / 成员资格，不是 kind** —— 群房间里**任意 active 成员（Human 或 Agent）都可被 @**（`CORE_V1_MENTION_TARGETS.md`；`api/mentionTargets.ts` 里 kind 只用于**排序偏好**）；选择后以结构化 `mentions` 随消息提交，渲染层按 `mentions` 高亮 `@名字`；direct / human_direct 不识别 `@`；禁止页面自造提及下拉；
- **消息操作菜单只有 `MessageContextMenu` 一种形态**（桌面右键、移动长按 500ms 合成 contextmenu 复用同一菜单），禁止第二套操作菜单；菜单项按消息归属裁剪：复制 / 回复 / **转发**（仅正文非空时出现，打开 `ForwardSheet` 选目标会话）/（Agent）查看主页 /（仅自己，destructive）撤回 / **删除 · 仅为我**（本机隐藏，见下）；撤回菜单只对真实 Core id、当前 Human 自己、已提交、未撤回的消息开放（Agent/他人消息不出现可执行撤回动作；菜单不按本地 120 秒隐藏，超时由 Core 返回明确用户错误）；
  ⚠ **转发的附件语义（2026-10-08 刀）**：转发 = 在目标会话发一条**新消息**；附件**一起转**，做法是**取原字节 → 在目标房重新上传一份 → 绑进新消息**（契约里附件绑定绑死在"上传者本人 ＋ 同 Room ＋ 24h 窗口"，跨房引用做不到 ⇒ 这是 §6 说的"归上层"那条路，**不动冻结契约**）。编排**只在 `lib/forwardMessage.ts`**（纯逻辑、`node --test` 直测），组件只接依赖与说结果；判据是**全有或全无**：任一附件没转成 ⇒ **那条消息不发**并点名是哪个文件（半转发比不转发更糟）。三条代价如实登记在 `client/README.md`「已知缺口」：**字节存两份**、**"转发自谁"没有事实可依**（正文是作者写的，客户端不伪造）、**撤回联动断开**（转出去的是副本）；发送那一半的失败走既有的失败气泡＋重试（**不在这里再报一次**）；
  ⚠ **「删除 · 仅为我」（2026-10-08）与本条其余动作不是一类**：撤回是**公共动作**（对面也看不见了，归 Core），这个是**本机视图** —— 世界里那条消息一动没动，只是这台机器上的名单把它滤掉了（名单判据只在 `lib/hiddenMessages.ts`，存取在 `api/hiddenMessages.ts`，世界作用域 ＋ 房间级）。所以：菜单里**名字必须带「仅为我」**（不许与撤回读成一件）；隐藏之后**必须说得出话**且说出真相 —— 「已隐藏 1 条 · **只在你这边**」＋**撤销**（这个仓库不用弹窗做确认）；**存不住要报**（`role="alert"`：「没能保存（刷新之后它还会回来）」）；一条都被隐藏时**不许**说成「这个会话还没有消息」（那是假话，要说「这个会话的消息都被你隐藏了（只在你这边）」）；`chatFeed` 仍按**世界里的条数**判，别拿过滤后的数目去判"有没有消息"；
  ⚠ **多选（2026-10-08）**：菜单里「**选择**」进入多选态（把这一条当第一项），此后**点气泡 = 勾选/取消**（`data-selected` 落在 DOM 上，样式用 ring）；**输入区被多选条换掉**（此刻底下那一格的动作是"对这批做什么"，不是"再写一条"），三个动作：**转发 / 复制 / 删除 · 仅为我**（后者的语义见上一条）＋取消。⚠ **批量动作的顺序一律按时间线**（不是点击先后）——判据只在 `lib/messageSelection.ts`（`selectedInOrder` / `copyTextOf`），调用方不许自己 `map`/`join` 一遍：按点击顺序发出去，对面读到的就是一段被打乱的对话，而且**没人会当场发现**。批量转发交给 `ForwardSheet` 的是**一批**（`messages: Message[]`），逐条转、**半途失败要说清"前 N 条已经转过去了"**（已经发出去的是真的发出去了，不许假装没发生）；多条时标题写明「按时间顺序」。
  ⚠ **2026-09-26 补登记「转发」**：这一项在代码里**早就存在**（`MessageContextMenu.tsx` 的 `onForward` + `ForwardSheet`），而**配方清单里没有它**，仓库文档、`outputs/`、`.workbuddy/` 也查不到任何登记 —— 审计把它列为"多出来的菜单项"。核实后判定**代码是对的、文档是旧的**：转发是一个真功能（有自己的 sheet 与目标选择），所以**登记进配方**，而不是删掉功能。同一次审计还发现未读角标出现了**第三处**（联系人资料卡），那一处是**代码错**，已摘（见下面「未读角标只在两处」）；
- **发送状态流转（mock 模式）**：本地发送先 `sending`（Clock 脉冲动画）→ 约 0.8s 后 `delivered`（小概率 `failed`）；失败文案「发送失败，点击重试」整行可点（`store.retry` 复用同一幂等 key）；`delivered` 是内部投递态、**不渲染任何回执 UI**（系统没有「对端已确认」这个事实，故不渲染借自传统 IM 的送达/确认回执），禁止把失败做成纯红字不可交互。Host/core 模式：pending/sending/failed 状态由 store 数据面驱动，失败可重试（重试复用同一逻辑发送的幂等 key），成功用 Core Message 替换 pending；
- **发送**：Enter 发送、Shift+Enter 换行；发送经 `store` 动作（mock 入 mock 内存后端，Host/core 走真实 HTTP 幂等提交），切换会话不丢失；同 tick 重复提交原子隔离（每个逻辑发送一个内存幂等 key，同一 tick 的重复点击/Enter 不会生成第二个 key 或公共消息）；
- **新消息可达性**：进入会话 → 滚到底 + 清零未读；自己发送 → 无条件滚到底；收到新消息时若在底部（距底 < 64px）→ 跟随滚动 + 清零，否则保留未读 → `NewMessagePill` 出现（count 为真实未读，点击滚到底 + 清零）；未读经 store 投影到 `useRooms`，会话列表角标与 AppShell 汇总自动联动；禁止硬编码 pill 或静态未读数；
- **未读角标只在两处**：会话列表行内 + AppShell 消息 tab 汇总（`bg-primary` 圆点，99+ 截断）；`UnreadDivider` 按未读消息 id 集合动态渲染（显示在第一条未读消息前，读完后消失），禁止硬编码消息 id 定位；
- **导航里的第二种点 = Agent 的存活点（不是未读语义，已登记）**（⚠ **该点已随 Host 线删除**：TASK_63 刀 C 删掉了 `api/hostUi.ts`、`agentLiveness` 与壳层那盏点，机房的读数现在只在运行状态页；以下规则作为**当时的设计记录**留着）：AppShell「运行状态」（`/app/runtime`，住在「Agent」组）tab 上的点表示本机执行端存活（判据单一事实源 `api/hostUi.ts` 的 `agentLiveness` + `AGENT_LIVENESS_TONE`）——**只在非正常时出现**（在岗/未知不挂点）；**颜色跟判据走**（组件层唯一映射 `components/celira/status-tone.ts`；TASK_69 名字收尾前叫 `liveness-tone.ts`）：读不到（`unreachable`，是「看不见 Agent」不等于 Agent 停了）= `bg-presence-timeout` 橙，自己出问题（`cut_off` 被世界拒 / `broken` 内部断）= `bg-destructive` 红（多 Agent 时仅红档示条数），未接入/换发中 = `bg-muted-foreground` 灰（后者不是故障）。与未读角标同处渲染但语义不同：**不得复用它表达未读，也不得为未读再加新位**；
- **时间分隔**：`DateDivider` 按 `Message.day`（今天/昨天/日期）变化渲染，禁止硬编码「今天」；消息气泡内始终 HH:mm；会话列表 `lastActivityAt` 显示「今天→HH:mm、其余→day 标签」；
- **系统消息只有 `SystemMessage` 一种形态**：居中 caption 行 + 小图标，**无气泡无头像、不参与 compact 成组、无操作菜单**（成员加入、Agent 上下线、撤回等事件都用它，禁止做成气泡）；「你/参与者撤回」使用真实 selfId/当前参与者投影，不用 fixtures（fail-closed：真实 resolver 上下文下 selfId 缺失绝不回退 fixtures SELF_ID；未知作者只显示中性「参与者」）；
- **同作者连续消息成组**：隐藏头像与名字、间距收紧至 12px（`-mt-3`）、气泡圆角收敛为 `rounded-xl`；禁止大圆角气泡紧贴；消息内附件与气泡之间同样保持 12px（`gap-3`）；
- **入口（正门）只有 `LoginGate` 一种形态**：产品定位是「client 连任意 core」，所以启动的第一问是「**连哪个服务器 + 你是谁**」，不是「我在不在本机 Host 里」。唯一分派点是 `api/entry.ts` 的 `decideFrontDoor(prefs)`（纯函数、有测试，**只收持久偏好一个参数**）：手里有可直接复用的直连凭据（`mode === 'core'` 且 token 非空）→ **自动直连**，不经过登录页；否则 → `/`、`/splash` 第一屏渲染 `LoginGate`（服务器地址**留空 = 同源** + token + 「连接」）。**Host 门（`HostGate`）与启动探测（`detectHost`）已随 2026-09-20 Host 线退役并删除**：收窄前「探不到」（网络失败 / 畸形响应 ⇒ fail-closed 留在 Host 语境）与「可证明不是 Host」（2xx + `text/html` ⇒ 直连或登录页）会导向不同结果，这个区分**随 Host 语境一并消失**——登录页本身就是「由用户自己决定连哪台」的诚实结果，所以不丢事实。禁止绕过 `decideFrontDoor` 另写一套入口判断；禁止把「非 Host 来源」做成错误墙；禁止静默回退 Mock。直连 Core 是所有自部署用户的正常登录，不是开发者选项（原 `HostGate` 的出口按钮即叫「**连接设置**」，该组件已删除）。连接设置唯一持久面 = `celira.connection.prefs`（`mode`／`token`／`serverUrl`；**新增字段必须带默认值**，空地址 = 同源），凭据不得写入任何其它持久层；
  - ⚠ **2026-10-03 补第三格（`local-ask`）**：桌面 app 的默认形态是**成品**（`PROJECT_SHAPE.md` §9.1），而 §8.6-1 裁的是「**第一次启动问一次**"要不要在这台机器上跑一个世界"（它会写磁盘、占端口、常驻后台）」⇒ 没有可用凭据、且**壳说还没表过态**时，第一屏问那一句。它是**机器级前置**，**不是第二条连接路径**：分派点仍然只有 `decideFrontDoor` 一处（它多收一格 `localAsk`）、组件仍然只有 `LoginGate` 一个（那一格是它的 `mode="run-local"`），而且**只在壳里可能出现**（判据是"有没有壳的内部注入"，浏览器里恒不存在）。两个出口都诚实：开 ⇒ 壳起世界、签一支会话、直接进；不开 ⇒ 落回「连接」那一格去连别的世界。
- **断线提示只有 `ConnectionBanner` 一种形态**：`connecting`／`offline`／`backend-offline` 三态，offline/backend-offline 带「立即重试」按钮，点击转 connecting（spinner），恢复后横幅消失；**已无「启动探测完成前只显示诚实加载态」这一档**（探测随 Host 线删除），措辞也不预设来源；入口归属一律按上面的**入口（正门）**规则走，绝不静默回退 Mock；
- **输入草稿**：切换会话后输入内容保留（`MessageInput` 模块级 drafts Map，发送成功或切换房间时恢复/清空），附件预览条与草稿不跨房间共享；
  ⚠ **附件预览的内存缓存（2026-10-08）**：图片/语音/视频三条路都走 `useAttachmentSource` ⇒ 那一层是**唯一**取字节的地方，也是唯一该做缓存的地方。判据：取回来的 object URL 进一块**有上限的会话内缓存**（`lib/previewCache.ts`：条数 ＋ 字节数两头都卡、LRU 逐出时 `revokeObjectURL`、同键 in-flight 去重）；**命中是同步的**（hook 的初值就从缓存里拿）⇒ 滚回去不重下、也不闪。⚠ **卸载时不许再 revoke**（那是从前的写法，等于每次滚回来重下一遍）；撤销只发生在**逐出 / 清空**时，清空点在**撤销会话**与**换世界**两处（`SettingsGeneralPage`）。⚠ **它不落盘、也不是第二份权威**：同一批字节、随时可整个丢掉、刷新即空；**落盘的附件缓存有意不做**（家里已经存着字节，再存一份要配配额/逐出/撤回失效三套策略，而浏览器形态还有 HTTP 缓存），理由登记在 `client/README.md`。⚠ **已知的机制重复**：`hooks/use-member-avatar.ts` 里有同款"Map ＋ in-flight"缓存，**不合并** —— 它要**负缓存**（"确定没有头像"必须记住，否则每次渲染都打一次 404）且数量受成员数约束，与"按字节卡的 LRU"是两条不同的策略（登记在 `client/README.md`）。
  ⚠ **整窗拖放（2026-10-08）**：拖进窗口**任意处**都能把文件加进这条消息（用户不会瞄着输入区拖）。⚠ **判据只有一处** —— 拖放全挂在 `window` 上（`MessageInput` 的 `dragover` / `dragleave` / `drop`），**容器上那三个 handler 必须没有**，否则同一次拖放被接两遍、**文件进两条**。三条纪律：**只接"拖的是文件"**（`dataTransfer.types` 里有 `Files`；页面内拖文字/排序不许被这张网拦下）；**传不了附件就完全不拦**（既不 `preventDefault` 也不亮提示，别挡浏览器自己的行为）；`dragleave` 只在**离开窗口**（`relatedTarget` 为空）时收提示（元素之间移动也会发它）。提示层文案是「**松开就加到这条消息里**」（拖进来只是加进待发条，还要用户自己按发送），且必须 `pointer-events-none` —— **忘了它，提示层自己会把 drop 吃掉，松手反而没反应**。
- **连接恢复（core 模式）**：WS 断线自动指数退避重连（2s 起 ×2、上限 30s，onopen 成功重置）；致命关闭码（1008/4401/4403 = 认证失败）停止自动重连，连接状态进 `reconnecting`；`retryConnection` 是唯一手动重连入口；恢复 online 后自动 `pollEvents` + `refreshRooms` 补拉；
- **多标签页（core 模式）**：BroadcastChannel `celira-v1` 只广播脏标记（rooms.changed / messages.changed / read.cursor / prefs.changed），**禁止跨 tab 传数据**——接收端一律走 `pollEvents`/REST 恢复循环；mock 模式不参与；Live 临时投影不走 BroadcastChannel（每 tab 独立 Live WS）；
- **浏览器通知**：唯一入口是设置页「新消息通知」真实开关（`Notification.requestPermission()` 授权流程，被拒时显示浏览器级提示，禁止假授权）；触发条件 = 新消息到达且页面在后台（`document.hidden`）+ 房间未免打扰 + 非自己发送；同房间 30s 窗口聚合为「N 条新消息」；点击通知聚焦窗口并跳转对应房间；⚠ **壳里那一半 2026-10-08 接上了，但机制不同**：Web 通知在壳里**永远开不了**（wry 只放行剪贴板读权限 ⇒ WebView2 恒 `denied`），壳里走**系统通知**（壳命令 `notify_message`）；两档的差别、权限那一格为什么"壳里没有"、以及两个已知缺口（点通知不跳房、不能替换上一条）**只在 `src/api/notifications.ts` 的文件头**，组件与设置页不重复判；设置页文案**按形态分**（壳里说"窗口不在前台时提醒新消息"与"系统的通知设置"，**不许**再说"浏览器设置"——那句在壳里是误导）；
- **IndexedDB 消息缓存（core 模式）**：库 `celira-cache`、store `rooms`、key=roomId、按 token 隔离；`loadRoomMessages` 先缓存秒显再网络覆盖；写缓存 500ms 防抖；`deleteRoom`/`leaveRoom` 清对应缓存；mock 模式不读写；**Host 模式对消息与幂等 key 不写 localStorage/sessionStorage/IndexedDB**；
- **长列表**：会话列表与消息列表一律虚拟化（`@tanstack/react-virtual`）；消息列表 dynamic 模式（measureElement + ResizeObserver，图片加载/高度变化自适应）、`overscan` 12；向上滚动近顶（<80px）触发 `loadOlder` 并用 scrollHeight 差恢复锚点（双 rAF）；吸底只发生在「自己发消息或原本近底」时，禁止每次新消息强制滚动；日期/未读分隔线作为消息行的一部分渲染；历史消息用 `MemoizedMessageBubble`（自定义比较：message 引用 + compact + highlighted）；搜索结果定位用 `scrollToIndex`，禁止 `getElementById`/`scrollIntoView`（虚拟化下未渲染行无 DOM）；
- **键盘快捷键**：输入区未聚焦时 `↑` 编辑自己最近一条消息（复用草稿区）；`Ctrl/Cmd+K` 聚焦会话列表搜索框、`Esc` 清空；其余快捷键保持现状；禁止新增与浏览器/无障碍冲突的全局键位；
- **会话列表搜索框**是全局搜索唯一入口：输入 ≥2 字符且聚焦时显示「消息（已加载范围）」结果分组（客户端过滤 `messagesByRoom`，core 模式标注范围，禁止伪造服务端搜索契约）；消息结果点击跳转房间并定位；
- **房间信息只有 `RoomInfoDrawer` 一种形态**：tab 按模式裁剪——聊天页「参与者」tab 是唯一成员面板（core 模式含角色徽标/邀请/移除/转让，接 store 动作，禁止页面直连）；~~Host 模式只显示成员，无置顶/免打扰/退出/成员管理（这些契约尚未经 Host BFF 接入）~~（**Host 模式已于 2026-09-20 退役**，见下节）；
- **覆盖物一律走 Radix 层，不靠插入顺序**（2026-10-03 记账）：房间抽屉（`Sheet`）、灯箱（`ImagePreviewLightbox`）、各 `Sheet` 都是 Radix Dialog primitive —— 一律 portal 到 body、层梯 `z-50`，**灯箱是 `z-[60]`**（它开在抽屉之上；主题的装饰扫描线同为 60，由 `body::after` 最后绘制，仍在最上）；每个覆盖物自管 `aria-hidden` 与焦点。**新覆盖物必须照此 portal**；两个覆盖物同层时**禁止**让「谁后挂上 DOM」决定谁在上面 —— 真机撞到过：抽屉开着时点「图片」页签缩略图，灯箱被抽屉压在下面 ⇒ 「下载/关闭预览」被抽屉子树截走指针、灯箱整体落进 `aria-hidden` 子树、键盘焦点被抽屉的焦点陷阱锁住（读数 `outputs/_seal-attachment-layer-probe.out`；修法见 `outputs/_seal-fix-lightbox-layering.out`）；
- **发起新会话只有 `NewRoomSheet` 一个入口组件**（会话列表头部 "+"、空态页按钮共用）：单选 Agent = **回到你们那间**——一间房 = 一段关系（Core 批次二，2026-09-27）：同一对 Human–Agent 只有一间私人房，已有直接进，没有则由 Core 建首房；Core 对"同一对再要一次"回放原房（`idempotency_replayed`），客户端不再有「新建话题房」这条路（连标题入参一起退场）；单选联系人创建私聊、填群名创建群聊（core 模式走真实端点）；
- **Agent 聚合行**：一行 = 一个 Agent（聚合行）/ Human / 群；同一 Agent 的全部直聊房聚合为单行（`ConversationList` 按 peer.kind==='agent' 且 2 人房间聚合，列表序=活跃序，不引入树/嵌套）；聚合行右侧上槽永远时间、下槽永远动态状态（未读=各房之和），**徽标禁止进右侧**；点击进最近活跃那间房。**一间房 = 一段关系（Core 批次二）之后，同一对的私人房至多一间** ⇒ 聚合在常见情形下就是一行一房。（⚠ 2026-09-28 服务层退役：原文这里还写着"它的剩余用处是把跨关系周期的服务房收成同一行"——服务房那第二种房已随 Agent Service 层删除 ⇒ **今天同一对只有一间房，聚合行的剩余用处是同一 Agent 的群房**）；
- **Agent 管理页**（`/app/agents`）是 Agent 所有权管理的唯一页面，ProfilePage 只放摘要行；页面结构映射契约的三个独立事实，必须分区呈现、不得合并：**所有权**（创建时间、责任声明）、**接入**（Backend Binding：状态徽章、接入方标签）、**凭证**（签发记录列表，撤销是 destructive 动作）；**接入徽章只说授权本身**——`生效中` / `已撤销`（与凭证那一间同一套词），**不许**说 `已连接` / `离线`、也不许借 `presence-*` 那颗点（2026-10-12 主人裁：绑定是授权事实，**不代表 Backend 在线**；用在场那一套词答授权那个问题，会在"绑定 active 而没人在跑她"时显示成假话 —— 真机上撞到过）。"她现在在不在"归**机器页**（本页那一段只给住址，不复述运行状态）；`OneTimeToken` 行内展示一次（含复制按钮），只存组件内存、不落 localStorage、关闭即丢弃，并标注「仅显示这一次」；**撤销与轮换都是当场生效**（同一事务里旧的那把失效 ⇒ 正在跑它的执行端会认证失败停下），这句必须写在按钮旁；「撤销 Agent」接 `DELETE /v1/agents/{id}`（两次点击确认式，禁止 window.confirm），**渲染判据是能力探测（`api/uiPolicy.ts` 的 `supportsAgentRevoke`）而不是模式表**；接入/凭证两栏实现不存在时**整块不渲染**，不给点了必然报错的按钮（**探测要探写面**：只探到读面会让按钮在演示通道里静默无操作——2026-09-26 审计发现的实际缺陷）；
- **设置页**（core 模式）：连接区是「**连接设置**」卡（服务器地址 + token + 「保存并连接」，与 `LoginGate` 同一份 prefs 与同一套错误呈现）与「退出登录」接 `DELETE /v1/sessions/current`（成功后清 token、断连、模式归 `core`（Host 已退役，`ConnectionMode` 只剩 `'core'`，重置后的形状就是「没连过的直连」）、**保留服务器地址**；destructive 双段式确认）；⚠ **以下为历史记录（Host 模式 2026-09-20 已退役）**：Host 模式显示连接、外观与唯一真实的「为我的本机 Agent 配置模型」入口（仅当前 Human 拥有且本机 Runtime 管理的 Agent 可保存 OpenAI-compatible 地址/模型/API Key 并做真实 probe；没有 Provider 目录、多 Provider 管理、市场或一般 Admin 代理）；通知仍不在 Host 模式暗示；页脚为 Host 真实数据表述。**今天**：模型配置走 `/app/runtimes/:entryId/config`（**按机器寻址**：清单增删改 + 供应商预设 + 模型目录；2026-10-11 起，旧地址 `/settings/providers` 是别名）与机器页「模型」房间（读数 + 换绑）。

## Host 模式能力边界（与 LOCAL_CLIENT_HOST.md 一致）

> ⚠ **本节描述的整个模式已于 2026-09-20 随 Host 线退役并整体删除**：`/api/v1/local/*` 窄 BFF、Host 浏览器会话、R7a 运行状态投影、Call 通道与 Host live stream 均已不存在（`LOCAL_CLIENT_HOST.md` 现为退役存档；考古走 Git 历史）。下列规则**作为历史留着**——它们记录的是那套窄 BFF 的界面行为约定，**没有任何实现与之对应，新规则不得据此外推**。（`/app/runtime` 页本身仍在；它的读数现在来自 `node home.js status` 的只读状态口，见 `src/api/homeStatus.ts`。）

- 身份/房间/成员只读（`/api/v1/local/me`、`/api/v1/local/rooms`、`/api/v1/local/rooms/{id}/members`）；
- 消息闭环：首屏权威 Core 历史、显式「加载更早」（仅当 next_before 存在）、纯文本发送 pending/sending/failed + 以原幂等 key 重试、显式提及、回复、作者撤回；
- 固定无事实 SSE 唤醒（收到 `events.available` 后重读已进入房间的权威 Core 首屏，含断线/重新上线补读）；Runtime 写回 Core 的 Agent 公共回复随唤醒重读自动出现并展示；
- **按需真实轨迹（Live）**：含真实 Agent 的房间提供「运行轨迹」入口，用户不打开时不建 Live EventSource、不轮询、不主动读取 Live；打开才 `GET /api/v1/local/rooms/{id}/live` 读权威快照 + 订阅同源 `GET /api/v1/local/live/stream`（固定无事实 `live.available`，REST 重读当前房），关闭/换房/断开/session 换代立即停流并清理旧房投影；同一时刻至多一条 Host live EventSource、去抖合并；面板只显示「当前执行」卡片（Agent 名 + phase + responding 允许正文草稿），activity 为空显示「当前没有运行中的轨迹」，绝不生成/展示 thinking 正文、tool 名称/参数/结果、system prompt 或 provider 信息，绝不复用/渲染 mock 虚构 steps；
- **R7a Agent 运行状态（只读）**：`/app/runtime` 页面经 `GET /api/v1/local/runtime/agents` Host BFF 展示真实 Core-owned Agent 的脱敏执行投影（managed/unmanaged/runtime_unavailable/remote_unsupported + lane/credential/provider/last_run 白名单）；Core display_name 是唯一显示源，绝不展示 system_prompt/owner_id/config_version/凭据/Provider Key/内部配置正文；无任何编辑字段；loading/error（可重试）/empty/未管理/不可用状态必须诚实呈现，绝不用 fixtures 回填；
- **不执行、不代理模型/Completion/ACK（无 token 流式）**；Host read state 已接入：Core unread 是唯一权威，只有 visible + loaded + rendered 的当前房间最新权威 Message 才推进 cursor；PUT 与 rooms 权威回读作为一个整体，生产最多首次加两次 bounded retry，回读成功后才算完成，delivered 且 recalled 的 Core tombstone 也可成为 cursor 目标。**BYOK P1 是唯一例外的配置控制面**：设置页只对当前 Human 拥有、且受管本机 Runtime 管理的 Agent 显示固定 OpenAI-compatible 地址 / 模型 ID / API Key 表单；Key 只在一次 Cookie+CSRF 保护的同源 Host BFF 请求中出现并立即从 React 内存清空，Host 不持久化或调用模型，Runtime 加密保存 Provider revision；「检查连接」是真实 Provider probe，但不写 Core Message、Run、Session、Completion 或 ACK。没有 Provider 目录、多供应商管理、自动发现或一般 Admin 代理，fixtures 绝不回填 Host 模式的身份/房间/消息；
- **Call（CORE_V1_CALL.md 窄解冻切片）**：2 人房间（有 peer）头部语音/视频按钮经 `POST /api/v1/local/calls` 发起，动作（`GET/POST /api/v1/local/calls/{id}[/accept|decline|hangup|signal]`）走 Host BFF 白名单代理；实时信令经同源 `GET /api/v1/local/call/stream`（带事实 SSE：`call.ring`/`call.signal`/`call.state` 含 SDP/ICE）订阅；**媒体由两端浏览器 WebRTC P2P 直连，Host/Core 不接触媒体字节**；未开通场景（群聊/非 host 模式）按钮诚实标注「**尚未开放**」——⚠ **文案已改**（原写「即将开放」）：本仓有一条措辞纪律「**不承诺"即将提供"**」（见 `FixturePageUnavailable`），"尚未"是当下的事实、"即将"是承诺；组件在 `ChatHeader.tsx`（`showCallHint`）；
- Host 不可用或会话身份不匹配一律 fail closed，绝不静默回退 Mock；Host 模式浏览器只拿 Host 会话 Cookie + CSRF + 安全视图，不接收、不存储任何 Core Bearer token。

## 工程约定

- 路由级代码分割：页面组件一律 `lazy()` + `Suspense`（`App.tsx`），新页面不得静态 import 进路由表；首屏只保留 `SplashPage` 静态加载，fallback 用统一的 `RouteFallback`；
- `src/api/` 是唯一数据入口（分层纪律）：`contracts.ts` 定义 Core 公共 JSON 类型；`host.ts`（Host 传输层：会话恢复、只读 BFF、消息闭环、无事实 SSE 唤醒）、`hostMessages.ts`（数据面纯逻辑：去重/合并、幂等与重试、会话 epoch、唤醒重读协调）、`hostUi.ts`（Host UI 策略）**——这三个模块已于 2026-09-20 随 Host 线整体删除（考古走 Git 历史）；`src/api/` 现在只有 `core.ts` 直连与 `mock.ts` 两条数据路**；`core.ts` 对接真实 Core HTTP 与 WS（Bearer token 在设置页配置；`GET /v1/human/events/stream` 首帧 authenticate → events.available 唤醒 → REST 补拉），不改 Core；`mock.ts` 是 mock 后端（fixtures 只读 + 本地覆写：回复模拟、置顶/免打扰/删除会话、投递状态流转、thinking 指示、未读/撤回覆写），兜底 Core 未实现或未部署的功能；`store.ts`（zustand）统一状态：模式（`ConnectionMode` 只剩 `'core'`）与 token 持久化、rooms/messages 缓存、乐观发送 + 失败重试、事件恢复循环。**组件禁止直连 fetch 或绕过 api 层消费 fixtures**；新接口必须先落在 `src/api/` 契约层再接入页面；core 模式不伪造 Backend 行为（无回复模拟/ACK 名单/thinking 动态）；
- 附件（core 模式）走两步式契约：文件选择 → multipart 上传（≤16 MiB 前端先拦）→ pending 附件加入待发条（本地 objectURL 预览）→ 发送时绑定进消息（上限 8 个，满 8 禁用）；**图片渲染一律用 `AttachmentImage`**（core 附件下载需 Bearer，组件内部 fetch blob + objectURL，加载骨架/失败占位），MessageBubble、RoomInfoDrawer 图片墙、Lightbox 都必须用它，禁止 `<img src="/v1/...">` 直连；**附件的显示归类只有一处判据**（`src/lib/attachmentMedia.ts`：图片 / 语音 / 视频 / 文件 —— 先看契约投影的 `kind`，再按 `content_type` 前缀分内联播放类型，**不认文件名**、不做大小写归一）；`MessageBubble` 的分流与 `ChatHistoryPage` 的类型页签都读它，**禁止在组件里另写一遍 `mime` 前缀判断**（那就是第二处判据，两处迟早分叉）；**视频渲染一律用 `VideoAttachment`**（内联 `<video controls>`，与图片/语音**同一条**鉴权 blob 源，禁止 `<video src="/v1/...">` 直连；源拿不到或浏览器解不了那个编码 ⇒ **降级为文件卡片 + 一句提示**，绝不留一块点不动的黑框）；⚠ **媒体元素（`<video>`/`<audio>`）的失败判据不许只靠合成的 `onError`** —— 媒体元素的 `error` **可能在它还没插进 DOM 的时候就落下来**，那一刻它没有祖先，React 挂在容器上的合成事件**收不到那一次**（2026-10-08 真机实测：元素上 `error.code=4` 明明在、`onError` 一次没被调用，界面留一块黑框）；一律**挂原生监听 ＋ 补读 `el.error` 终态**（原件：`VideoAttachment.tsx` 的 `attachVideo`，`key={src}` 保证换源就是新元素）；**下载落哪（2026-10-08 建）**：**落点由 capability 的作用域强制**（壳里只放开 `$DOWNLOAD/**`，见 `app/src-tauri/capabilities/default.json`）—— 页面算出来的名字再离谱也出不了那个圈，这是这一刀唯一的安全边界；文件名是**不可信输入**（对方给的，可能是模型生成的）⇒ 消毒（路径成分 / 控制字符 / Windows 保留名 / 尾随点与空格）＋ **重名加 ` (N)`、绝不覆盖**（覆盖用户已有文件是不可逆操作），判据只在 `lib/downloadName.ts` 一处；**收场读数分两档**：**壳里必须说**（那边没有浏览器那套下载 UI，不说用户就不知道东西存没存下来、存哪了）—— `{ok:true,readout}` ⇒「已保存到 <目录>」；**浏览器里不说**（浏览器自己有下载条，页面不许替它宣布成功）。判据与两档差别**只在 `lib/downloadOutcome.ts` 一处**，组件只说 `downloadOutcomeText(outcome)`；⚠ **壳里落盘走 fs 插件**（Tauri 的下载钩子在本壳里不触发，四组真机实验的账见 `app/README.md` §3.1）；mock 模式保持 demo 附件（原文此处的「Host 模式无附件契约，相关入口隐藏」随 Host 模式 2026-09-20 退役成为历史记录）；
- 已读推进（core 模式）：`markRoomRead` 取房间最新消息 id → `PUT /v1/rooms/{id}/read-cursor`（单调推进），未读权威值来自 `listRooms` 的 `read_state.unread_count`（事件到达后 `refreshRooms` 刷新角标/pill/房间预览）；（原文此处的 Host 模式未读口径——`GET /api/v1/local/rooms` 白名单视图中的 Core `read_state.unread_count` 为唯一权威——随 Host 模式 2026-09-20 退役成为历史记录）当前可见房间只有在历史 loaded 且最新权威消息完成一次 React commit 后才推进 cursor，PUT 与 rooms 权威回读必须共同成功，临时失败走最多两次 bounded retry，回读成功后才算完成，recalled delivered tombstone 可作为合法目标。
- 联系人/群组（core 模式）：ContactsPage 的**联系人**分组来自 `GET /v1/contacts`（pending 显示「接受」按钮）——**这一组不再只是 Human**（批次三第二刀「关系对主体开放」：对端可以是 Agent），所以真实分支按 `peer.kind` 落位并**按 id 去重**（我自己拥有的 Agent 归「我的 Agent」，不在联系人组里重复出现）；新的朋友页请求走 `POST /v1/contacts/requests`（**字段名 `participant_id`**，输入对方 UUID，Human 或 Agent 都可以），NewRoomSheet 单选联系人 → `POST /v1/human-direct-rooms`（仍只接受 Human 对端）、填群名 → `POST /v1/groups`；`member.joined/left` 事件合成「X 加入了/离开了会话」系统行；mock 模式维持 fixtures 演示；
- 后端绑定/凭证（core 模式，MyAgentsPage）：绑定区接 `POST/GET /v1/agents/{id}/backend-bindings` 与 `DELETE .../{binding_id}`；凭证区接 `POST/GET .../credentials` 与 `DELETE .../{credential_id}`——首次签发 `replaces_credential_id: null`、轮换传当前 active Credential ID（CAS）；**token 只显示一次**（OneTimeToken 行内展示，只存组件内存、不落 localStorage、关闭即丢弃）。
- **消息里的链接**（2026-10-08 建）：一律渲染成 `<a href target="_blank" rel="noreferrer">`（`MarkdownText` 的 `a` 覆写），**页面不做任何点击拦截** —— 两种形态各自成立：**浏览器里**就是新标签页；**壳里**由壳的 `on_navigation` / `on_new_window` 接过来，**交给系统浏览器，壳自己永不导航走**（判据与真机证据见 `app/README.md` §3.2）。⚠ 所以**不要**在页面里加"外部链接就 `invoke` 开浏览器"那类拦截：那是第二处判据，而且壳里 `target="_blank"` 那条会先被 wry 吃掉（点了没反应）。协议消毒归 `react-markdown` 的默认 `urlTransform`（**不许**为了"支持更多链接"把它放开）。

## Lint 例外登记

`token-check:allow` 行内注释可豁免令牌检查，已登记的合法例外：

| 位置 | 原因 |
|------|------|
| `GalleryPage.tsx` Swatch 内联 `backgroundColor` | 动态令牌色板，值只能是 `var()` |
| `celira/ProviderMark.tsx` 的 `MONO_INK` 表（**1 行**，只有 `groq`） | **品牌身份色**：Groq 的官方色 `#F55036`（取自上游 `src/Groq/style.ts` 的 `COLOR_PRIMARY`），必须**跨 14 个主题 × 明暗都不变** ⇒ 不该进 `tokens.css`。**只有这一家**：其余 17 家的颜色在 SVG 文件里（`.svg` 不在本检查的扫描范围内），OpenAI / Anthropic / xAI 是 `fill="currentColor"`（本该跟着主题走）。 |

新增例外必须在此表登记，否则视为违规。

## 渲染健全性检查（**不是**像素对齐）

> ⚠ **2026-09-25 裁决：像素对齐拆掉了。** 主人原话：「不要走这种截图对齐了，这种没啥意义」。
> 理由：像素 diff 只能证明"变了"、永远证明不了"变好还是变坏"，于是每次改动都要人肉裁决——
> 成本落在人身上，收益只是"有人看过这一眼"；而**"人看过这一眼"本就不可靠**：同一轮里我自己
> 拿一张压缩到 63KB 的截图去分辨 40px 头像上的青（`#6ec6ff`）与紫（`#c5b8f5`）——**看错了，
> 还把结论当事实说了出来**。连人都会读错的图，做成机械基线，拦住的到底是退化还是误读，分不清。
>
> **真正防「AI 乱写新样式」的不是这个**，是**固定组件 + 语义令牌**：颜色由
> `scripts/check-tokens.mjs` 机械守住（裸 hex / 裸色阶 / arbitrary 颜色 / 内联色一律进不来），
> 视觉决定收进组件（一处决定、全站生效）。视觉回归从来不是那条防线。

- `npm run shots:catalog` 现在只做**渲染健全性检查**，三条判据**都不依赖像素**（所以不会因改样式而误报）：
  1. `dist` 不比 `src` 旧（否则看的是旧界面，绿色是骗人的）；
  2. 页面不许出现 fixture 守卫那句诚实门文案（那是"进错通道"的机械证据）；
  3. `SECTIONS` 里每个展示节必须真的渲染出来**且有面积**（只查存在会放过 0 高度的空节）。
- `npm run verify` 在 lint、build 之后自动跑它。**样式改动不需要更新任何基线**——基线这件事已经没有了：
  `design-baseline-catalog/` 目录、`pixelmatch` / `pngjs` 依赖已一并删除。
- **不要再把像素 diff 加回来。** 要看"这一眼"就直接开 dev server 用眼睛看，或写一次性截图脚本（看完即删）——
  那比维护一份二进制基线便宜得多。
- `design-baseline/`（全页面 × 双主题 × 桌面/移动）是 **Host 通道的遗留基线**：更新它的 `npm run shots`
  （`scripts/screenshot.mjs`）已于 2026-09-20 随 Host 线删除（考古走 Git 历史），该目录**保留但当前没有更新入口**。

### 为什么陈列室单开一条通道（`shots:catalog`）

`/gallery` 挂在 `FixtureRouteGuard` 之下（原 `HostRouteGuard`，TASK_56 改名）：**真实会话（手里有凭据）下它渲染的是诚实门禁页**。于是用整页截图那条通道去截 gallery，拿到的只是那张门禁页——看着有覆盖，实际全无（曾长期如此）。而陈列室里住着**只可能以合成态存在**的展示节（停机告警条三态、连接横幅三态）：真实部署健康时不出告警，这些状态没有真实数据可用。

所以：**陈列室走 legacy 通道**——脚本自己起 `vite preview`，载入前把连接偏好钉成 `core` + 空 token（`DEFAULT_PREFS` 正是这个形状；「core + 空 token」= 演示通道，fixtures 页面放行）。前置只需 `npx vite build`（脚本会校验 `dist` 不比 `src` 旧，过期即拒）。脚本对「截到门禁页」**fail loudly**：拿到诚实门文案即报错，绝不静默放过。

覆盖边界（刻意）：陈列室**其余部分不承担回归职责**——它的定位是「写新组件前先对齐它」的开发参照。新增只可能以合成态存在、又承重的状态节时，把它加进 `scripts/screenshot-catalog.mjs` 的 `SECTIONS`（那只影响"必须渲染出来"这一条，不产生任何基线）。
