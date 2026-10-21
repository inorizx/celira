# Celira 桌面 app（`app/`）

> 状态：**骨架已编译、已运行；壳的接线（`setTransport`）已做**（§3）；**「安家」执行层已落地**
> （§4.1，2026-09-30 首刀）；**「本机世界」已落地**（§4.2，2026-10-03 刀：起世界 + 首次布置 +
> 正门直进 —— 打开就在自己的世界里）。
>
> 实测（2026-09-29，Windows）：窗口标题 "Celira"、WebView2 子进程正常、关闭无残留。
> 实测（2026-10-03，Windows）：**冷启动在临时家里真的起了世界、铸了首位 Human、记下 `human_id`**
> （`examples/local_world_drill.rs` 的读数）；**真壳里第一次启动渲染出那一问**（"你自己的世界"）。

## 1. 它是什么

**完整 app 的桌面形态**（`PROJECT_SHAPE.md` §9.1：**Windows ＝"完整 app：壳 + 本机世界 +
本机执行端"**）——**默认是成品，同时也当得了壳子**（§9.3 那条纪律：*app 里只有一条路：
家条目统一为"地址 + 钥匙"，本机那条只是地址为 loopback 的一条*）。同一个二进制、同一份构建：
打开就在自己的世界里；连别的世界、接入别的机器上的家，是**高级设置里的加法**。

⚠ **"壳"只是它的一个组成面**（把 `client/dist` 端出来 + 出网走 Rust），**不是它的身份**：
从前那句"一份前端构建产物 + 一个窗口"是本目录的旧定位，已被 §9.1 的裁决取代。

进程里的职责四层：

1. 把 `client/dist` 端出来；
2. **当请求的发出者**（见 §3）；
3. **本机世界**（§4.2）：探活优先地确保这台机器上的世界在跑；家里还没主人 ⇒ 首次布置
   （铸首位 Human + 签一支会话）；把本机地址与那支会话交给页面 ⇒ **正门直进**。
   ⚠ **不建 Agent**（§8.5："不存在默认 Agent"）；
4. **安家**（§4.1）：把世界里一支已存在的 Agent 接到**这台机器**上。

⚠ **手机壳是另一回事**（§9.1 / §9.3）：手机操作系统不许常驻后台 ⇒ 手机壳**不能承载世界，
也不能承载执行端**，"壳"那个词只对那一侧成立。

## 2. 怎么构建

```powershell
# 前置：Node.js（前端构建）＋ Rust 工具链（壳）＋ Tauri CLI（见下）
cd app

npm run dev      # 开发：起 vite dev（前端热更新）+ 一个连到它的窗口
npm run build    # 出包：先构建前端，再打 Tauri 安装包
npm run check    # 只做 Rust 侧类型检查（cargo check --all-targets）
```

> ✅ **已于 2026-10-18 收成 `devDependency`**（原样留档如下，方便看出改了什么）：
> 原来的样子是——`tauri` **必须在 PATH 上，而本仓没有声明它**（实测 `@tauri-apps/cli` **全仓零命中**），
> 三条脚本调的是**裸 `tauri`**，实际来源是**全局安装**（`npm i -g @tauri-apps/cli`，本机实测 `tauri-cli 2.11.2`）
> ⇒ **换一台机器 clone 下来直接 `npm run build` 会停在「找不到 tauri」**（与 §6 那两条卡点不同：
> 那两条是网络与平台，这一条是**没写下来的前置**）。
>
> **现在**：`app/package.json` 有 `devDependencies.@tauri-apps/cli`，`app/package-lock.json` 与它一起落盘
> ⇒ **`npm install` / `npm ci` 自带**，不再依赖全局安装，也不再依赖 PATH。⚠ **本仓今天没有 `.github/`
> 也没有 CI**，所以这条前置此前**没有任何机械判据会拦**；现在它至少是**声明出来的**。

> ⚠ **第二个没写下来的前置：四个 release 二进制必须先在场**（2026-10-18 实测，在**导出的发表面**上跑
> `cargo check --offline --all-targets` 抓到的）：`tauri.conf.json` 的 `bundle.resources` 指着
> `../../target/release/celira-im.exe` 与 `../../agent/target/release/{agent_host,agent_run,agent_host_status}.exe`
> —— 那是**构建产物、正确地不在源码里**，但**壳的 build script 在解析资源时就会去找它们**，
> 找不到就 `resource path ... doesn't exist` 并**构建失败**。
> ⇒ **于是三条脚本在新 clone 上只有一条能用**：
>
> | 脚本 | 新 clone 上 | 为什么 |
> |---|---|---|
> | `npm run build`（`tauri build`） | ✅ **能过** | `beforeBuildCommand` 会**先**编那四个（`cargo build --release` 两条） |
> | `npm run dev`（`tauri dev`） | ❌ 失败 | `beforeDevCommand` **只起 vite**，不编二进制 |
> | `npm run check`（裸 `cargo check`） | ❌ 失败 | 它**不跑** `beforeBuildCommand` |
>
> **绕过办法（今天就能用）**：先编一次 release ——
> `cargo build --release --manifest-path ../Cargo.toml` 与 `cargo build --release --manifest-path ../agent/Cargo.toml`
> —— 之后三条脚本都能跑（本机四个二进制实测都在：10.4 / 6.5 / 6.4 / 1.8 MB）。
> **待裁（归第 4 步）**：要不要让 `dev` 与 `check` **自己先编**（改脚本），还是只留这段文档。

**依赖顺序（配置里已写死，不靠人记）**：

```text
client/src  ──(npm --prefix ../client run build)──>  client/dist  ──>  壳读它当窗口内容
```

`client/dist` 是 **gitignore 的构建产物**，所以壳的构建必须排在前端构建之后。这条由 `tauri.conf.json` 的 `beforeBuildCommand` 承担；`beforeDevCommand` 同理负责起 vite dev。

## 3. 为什么 HTTP 从 Rust 侧发（这一刀的技术理由）

浏览器有一条规矩：网页不许拿着身份去跟"别的源"说话，除非服务端明确允许（CORS）。Celira 的 im 服务端**不发任何跨源头**，所以网页版的页面只能跟它同源。

而 app 里请求由 **Rust 侧**发出 —— 那条规矩是加在*网页*这个身份上的，对 Rust 进程不成立。于是：

> **「app 连任意地址的世界」不需要 im 服务端新增任何东西**（不开 CORS、不改任何契约）。

⚠ **这条只在请求真的经由本进程时成立。** 前端必须用 `tauri-plugin-http` 的那份 `fetch`（走 Rust），**不是** webview 自带的 `window.fetch` —— 后者仍然受同源策略管。

**接线已经做完，位置在前端**（2026-09-29）：`client/src/api/shell.ts`，由 `main.tsx` 的第一行调用。

```ts
// client/src/main.tsx —— 在任何网络请求之前
void installShellTransport()
```

```ts
// client/src/api/shell.ts —— 判据是"有没有壳的内部注入"，不是平台名
if (!isTauriRuntime()) return              // 浏览器里什么也不做
const { fetch: rustFetch } = await import('@tauri-apps/plugin-http')
setTransport(shellTransport(rustFetch))    // 壳里换成 Rust 侧出网
```

**为什么放在 `client/` 而不是 `app/`**：页面代码只存在于 `client/` 这一份（`app/` 只负责把它端出来）。**同一份构建在有壳时自动接管、没壳时安静跳过**，因此不需要"web 版一个构建、app 版另一个构建"。

**三个守住点**（判据在 `client/src/api/shell.ts` 的文件头与 `tests/shell.test.ts`）：

1. **相对路径在壳里响亮失败**（`resolveShellUrl`）——壳里没有"页面来源"可解析；静默打到一个与用户意图无关的地址，比报错糟得多。⚠ **这条同时是下一步（家的管理模型）的入口**：app 里那条世界地址从哪来，定了它才不会再产生相对路径；
2. **`signal` 要传下去**——家通道（`homeStatus` / `homeConfig`）靠它做超时，不传就会挂在转圈上；
3. **不要去改 `client.ts`**——它对"谁在发"一无所知，这是这一刀的全部意义。

**WebSocket 不用换**：它不受同源策略管，webview 那份照旧能用（`client/src/api/transport.ts` 纪律 3）。

**能力面（⚠ 2026-10-03 修过一处真缺陷）**：`capabilities/default.json` 的 `http:default` 作用域是
**`http://*:*` 与 `https://*:*`**。

> **那两个 `:*` 不是装饰，是 bug 的修法。** URL 模式的**端口是独立分量**，而 `tauri-plugin-http` 的
> 解析器只补 `pathname` / `search` / `hash`（见该 crate 的 `scope.rs`），**不补端口** ⇒ 旧写法
> `http://*` 只匹配**不带端口**的地址，而世界地址永远带端口（`http://127.0.0.1:8080`）。
> 真机读数（2026-10-03）：页面里每一次出网都回
> `url not allowed on the configured scope: http://127.0.0.1:8080/v1/me` ——
> **也就是这个 app 从写下那句"连任意地址的世界"那天起，从来没真正连上过任何世界**
> （它总是停在登录页；而登录页本身在这个环境里也永远连不上，所以没人碰得到这条缝）。
> 这条路直到 2026-10-03 那一刀（正门直进）才第一次被端到端走到，当场撞出来。
> **判据**：作用域是"**能力面**"，它开错时**失败在出网之前**（网络层根本没有请求），
> 于是世界那侧的日志**一行都不会有** —— 查这类故障要看**页面自己的报错**，不是服务端日志。

**作用域为什么放到任意地址**：世界与执行端的地址由**用户在运行时**填（`PROJECT_SHAPE.md` §4 那条硬要求：绝不编进构建产物），所以构建期不可能收窄成某个域名或端口。真正的授权边界是**用户填的那个地址 + 他手里的 Bearer 凭据**，不是这个通配。

## 3.1 下载落哪（2026-10-08 刀）

**现象**：附件那颗「下载」在**浏览器里是对的、在壳里一点不落盘** —— 点了之后 Downloads 里
一个文件都没有（真机实测：`blob:` 源与同源 `http` 源**都不落**）。

### 走过的弯路：Tauri 的下载钩子在本壳里**不触发**（如实记下来，别有人再走一遍）

第一版按 Tauri 文档接了 `on_download`（`DownloadEvent::Requested` 改 `destination` ⇒ wry 随后
`SetResultFilePath` ＋ `SetHandled(true)`）。**真机上一次都没触发**，四组实验：

| 实验 | 读数 |
|---|---|
| 合成点击（`element.click()`） | 不落盘、**零事件** |
| 可信点击（Playwright 真实输入） | 不落盘、**零事件** |
| 显式用户激活（CDP `Runtime.evaluate` 的 `userGesture: true`） | 不落盘、**零事件** |
| 临时插桩（钩子装没装） | 日志有「attach 被调到了」⇒ **钩子装上了**；`Requested`/`Finished` **一行都没有** |
| 点击是否到达锚点 | `__clicks` 两条都在 ⇒ **点击真的到了** |
| 同源 http 那次点击后 URL | 没导航 ⇒ `download` 属性**被认了** |

