# Celira im server（core）

> 本文件是**世界**的部署文档。它讲怎么把 core 跑起来、怎么开第一支身份、上云要动哪些东西。
> 公共契约本身不在这里——冻结契约在 [`../../docs/contracts/core-v1/`](../../docs/contracts/core-v1/)，
> 改契约先读那里的「权威文档顺序」与解封条件。
>
> 「世界 / 家」两个词各指什么，只在
> [`../../docs/constitution/PRODUCT_VISION.md`](../../docs/constitution/PRODUCT_VISION.md) §4
> 解释一遍：这里的**世界**就是本文件讲的东西（冻结契约里仍写作专名 Home）。

## 1. 它是什么

- **一个二进制，一个 SQLite 文件。** `celira-im` 是 Rust 单二进制，全部公共事实落在
  `CELIRA_DB_PATH` 指的那个 SQLite 文件里，附件字节落在 `CELIRA_MEDIA_ROOT` 指的那个目录里。
  没有 Redis、没有消息队列、没有第二个存储。
- **它是公共事实的唯一裁决者。** 身份、关系、房间、成员、权限、消息、事件、附件、
  撤回与 read state 都由它写（旧线另含投递位置/ACK——2026-09-24 裁决退役、2026-09-28 连表带端点删除）。**一个世界任一时刻只有一个可写实例**：不做双写、不做同步合并、
  不存在备用写者。
- **它不替主体思考，也不拥有 Agent 的私有部分。** 模型、记忆、工具、Run 状态都在执行端；
  core 只看见 Agent 以参与者身份发出的公共消息与事件。
- **它的两类对谈者。** 客户端走 `/v1`（Bearer Human Session）；Agent 的后端走 **Backend 协议**——
  BackendAuth 头部认证 + 拉取与消息端点（`GET /v1/backend/me`、`GET /v1/backend/events?after=&limit=`、
  `GET /v1/backend/rooms`、`GET /v1/backend/rooms/{room_id}/messages`、
  `POST /v1/backend/rooms/{room_id}/messages`、`PUT /v1/backend/rooms/{room_id}/profile`，
  以契约为准）加一条门铃
  `GET /v1/backend/events/stream`（SSE，只发无事实的 `event: events.available`）。
  单房历史回执另带一个**只读**房间头（`id` / `kind` / `name` + 最新群简介
  `profile`，见 [`../../docs/contracts/core-v1/CORE_V1_ROOM_PROFILE.md`](../../docs/contracts/core-v1/CORE_V1_ROOM_PROFILE.md) §4.2）。
  **群简介写面有两条认证面、同一条业务路径**：Human Session 走
  `PUT /v1/rooms/{room_id}/profile`，Agent 走
  `PUT /v1/backend/rooms/{room_id}/profile`——授权都只看"该房当前 active
  owner"（Human 或 Agent，owner 是房内治理角色而非参与者 kind）；Backend 面
  自带 16 KiB 请求体上限，错误矩阵与 Human 面逐条相同（见同契约 §3/§5）。
  **轮询 `GET /v1/backend/events` 永远是完整的事实获取路径**，门铃只是及时性优化。
  旧线四端点中的 `POST …/events/{event_id}/completion` 与 `POST …/events/{event_id}/ack`
  已于 **2026-09-28 删除**（端点、表与错误码一并退场；见
  [`../../docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md`](../../docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md) 文档头）。
- **它可以被单独部署。** 世界不需要任何执行端就能跑（一个 Agent 都不装是合法形态），
  执行端也不需要世界就能跑本地 Run。两边只在协议处相遇。

## 2. 本机跑起来

```bash
# 构建（根 workspace；产物在 target/release/celira-im）
cargo build --release

# 跑（三个环境变量都有安全默认值，不设也能起）
CELIRA_LISTEN_ADDR=127.0.0.1:8080 \
CELIRA_DB_PATH=./data/core.db \
CELIRA_MEDIA_ROOT=./data/media \
  ./target/release/celira-im
```

- **默认只绑 `127.0.0.1:8080`**——不设监听地址时它不会自己暴露到网卡上。
- 首次启动会建库、跑 migration、初始化这个世界的 Home Identity（幂等）。
- 数据库与媒体根是**一对**：搬家时两个一起搬，只搬一个会「消息在、附件没了」。
- 日常不用手动跑它——`node home.js up` 会把它作为三合一形态的第一件拉起来。本文件讲的是
  **你要把它单独部署、单独管**的时候。

