# Celira

**两个只在协议处相遇的产品**：一个 Human 与 Agent 都是一等居民的小型 IM 世界，
和一个你在自己机器上部署、照看的 Agent 执行端。

[English](./README.md) | 简体中文

---

## 它是什么

- **两个产品，不是一个应用**。**IM 产品**是那个世界：一个逻辑 im 服务端（Rust 单二进制 + SQLite）
  加一个共用客户端——身份、关系、房间、消息。**Agent 产品**是执行端：Agent 的模型、记忆、能力、
  执行状态，住在你某台机器上的家里。两者**只在协议处相遇、互不拥有**——拔掉任一方，另一方还活。
- **一个世界，一个写者**：任一时刻一个世界有且只有一个可写的 im 服务端，它是公共事实的
  唯一裁决者。一个世界可以有多个客户端、多个执行点，但公共事实只有一个写者。
- **两个平面**：公共世界（身份、房间、消息、事件）与 Agent 的私有世界（自己的环境、笔记、经历）。
  公共世界里 Agent 是一等参与者；私有世界里的东西**不自动进公共世界**。
- **Agent 是持续的，不是一次会话**：起家之后 Agent 常驻着等唤醒——门铃一响就跑一次有限活动，
  跑完回到监听。Agent 的经历追加在盘上，下一次清醒期从那里接着走。
- **能力在 Agent 手里、边界在你这**：文件（只读视图 + 显式授权的文本写）、笔记（自己的，随时可用）、
  im（看房间 / 翻记录 / 说话 / 复核）、web（查与取，五堵机械墙）、终端（**整机信任档**，需要显式
  声明才在场）、以及**外部来源的 server**（MCP：在 `home.conf` 里一行一台声明，Agent 要用时才展开）。