排除项：tauri/wry 的接线在源码里完整（`WebviewBuilder::on_download` → `pending.download_handler`
→ `with_download_started_handler`）、WebView2 运行时是 154（`ICoreWebView2_4` 在）、capability 不缺、
页面里没有全局链接拦截、CSP 里没有管下载的指令。⇒ **结论是"这条路在本壳里不通"**，不是"少配了一格"。

### 现在的形状：fs 插件 ＋ 只放开下载目录的 capability

| 环节 | 谁定 | 依据 |
|---|---|---|
| **名字** | 页面（`lib/downloadName.ts`：消毒 ＋ 重名加 ` (N)`） | 名字来自对方（可能是模型生成的）⇒ 不可信输入；这一格跟落盘那一格走，所以搬到了页面侧 |
| **落点** | **capability 的作用域**（`$DOWNLOAD/**`） | 页面算出来的名字再离谱也**出不了那个圈** —— 这是比"页面自觉"硬的边界，也是这一刀唯一的安全边界 |
| **落盘** | 页面调 fs 插件的 `writeFile` | 字节经 IPC（≤16 MiB）；**没有壳侧自定义代码** |
| **收场** | `writeFile` 的 promise | 成功就说得出落在哪（`downloadDir()` 是我们自己算的，macOS 也一样） |

**最小面**：只加 `fs:allow-write-file` 与 `fs:allow-exists` 两条、作用域都只给 `$DOWNLOAD/**`
（**不放开 `fs:default`** —— 那会连带读、删、目录遍历）；**没有 dialog 插件**（"每次都问我"是
另一档，那时才需要它）。浏览器形态**一个字没动**（那边浏览器自己给下载条，页面不宣布结果）。

⚠ 一个如实登记的边界：**`<a download>` 这条路在壳里仍然不落盘**（就是上面那四组实验）。今天
附件下载不走它；页面里 `triggerDownload` 只剩演示源（`localUrl` / `/mock/`）那一支在用。

## 3.2 消息里的链接：交给系统浏览器，壳里永远不导航走（2026-10-08 刀）

**两条都是真机核出来的，不是设想**：

1. **`target="_blank"` 那条今天什么都不发生** —— 而消息里的链接**正好**是这种（`MarkdownText.tsx`
   渲染 `<a href target="_blank" rel="noreferrer">`）⇒ 走 WebView2 的 `NewWindowRequested`，
   而 **wry 在没有注册新窗口处理器时走的是 `args.SetHandled(true)`**
   （`wry-0.55.1/src/webview2/mod.rs:781-783`）：**请求被吃掉**，不开浏览器、也不报错
   —— 用户看到的就是"点了没反应"；
2. **没有导航守卫**：不带 `target` 的链接、`window.location`、重定向都会**把整个 app 顶掉**
   （壳里没有地址栏、没有后退键）。

**修法**（`src/external_links.rs` ＋ `lib.rs` 建主窗口时挂两个处理器）：页面**一个字没改**。

| 处理器 | 管什么 | 动作 |
|---|---|---|
| `on_navigation` | 壳里**原地导航**（不带 target 的链接、`location.href`、重定向） | 自己的来源（`is_app_origin`）放行；http/https 交给系统浏览器后**取消**；其余**取消且不开** |
| `on_new_window` | `target="_blank"` / `window.open` | 同上，然后 `NewWindowResponse::Deny` |

判据是**两个纯函数**（`cargo test --lib` 直测，5 条）：`is_app_origin` 认两种形态
（Windows `http://tauri.localhost`、macOS/Linux `tauri://localhost`，且**主机相等而不是含子串**
—— `https://tauri.localhost.evil.com` 不许蒙混）；`external_target` **只认 http/https**，
`file:` / `javascript:` / `data:` / `mailto:` / 自定义 scheme 一律 `None`。为什么这条要紧：
URL 来自**对方**（可能是模型生成的），`file:` 能在用户机器上打开任意本地文件或拉起关联程序。

**最小面**：用 `tauri-plugin-opener` 的 **Rust 自由函数** `open_url`，**不 `init()` 插件**
⇒ 页面拿不到这个能力，**不需要任何 capability**；边界由上面那两个纯函数守着，不由页面自觉。

**真机证据**（本机跑的一次性探针，跑完即删）：本地服务器收到 `/blank`（`target="_blank"`）
与 `/plain`（导航那条）⇒ 浏览器**真的来取了**；`file:///C:/Windows/System32/calc.exe`
⇒ **谁都没开、计算器没起来**；三次点击后 app 都还在 `http://tauri.localhost` 里。

## 3.3 消息提醒：壳里走**系统通知**（2026-10-08 刀）

**现象（真机量的，不是设想）**：壳里 `Notification` 构造器**在**、`permission` 是 `default`，
而 `Notification.requestPermission()` 回 **`denied`** ⇒ 用户看到的是「**浏览器已拒绝通知权限，
请在浏览器设置中开启**」——**在壳里这句是误导**：没有浏览器设置可开，而且它永远开不了。

**根因**：WebView2 的 Web 通知要**宿主显式回 `Allow`**（规范原文：`PermissionKind.Notification`
的 `PermissionState.Default` **一律当拒绝**），而 wry 注册的权限处理器**只放行剪贴板读**
（`wry-0.55.1/src/webview2/mod.rs:500-513`）⇒ 没人应答，恒 `denied`。

⚠ **`@tauri-apps/plugin-notification` 的 JS 那一侧救不了这一格**（读源码读出来的）：
它在桌面端只是 `window.Notification` 的薄包装 —— `requestPermission()` 直接转发给 Web 那一套、
`sendNotification()` 直接 `new window.Notification(...)`。**能用的是它的 Rust 侧。**

**修法**：壳里加一条自己的命令（`src/native_notify.rs` 的 `notify_message`，用
`NotificationExt` ⇒ Windows 上走系统 toast，**不经过 Web 权限**）；页面侧**只换一个模块**
（`client/src/api/notifications.ts`，它的文件头本来就写着"app 壳那一步要在这里接原生通知——
调用方一行不改"）。

| 环节 | 浏览器 | 壳 |
|---|---|---|
| 能力 | Web Notification API | 壳命令 `notify_message` |
| 权限 | 浏览器自己记（要用户点一次） | **没有这一格**（系统通知的开关归系统设置）⇒ 恒 `granted` |
| 点通知跳房 | `onclick` ⇒ `celira:focus-room` | ⚠ **还没有**（登记在 `api/notifications.ts` 文件头） |
| 同房"替换上一条" | `tag` + `close()` | ⚠ 做不到（系统通知没有句柄）⇒ 聚合窗口内标题带「（N 条新消息）」 |

**最小面**：**不 `init()` 这个插件、不加任何 capability**（页面调的是我们自己的命令，
而 app 自己的命令不走权限系统）；`@tauri-apps/plugin-notification` 这个 npm 包**没装**
（JS 那一侧在壳里没用）。

**真机证据**：设置页那句变成「窗口不在前台时提醒新消息…」（误导文案没了）；开关点一下
**能打开**（从前会被拒）；`invoke('notify_message', …)` 交给系统成功 ⇒ **屏幕上真的弹出
一条 Windows 通知**。

## 4. 这一层**不做**的事（护栏）

- **只做「安家」这一件机器侧的事**（§4.1）：起**一支 Agent 自己的进程**（每支 1 个，一次性），并如实回报。⚠ 它**不是进程管理器**：崩了**不自动重启**（`PROJECT_SHAPE.md` §8.6-2）、不做 **PID 认领**（§8.7）、不做**托盘与退出语义**（§8.2-8.3）——那些在后续刀；**起世界**归 §4.2（本机世界），不在本层。
- **不做同源代转**：web 版靠 `vite.config.ts` / `serve_client.mjs` 代转 `/home-status`、`/home-config`；**app 里没有这一层** —— 本机条目由壳解析出 **loopback 绝对地址**交给页面（§4.2），远程条目一律「绝对地址 + 家钥匙」。
- **不代理、不中继**：壳不是连接平面。
- **不存凭据**：Human token 与家钥匙**不落盘**——首布置/补发拿到的那支 Session 只活在内存里、经壳→页面交一次，随即落进**页面自己的**唯一持久面（`celira.connection.prefs`）；Agent 的后端凭据落**按身份分条**的本地存储（与 `scripts/home/credentials.js` 同一处，一份事实一个权威位置）。

## 4.1 「安家」执行层（2026-09-30 首刀）

**是什么**：`src-tauri/src/anjia/` —— 把世界里**已存在**的一支 Agent 接到**这台机器**上。
六步照 `PROJECT_SHAPE.md` §8.5：核实「这个 id 是我的、且 active」（`GET /v1/agents`，Human Session
经既有 operator CLI 补发）⇒ 建它的环境根（`<家>/agents/<id>/`，**一支一个**）⇒ 写它的身份 ⇒
签后端凭据（既有 HTTP 面；幂等键用壳自己的 `anjia-*` 前缀，**不与 `scripts/home` 的键打架**）⇒
起它的进程（`agent_host`，参数与 `scripts/home/up.js` 逐字节对齐）⇒ **如实回报**（成功 / 失败 / 读不到）。

**三条判据**（照 `scripts/home/adopt.js`，只改第三条）：只能认领自己的（`status=active`，撤销过的不许安家）／
**问不到就不动**／「**这支**是否已经安家」（不是「这个家有没有主」，承 §8.0）。

**安家这只手同时是「住户登记」的写者**（2026-10-03 刀）：建环境根、签凭据、起进程之外，
它**还写**三样机械事实 —— 否则同一台机器上的另一只手（`home.js`）看不见这支住户：

| 写什么 | 落在哪 | 形状权威 |
|---|---|---|
| **支级登记** | `<环境根>/agent.conf`（`agent_id` / `room_id` / `slot` / `display_name`） | `scripts/home/residents.js` 的 `renderAgentConf` |
| **住户账本** | `<家>/logs/agents/<支>/host.pid` ＋ `host.out` | `paths.js` 的 `agentPidFile` / `agentHostLog` |
| **后端凭据** | `<凭据>/<世界摘要>/<身份>.token`（按「世界 × 身份」分条） | `credentials.js` 的 `credentialFile` |

⚠ **房（`room_id`）是问世界的**：`GET /v1/rooms` 里找 `room.kind == "direct"` 且 `peer.id` 就是她的那间
（`adopt.js` 的 `findExistingDirectRoom` 原样）；一间都没有 ⇒ 写空（形状允许 `room_id=`），**不猜**。
⚠ **序号**：`max(已有 slot) + 1`，单调、**绝不回收**（`allocateSlot` 原话：回收会让布置键撞上旧记录）。
⚠ 这三样全是**编排事实**（这台机器上的第几支、跟哪间房绑着、账本在哪）——**主体自己的经历与判断
一个字都不在这里**（判据 9：系统补能力缺口、不替认知判断）。形状与判据收在
`src/registration.rs`（唯一写者 ＋ 唯一形状），并有跨语言钉住的用例（世界摘要对着
`credentials.js` 算出来的真值）。