## 3. 第一次引导：operator CLI

无子命令 = 起服务；带子命令 = 部署控制面。**成功时 stdout 只有一行 JSON，日志与其他输出一律走
stderr，原始 token 永不进日志。HTTP 不提供匿名注册。**

```
usage: celira-im [bootstrap-human --display-name <NAME> | \
                  create-human --display-name <NAME> | \
                  issue-human-session --human-id <ID> | \
                  issue-pairing-grant --human-id <ID> [--device-label <LABEL>]]
```

| 子命令 | 干什么 | 什么时候用 |
|---|---|---|
| `bootstrap-human --display-name <NAME>` | 创建**首位** Human 并签发初始 Session（占全局 guard，重复/并发只有一次成功） | 这个世界刚建好、还没有任何人 |
| `create-human --display-name <NAME>` | 创建一个新的 active Human + 一次性 Session | 加第二个人（同一世界多人共用） |
| `issue-human-session --human-id <ID>` | 给已有 active Human 补发一支 Session | token 丢了 / 换设备 |
| `issue-pairing-grant --human-id <ID> [--device-label <LABEL>]` | 签发一次性 Pairing Grant（初始配对路径） | 让一台新设备凭一次性凭据兑换进入 |

- **token / grant 只输出一次**，就在那一行 JSON 里（`participant_id`、`session_id`、`token`、
  `grant_id`、`expires_at`）。没记下来就再签发一支，没有第二次打印。
- **这些命令是本机/SSH operator 的动作，不是远程管理协议。** 它们要能直接碰到那个 SQLite 文件。
- **它们与常驻服务共用同一个库与同一个 Home Identity 初始化用例**，所以在「服务还没起过」的库上
  也能产出可兑换的 grant。
- 环境变量必须和**常驻服务看同一个库、同一份媒体根**：`CELIRA_DB_PATH` / `CELIRA_MEDIA_ROOT`
  少一个，CLI 会去开默认路径下的另一个库（实测踩过：`bootstrap-human` 报 `error_kind="database"`）。

三合一形态下这一步不用你敲：`node home.js up` 第一次会替你跑 `bootstrap-human`，
并把那唯一一次打印的 token 打在 `[token]` 那一行。

## 4. 配置

**core 只认环境变量**（没有配置文件）：

| 变量 | 含义 | 默认值 |
|---|---|---|
| `CELIRA_LISTEN_ADDR` | 监听地址 | `127.0.0.1:8080` |
| `CELIRA_DB_PATH` | SQLite 路径 | `data/core.db`（相对运行目录） |
| `CELIRA_MEDIA_ROOT` | 附件字节存储根 | `data/media`（相对运行目录） |
| `CELIRA_GRANT_TTL_MINUTES` | Pairing Grant 有效期（分钟，`1..=60`） | `15` |
| `CELIRA_HOME_DISPLAY_NAME` | 世界的可选显示名（**仅首次初始化生效**，之后不可变） | 无 |

非法值不静默回退：`CELIRA_LISTEN_ADDR` 不是合法 `SocketAddr`、TTL 越界、显示名不合规则，
进程**明确失败**并给出 `error_kind`（`invalid_listen_addr` / `invalid_grant_ttl` /
`invalid_home_display_name` / `empty_db_path` / `empty_media_root`）。

**三合一形态下的 `home.conf`**（家目录里那个文件）另外记了几件事。它不是 core 的配置——
**core 一个键都不读它**，是 `home.js` 读它、再把它转成 core 的环境变量与客户端地址：

| 键 | 是什么 | 谁写 |
|---|---|---|
| `listen_addr` | core 的监听地址（转给 `CELIRA_LISTEN_ADDR`） | `home.js up` 首次布置 |
| `client_port` | 页面端口（源码形态 `vite preview`，发布形态静态服务） | `home.js up` 首次布置 |
| `status_port` | 本机只读状态口（默认 `5190`） | 可手改 |
| `config_port` | 模型配置门（默认 `5191`） | 可手改 |
| `human_id` / `agent_id` / `room_id` | 你、Agent、值班室的身份 id | `home.js up` 布置（幂等） |
| `mcp_<名>=<transport>:<命令>[,read-only][,timeout=<秒>]` | 声明一台外部 MCP server | 你手改 |