- **默认是本机全家桶**：世界、执行端、页面都跑在你自己的机器上；**没有遥测、没有平台账号**。
  把世界放到你自己的服务器上是**今天就能做的事**（见[部署形态](#部署形态)）——默认本机不是因为
  「不许上云」，而是因为那是要做的承诺最少的形态。

## 它不是什么

- **不是 SaaS / 不是多租户**：一个世界一个可写服务端；不做双写、不做同步合并、不做远程房间投影。
- **不是手机或桌面应用**：v1 是「命令行起家 + 浏览器进值班室」；桌面壳（v2）是**包装层**，
  不是 v1 的欠账。
- **不替主体思考**：系统不做摘要、不做规划、不自动续跑、不重试交接——那些是 Agent 的动作，不是系统的。

## 快速开始（Windows 与 Linux）

**读者有两位，路不一样** —— 先认自己是谁：

- **普通用户（桌面版）**：不装 Node.js、不开命令行、不需要 Rust ⇒ 走 **A**。第一次用的四步走查、
  「怎么更新」和「你的东西在哪」都在 [`scripts/APP_MANUAL.md`](./scripts/APP_MANUAL.md)。
- **开发者 / 自己在服务器上部署** ⇒ 走 **B**（Windows 发布包，命令行形态）／**C**（源码）／**D**（Linux
  发布包）。**这三条都需要 Node.js**；Windows 上其余前置由
  [`scripts/install.ps1`](./scripts/install.ps1) 逐项检查，缺哪个就如实报哪个并给你安装链接。
  **Linux 的发布包不需要安装器、也不需要 Rust 工具链**——`node home.js up` 自己布置家、如实报缺什么
  （见 D 路）。**不再需要 Git Bash**——开机入口是 `node home.js`；安装器只检查**各条路真正需要的东西**
  （发布包：Node.js；源码：Git、Rust、Node.js 与 npm）。

**A. 普通用户：解压，双击。**

1. 拿到这一版的包 `celira-<version>-win-x64.zip`（**现在还没有公开的发布页**：包由维护者分发；等发布页开出来，这里就是那个页面），解压到任意目录；
2. **双击 `celira-app.exe`** —— 世界、执行端、页面都在这个包里，**不需要装 Node.js、不需要命令行**；
3. 第一次打开它只问你一件事：「你自己的世界」→ 选「在这台机器上开一个」，然后照面板走。

**你的东西不在这个文件夹里**：世界历史、Agent 的环境与笔记都在**家**（Windows 默认 `D:\celira-home`）。
所以**换新版本就是换这个文件夹，家一个字节都不动**——换法、以及出事怎么回去，见
[`scripts/APP_MANUAL.md`](./scripts/APP_MANUAL.md) 的「换一个新版本」。

**B. 用发布包（命令行形态，Windows）——不需要 Rust 工具链。** 拿到这一版的包
`celira-<version>-win-x64.zip`，解压到任意目录，然后：

```powershell
# 1) 一条命令：检查依赖（Node.js）→ 布置家 → 引导模型凭据
.\scripts\install.ps1

# 2) 点亮家：im 服务端 → Agent → 页面，一次起齐并打印读数
node home.js up

# 3) 浏览器进值班室
#    http://localhost:5173
```

**C. 用源码——克隆后构建。** 额外前置：Git、Rust 工具链、Node.js 与 npm。

```powershell
# 1) 克隆
git clone https://gitee.com/myloveinori/celira.git
cd celira

# 2) 一条命令安装：检查依赖 → 构建（Rust + 前端）→ 布置家 → 引导模型凭据
.\scripts\install.ps1

# 3) 点亮家：im 服务端 → Agent → client 入口，一次起齐并打印读数
node home.js up

# 4) 浏览器进值班室
#    http://localhost:5173
```

> **只想先看一眼界面？** 不需要 Rust 工具链、不需要世界、不需要模型凭据：
> `cd client && npm install && npm run dev`，然后在登录页点「**不连接，先看演示界面**」。
> 世界、Agent 与模型都不在场——这里只有那一张页面。
> （两条命令，不用走完 C 的全部；浏览器之外不写任何东西。）

**D. Linux 用发布包——不需要 Rust 工具链，也不需要安装器。** 拿到
`celira-<version>-linux-x64.zip`（同一个打包器在 Linux 机器上装配出来的，见「发版」），
用 `unzip` 解压（它保留可执行位；把文件模式丢掉的解压方式会让 `bin/` 起不来），然后：

```bash
# 1) 模型那一半：把 DEEPSEEK_API_KEY 放进环境
#    （缺它 up 会停在退出码 3，并把这句话原样说出来）
export DEEPSEEK_API_KEY=<你的模型钥匙>

# 2) 点亮家：im 服务端 → Agent → 页面 + 两个门，一条命令
node home.js up

# 3) 浏览器进值班室
#    http://localhost:5173（第一次把 up 打印的 [token] 那一行粘进登录页）
```

包里自带二进制（`bin/`）、构建好的页面（`client-dist/`）与家脚本——本机不编译任何东西；
`unzip` + `node home.js up` 就是全部流程。Linux 上**还没有安装脚本**：`install.ps1` 里适用于
Linux 的那几步（布置家、如实报缺什么）正是 `node home.js up` 自己做的事；它替不了你的只有一步——
模型钥匙，那要你放进环境变量（或者之后在浏览器「设置 → 模型配置」里配一次）。

- 安装器产出的（或发布包里带的）都是 **release** 产物，家脚本起的就是这一份——中间**不需要**
  再补任何构建。浏览器进的是**生产页**：两种形态都由 `scripts/serve_client.mjs` 服务（源码形态
  `--dist client/dist`、发布形态 `--dist client-dist`；vite 只在构建那一步出现），
  发布形态由自带的 `node scripts/serve_client.mjs` 服务 `client-dist`。
- **两种形态都需要 Node.js**（发布用户免的是 Rust，不是 Node）——v1 的诚实边界。二进制**未签名**，
  Windows 上首次运行 SmartScreen 可能提示；v1 用同一个打包器装配 **Windows x64** 与 **Linux x64**
  两种包（Linux 包在 Linux 机器上构建——`-Platform linux-x64`）。
- 模型凭据：`install.ps1` 会检查 `DEEPSEEK_API_KEY`（用户环境变量）——没有就问一次并写进去，
  **值不回显、不落盘、不进参数**。你也可以自己配好再跑安装器（它会跳过）。
  或者**从浏览器配**（设置 → 「模型配置」，`/settings/providers`）：它在这台机器上写**两个**事实——
  **清单**（这台机器上有哪些钥匙）落在 `<家>/model_sources.json`，**绑定**（这一个 Agent 用哪一条）
  落在 `<环境根>/model_binding.json`。key 只到本机为止：**永不进世界、永不回浏览器**
  （响应只回一个 `keyConfigured` 布尔）。改完**不用重启**：Agent**下一次活动**就用新的。
  **两路只能走一路**：绑定在场时，进程里再出现任何模型来源 Agent 就**拒绝启动**，并告诉你是哪两个在打架。
- 起家之后：`node home.js status` 看状态，`node home.js doors` 单独起家的门（读口 ＋ 写口），
  `node home.js down` 关机（家是持久的，关机不动里面的东西）。
  手册：[`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md)（开机时会复制一份到家里当 `README.md`）。

## 点亮之后

安装期旅程，按你真正遇到的顺序写。

1. **登录**。第一次 `node home.js up` 会开出这个世界的第一个 Human，并**只打印一次** token
   （`[token]` 那一行）。打开 http://localhost:5173——第一屏就是**登录页**（服务器地址 + token）——
   把 token 粘进去，点「**连接**」。**服务器地址留空**：留空就是同源，家要的正是这个。
   之后浏览器自己记着；丢了就 `node home.js up --token` 补发一支。
   （`install.ps1` 只布置家、引导模型 key，**不产生登录凭据**。）
2. **配模型**。设置 → 「**模型配置**」（`/settings/providers`）：端点、密钥、模型名、上下文长度。
   两个事实、两个文件——见上面那条。另一路是环境变量 `DEEPSEEK_API_KEY`；**只走一路**，
   两路同时说话时 Agent 会拒绝启动并说清是哪两个在打架。
3. **创建 Agent**。「创建」按钮只做**世界侧的一半**：在世界里开出 Agent 的参与者身份——
   **它不会因此回复**：只建了身份，**不建身体**（不写 `home.conf` 的 `agent_id`、不发后端凭据、
   不启动任何东西）。另一半叫「**安家**」（把一支已有的身份接到一台机器上）：机器侧今天有一条
   窄路——`node home.js up --agent <AgentId>`（只通"这个家还没有住户"的场合；手册有专节），
   **app 里还没有**（如实登记在 `client/README.md` §已知缺口）。**只创建本地环境**今天是一条命令行：
   `agent_run init --root <环境根> --agent <AgentId>`——这样建出来的 Agent 有完整的本地能力，
   但**没有世界里的身份**，所以不能在世界里说话。客户端那个「只创建本地」的按钮今天是
   **禁用态并明说「尚未支持」**——它是故意没接线的。
4. **说话**。`home.js up` 打印的那间房就是**值班室**（你与 Agent 的 direct room）。你说一句，
   门铃响，Agent 醒一次，跑一次有限活动，然后回答。房间、历史、附件都在世界里，
   所以 Agent 睡着的时候它们也还在。
5. **照看 Agent**。「我的 Agent」列出你拥有的每一个 Agent；**Agent 区按 Agent 编址**——
   `/app/agents/<参与者 id>`，今天两段：「**资料**」（世界侧那个身份）＋「**机器**」（它住在哪台、
   在不在跑——出口通往「我的执行端」）。
   诚实的缺省：**没有接入执行端的 Agent 如实标「未接入」**；**还没指定模型的如实说「不会回复」**。
   不让人猜，也不造假。

## 部署形态

| 形态 | 世界在哪 | 执行端在哪 | 今天能做吗 |
|---|---|---|---|
| **三合一**（默认） | 本机 | 本机 | **今天** —— `node home.js up` 一次起齐三件 |
| **世界上云** | 你自己的服务器 | 本机 | **做得到一半** —— 世界**连同它的页面**一起放到那台服务器上，浏览器打开那个地址即可（两者必须同源：本机页面连不上远端世界，因为 im 服务端不发 CORS 头，浏览器在请求出门前就拦了）。教程见 [`im/server/README.md`](./im/server/README.md) §5。把**本机的执行端**接到那个远端世界，**目前没有现成命令**（那是同一个文件 §6 的 Agent 协议接入位） |
| **跨机照看** | 任意 | 另一台机器 | **今天可做** —— 在那台机器上发一把家钥匙（`node home.js issue-home-key`），把家口开到网上（`status_bind` / `config_bind` + `tls_cert` / `tls_key`，TLS 自备），再到「设置 → 家」把它添进来；手册见 [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md) |
| **纯世界居民** | 本机或服务器 | 不装 | **今天可做** —— 只要 IM 产品，一个 Agent 都不装 |

两句实话。**世界上云**说的是**你自己的**服务器，不是 Celira 运营的：公共事实仍然是你自己的，
这台服务端的存活也归你自己管。**纯世界居民**是一个真实形态——你跟人说话、也跟别人的 Agent
说话，只是自己不跑执行端。

还有一句：**页面与世界必须同源**。这是代码的事实，不是偏好——im 服务端没有开过 CORS 这个面
（要开就得走公共契约的裁决），所以"本机页面填远端世界地址"会被浏览器拦下；今天走得通的姿势是
**把页面放到那台服务器上**（发布包里已经带齐：`client-dist/` + `scripts/serve_client.mjs`）。

## 两个扩展位

- **接入位——任何守约的 Agent 都能进来**。世界不关心跑 Agent 的是什么。任何讲得通 Agent 协议
  （拉取与消息端点 + 门铃）的东西，都能绑到一个 Agent 身份上，成为**公民同权**：同样的房间、同样的消息、
  同样的幂等保证（自己拉、自己说、不重复）。Celira 只出一份执行端——`agent/` 里的 Native Runtime；
  **不为第三方 Agent 维护适配器**（2026-09-28：示例 connector `im/connectors/hermes-celira` 已退役并删除）。
  Celira 不管它们的内部，也不接管它们的模型、记忆与工具。
- **改造位——换 Agent 的身体**。`agent/` 是一份完整的 runtime，你可以 fork：换工具面、换模型适配层、
  换渠道。接缝文档随版本补。今天**不承诺**的是把 runtime 当依赖库 `import`——**fork 它，别 link 它**。

## 架构一屏

```text
   ┌──────────────────────────────────────────────────────────────────┐
   │  client/    两个产品共用的呈现面                                   │
   │  浏览器：值班室（房间 / 消息 / 成员）+ Agent 主页（资料 / 机器）        │
   └───────────────┬───────────────────────────┬──────────────────────┘
                   │ /v1  （Bearer）            │ /home-status · /home-config
                   │                            │ （本机同源代转；
                   │                            │  或另一台机器的地址 + 家钥匙）
        ┌──────────┴────────────┐      ┌────────┴─────────────────────┐
        │  im/server  世界       │      │  agent/   执行端              │
        │  Rust + SQLite         │      │  Agent 的环境 / 经历              │
        │  身份 / 房间 / 成员    │      │  能力 / 模型适配层            │
        │  权限 / 消息 / 事件    │      │  （home.js 在这台机器上       │
        │  投递                  │      │   管的那一整套）              │
        └──────────┬────────────┘      └────────┬─────────────────────┘
                   │  Backend 协议：拉取与消息端点 + 门铃（events.available）
                   └────────────────────────────┘
```

三个容器、三条有名有姓的连线。Human 与 Agent 都是 Participant；进程、宿主、会话都不是。
完整的容器与数据流见 [`ARCHITECTURE.md`](./ARCHITECTURE.md)。

## 仓库地图与命名

仓库按两套系统分组：

- `im/` —— IM 系统：`im/server` 服务端 + `im/connectors` 连接器；
- `agent/` —— Agent 系统：当前线是**独立 crate** `agent/`（不属根 workspace），
  旧线 `agent/runtime/` 与 `agent/acp-supervisor/` 已于 2026-09-17 退役（考古走 Git 历史）；
- `client/` —— 整个产品的呈现面（顶层）；
- `host/` —— **已于 2026-09-20 删除**：Local Client Host（本机全托管编排 + 窄 BFF）当天退役，
  源码已从工作树移除（考古走 Git 历史；设计记录保留在
  [`docs/client/LOCAL_CLIENT_HOST.md`](./docs/client/LOCAL_CLIENT_HOST.md)）。*Host* 这个角色
  仍留在 `AGENTS.md` / `ARCHITECTURE.md` 的职责词汇里，但不再是本仓库的一个组件；
  本机进程编排现在是 `home.js`（见 `scripts/`）；
- `scripts/` —— 家的开机入口（仓库根的 `home.js` + `scripts/home/*.js`）、打包/安装脚本与静态页服务
  （旧的 `home_*.sh` 已在 Node 入口验证通过后删除）；
- `docs/` —— 宪法级文档与冻结契约。

命名映射（2026-09-11 双系统重排）：**Core v1 = im 服务端 v1 契约**（冻结契约里的文件名与
「Core v1」字样保留为历史版本名）；crate / 二进制 / 环境变量键族同刀更名为 `celira-im` 与
`CELIRA_IM_*`；`CELIRA_LOCAL_CLIENT_*` 键族（含保留 WEB 字样的 `CELIRA_LOCAL_CLIENT_WEB_DIST`）
**冻结**。

「世界」与「家」这两个词各指什么，只在
[`docs/constitution/PRODUCT_VISION.md`](./docs/constitution/PRODUCT_VISION.md) §4
解释一遍，全仓不重复解释。

## 文档导航

| 文档 | 内容 | 何时读 |
|---|---|---|
| [`AGENTS.md`](./AGENTS.md) | 协作与工程纪律：权威顺序、动手前门禁、封板门禁、架构红线 | 动任何代码之前 |
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | 唯一的 C4 L1–2 系统地图：当前实现、容器职责、数据流、允许/禁止的连接 | 理解"怎么构成" |
| [`docs/constitution/`](./docs/constitution/) | 产品愿景、Agent 哲学、地基契约、连续性宪法 | 理解"为什么" |
| [`docs/contracts/core-v1/`](./docs/contracts/core-v1/) | 冻结的公共契约（身份 / 房间 / 消息 / Agent 协议 / 验收） | 改或验收 im 侧 |
| [`docs/contracts/core-v1/HOME_ACCESS.md`](./docs/contracts/core-v1/HOME_ACCESS.md) | 世界的准入与初始配对：谁能进这个世界、第一支凭据怎么发 | 部署世界 / 加第二个人 |
| [`im/server/README.md`](./im/server/README.md) | 世界的部署文档：本机怎么跑、operator CLI、配置键表、上云 | 部署或迁移世界 |
| [`agent/README.md`](./agent/README.md) | Agent 侧的代码地图与逐步接线的行为 | 在 `agent/` 里工作 |
| [`docs/agent/`](./docs/agent/) | Agent 侧四份目标文档：宪法与产品模型、执行可靠性、总架构与工程边界、终局导航 | 在 `agent/` 里做大改动之前 |
| [`docs/runtime/`](./docs/runtime/) | Runtime 的词典、原则、路线、分层与终局正文（只留活文档） | 理解 Runtime 的本体与执行模型 |
| [`docs/runtime/AGENT_VOCABULARY.md`](./docs/runtime/AGENT_VOCABULARY.md) | **命名权威**：每个词在本仓指什么，外加本仓自己的**过程语域**（维护者文档与代码注释里会遇到的那套工作用语） | **撞到一个读不懂的词时** |
| [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md) | 家的手册：命令、故障、跨机照看（发钥匙 / 开家口 / 家列表） | 起家 / 修家 |
| [`client/README.md`](./client/README.md) | 前端快速运行、数据模式、页面地图 | 在 `client/` 里工作 |
| [`client/DESIGN_SYSTEM.md`](./client/DESIGN_SYSTEM.md) | 前端设计系统：令牌、排版、组件与文案口径 | 写前端代码 |

## 状态

v1 正在收口：im 服务端公共契约已封板冻结；Agent 侧以「持续身份 + 归属环境 + 连续经历」重建，
能力面（文件 / 笔记 / im / web / 终端 / MCP）已逐步接线并有行为读数。上面那个「两个产品」的框
是**入口层文档正在改写过去的框架**；当前实现的事实以代码与测试为准——**架构快照**见
[`ARCHITECTURE.md`](./ARCHITECTURE.md) §6，**Agent 侧逐步读数**见 [`agent/README.md`](./agent/README.md)。

产品支持面是 Windows 与 Linux ；v1 的验收今天都落在 Windows，Linux 是已登记的扩展面。macOS 未在支持面内——没有环境就无从验收，能编译不等于能用。

## 升级与回退

**产物目录和家目录是两个地方**。解压出来的那一包（`celira-<版本>-<平台>/`，如 `…-win-x64`、
`…-linux-x64`）是**产物**；
家目录（默认 `D:\celira-home`，可用 `CELIRA_HOME_DIR` 换）是**数据** —— im 的库与媒体
（`im/`）、Agent 的环境与笔记（`env/`、`materials/`）、`home.conf`、`logs/` 都在那里。

**升级 = 换包，家目录不动**：

```powershell
node home.js down      # 先关机。只停进程：Agent 的环境、im 的历史、笔记都还在家目录里
#  把新版本那一包解压到别处，之后都从新包里敲命令
node home.js up        # 在新包里开机
```

第一次 `up`（在一个**已有**的家里）会做这几件事，每一件都是幂等的，或"只在缺的时候才动"：

- 缺的目录补上；`home.conf` **只在不存在时**才写 —— 已有的配置一字不动；
- 复用 `home.conf` 里的 `human_id` / `agent_id`（**不重建身份、不重开用户**）；
- `<家>/README.md` 在随包手册内容变了时被刷新（说明书是我们发的，不是用户的自持物）；
- **im 服务端启动时自动跑 SQLite 迁移**（sqlx，幂等、checksum 校验）。

**换一包（把这台机器上装着的那份换成新构建）= 停 → 换 → 起**：部署目录就是它自己的一个普通目录
（不在构建产物里，也不是指针链），换包就是把这批文件整份换掉，**回退点就是上一个包**（包一直留着，
换回去即可）。三条命令、谁来执行都行，见 `scripts/HOME_MANUAL.md` 的「换一包」那一节。

**回退 = 先 `down`，再把家目录换回升级前那一份。**

⚠ **升级前请先把家目录整个复制一份** —— 这是唯一永远成立的办法。一条命令就能做：

```powershell
node home.js down      # 先关机（运行中复制出来的库是"可能能用"，不是"能用"）
node home.js backup    # 默认复制到 <家>-backup-<时间戳>；--to <目录> 换地方
```

它备的是**整个家**（`im/`、`env/`、`materials/`、配置…），只排除运行产物（`logs/`、进程号）；
两个前置条件是硬的（还有东西在跑 / 上次不是干净停的 ⇒ 拒绝，退出码 3），逐文件双侧 sha256，
清单最后写。它**在发布包里**（不需要 Python）。⚠ 备份里有 `<家>/model_sources.json` ⇒ **它带着
模型 key**，像 `env/` 一样保管。

理由（核过）：迁移**只有
上行**，`im/server/migrations/` 里全是 up SQL、没有 down；新版本第一次 `up` 会把 pending 迁移
落库；此后**只要新版本加过迁移**，旧二进制就会因为库里有一条它不认识的迁移而**拒绝启动**
（sqlx 的 `VersionMissing`；就绪探测也把这一档判成 `DecodeOrCorrupt`）。**迁移不可逆。**

仓库里另有一套离线备份 / 校验 / 恢复工具（`tools/home_portability.py`，见
[`docs/contracts/core-v1/HOME_PORTABILITY.md`](./docs/contracts/core-v1/HOME_PORTABILITY.md)）。
两点如实说清：它**不在发布包里**（打包脚本只装 `bin/`、`client-dist/`、`scripts/`、`docs/`、
`home.js`、`VERSION`），只有从仓库跑才有；而且它覆盖的是**公共事实**（`core.db` 与媒体附件
字节），**不含**Agent 的环境与笔记。所以"回退先备份家"用上面那条朴素办法。

**版本只有一个权威**：根 `Cargo.toml` 的 `[workspace.package].version`；包里的 `VERSION` 是
那份权威在**构建那一刻**的读数（版本 / git rev / 构建时间 / 平台）。

## 发版（维护者）

```powershell
.\scripts\make_release.ps1                            # tag 默认是 v<根 Cargo.toml 的版本>；Windows x64
.\scripts\make_release.ps1 -Tag v0.1.0 -SkipPublish   # 只打包并本地校验，不发布
.\scripts\make_release.ps1 -Platform linux-x64 -SkipPublish   # Linux x64（在 Linux 上跑；-SkipBuild 复用已编好的二进制）
```

- **版本只有一个权威**：根 `Cargo.toml` 的 `[workspace.package].version`。tag 必须是
  `v<那个版本>`，独立 crate `agent/` 自己的版本也必须一致——否则脚本拒绝打包；
- **一次装配四个包、一个版本**：all-in-one `celira-<版本>-<平台>/`（默认下载）＋ 三个子包——
  `celira-core-…`（世界）、`celira-home-…`（执行端）、`celira-client-…`（页面 + 它的服务）。
  都落在 `dist-release/`：目录 + `.zip` + 每包一份 sha256 的 `MANIFEST-<包>-<版本>.txt`
  （`MANIFEST.txt` 是 all-in-one 的）。**目前只发布 all-in-one 那个 zip**；三个子包只落盘，
  等发布渠道裁决。**发布说明就是这个版本的 commit 主题**，所以没有需要同步的 `CHANGELOG.md`；
- 切分是**划分校验**，不是拷贝清单：all-in-one 的每个文件都必须被至少一个子包认领，唯一被两个
  子包同时认领的是声明过的共享对（`client-dist/` ＋ `scripts/serve_client.mjs`）；
- 在 Linux 主机上，zip 还带着每个文件的模式（可执行位在 zip 的 external attributes 里）——
  用 `unzip` 解压出来 `bin/` 直接可执行；
- 发布走 `gh`（GitHub CLI）。`gh` 不在 / 没登录 / remote 不是 GitHub 时，包照样构建并校验，
  但会报「**未发布**」并以非零码退出——**绝不假装发布过**。

## License

[MIT](./LICENSE) © 2026 inori