**已经安家的住户可以补登记**（`ensure_registration`，2026-10-03 刀）：在壳这只手学会写登记
**之前**安下来的住户，环境根与凭据都在、登记却缺 —— 补登记**只补机械事实**（不碰环境根、
不碰进程、内容没变就一个字都不写），并把老形状（扁平那份）的凭据搬进按世界分条的那一格。
命令行把手：`cargo run --example local_world_drill -- register <agent-id>`。

**壳 ↔ 前端（一个命令）**：`anjia_start(agentId)`（`src/anjia/commands.rs`，注册在 `lib.rs`）。
它回的是**读数**——`kind` 稳定词（八态 ＋ 本层新增的 `in-flight`，见 `AnjiaOutcome::kind()`）
＋ pid/路径 ＋ 原样读数行；**人话与四态归页面**（`client/src/api/anjia.ts` 的 `anjiaReadout`，
纯函数 ＋ 用例），壳这边不写第二套话。同一支 Agent **同时只许一轮**（进程内的闸 `AnjiaGate`：
页面重试/StrictMode 双发不会起出两个进程；重复调用拿到 `in-flight`，什么都不动）。
前端按**能力探测**分支（判内部注入，不判平台名），web 侧**安静缺席**。

**怎么手动触发**：

```bash
# ① 壳里（正式面）：前端在 app 里创建成功即自动叫它；命令行验它用临时入口——
CELIRA_HOME_DIR=<家> CELIRA_REPO_DIR=<仓库根> cargo run --example anjia_drill -- <agent-id>
# （`examples/anjia_drill.rs` 是**临时**入口：端到端 drill 收工后即删，删除要登记。）
```

**前置**：世界在跑（本刀**不起世界**）；`home.conf` 里有 `human_id`；模型那一半在场
（`DEEPSEEK_API_KEY` 或环境根里的模型绑定）。缺哪一样它都**拒绝且什么都不动**，并把缺什么写清。

**本层刻意的边界**（如实登记）：① **转发 `home.conf` 的 `mcp_*` 声明**——家级授权（这份家装了的，
每一支住户都看得见），壳起宿主时**原样**交成 `--mcp <名>=<transport>:<命令>`，与 `up.js` 的
`mcpDeclarations` 同一条。**声明面只有配置文件一处**：没有页面、没有安装器——人直接改 `home.conf`
（有 `terminal` 档的 Agent 也能自己改），**改了要重开 Celira 才生效**（宿主起进程时才读）；
② 「已经安家但进程没在跑」**不代它再起**（那是进程生命周期，后续刀）；
③ 平台缝照 `up.js`：`--terminal full` 只给**有终端臂的平台**（Windows 与 Linux——Linux 自五刀起
有 bash 那只手）**并且那一支显式声明了 `terminal=full`**（2026-10-14 主人裁：**平台默认不给手**，
要手只能显式声明）；其余平台一支都不给。**核这条判据时以 `scripts/home/up.js` 为准**（2026-10-01
同步主线时跟齐过一次：旧判据停在「只装 Windows」，会让 Linux 上少给主体一只终端的手）。

## 4.2 「本机世界」执行层（2026-10-03 刀）

**是什么**：`src-tauri/src/local_world.rs` —— **完整 app 自带的那一半**：这台机器上的世界在不在、
地址是什么、我（主人）在不在里面、怎么拿到一支会话。四件事，别的一件不做：

| # | 事 | 判据（与 `scripts/home/` 逐条对齐） |
|---|---|---|
| 1 | **报读数** | 只读：家在哪、世界在不在（`GET /health` 回 200）、主人是谁、家的门两个口 |
| 2 | **起世界** | **探活优先**（已经在跑就不动它，`up.js` 同款）；不在才起：无参数、`cwd` = 家、三个 `CELIRA_IM_*` 环境、日志 `logs/im.out`、就绪判据 `GET /health`（30 秒预算） |
| 3 | **首次布置** | 家里没有 `human_id` ⇒ 页面先问名字（`no-human` ⇒ `NameYourselfCard` ⇒ `local_world_bootstrap { displayName }`），壳再 `bootstrap-human --display-name <人给的那个>`，并把 `human_id` 记进 `home.conf`（**唯一允许的写**，写法照 `conf.js` 的 `confSet`）；已有 ⇒ `issue-human-session` 补一支。⚠ **壳不再从这台机器的用户名造名字**（2026-10-20）：那是机器事实，写进公共世界就是拿它冒充公共事实；三条规则（禁 NUL / ≤80 个 scalar / trim 后非空）与 Core 同一套，权威在 `local_world::normalize_human_name` |
| 4 | **交一支会话** | token **只在内存里交给页面一次**（`local_world_session`）；壳不落盘、不打印、不进日志。**它不铸人**：没有主人时回 `no-human`（一个待回答的问题，不是失败） |
| 5 | **起本机住户**（2026-10-03 第二刀） | 世界在跑 ⇒ 这个家的住户**也该在跑**（§8.1：本地进程是本地 app 的孩子）。判据见下 |

**「打开就进 ⇒ 她就在」**（2026-10-03 第二刀，`src-tauri/src/local_residents.rs`）：
`local_world_enable` 在起完世界之后**确保本机住户都在跑**。三条判据，都不发明：

| 问题 | 判据 |
|---|---|
| **谁是住户** | `<家>/agents/<支>/` 且（有 `agent.conf` 或 `identity.json`）——与 `residents.js` 同款，形状权威是 [`registration`] |
| **主体在这台机器上在不在跑** | **账本**（`logs/agents/<支>/host.pid`，谁起的谁记账）＋ **身份核对**（那个 pid 现在还是我们的宿主吗 —— `process_facts`，照 `proc.js`「杀之前先验身份」）。**不扫进程表**（§8.7 撤掉的正是"认领别人的进程"） |
| **该不该起** | **账本 ＋ 身份核对**决定动作（判据表在 `local_residents::action_for`，带用例）：在跑 ⇒ 不动；账本不在 / 陈旧 / 指向别人 / **核对不了** ⇒ **起它**（接管，`up.js` 同款）。⚠ **presence 只当读数、不当闸**：`GET /v1/rooms/<房>/live` 照问照报（`presence_online` 那一格），但它按契约有 `SEEN_TTL` 滞后 —— 拿它当硬闸，**刚停过的住户就再也起不来**（真机栽过：陈旧账本指向一个打不开的 pid ⇒ `unverifiable` ⇒ 被自己的闸挡死，日志读数就是 `住户 小月：unverifiable`） |

边界（如实登记）：**不起世界**（那是上面那条）；**不轮换凭据**（起的时候**读**本地存储里按
「世界 × 身份」分条那份；读不到 ⇒ 如实拒绝）；**不等报到**（起住户不等 `[listening]` ——
不为了报一句读数把正门堵二十秒；状态由下一次读数给出）；**不承诺生命周期**（崩了不自动重启，
§8.6-2；**停**她归托盘/退出语义那一刀 §8.2-8.3）。

真机验证（2026-10-03，真家）：它的账本是陈旧的（pid 16736，进程早没了）⇒ 读数如实说
`not-running（账本 16736）` ⇒ **起它**：`started —— pid 4188（账本已记）`；它的日志里出现两条
`[wake]`（世界的门铃真的送到了她那儿）；**再跑一次** ⇒ `running（账本 4188）`、
**不起第二个**（幂等）。

### 4.2.1 本机服务的**起 · 看 · 停**（§8.1 / §8.2 / §8.3，2026-10-03 第三刀）

| 动作 | 结果 | 判据/实现 |
|---|---|---|
| **打开 app** | 表过态要跑 ⇒ 世界与住户**都在跑**（幂等） | `ensure_household_at_startup`（**启动那一层**，不是页面那一层 —— 见下）＋ `local_world_enable` |
| **点窗口的 ×** | 窗口收起，**app 还在托盘**，本地进程照旧跑 | `on_window_event` 的 `CloseRequested` ⇒ `prevent_close()` + `hide()` |
| **看"还有没有东西在跑"** | 托盘图标换**带绿点的那一个** ＋ 悬停一句话 ＋ 菜单里一行状态 | `refresh_tray`（§8.2 点名"必须一起做"的那条；图标变体见 `icons/make-tray-running.py`） |
| **托盘菜单「退出（停止本机服务）」** | 世界与住户**跟着停**，然后 app 退出 | `local_services::stop_all`（**停之前先验身份**：只有 `Ownership::Ours` 才动手） |
| **面板总开关**（§8.3） | **界面那一格也做了**（`[全部停止]` ＋ `退出时：停止 / 保持运行`，见 §4.2.1.1） | 四个命令：`local_services_readout` / `local_services_stop` / `local_quit_behaviour_readout` / `local_quit_behaviour_set`；**判断在纯函数里**（`client/src/api/shellLocal.ts`），组件只渲染 |

**为什么"打开 app ⇒ 她就在"必须放在启动那一层**（真机发现的缺口）：回到 app 的用户手里
已经有凭据，正门走的是"直进"、**连本机计划都不做** ⇒ 页面那条 `enable` 根本不会被调到，
于是"世界在跑、而住户不在跑"这一态没人管（而那正是托盘语义下的常态：关窗口/退出 app 时
本地服务照旧跑）。⇒ 承诺是**机器级**的，就由 app 自己在启动时兑现（另一个线程里做，
不冻住窗口；只在**表过态要跑**时做，§8.6-1）。

**退出行为**（§8.3 的 `退出时：[保持运行 ▸]`）：机器级设置 `quitStopsServices`
（`machine_settings.rs`，与 `runLocalWorld` 同一份文件、**读-改-写**互不覆盖）。
**没表态时默认"停"**（§8.3：默认不替用户做隐藏的事），菜单项的名字跟着它变
（`退出（停止本机服务）` / `退出（保持运行）`）。

真机验证（2026-10-03）：`stop` 那条路 ⇒ `小月：已停（pid 4188）`、`世界：已停（pid 24188）`、
账本两个都清掉、进程一个不剩；**点窗口的 ×** ⇒ `Tauri Window | visible=False`（收起了）
而 app 进程还在、`tray_icon_app` 那个托盘窗口在、世界与住户照旧跑；**重开 app**（她先被人为
停掉）⇒ 启动日志：`[本机服务] 世界本来就在跑（没动它）` ＋ `[本机服务] 住户 小月：started`。