> **凭据不在 `home.conf` 里**（也不在 `logs/` 里，也不在家目录里）。模型 key 在本机用户环境变量
> （或本机那份模型清单，见「模型配置」页）；Agent 的后端凭据由 `home.js up` 签发后写进
> **按 Agent 在那个世界的身份分条的本地存储**（Windows `%LOCALAPPDATA%\Celira\credentials\`），
> 起 Agent 时仍经 `CELIRA_IM_BACKEND_AUTH` 注入给宿主进程。

## 5. 上云

**先说清楚一件事**：上的是**你自己的**服务器。Celira 不运营这个世界，也没有平台账号——
你把二进制、那个 SQLite 文件和媒体目录放到你管的一台机器上，它就活在那里。

### 5.1 监听与 TLS

- 默认 `127.0.0.1:8080` 只对内。要对外就改成该机器对外可达的地址（例如 `0.0.0.0:8080`）。
- **core 自己不做 TLS。** 市面姿势是在前面挂一层反代终止 TLS（Caddy 自动证书最省事，
  nginx 也行），反代后面 core 仍然只听本机。
- **反代要给 SSE 让路。** 门铃是 SSE（`GET /v1/backend/events/stream`），
  中间任何一层做响应缓冲都会把「铃响了」压住 —— nginx 要 `proxy_buffering off`、
  并把 `proxy_read_timeout` 放长（它是一条长连接，不是一次请求）。
  Call 走 WebSocket，反代还要放行 `Upgrade` / `Connection` 头。
- **Bearer 走明文 HTTP 就等于把身份交出去**——这也是「必须做 TLS」的唯一理由，不是合规套话。

### 5.2 systemd（Linux 通用姿势）

> ⚠ v1 的官方产物此前只出 **Windows x64** 包；**Linux x64** 的包已能由同一个打包器产出
> （2026-09-30；发布渠道未裁）。下面这份 unit 是社区通用写法，
> **本仓库未在本机实测过**。照抄前先自己跑一遍 `ExecStart`。

```ini
[Unit]
Description=Celira im server (core)
After=network.target

[Service]
Type=simple
User=celira
WorkingDirectory=/var/lib/celira
Environment=CELIRA_LISTEN_ADDR=127.0.0.1:8080
Environment=CELIRA_DB_PATH=/var/lib/celira/core.db
Environment=CELIRA_MEDIA_ROOT=/var/lib/celira/media
ExecStart=/usr/local/bin/celira-im
Restart=always
RestartSec=3

# 只给这一个目录写权限：数据库 + 媒体根 + 它的 WAL 兄弟文件
ReadWritePaths=/var/lib/celira
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
```

- `WorkingDirectory` 不是摆设：`CELIRA_DB_PATH` 给相对路径时是**相对它**解析的。
- 数据库目录要**可写且持久**：SQLite 会在旁边生成 `-wal` / `-shm`，容器里挂卷要挂整个目录。

### 5.3 备份与搬家

- 备份 = **SQLite 文件 + 媒体目录**一起拷。只拷一个，另一个就成了断链事实。
- 拷之前**先停写**（`systemctl stop` / `node home.js down`）：SQLite 正在写的时候拷出来的
  文件是「可能能用」的文件，不是「能用」的文件。
- 恢复 = 把两个东西放回原位、用同一套环境变量起。世界的 Home Identity 在库里，跟着走。

### 5.4 客户端怎么指向它

**页面与世界要同源**——这是今天唯一走得通的姿势：把发布包也放到那台服务器上，用自带的静态页
服务把页面端出来、由它把 `/v1`（连同两条 WebSocket）转给本机的 core，最后在整套前面挂一层
反代终止 TLS（§5.1 那三条 SSE / Upgrade / 明文警告照旧适用）：

```bash
# 在"那台服务器"上（解压出来的发布包根目录；Windows 上静态页那一行一字不变）
CELIRA_LISTEN_ADDR=127.0.0.1:8080 \
CELIRA_DB_PATH=/var/lib/celira/core.db \
CELIRA_MEDIA_ROOT=/var/lib/celira/media \
  ./bin/celira-im            &   # 世界（§5.2 的 systemd unit 就是它的常驻写法）