**首位 Human 的显示名 = 公共事实，由人给**（2026-10-03 立判据，2026-10-20 改来源）：
规矩是**问进这个世界的人一次**（app 首屏的 `NameYourselfCard`），页面把它交给
`local_world_bootstrap { displayName }`；命令行形态是 `node home.js up --human-name <名字>`，
**没给就用占位 `未命名` 并在读数里说出来**（占位不是称呼）。

- 为什么不再取机器用户名（2026-10-20 撤掉）：那是**机器事实**，写进**公共世界**就是拿它冒充
  公共事实；而且它常常不是你（`Administrator` / `user` / `DESKTOP-8K2LQ`），同机两个人也分不开。
- 为什么不写死 `主人`（更早的那一刀）：那会把 **IM 里可变的昵称**与**世界侧的 ownership**
  （`role='owner'` 是权限事实）压成一个词，而且 Agent 拿到就照着叫（真机读数："主人你好呀"）
  ⇒ 等于**系统借一个昵称替她认定了关系**（宪法判据 9）。关系归**主体自己的记忆**。
- 三条规则与 Core 同一套（契约 4.1）：禁 U+0000、至多 80 个 Unicode scalar value、trim 后非空。
  权威在世界的 `DisplayName::parse`；壳的 `normalize_human_name` 与页面的 `humanNameProblem`
  是**同一套规则的输入提示**，不是第二个裁决者。

**改名有路了**（2026-10-18 切片，`CORE_V1_PARTICIPANT_PRESENTATION.md` §4）：`display_name` 是
既有的 Participant 身份事实 ⇒ 改名是**状态写**，**只有本人能写**（Human 没有 Owner 这一支），
入口在「我的资料」。所以初次起错、或用占位铸下，都换得掉 —— 不再是长久债。

**壳↔前端（四个命令）**：`local_world_readout`（只读；`runLocalWorld` 用 `null` 表示
**还没表态**）／`local_world_enable`（写下表态 + 确保世界在跑）／`local_world_disable`
（只改表态，**不停已经在跑的世界** —— "停"是退出语义那一格）／`local_world_session`（一支会话，
四态 `ok` / `world-down` / `no-core` / `failed`）。**人话归页面**
（`client/src/api/shellLocal.ts` 的 `planLocalWorld` / `adoptDecision` / `shouldStartLocalWorld`，
纯函数 ＋ 用例）；页面只在**自己手里没有凭据**时采用本机条目（用户上次连的是哪个世界就是哪个世界）。

**"问一次"之后按那次表态来**（2026-10-03 补）：表过态要跑、而世界只是**没在跑**（比如重启电脑之后）
⇒ 页面**按那次表态把它起起来**（`shouldStartLocalWorld` 那一条判据；`enable` 是幂等的），
**不再问你一遍**。缺可执行件 / 起失败是**另一种事实**，照旧如实摆出来（不偷着重试）。

**"问一次"落在哪**（§8.6-1：*第一次启动问一次"要不要在这台机器上跑一个世界"（它会写磁盘、
占端口、常驻后台）；之后按面板的开关来*）：表态落**机器级**的
`<app 配置目录>/settings.json` 的 `runLocalWorld`（`src-tauri/src/machine_settings.rs`）。
三态是**有意的**：文件不在 / 读不懂 ⇒ `null` = **还没表态**（界面问一次）；`true` / `false` =
表过态。它**不装凭据**、**不装家条目**（那是页面那一侧）、**不重复家的配置**（那在 `home.conf`）。

**★ 答完之后在哪儿改（2026-10-08 落地，主人当次裁决）**：`machine_settings.rs` 那句"**之后按面板
的开关来**"从前**没有落点** —— `local_world_enable` / `local_world_disable` 只有**第一次那张卡**
会调，答完就改不了（要么手删 `settings.json`，要么把 webview 档案挪开重新走第一次的路）。
现在它落在**「本机服务」那一格**（`LocalServicesPanel`，与「退出时：停止 / 保持运行」**并排**
—— 同一类事实：这台机器上的 app 该怎么表现）：一行三态读数（`localWorldChoiceLabel`：开着 /
不跑（打开时直接去连别的世界）/ **还没表态（下次打开时问你一次）** —— ⚠ 读不到时自己说"读不到"，
**不冒充"没答过"**）＋ 两颗按钮（`开一个` / `不跑`）。两条边界写在那一段的注释与界面上：
① **「不跑」只改表态** —— 壳那一边的 `local_world_disable` 文档写着"**只改表态，不停已经在跑的
世界**"（"停"是退出语义那一格的事），界面照旧指路上面那颗「全部停止」；
② **「开一个」会真的把它起起来**（`local_world_enable` 顺手 `ensure_world` ＋ 带起住户）⇒
上面那两行**状态**读数跟着变，所以写完顺手重读一次（不重读就会出现"表态说开着、状态说没在跑"）。
判据在纯函数里（`client/src/api/shellLocal.ts` 的 `localWorldChoiceLabel` ＋
`tests/shell-local.test.ts` 一条三态用例），组件只渲染 —— 与「退出时」那一格同一条先例。

**怎么手动触发**（临时入口，与安家那个同款处置）：

```bash
cargo run --example local_world_drill -- readout   # 只读读数
cargo run --example local_world_drill -- ensure    # 确保世界在跑（探活优先）
cargo run --example local_world_drill -- session   # 交一支会话（需要时先铸首位 Human；不打印原文）
```

**一条诚实：账本照写**。壳起的这个世界**也写 `logs/im.pid`**（与 `up.js` 同一份文件、同一格式），
否则 `node home.js status/down` 会如实报"没在跑"而它明明在跑（假读数）。这不是"PID 认领"
（§8.7 撤掉的是**认领别人的**进程），是"**谁起的谁记账**"。

**本层刻意的边界**（如实登记）：① **不做进程生命周期**——关 app 世界跟不跟着停、托盘、退出语义
都在后续刀（§8.2-8.3）；② **起本机的执行端住户**不在这里（那一刀要让"她也在"成立）；
③ **不动 `home.conf` 的其它键**（身份三格里的 `agent_id` / `room_id` 是住户化之后搬进
`agents/<支>/agent.conf` 的支级事实）；④ **不改任何既有迁移**、不新增任何 im 契约
（只用既有 operator CLI 与既有 HTTP 面）。

**与 `home.js` 的关系**：同一个家**同一时刻只有一只布置的手**——有壳时是这里，无壳时是
`node home.js up`（Linux/服务器形态，§9.1 明确"不要壳"）。两边写**同一份文件、同一套键、
同一条就绪判据**；这是 `anjia` 与 `adopt.js` 那条先例的同款处置（照抄判据、不共用代码）。

### 4.2.1.1 §8.3 的**面板总开关**（2026-10-03 第七刀）

宪法原话：**面板上给总开关**（`[全部停止]` ＋ `退出时：[保持运行 ▾]`）——"只想留壳子"的用户
一次点击就能做到，**并且知道自己做了什么**；**默认值：不替用户做隐蔽的事**。

落在**「我的机器 → 运行」那一间的下半张卡**（`client/src/components/celira/LocalServices.tsx`）：

- **只在壳里、只对本机那台出现**：本机服务是"这颗 app 自己起的东西"（§8.1），浏览器形态没有
  这一回事，别的机器上的服务也不归这颗 app 停（组件里判：不在壳里 ⇒ 整个不渲染）；
- **判断全在纯函数里**（`api/shellLocal.ts` 的 `localServiceRows` / `quitBehaviourLabel` /
  `canStopLocalServices`，组件只渲染 —— 与 `agentRowStatus` 同一条先例），用例钉住三条：
  ① **三态**：没表过态时那句必须写「（默认）」（不许说成"你选过"）；
  ② 读不到 ≠ 没在跑（缺省不等于零，整格如实说读不到）；
  ③ **有东西在跑才可按**「全部停止」（空动作不骗点击）；
- **写失败与读不到分开说**：一笔写没成 ⇒ 保留上一次那份读数，另说一句「这一次没改成」——
  **不许**把已经读到的读数抹成"读不到"（把一次没做成的动作说成世界没了，是另一种撒谎）。

**真机验证（2026-10-03）**：app 里走到 `/app/runtimes/local`，那一格渲染出
「本机服务 / 世界 在跑 / Agent 1 / 1 支在跑 / 退出时：停止本机服务（默认）」；
**真鼠标点**「保持运行」⇒ 那一格变成「退出时：保持运行」且 `%APPDATA%\com.celira.desktop\settings.json`
落进 `quitStopsServices: false`；点回「停止」⇒ 复原；把那一格键删掉（回到"没表过态"）⇒
页面重新写「（默认）」✓。

**这一刀抓到的真缺陷**（点一次就现形，光看渲染看不出来）：组件里写的是 `setInvoke(seam)`，
而**壳的接缝本身是一个函数** ⇒ React 把它当**更新函数**调用 ⇒ 存进去的是"拿旧状态当命令名调一次"
的结果 ⇒ 面板渲染正常，但**三颗按钮一点就报 `invoke is not a function`**。修法：
`setInvoke(() => seam)`（并把这个坑写在那一行上面）。

### 4.2.2 家的**状态口**（app 自己端，2026-10-03 第四刀）

**为什么有它**：「我的 Agent」每行那三格（在不在跑 / 用哪条模型 / 最近活动）与机器页那三间，
读的都是**家口**（loopback HTTP）：web 形态由 `node home.js doors` 供 ⇒ **app 里没人供** ⇒
页面显示"状态未知"。这一片把这道门在壳里端起来（不依赖 node —— 那是打包 exe 的前置）。

**判据（不发明）**：

| 事实 | 权威 | 壳怎么做 |
|---|---|---|
| 载荷形状、三态、TTL/`stale`、端面只有两条 | `scripts/home/status-server.js` | 逐格照抄（`GET /status` ＋ `GET /healthz`） |
| **处境**（admission / activities / runs / next） | **运行时自己的 CLI** `agent_host_status` | **spawn 它**（与 JS 口子同一个权威）——**壳不复制那套投影** |
| 文本输出的解析 | `host_status_parse.js` | 移植（严格：认不出就整体 `unreadable`，绝不猜）。⚠ 2026-10-05 补齐**第三类活动行**：CLI 的 `state=unknown reason=dispatched-without-result unresolved=1` 两段（老解析器认不出 ⇒ 账本里只要有一条"已派发却没有结果"，**整份读数**变"读不到"，app 与网页一起瞎）|
| `lastWake` | 宿主日志最后一行 `wake ` | 读同一份日志 |
| `im` | `GET /health` 的状态码 | 同判据（连不上 ⇒ `health: null`，**不把"读不到"说成 HTTP 0**） |
| `agent.agentId` | 环境根 `identity.json` 的 `{agent: <id>}` | 照读（**与 Agent 的状态无关**：在忙 / 缺件 / 读不出账本也照给） |
| **零住户那一态**（`no-resident`） | `status-server.js`（2026-10-05 起与 `missing-binary` **分开**） | 同款：`<家>/agents/` 里一支都没有 ⇒ `no-resident`。⚠ 从前两者共用 `missing-binary` ⇒ 页面把"还没有 Agent"说成"本机缺少必要的程序"（**假警报**，真机撞到）；判据：**缺省不等于故障** |

**边界（如实登记）**：

- **不起世界、不起住户**（那是上面两条）；**探活优先**（`GET /healthz` 答 200 ⇒ 已经有人供，
  一个字不动 —— 与 `node home.js doors` 不抢）；
- 起的是**壳进程里的监听线程**（不是子进程、没有第二个可执行件；壳退出口自然消失）；
- **`experience`（经历投影）这一刀不给**：那是 `status-server.js` 里一套有权威的投影，
  壳里抄第二份就是造第二个权威 ⇒ 载荷缺这一格。**页面按它缺省降级**（2026-10-05 改）：
  从 `agent.runs`（**门本来就给的那一层**：每一场怎么收场）排出一列"一次活动一行"，
  并**如实标出来源** —— 明细那一半（做了哪几件事 / 走了几拍 / 为什么醒）一个字都不编，
  页面上写"这一面只读得到每一次怎么收场；动手明细这台机器没给"。
  ⚠ **两处如实登记**：① Agent **正忙**时门那一格只有 `{state:'busy', note}`（照权威的
  `already-open` 那一支，没有 `runs`）⇒ 那一拍活动记录仍说"它这一拍正在做事，等这一拍过去再看"；
  ② 明细那一路（`experience`）在 web 形态照旧是**更全**的那一份；app 里少的是明细，不是收场。
- **口上无秘密**：不读凭据目录、不报"有没有凭据"（JS 那份的理由原样成立）；
- **env 白名单**（照 `minimalEnv`：`SystemRoot`/`windir`/`PATH`/`PATHEXT`/`TEMP`/`TMP`/
  `NUMBER_OF_PROCESSORS`/`OS`/`PROCESSOR_ARCHITECTURE`）：那个二进制本来就不吐凭据，
  但"不吐"是它的属性、"不给"是我们的纪律；
- **双平面**：这道门**只读家**（`home.conf` / `identity.json` / 宿主日志 / 运行时 CLI），
  除了 `GET /health` 那一句**探活**之外**一个字节都不问世界**；世界的公共事实也不进这道门；
- **配置门（5191）已端全**（2026-10-03 第五刀读 ＋ 第六刀写与探测）：
  - `GET /home-config` 给**三合一快照**（清单 ＋ 各支绑定 ＋ 由绑定算出来的派生字段），
    形状权威是 `config-server.js` 的 `snapshot()`；清单的形状权威是 `model-sources.js`
    （逐条严格：`id` 必须 `ms-<序号>`、`endpoint`/`model`/`context_window` 缺一不可 ——
    不合形状的**跳过**，一条都不合 ⇒ `no_valid_entry`，**不是**"没配"）；
  - `POST` 新建 / `PUT` 更新（`pickEntryId` ＋ `validateDeclaration` 的判据与**顺序**逐条照抄：
    地址 → 模型 → 名字 → 绑 → 窗口 → 协议 → 钥匙）、`DELETE ?id=` 删一条（**被绑定挡住 ⇒
    `409 entry_bound`**，绝不悄悄删）、`DELETE`（不带 id）＝ 解除绑定、`PUT /home-binding` 换绑；
    落盘形状（键序、2 空格缩进、尾换行）与 `writeList` **逐字节同形**，原子写（临时文件 ＋ 改名）；
  - `POST /home-config/probe` 只读探测：`GET <基址>/models`（**端点是完整地址** ⇒ 摘掉线协议
    那一段再拼）、`Authorization: Bearer`、8 秒超时；**失败是读数不是错误**（连不上 / 超时 / 401
    都回 200 ＋ `probe.ok:false`），只有**请求形状**不对才 4xx；`contextWindow` 与 `windowField`
    按**有序字段表**读（点号往下一层找），读到就回是哪一件命中的；
  - **窗口那三档**照抄：给了 ⇒ `given`（先落盘再试连）；留空 ⇒ 先试连（`probed`）；两处都空 ⇒
    400 ＋ **病因分开**（`credential_rejected`（401/403，该去换钥匙）与 `window_unavailable`
    （该去手填窗口）—— 它们要用户做的事**相反**，所以不许合并）；
  - 错误词表是**封闭表**，逐字对齐；请求体上限 64 KiB（**超限时先把拒绝写出去、再有界收尾** ——
    直接断连会让客户端只看到 `ECONNRESET`、读不到那句"体太大"，真机验过）；
  - **来源校验**照 `home-key.js` 的 `localOriginAllowed`：`Host` 必须 loopback（挡"某个域名
    解析到 127.0.0.1"那类），带 `Origin` 时它也得是 loopback ⇒ 否则 `403 origin_not_allowed`；
  - **`api_key` 只到那台机器为止**：它落 `<家>/model_sources.json`（原子写）、进内存供写路径用，
    **载荷里只有 `keyConfigured` 布尔**（真机验过：响应里没有那把钥匙的取值）。
  - ★ **这两份配置有一本见证**（2026-10-18：「谁最后改过这一格」）：壳这道门**写过的每一份**
    都记一条到 `<家>/logs/model-witness.ndjson`（**只追加 ＋ 链值**；
    `{at-ms, file, size, sha256, prev}`，`file` 是相对家的路径，`sha256: null` = 那一刻这一份不在。
    **凭据一个字节都不碰** —— 只记字节数与摘要，账里回不出内容）。
    **缝在门上，不在 `model_sources::write_atomic` 上** —— 那一个函数也承载**不是页面**的写
    （`home.js up` 的迁移、布置时的绑定携带），在那里记账会把**布置**说成**"页面改的"**。
    读面多三处：顶层 `listWitness`、逐支行上的 `witness`（见证跟它见证的那一份走）、
    `witnessChainBreakAt`（链的第一个断点）。两档**分开**说：**页面改的，有时刻**；
    **不是页面改的，就只知"变了"，不知是谁**；**"清"也是一笔写**（`sha256: null`）。
    判据与边界（**判不了 ≠ 没改过** · 报"变了"不等于报"被篡改了" · **不是封印** ·
    **记不下绝不让那一笔写失败**）与 JS 那一道门**逐条同形**：形状权威是
    `scripts/home/model_witness.js`，两侧用例**钉同一个测试向量**（改任何一边的规范行必有一处红）。
  - **两处与 JS 的差别（有意的、如实登记）**：① **不端各家的原生目录层**（`probeNative`，
    只有百炼那一家有）—— 权威自己写着它是"锦上添花"、拿不到就退回兼容端点；壳里不端它，
    **原生目录少给的这里也少给**，不假装；② **不端社区窗口兜底表**（`model-windows.js` 那份
    静态数据）—— 嵌一份就是造第二个权威 ⇒ 那一路如实落到"请用户自己填"（`window_unavailable`）。
    实际影响很小：**DeepSeek 的 `/v1/models` 自己就带 `context_window`**，而页面保存时窗口那格
    通常已由预设填好 ⇒ 走 `given`。
  - ⚠ **远程模式（家钥匙）不做**：这两片只端 loopback（模式一，免钥匙）—— 壳的这道门服务的是
    **本机那一条家条目**，远程条目归别的机器自己的门。

**真机验证（2026-10-03）**：

- **状态口**：`GET /healthz` → 200 `{"ok":true}`；`GET /status` → `agent.state=loaded`、
  `agent.agentId=e4e4410d-…`、`activities=40`（明细 32 条、`truncated=true`、其中 12 条未收尾）、
  `im={listening:true,health:200}`、`lastWake=wake run=host-26360-… ending=subject:completed`、
  `readAt/stale = 2026-10-04T06:49:20.900Z / false`；第二次请求 **23 ms**（TTL 命中）。
  ⚠ **这一行是当时那一次的原样读数**（2026-10-03）：其中派生的 `listening` 那一格
  **2026-10-11 已删**（`build_payload` —— 拿它当主词会把"世界活着但自报 500"写成
  "没在跑（HTTP 500）"）⇒ 今天的形状只有 `health`（`null` ＝ 它没答话）。
- **配置门**：`GET /home-config` → `state=absent`、`sourcesState=loaded`、`entries=0`、
  `bindingState=absent`、`bound=null`、`boundPerResident=[e4e4410d…/absent]` —— 那就是这个家的实情
  （**没配过清单、也没绑定**）⇒ 页面上「模型」那一格因此说「**未指定**」而不是「未知」
  （"没配"与"读不到"是两件事）。
- **配置门的写与探测**（可逆走一遍，收尾**把这个家还原到起手状态**：清单文件与绑定文件都不在）：
  ① 假钥匙探测 ⇒ 200 ＋ `ok:false` / `status:401`，`url=https://api.deepseek.com/models`
  （端点是完整地址 ⇒ 摘对了那一段），响应里**没有**那把钥匙；
  ② 没给钥匙 ⇒ 400 `key_required`（形状错才 4xx）；
  ③ 新建（`bind:false`）⇒ 200 ＋ `entryId=ms-1` / `creating:true` / `windowSource=given`，
  清单一条、`keyConfigured:true`、**绑定照旧是 null**（`bind:false` 不许动它）；
  ④ **落盘形状与 JS 逐字节同形**（键序 `id,name,endpoint,api_key,model,context_window`、
  2 空格缩进、尾换行）；
  ⑤ 改一条（`PUT` 点名 `ms-1`、**留空 key**）⇒ `creating:false`，钥匙**沿用**（没有重贴）；
  ⑥ 换绑 ⇒ `bound=ms-1`；此时**删它 ⇒ 409 `entry_bound`**（被绑定挡住，绝不悄悄删）；
  ⑦ 解绑（不带 id 的 `DELETE`）⇒ `bindingState=absent`；再删 ⇒ 清单回到 0 条；
  ⑧ 裸 socket 验来源校验与上限：伪造 `Host: evil.example.com` ⇒ **403 Forbidden**、
  跨站 `Origin` ⇒ **403**、同源 `Origin` ⇒ 200、体 64 KiB+1 ⇒ **400 Bad Request（读得到的拒绝）**、
  `GET /home-binding` ⇒ 405 ＋ `allow: PUT`（按路由给）。
- **端到端**（把两道门的真载荷喂给**客户端自己的读取层**）：`isHomeStatusView → true`、
  `isHomeConfigView → true`、`agentAgentMatch → same`；三格 = 「在这台机器上」／「未指定」／
  「2026/10/4 14:44:02（推算）」；并断言载荷里**没有** `api_key` / 任何 `sk-` 取值。
  ⚠ 2026-10-20：住址那一格从「在本机运行」改成「在这台机器上」（`api/agentRowStatus.ts`
  的 T2：「在不在跑」只归机器那一句，住址那一格只答住址）。