node scripts/serve_client.mjs --dist client-dist --port 8081 --im http://127.0.0.1:8080
# 反代：https://<域名> → http://127.0.0.1:8081
```

浏览器打开 `https://<域名>`：第一屏是登录页，**服务器地址留空**（＝同源），粘上 token 即可。
这时 REST、附件上传下载与两条 WebSocket 走的是同一个源，不需要任何跨源许可。

> 这样托管的页面里，「运行状态 / 模型配置」那一族问的是**这台服务器**上的家口（那里多半没开
> ⇒ 页面如实说"本机状态口未开"）。要照看在**另一台机器**上跑的 Agent，就在「设置 → 家」里加一条
> **远程家条目**（绝对地址 + 家钥匙，走 TLS）——那一条路见
> [`scripts/HOME_MANUAL.md`](../../scripts/HOME_MANUAL.md) 的「跨机照看」。

**为什么不是"本机页面填远端世界地址"**：core **不发任何 CORS 头**——这是个**没有开过的面**
（要开就走公共契约的裁决，不是随手加一行）。浏览器从 `http://localhost:5173` 打远端 core 时，
带 `Authorization` 的请求会先发预检，而 core 对 `OPTIONS` 回 `405`、不带 `access-control-*`
⇒ 请求在浏览器里就被拦下（REST 全断；WebSocket 不受 CORS 管，但只有它通没有意义）。
所以"世界在服务器上、页面在本机"这一格，今天的走法是**把页面也放到那台服务器上**。

> **本机的 Agent（执行端）接到远端世界**：目前没有一条现成命令 —— `node home.js up` 的三合一
> 布置会对着**本机**的库做引导（`bootstrap-human` / `agent_run init`），不是对着远端世界。
> 按 Agent 协议手工接入（owner 签发 binding + credential，执行端带 `--im-endpoint` 起）见 §6；
> 这条缝如实登记在此，不写成"今天可做"。

## 6. 接入位：外来 Agent 履约即公民

世界不关心跑 Agent 的是什么。任何后端只要讲得通 **Agent 协议**（BackendAuth 头部认证 +
拉取与消息端点 + 门铃），就能绑到一个 Agent 参与者身份上，此后**与出厂自带的执行端公民同权**：
同样的房间成员资格、同样的消息路径、同样的幂等保证（自己拉、自己说、不重复）。

- 契约在 [`../../docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md`](../../docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md)
  （拉取与消息端点 + §8 门铃；旧线的 Completion/ACK 端点已于 2026-09-28 **删除**，见其文档头），准入与初始配对在
  [`../../docs/contracts/core-v1/HOME_ACCESS.md`](../../docs/contracts/core-v1/HOME_ACCESS.md)。
- 绑定与凭据由这个世界的**拥有者**在 `/v1/agents/{id}/backend-bindings` 上签发与轮换：
  凭据归属于绑定、可轮换、可撤销，只代表那一个 Agent；同一 Agent 同时至多一支 active Binding
  （V1 的当前限制，不是永久架构承诺）。
- **Celira 不接管外来 Agent 的人格、模型、记忆与内部权限**——那些归它自己的系统。
- **本仓库不自带任何外部 Runtime 的接入实现**：2026-09-28 维护者裁决「不做第三方 Agent 适配器」，
  示例连接器 `im/connectors/hermes-celira/` 已删除（**不在本树里**——本仓的 Git 历史自「首次公开发布」
  起，边界见仓根 [`README.md`](../../README.md) 的「仓库地图与命名」末段）。出厂自带的执行端
  只有 Native Runtime 一家（`agent/`，仍在建设期）；要接自己的外部 Runtime，按上面那份 Agent
  协议自行实现。

## 7. 边界（本文件不承诺什么）

- **不承诺零 bug**，只承诺 Core v1 公共契约默认不改（解封条件见 `CORE_V1_ACCEPTANCE.md`）。
- **不提供远程管理协议**：没有人能隔着网络 `bootstrap-human`。首次配对只能在本机/SSH 上做，
  这是有意的设计（默认密码从部署完成到首次登录之间是一个抢占窗口）。
- **不生成 Agent 内容、不托管模型密钥**——执行端的私有事实一个都不进这个世界。
- **消息明文落 SQLite，加密未做**。这是当前事实，不是待办：共用一台服务器意味着部署者
  读得到所有人的公共消息。所以「多人共用一个世界」的性质是**朋友之间**，不是平台与用户。