**同一刀撞出并修掉的权威 bug**（`host_status_parse.js`）：真实输出里**未收尾的活动没有
`outcome=`**（`run=… state=unfinished`），而那一版的正则要求它 ⇒ **账本里只要有一条未收尾活动，
整份读数被判 `malformed-run-line`** ⇒ 页面说"读不到账本"，而 Agent 其实好好的（真机读数：
38 条活动里 12 条未收尾 ⇒ 三格与三间全部读不到；网页版同样中招）。修法三处一起：
**权威**（`outcome=` 改可选、`outcome: null`）、**客户端守卫**（`HomeStatusRun.outcome: string | null`，
`isRun` 容忍缺席/null）、**壳的移植**（同款），各带用例。

## 5. 尚未做完的（如实登记）

| # | 事项 | 边界 |
|---|---|---|
| 1 | ~~前端传输面还没接过来~~ | **已完成**（`client/src/api/shell.ts` + `main.tsx` 第一行的 `installShellTransport()`）：壳里出网换到 Rust 侧，浏览器里是空操作（判据是"有没有壳的内部注入"，不是平台名）。**web 产物未被污染**：插件被拆成 1.9 KB 的懒加载 chunk，只有壳运行到才加载 |
| 2 | ~~**本机同步读**（`client/src/api/store.ts` 的模块启动读 prefs 与登出清整族仍是同步 `localStorage`）~~ | **已收（2026-10-08 两刀）**。**第一刀**：连接偏好搬进家里的文件存储（两段式启动 ＋ 读穿迁移），并顺手修掉一个真 bug —— 壳里**登出原先没清掉家里那一族**（cursor／系统消息／房间缓存）。**第二刀**：同一条判据铺满其余四族（房间偏好／主题／壁纸／草稿）⇒ **全仓已无同步读持久面的调用点**，机械守卫在 `client/tests/local-state.test.ts`（`src/` 里 `localStorage` 这个标识符**只许出现在 `client/src/api/storage.ts`**；五族补水都排在 `createRoot` 之前）。判据与真机读数见 `client/README.md`「两段式启动」两节。⚠ 保留下来的历史读数（**2026-10-05**，它当年回答的是"能不能用"）：壳里 `localStorage` 的 prefs **跨启动留得住、也读得回来** —— 换了一颗壳之后，页面仍按上一次那支**已失效**的凭据走过一次直连、撞 401 才回登录页；把它清掉再重载 ⇒ 正门改走本机世界那条路直进。现在这一族不再住那里，老位置只被**搬家读**读过一次 |
| 3 | ~~**外观与草稿未收口**：`use-theme.tsx`、`lib/wallpaper.ts`、`MessageInput.tsx` 仍直接读 `localStorage`~~ | **已收（2026-10-08 第二刀）**：四族都改走存储窄口 ＋ **首帧快照**（模块求值期只有缺省值 → `main.tsx` 在挂载之前补水 → Provider/组件读同步快照）⇒ 第一帧就是用户自己的配色/壁纸/置顶/草稿，**不闪也不丢**；从前那条"改异步会闪一下"的取舍因此不成立了。家族与各自的持久面见 `client/README.md` 那张表。⚠ **仍开着的一格**：草稿与壁纸的按会话覆盖**只按房间 id 分、没有世界作用域**（房间 id 是服务端铸的 `uuid v4` ⇒ 两个世界撞同一个 id 的概率可忽略）；要收得先定"按世界分还是按机器分"，且**键名一改用户手上那份就没了** ⇒ 另作一刀，登记在 `client/src/lib/messageDrafts.ts` 文件头。另：**IndexedDB 那两份**（`api/idb.ts` 的房间/消息缓存、自定义壁纸图片 Blob）仍在 webview 自己的库里 —— 它们是缓存与二进制，不是"状态层的键" |
| 4 | ~~相对路径在壳里会响亮失败 ⇒ app 里那条世界地址从哪来~~ | **本机那一半已收**（2026-10-03 刀，§4.2）：壳解析出本机世界的地址与家的门两个口交给页面（§8.4 推论 2）。**远程那一半不在本刀**：远程条目仍是"用户填的绝对地址 + 家钥匙"（§8.4 那张表的后半）。⚠ `resolveShellUrl` 对**相对地址**照旧响亮失败 —— 那是判据，不是缺口 |
| 5 | ~~未编译验证~~ | **已编译、已运行**（2026-09-29）：`cargo check` 67 s 通过、`cargo build` 82 s 出 `celira-app.exe`（24.2 MB）、运行后窗口标题 "Celira"、WebView2 子进程正常起来、关闭无残留。**2026-10-03 补**：真壳里**冷启动真的起了世界、铸了首位 Human、记下了 `human_id`**（临时家读数），第一次启动渲染出"你自己的世界"那一问。**同一天第一次真的连上了世界**（"自己的世界"那条路第一次被端到端走到）——**当场撞出 §3 那条作用域缺陷**：在那之前 app 的出网**一次都没成功过** ⇒ 旧行的"已编译、已运行"从不等于"连得上世界"，这一条现在有真读数了：进出网 200、`connected`、房间列表读得回来（窗口里就是那些房间）。**2026-10-05 补**：真壳里**点**了那一问的「在这台机器上开一个」（CDP 派真鼠标事件，临时家）⇒ 起世界 ＋ 铸首位 Human（显示名 = 这台机器的用户名）＋ 记 `human_id` ＋ 交会话 ＋ **直进 `/app/chat`**（空世界；**不建默认 Agent**，面板如实写"这台机器上还没有 Agent"），机器级表态落 `settings.json` 的 `runLocalWorld: true`；点「全部停止」⇒ 那个家的世界停干净、账本清空。**在同一颗壳里也复验了 §8.3 那一格**：世界 在跑 / Agent 1 / 1 支在跑 / 「退出时：停止本机服务（默认）」⇒ 点「保持运行」落 `quitStopsServices: false`、点「停止」复原、把那格键删掉重载 ⇒ 页面重新写「（默认）」 |
| 6 | **「安家」缺一块**（2026-09-30 首刀落地执行层 + 壳↔前端一个命令 + 前端四态；2026-10-02 刀补读数跟到落地页 ＋ `refused` 子词） | ~~① 本机条目在壳里的地址解析没做 ⇒ 家面三格读数在 app 里还没接通~~ **已收（2026-10-03 刀，§4.2）**：本机条目在壳里走 loopback 绝对地址（`client/src/api/homes.ts` 的 `setLocalHomeBase`），`/home-status`、`/home-config` 两族读数在 app 里读得到。~~② 四态读数只在内联创建面板可见~~ **已修**：创建是**就地**开面板（「我的 Agent」页右上角「添加」），四态读数就落在那块面板里、不跳走（**2026-10-20 收窄**：原先还有一条「新的朋友 → 添加 Agent」的入口，随"创建归所有权、不归关系"一起删掉），`refused` 拆出封闭子词（`RefusalReason`）供页面给人话与下一步。本层**不做**进程生命周期（§8.2-8.3 / §8.6）与 PID 认领（§8.7） |
| 7 | **托盘与退出语义**（§8.2-8.3）：**行为与界面都做了**（2026-10-03 第三刀行为、第七刀面板） | 已做：× ⇒ 收起（app 留托盘）、托盘图标看得出"还有本地服务在跑"、菜单「退出（停止本机服务）」⇒ 世界与住户跟着停（**停之前先验身份**）、机器级设置 `quitStopsServices`（默认停）；**面板上的总开关**（`[全部停止]` ＋ `退出时：停止 / 保持运行`）落在「我的机器 → 运行」那一间的下半张卡（`LocalServicesPanel`，§4.2.1.1）：**只在壳里、只对本机那台**出现，三态如实（没表过态 ⇒ 写明"（默认）"），写失败与读不到分开说。**如实登记的边界**：**托盘菜单本身点不了自动断言**（图标变体与那一项的名字只有真鼠标点托盘才验得到，页面级门禁碰不到）—— 2026-10-03 与 2026-10-05 两次真机验的都是**同一条停服务路径**（面板那一颗）与读数，菜单点击本身仍靠人 |
| 8 | **家的两个门由壳自己端**（`/home-status` 5190、`/home-config` 5191） | **两道门都端全了**（2026-10-03 第四·五·六刀）：`GET /status`、`GET /healthz`、`GET/POST/PUT/DELETE /home-config`、`PUT /home-binding`、`POST /home-config/probe`；探活优先、进程内线程、委派 `agent_host_status`、TTL 15s、封闭错误词表、来源校验、`api_key` 不进载荷。**如实登记的差别**：不端各家原生目录层与社区窗口兜底表（两处都写在上面的边界里）；远程模式（家钥匙）不做（只端 loopback）；`experience`（经历投影）按判据**故意不给**（不造第二个权威）——**页面从 `agent.runs` 降级渲染**（2026-10-05，见 §4.2.2 那条边界） |
| 9 | ~~**老家形态（`<家>/env`）在壳里不认**~~ ⇒ **已按裁决收口**（2026-10-05 真机发现，同日裁＋修） | **裁决（主人 2026-10-05）**：**壳自己搬** —— 判据两条：**用户直觉**（app 是"不用命令行那个形态"，不该让用户去敲 `node home.js up`）＋**主流做法**（内部布局的迁移在打开时自动做；而且**产品自己早就这么做了**：CLI 形态的 `up` 首次开机就会自动迁 ⇒ 同一个家两种口径才是病）。**落地**：`home_shape.rs` 逐条镜像 `residents.js` 的 `migrateHomeShape`（六种结局 `none / migrated / aside / finish / refuse / failed`），开机序列里排在**读住户之前**（与 `up.js` 同一处语义），幂等；**只改名不删内容**（`<家>/env` 整份 `rename` 到 `<家>/agents/<支>/`；目标被占 ⇒ 旧那份整份让位 `<家>/env.old-<支>`，不覆盖不合并）；**老宿主还在跑时一律停手**（判据同 JS 两条：账本 `logs/host.pid` 的 `ours`／**核对不了**，＋ 进程表里 `--root` 落在 `<家>/env` 的宿主）；`home.conf` 的 `agent_id`/`room_id` 让位给支级 `agent.conf`。**壳做不到的一条如实拒绝**：更老的那一档（`home.conf` 没有 `home_id`）换形状会在世界里**另建一支**，那要动世界 ⇒ 只有 CLI 能做（那是真路径，不是假路径）。**真机两档各验一次**：可迁的家 ⇒ 开机就搬成住户（笔记内容一字未动、账本跟着搬、`home.conf` 只留 `home_id`）；世界级键的家 ⇒ **一个字都没改**，日志里如实说清为什么。**页面说准是哪一种**（2026-10-05 同日补）：那次迁移的**稳定原因码**（`legacyBlocker: world-key / host-running / move-failed`）记在进程里、由状态口那一格带出去，页面按码选人话（三种下一步完全不同：**停掉它下次自己搬** ／ **只有 CLI 能做** ／ **重开一次就重试**）；认不出的码走兜底那句（不替它猜）。真机读数：`legacyHome=true` ＋ `legacyBlocker=world-key` ⇒ 机器页显示"它是最老的那一档…**只有 CLI 能做**：node home.js up" |
| 10 | **打包形态里「模型」那一间读不到**（2026-10-05 真机发现的缺口，**待裁**） | **现象**：`tauri build` 那一形态（页面走内嵌 `client/dist`，来源是 `http://tauri.localhost`）里，机器页「模型」那一间显示「读不到模型配置 ／ 这个来源不被允许（只允许从本机访问）」。**判据实验**（同一请求只改来源头）：状态口 5190 对 `tauri.localhost` 放行（200，读端面本来就是"可远程"那一档），**配置门 5191 拒**（403 `origin_not_allowed`），而 `localhost:5173` 两个都放行 —— 所以 **dev 形态（页面在 5173）一直是绿的**，这一条只在产品形态上成立。**根因**：配置门的模式一来源守卫（`home-key.js` 的 `localOriginAllowed`，壳里 `home_doors.rs` 的 `origin_allowed` 是它的镜像）要求 `Origin` 也是 loopback；打包形态页面自己的来源不是 loopback ⇒ 403。**波及面**：所有走配置门的面（机器页「模型」那一间的读 ＋ 换绑写）；「运行」「活动」「本机服务」读的是状态口与壳命令 ⇒ 不受影响。**要收它有三条路，都碰安全边界或命令面，等主人裁决**：① 让壳自己的门把自己的来源认作本机；② app 里读/写模型配置改走壳的命令（与状态读数同一条路）；③ 打本机门时不带 `Origin`（守卫明文写着"没有 `Origin` 的放行"）。**已按 ① 收口**（2026-10-05 主人裁）：`home-key.js` 的 `SHELL_ORIGINS` ＋ 壳里那份镜像都认 `http://tauri.localhost` / `tauri://localhost`（**协议与主机成对**，外域照旧 403）。**真机两半都验过**：读（「模型」那一间读到"那台机器上没有模型配置 ⇒ 走环境变量"）；**写**（打包形态里真点击「换成这条」⇒ 盘上 `<环境根>/model_binding.json` 落成 `{"source":"ms-1"}`、那一间变"已指定／正在用"、门回 `state=configured bound=ms-1`；写路径的来源守卫同款：外域 `https://evil.example` 与 `tauri://evil.example` 一律 **403**，壳自己的来源 **200** 且真的落盘） |
| 11 | **安装包不带运行时 ⇒ 装了也跑不起来**（2026-10-05 第一次真的走发布路径时发现，同日修） | **现象**：`tauri build` 出得来 MSI/NSIS，但 `bundle.resources` / `externalBin` **都是空的** ⇒ 壳要的 `celira-im.exe` / `agent_host.exe` / `agent_run.exe` / `agent_host_status.exe` **一个都没进包**；而壳从 `resolve_repo_dir` 往上找"像仓库根"的目录（判据是 `bin/agent_host.exe` 在不在，`detect_layout` 的 `packaged` 支）—— 装到别人机器上必然落空 ⇒ **产品形态自足不了**。**修**：`bundle.resources` 把四个可执行件按 **`bin/`** 放进包（`"../../target/release/celira-im.exe": "bin/celira-im.exe"` 那一组），`beforeBuildCommand` 改成**先编**两个 workspace 再编前端（`tauri build` 从此自足）。**真机证据**：装到 `D:\celira-install-test` ⇒ 目录形状 = `celira-app.exe` ＋ `uninstall.exe` ＋ **`bin/`（四个都在）**；跑起来后进程路径 = `D:\celira-install-test\bin\celira-im.exe`（**它自己的 bin**，机器上**没有仓库**）⇒ 世界照起、两道门照起、门禁 **17/17**。安装包体积：MSI 10.5→**20.6 MB**、NSIS 8.8→**14 MB**。**两条如实登记的边界**：① 打包会把资源**复制到 `target/release/bin/`** ⇒ 从仓库直接跑 release 壳时它走那份**副本**（只 `cargo build --release` 编 im 而不再 `tauri build` ⇒ 副本会过期，与"dist 过期"同一类坑）；② **卸载器不停正在跑的本机服务** —— 世界还在跑时卸载会留下进程与 `bin/`（真机撞到：得先手动停）。**发布形状的归属**（2026-10-05 主人裁）：**一体包（`celira-<版本>-win-x64/`）就是普通用户拿到的那个** —— 它现在**含桌面壳**（`celira-app.exe` 在包根 ＋ `bin/` 六个可执行件，双击即用，不需要 Node.js）；**拆分包（core / home / client）是给开发者与高端用户自己部署的**（Linux／开源那一路，`-Platform linux-x64`）。本行只补齐**一体包那一半**：`make_release.ps1` 的构建步骤多了一步"桌面壳（release，内嵌产物）"，壳落在**包根**（壳的 `resolve_repo_dir` 判据是 `<dir>/bin/agent_host.exe` 在不在），分区认领归 `home`（与 `install.ps1` 同一条理由：它是这台机器的控制面）。**真机证据**：包复制到**仓库外面**（`D:\celira-pkg-test`）⇒ 跑 `celira-app.exe` ⇒ 世界从 **`D:\celira-pkg-test\bin\celira-im.exe`** 起来、两道门起、门禁 **17/17**；一体包 zip 16.4→**26.7 MB**，分区检查 288 文件全认领、无未声明重叠。**未发布**（`gh release create` 是主人的动作） |
| 12 | **release 形态一个字日志都不留**（2026-10-05 第一轮发布构建时发现，同日修） | `lib.rs` 原先只在 `cfg!(debug_assertions)` 装 `tauri_plugin_log` ⇒ **发布出去的那一颗既没有控制台（GUI 子系统）也没有日志文件** ⇒ 用户那边出问题时谁也看不到发生过什么（我自己验 release 也没有读数可读）。**修**：两档都装、级别 Info（日志里没有凭据）。**修完立刻见效**：release 日志当场抓出下一行那个并发 bug |
| 13 | **本机存储并发写丢更新 ＋ 抢 `.tmp` 报 `os error 2`**（2026-10-05 由 release 日志抓出，同日修） | 现象（发布形态日志每 15 秒一条）：`写 celira.world:<id>:cursor 失败：存储落盘失败（…local-store.json）：系统找不到指定的文件。(os error 2)`。**根因**：命令是并行跑的（`spawn_blocking`），而"写一个键"是**读整表 → 改一格 → 写回** ⇒ 两个写者 ① 各自读到同一份旧表，后写的覆盖先写的（**丢更新**）；② 抢同一个 `.tmp` 名字，一个 `rename` 把 tmp 搬走后另一个就找不到源（**os error 2**）。**修**：`local_store.rs` 加进程内写锁，**读-改-写整段在锁里**（只锁写不够 —— 用例先红后绿：8 线程 × 6 键，修之前必丢键，修之后一个不少）。**边界（2026-10-14 更新）**：~~锁只护同一进程；跨进程（同机开两个壳）不在本刀范围~~ → **已收**：`local_store.rs` 另加一把**文件级**建议锁（`local-store.json.lock` ＋ `File::try_lock`，**有界等 2 秒** ⇒ 拿不到就**如实说没写进去**），跨进程那一半由此补齐；用例 `a_second_holder_is_refused_and_the_lock_comes_back_when_dropped`（同一进程两个句柄 ＝ 两个壳的同形：操作系统级建议锁按**句柄**判）。**如实登记仍未做的**：① 同目录的 `settings.json`（`machine_settings.rs`）**没有**加这把锁——同类形状、另一处，但它是**显式 UI 动作**触发的，频率低得多；② 页面那一侧的存储（webview 的 `localStorage`/IndexedDB）**另有其主**，不在壳这把锁里；③ 网络文件系统上的建议锁**不可靠**——但这份存储按定义是"本机的"（不跟家走），本来就不该放在那儿 |
| 14 | **第一次的路没有"配模型"这一步**（2026-10-07 主人第一次以用户身份走完全程时发现） | **现象**：全新家、全新 app、什么都没配 ⇒ **她直接就能聊**。真机读数：这个家**没有** `model_sources.json`、这一支**没有** `model_binding.json`，而宿主照旧唤醒并完成了回合。**根因**：宿主只有两个来源——**绑定**（`<环境根>/model_binding.json`）或**进程环境那两条**（`DEEPSEEK_API_KEY` ＋ 可选 endpoint）；这台机器的**用户环境**里早就有那把钥匙（`HKCU\Environment`，长度 35）⇒ 壳起的宿主继承它 ⇒ 零用户动作就能跑。**产品上的问题不是"用了环境变量"**（CLI/无头形态的配置本来就是它，真机门禁也靠它），**是"配模型"没有出现在第一次的路上**：首次布置只问了"要不要在这台机器上跑世界"（`runLocalWorld` 三态），模型那一半一个字没提，于是用户以为不用配。**要做的形状**（2026-10-07 主人裁：**"你想下主流的做法，不都是自己填模型 api key 那种吗？选择模型供应商什么的"**）：**不新增机制、不做三态**——页面里那套**已经做好了**（`client/src/pages/ModelProvidersPage.tsx` ＋ `api/modelCatalog.ts` 的供应商目录，一条来源＝`name/endpoint/model/context_window/protocol/api_key`，清单落 `<家>/model_sources.json`、绑定落 `<环境根>/model_binding.json`，`api_key` 只进内存、载荷只回 `keyConfigured`）；缺的只是**把那一页接到第一次的路上**（首次布置／安家之后，没有模型配置就直接带过去）。**边界**：只动 `client/` ＋ `app/` 的引导，`agent/` 与冻结契约不碰；环境变量那条**保持原样**（不设闸、不迁移凭据）。**⇒ 2026-10-08 复核：这条已落地（`3e0c9b8`／`c52145b`，只动 `App.tsx`），但它当时在最需要它的形态里恒不触发** —— 判据认的是 `sourcesState === 'absent'` 这**一个词**，而**两个门对"清单文件不在"给的是两个词**：web 门（`scripts/home/model-sources.js`）给 `absent`，**app 门（壳自己端 `/home-config`）给 `loaded`（空表）**（`app/src-tauri/src/model_sources.rs` 文件头写着"文件不在 ⇒ **空表**"，且壳自己的用例 `home_doors.rs` 把它钉着）⇒ 那条分支在壳里**一次都没执行过**，而在 web 形态**假绿**（web 门恰好给旧判据要的词）。**2026-10-08 收口**：判据换成**两个门共有的那两格**（`entries` 与 `bound`，并**排除 `unreadable`** —— 清单读不出来时门给的**也是空表**，"配置坏了"不许认成"没配"），搬进纯函数层（`client/src/api/homeConfig.ts` 的 `modelSourceAbsent`，带 8 条用例），并且**两条路走同一段落点判定**（入口定案 ＋ `local-ask` 按"在这台机器上开一个"）；安家被拒那一句（`missing-model-credential`）同时**带上出口**（去机器页的「模型」那一间）。**证据**：`npm run verify` 全绿（604/604 ＋ 28 条路由渲染正常）· `probe-ui.js` **91/91 通过 · 3 条不适用 · 0 失败**（其中"点进 Agent 主页"正是被重构的那条选行路径）。⚠ **仍开着的一格**：两个门的整机 `bound` 都是"取**第一支**绑的那条" ⇒ 一台机器跑多支住户时，"这台机器有没有模型配置"这句话要重新定义；载荷里已有 `boundPerResident[]` / `bindingDivergent`，客户端**今天不解析**（另记账） |
| 15 | **世界连不上时页面不回登录页**（2026-10-07 同一次真机发现） | **现象**：`error sending request for url (http://127.0.0.1:8080/v1/me)`。**根因**：页面把上一次会话的**世界地址与凭据**存在 **webview 自己的 localStorage** 里（§5-2 记过它跨启动留得住）⇒ 手里有凭据 ⇒ 正门走"直进"（`entry.ts` 的 `core-auto`）⇒ 直奔那个**早就不在**的旧世界 ⇒ 连接被拒。⚠ 与 §5-2 那条的差别：凭据**失效**（401）会回登录页，世界**整个不在**（连接错误）**直接报错** ⇒ 用户手上没有"换一个世界 / 重新开始"的出口（这一次是靠手工把 `EBWebView` 整份挪开才回到第一次的路）。**待修**：连接错误也回登录页（或给出本机世界那条路），而不是把一次"世界不在"呈现成一句请求失败。**2026-10-08 修（主人当次裁决）**：① **那一句不再是库的原文** —— 出网那一层（`client/src/api/transport.ts` 的 `sendRequest`）把"发不出去"折成一个**有名字的分类**（`TransportFailure`，码 `transport_unreachable`），文案表（`contractErrors.ts`）给它一句人话「**连不上这个世界 —— 地址或端口不对，或者那个服务没在跑**」；连接失败那一格（`store.ts` 的 `connectionError`）与登出失败那一格都改走 `errorText` ⇒ **reqwest 的原文与 `TypeError: Failed to fetch` 再也上不了屏**（原文只进控制台，诊断留得住）。⚠ 为什么以前没有这一句：`contractErrors.ts` 里那条"网络层的人话"当年被删过，理由是"**没有真正的生产者**"，并留了一句"那是另一刀的事" —— 这一刀就是它（只补这一条；其余四个旧合成码仍然没有生产者，不许顺手复活）。② **出口本来就有**（本次复核，不是新做）：`App.tsx` 的启动直连失败会 `setNeedsLogin(true)` 回落正门（原注释："而不是留在一个没有出口的空态里"），登录页的地址与 token 都可改 ⇒ 2026-10-08 用户看到的正是那一屏。**2026-10-08 第二刀（主人当次裁决：app 不该让人手填）**：③ **直连失败 ⇒ 本机世界接手** —— 入口改成**两轮**（`App.tsx` 的 `attemptEntry` ＋ `resolveFirstScreen`）：第一轮按今天那条（手里有凭据 ⇒ **本机那条不插队**，用户上次连的是哪个世界就是哪个）；那一轮以"连不上"收场时，**那份凭据已被证明没用** ⇒ 第二轮让本机世界接手（起世界 / 取一支会话 / 采用它 / 直进），两轮都不成才回登录页。**判据仍只有一处**：`planLocalWorld` 多一个 `credentialsFailed`（`shellLocal.ts`），**只放宽"不插队"这一条** —— 没壳 / 表过态不跑 / 还没表态三态一字未变。④ **换了世界必须说出来**（静默换世界 = 欺骗）：进去了就在壳顶上摆一行「上次连的世界（<地址>）连不上，已经带你来这台机器上的世界。要回到那个世界：设置 → 连接设置。」（一次性的，只活这一次启动；`EntryOutcome.switchedFrom`）。**证据**：`client/tests/shell-local.test.ts` 两条新用例（接手 ＋ "那一条放宽只管不插队"），**证伪过**（把 `credentialsFailed` 那一格去掉 ⇒ 当场红 2 条）；`npx tsc -b` 0、`npm test` **490 passed / 0 failed**、`npm run verify` 0。⚠ **仍登记的两处同类泄漏**（各有自己的文案族，本刀没动）：`AgentPresentationEditSheet` 的 `setError(e.message)`（域表在 `presentation.ts`）—— 顺带更正它那条 `room_presentation_stale` 分支的注释："稳定文案已展示"**不成立**，它显示的是 `ApiError.message`，即 `code (HTTP status)` 那种机器话 |
| 16 | **壳里麦克风那一格**（2026-10-09 真机发现，同日裁＋修）：语音消息在 app 形态里**假可用** | **现象**：输入区的麦克风按钮可见可点（`isVoiceRecordingSupported()` 只问"API 在不在"），按下去**界面什么都不发生** —— 没有录音态、没有错误、没有出口。**根因**：壳里那份 WebView2 的权限处理器由 wry 装，**只放行剪贴板读**（`wry-0.57.0/src/webview2/mod.rs` 的 `PermissionRequested`）⇒ `getUserMedia({audio:true})` **永远不答**。**真机两次实验（只改浏览器参数，同一颗壳同一个家）**：A 默认参数 ⇒ **10 秒无应答**；B 加 `--use-fake-ui-for-media-stream --use-fake-device-for-media-stream` ⇒ **`granted`（1 条音轨）** ⇒ 管道没问题，**堵点就是权限那一格**。**裁决（主人 2026-10-09）**：**放行，且只在用户按下录音/接听时请求**。**修法两半**：① 客户端 `client/src/lib/voice.ts` 给 `getUserMedia` 一条**有界等待**（8s）＋**稳定分类**（`unsupported`/`no_answer`/`denied`/`no_device`/`failed`，逐格一句人话；`no_answer` 那句给出出口），**迟到的流必须放掉**；② 壳侧 `src/lib.rs` 的 **`on_permission_request`**（Tauri 2.12 现成的口，**没加依赖**）：**只放行 `Microphone`**，`Camera` 一律 `Default`（= 拒）—— 今天只有语音消息这一个消费者，将来接通话的视频面时**在那时**加一行。三条边界写在那一处代码里（只放行麦克风 / 页面只在用户动作时请求 / 拿得到它的只有我们自己的页面：导航只放行自己的来源、外链交给系统浏览器）。**读数（改后同一条探针）**：A 档从"10 秒无应答"变成 **`granted`（1 条音轨）**。**证据**：`client/tests/voice.test.ts`（8 条，含**接线判据**：`getUserMedia` 全文件只有一处调用点 —— 退回无界 `await` 当场红）＋ 登记在案的真机探针 `node client/scripts/probe-shell-mic.mjs`（退出码 0/1/2，见它的文件头） |

**临时入口**（登记：存在过、将来要删）：`examples/anjia_drill.rs`（安家那一刀）与
`examples/local_world_drill.rs`（2026-10-03 刀）都是**只为验收提供命令行把手**的临时文件；
前端接上各自的 Tauri 命令之后即可删除，删除时在本表与竣工报告里登记。

## 6. 构建环境的卡点

**① 本机 crates 源会抖，但可修（2026-09-29 实测，结论更正过一次）**

首次 `cargo fetch` 失败 —— 本机 `~/.cargo/config.toml` 把 crates-io 指向了 `rsproxy.cn`，而那个镜像**连接被关闭**。

当时据此写下的"拉不到 crate、本机无法编译"**不准确**。逐项复测后：

| 端点 | 读数 |
|---|---|
| 索引 `index.crates.io` | **可达**（200） |
| 下载 `static.crates.io` | **可下载**（serde 79 KB / 1.0 s、log 48 KB / 0.6 s） |
| 五个国内镜像（rsproxy / tuna / ustc / aliyun / sjtu） | 探针时**全部不可达**（但不稳定，中途 rsproxy 与 npmmirror 也各自失败过一次又恢复） |

⇒ **真因是网络抖动叠加 cargo 默认超时太短**（默认 30 s，抖一下就 `[28] Timeout`）。可用的跑法：

```powershell
# 一个干净 CARGO_HOME，里放 config.toml：[source.crates-io] registry='sparse+https://index.crates.io/'
$env:CARGO_HOME = '<干净目录>'
$env:CARGO_HTTP_TIMEOUT = '120'         # 默认 30s 在这种网络下必超
$env:CARGO_HTTP_LOW_SPEED_LIMIT = '1'
$env:CARGO_NET_RETRY = '6'
cd app/src-tauri; cargo build
```

实测：依赖拉取 253 s → `check` 67 s → `build` 82 s。

**为什么不把 `.cargo/config.toml` 提交进仓库**：它会**覆盖使用者自己的镜像设置**（这正是本仓"不改既有迁移"那一类纪律的同类问题）。这台机器的全局配置指向一个连不通的镜像，**修它是本机运维，不是仓库事实**。要固化，应改使用者自己的 `~/.cargo/config.toml`。

**② macOS 只能在 macOS 上构建**

`PROJECT_SHAPE.md` §7 缺 8：Tauri 的 macOS 产物**只能在 macOS 上做**。本目录的构建矩阵与 `scripts/make_release.ps1`（只认 `win-x64` / `linux-x64`）**尚未合流** —— 壳怎么进发布物是**待裁**的，本刀不动发布脚本。

**iOS 不在目标平台集里**（`PROJECT_SHAPE.md` §9.1，2026-09-29 主人裁决）；Android 手机壳在计划内（纯客户端），另需 Android 构建环境。

**③ Linux 上 `cargo check` 需要系统依赖（2026-09-30 本机实测）**

`cargo check --all-targets` 在 Linux 上要先把 Tauri 的系统依赖装上（否则 `glib-sys` 的 build script 直接失败：`The pkg-config command could not be found`）：

```bash
sudo apt-get install -y pkg-config libglib2.0-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
                        libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev
```

本机（Ubuntu 24.04）实测：装齐后 `npm run check` 通过（首轮要把依赖编译一遍，数分钟）。这是**机器级前置**，与仓库无关（同 §6-① 的处置：本机运维，不进仓库）。

## 7. 与仓库其余部分的关系

| 关系 | 说明 |
|---|---|
| `client/` | **上游**：壳消费它的构建产物。壳不改前端代码，前端也不知道自己在哪个壳里 |
| 根 `Cargo.toml` | **无关**：`app/src-tauri` 是**独立 workspace**（空 `[workspace]` 表，与 `agent/` 同款处置），根 members 仍只有 `im/server` |
| `scripts/make_release.ps1` | **尚未合流**：壳的产物怎么进发布物待裁（见 §6-②） |
