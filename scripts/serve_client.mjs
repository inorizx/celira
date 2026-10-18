#!/usr/bin/env node
// 发布形态的**静态页服务**：把 `client-dist/` 端给浏览器，并把 `/v1/*` 转给 im 服务端、
// `/home-status/*` 转给本机只读状态口。
//
// 为什么需要它：发布形态（解压 zip）里**没有 client 工程**（没有 `package.json`、没有
// `node_modules`）⇒ `vite preview` 用不了。而 client 页面是按**同源相对路径**访问的
// （REST + 两条 WebSocket：`/v1/human/events/stream`、`/v1/human/live/stream`），所以静态托管
// 必须**自带同一套代理**，否则页面能开、功能不通。
//
// 第二段代理（TASK_61）：页面的"运行状态"要读**本机状态口**（`home.js status --serve`），
// 而那个口开在 `home.conf` 的 `status_port` 上（可改，默认 5190）。前端是静态托管、
// **读不到 `home.conf`** ⇒ 让页面自己知道端口这件事没有省事的路；于是页面只问**同源**的
// `/home-status/*`，由这里代转 —— "口在哪"因此是**运行期事实**，改口页面自己跟着走。
// 顺带的好处：同源之后状态口不需要 CORS 了（那一层响应头已随 TASK_61 删掉）。
//
// 第三段代理（TASK_77）：同一条道理，写口那边也一样 —— 页面的「模型配置」要写**模型配置门**
// （`home.js config --serve`，`home.conf` 的 `config_port`，默认 5191）。页面同样只问同源的
// `/home-config`，端口由这里代转 ⇒ 页面永远不知道它开在哪。
//
// 零依赖（只用 node: 内置模块）。诚实边界：**release 用户免 Rust、不免 Node**——
// 曾设想的"零 Node 甜点"（本机托管形态）需要 CORE/RUNTIME/LAUNCHER 三个子进程
// 可执行件，**不是**纯静态托管 ⇒ 已随 Host 线退役，这里就是唯一的静态托管形态。
//
// 用法：
//   node scripts/serve_client.mjs --dist client-dist --port 5173 --im http://127.0.0.1:8080 \
//                               [--status-port 5190] [--config-port 5191]

import { createServer, request as httpRequest } from 'node:http'
import { connect } from 'node:net'
import { createReadStream, statSync } from 'node:fs'
import { createRequire } from 'node:module'
import { extname, join, normalize, resolve, sep } from 'node:path'

// 读那个家自己的 `home.conf`（**门的端口由那个家自己说了算**，见下面的 `doorPortOf`）。
// 复用 `scripts/home/conf.js` 的读法：一份事实只有一个权威位置。
const { confGet } = createRequire(import.meta.url)('./home/conf.js')

const argv = process.argv.slice(2)
const option = (name, fallback) => {
  const at = argv.indexOf(name)
  return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback
}

const dist = resolve(option('--dist', 'client-dist'))
const port = Number(option('--port', '5173'))
const upstream = new URL(option('--im', 'http://127.0.0.1:8080'))
// **家事实**（2026-09-25）：`home.js up` 起这一支时会带上 `--home <家目录>`。
// 它对**行为**没有任何影响（这一支只认 dist/port/im 与两个代转端口），但它有两件用处：
//   1. 它进进程表 —— 同一份发布包服务多个家时，"这支属于哪个家"只有命令行说得清
//      （认领判据的第二维，见 `scripts/home/proc.js` 的 `homeFlagVerdict`）；
//   2. 它进读数 —— 日志里一眼看得出这一支是哪个家的。
// 手工直敲（手册里那条命令）不带它 ⇒ 那一维缺席，判据退回端口，行为与从前一字不差。
const homeDir = option('--home', '')
/**
 * **门的端口由那个家自己说了算**（2026-10-11）。
 *
 * 缺省链三条，顺序就是优先级：
 *   ① 显式给了 `--status-port` / `--config-port` ⇒ 用它 —— `home.js up` 起这一支时走的就是
 *      这条路（它先把两个口从 `home.conf` 读出来再传进来，那条路一直是对的）；
 *   ② 没给那个旗标、但给了 `--home <家目录>` ⇒ **去问那个家**（读它自己的 `home.conf`）；
 *   ③ 两样都没有（手册里那条手工直敲）⇒ 仍是默认的 5190 / 5191，行为与从前一字不差。
 *
 * ⚠ **为什么必须有 ②**（2026-10-11 实测的事故）：临时世界那几条**真机门禁**自己 spawn 这一支时
 * 只给了 `--home`，于是两个口落到默认的 5190 / 5191 —— 而那两格在"主人正开着自己的家"时
 * **属于主人家的门**。页面的 `/home-status/*` 与 `/home-config` 代转因此**打到了主人家的门上**：
 * 实测一次 `probe-zero` 把主人家的模型清单加了一条、并把两支 Agent 的绑定都改到了那一条。
 * 判据一句话：**同一条纪律已经在「世界口」上落地了（`conf.worldBaseOf`），门这两个口是它的
 * 另一半** —— 半修等于没修。
 *
 * 形状不对时**兜默认值并如实说一句**，不拒绝启动：这一支拿到的只可能是被人手改坏的
 * `home.conf`（`home.js` 那边会先拒），而"静态页照样能开"比"因为一个端口数字整页打不开"重要得多；
 * 真兜错了也只是页面照常说"口子未开"——不撒谎。
 */
function doorPortOf(flag, confKey, fallback) {
  const declared = String(option(flag, '')).trim()
  if (declared !== '') return declared
  if (!homeDir) return fallback
  return confGet(join(homeDir, 'home.conf'), confKey, fallback)
}

function parsedPort(raw, fallback, flag) {
  const value = Number.parseInt(String(raw), 10)
  if (Number.isInteger(value) && value > 0 && value <= 65535) return value
  console.log(`[client] ${flag} 读不懂（${raw}）⇒ 按默认 ${fallback} 代转`)
  return fallback
}

const statusPort = parsedPort(
  doorPortOf('--status-port', 'status_port', '5190'),
  5190,
  '--status-port',
)
// 模型配置门那一支（TASK_77）：**同一条缺省链、同一种兜法**（理由与上面同款）。
const configPort = parsedPort(
  doorPortOf('--config-port', 'config_port', '5191'),
  5191,
  '--config-port',
)

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.gif': 'image/gif',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.map': 'application/json; charset=utf-8',
  '.wasm': 'application/wasm',
  '.txt': 'text/plain; charset=utf-8'
}

function serveFile(res, filePath, status = 200) {
  const type = TYPES[extname(filePath).toLowerCase()] ?? 'application/octet-stream'
  res.writeHead(status, { 'Content-Type': type })
  createReadStream(filePath).pipe(res)
}

// 静态：先按 URL 找文件；找不到就回 index.html（SPA 路由交回前端）。
// 只允许 dist 之内的路径（`..` 与绝对路径一律拒绝）。
function serveStatic(req, res) {
  const path = decodeURIComponent((req.url ?? '/').split('?')[0])
  const relative = normalize(path).replace(/^([/\\])+/, '')
  const candidate = join(dist, relative)
  if (!candidate.startsWith(dist + sep) && candidate !== dist) {
    res.writeHead(403).end('forbidden')
    return
  }
  try {
    if (statSync(candidate).isFile()) {
      serveFile(res, candidate)
      return
    }
  } catch {
    // 落到 SPA 回退
  }
  try {
    serveFile(res, join(dist, 'index.html'))
  } catch {
    res.writeHead(404).end('not found')
  }
}

// HTTP 代理：`/v1/*` 原样转给 im（方法、头、体、状态码、响应头都带过去）。
function proxyHttp(req, res) {
  const forwarded = httpRequest(
    {
      host: upstream.hostname,
      port: upstream.port,
      method: req.method,
      path: req.url,
      headers: { ...req.headers, host: upstream.host }
    },
    (answer) => {
      res.writeHead(answer.statusCode ?? 502, answer.headers)
      answer.pipe(res)
    }
  )
  forwarded.on('error', () => {
    if (!res.headersSent) {
      res.writeHead(502, { 'Content-Type': 'text/plain; charset=utf-8' })
    }
    res.end('bad gateway: im 服务端不在？')
  })
  req.pipe(forwarded)
}

/**
 * 同源门后面的**状态口路径表**：`/home-status/` 之下**只有这两条**。
 *
 * 为什么是穷举映射而不是"剥掉前缀往后转"：状态口的端面本来就是**穷举两条**
 * （`/status` / `/healthz`，其余 404）。代理若剥前缀泛转，那个口哪天多长一条路由，
 * 这里就**静默地**把它也暴露到同源面上（可达面悄悄变大）。写成表，这一屏就说清了
 * 「同源门能通往口子的哪两个地方」。
 */
const STATUS_ROUTES = new Map([
  ['/home-status/status', '/status'],
  ['/home-status/healthz', '/healthz']
])

/**
 * 状态口代理：把同源路径转给 `127.0.0.1:<status_port>`（方法、头、状态码、响应头都带过去，
 * 与 `/v1` 那一段同款）。**不代转任何别的东西。**
 *
 * 口子没起（还没 `up`、或已经 `down`）⇒ 如实 502 ⇒ 页面走既有降级文案
 * 「本机状态口未开」——那是**本机管家没答话**，不是 Agent 的故障。
 */
function proxyStatus(req, res, targetPath) {
  const forwarded = httpRequest(
    {
      host: '127.0.0.1',
      port: statusPort,
      method: req.method,
      path: targetPath,
      headers: { ...req.headers, host: `127.0.0.1:${statusPort}` }
    },
    (answer) => {
      res.writeHead(answer.statusCode ?? 502, answer.headers)
      answer.pipe(res)
    }
  )
  forwarded.on('error', () => {
    if (!res.headersSent) {
      res.writeHead(502, { 'Content-Type': 'text/plain; charset=utf-8' })
    }
    res.end('bad gateway: 本机状态口没开（node home.js status --serve）')
  })
  req.pipe(forwarded)
}

/**
 * 同源门后面的**模型配置门路径表**：门自己的**四条**路径**逐条**列出（方法照传）。
 *
 * 与状态口那张表同款：代理不剥前缀泛转 —— 口子哪天多长一条路由，不该经由这里**静默**
 * 变成同源可达。四条都是**同名透传**（不剥皮、不改名）：`/home-config`（清单读写）、
 * `/home-config/probe`（只读探测，`POST`）、`/home-config/catalog`（只读目录，`POST`；
 * 2026-10-14 加）、`/home-binding`（换绑 / 解绑，`PUT`）。
 *
 * ⚠ 从前这里只有 `/home-config` 一条 —— 于是发布形态（普通用户的默认形态）里
 * 「获取模型列表」与「换绑 / 解除指定」两个动作**打不到口子**：它们落到 SPA 回退，
 * 拿到一份 `200` 的 `index.html`，页面按"读不懂的回执"报错（v1 收口三小债之一）。
 * ⚠ 2026-10-14 同族的第二次：新加的 `/home-config/catalog` 忘了进这张表 ⇒ 页面上
 * 「看这家的型号…」拿到的是 HTML，只能说"没能问到那一家"（**真机门禁当场抓到** ——
 * `verify-ui` 里那条"点了要问得到"的判据就是为这一类加的）。
 */
const CONFIG_ROUTES = new Map([
  ['/home-config', '/home-config'],
  ['/home-config/probe', '/home-config/probe'],
  ['/home-config/catalog', '/home-config/catalog'],
  ['/home-binding', '/home-binding']
])

/**
 * 模型配置门代理：把 `GET`/`POST`/`PUT`/`DELETE` 原样转给 `127.0.0.1:<config_port>`
 * （方法、头、体、状态码、响应头都带过去，与 `/v1` 那一段同款）。**不代转任何别的东西。**
 *
 * ⚠️ 写方法要**带上请求体**（`req.pipe(forwarded)` 已经做了这件事，与 `/v1` 同一条路）——
 * 少了它 `PUT` 会变成"保存一份空配置"，而页面看起来像是保存成功。
 *
 * 口子没起（还没 `up`、或已经 `down`）⇒ 如实 502 ⇒ 页面走既有降级文案
 * 「模型配置门没开」——那是**本机管家没答话**，不是 Agent 的故障。
 */
function proxyConfig(req, res, targetPath) {
  const forwarded = httpRequest(
    {
      host: '127.0.0.1',
      port: configPort,
      method: req.method,
      path: targetPath,
      headers: { ...req.headers, host: `127.0.0.1:${configPort}` }
    },
    (answer) => {
      res.writeHead(answer.statusCode ?? 502, answer.headers)
      answer.pipe(res)
    }
  )
  forwarded.on('error', () => {
    if (!res.headersSent) {
      res.writeHead(502, { 'Content-Type': 'text/plain; charset=utf-8' })
    }
    res.end('bad gateway: 模型配置门没开（node home.js config --serve）')
  })
  req.pipe(forwarded)
}

const server = createServer((req, res) => {
  if ((req.url ?? '').startsWith('/v1/')) {
    proxyHttp(req, res)
    return
  }
  const target = (req.url ?? '').split('?')[0]
  // ★ **查询串必须一起带过去**（2026-10-14 真机实测抓到）：模型配置门的「删一条」是
  //   `DELETE /home-config?id=ms-3`，而**丢了 `?id=`** 就退化成"解除绑定"——**静默做错事**
  //   （200、页面说删好了、那条还在）。它只在这一条代转路上发生（壳形态走 Rust 传输，不经这里）。
  const query = (req.url ?? '').includes('?') ? `?${(req.url ?? '').split('?').slice(1).join('?')}` : ''
  const statusTarget = STATUS_ROUTES.get(target)
  if (statusTarget !== undefined) {
    proxyStatus(req, res, statusTarget)
    return
  }
  const configTarget = CONFIG_ROUTES.get(target)
  if (configTarget !== undefined) {
    proxyConfig(req, res, configTarget + query)
    return
  }
  serveStatic(req, res)
})

// WebSocket（以及任何 Upgrade）：把整条连接原样转给 im（双向 pipe）。
// 少了这一段，页面能开但**实时唤醒那条链**不通。
server.on('upgrade', (req, socket, head) => {
  if (!(req.url ?? '').startsWith('/v1/')) {
    socket.destroy()
    return
  }
  const target = connect(Number(upstream.port || 80), upstream.hostname, () => {
    target.write(`${req.method} ${req.url} HTTP/1.1\r\n`)
    for (let index = 0; index < req.rawHeaders.length; index += 2) {
      target.write(`${req.rawHeaders[index]}: ${req.rawHeaders[index + 1]}\r\n`)
    }
    target.write('\r\n')
    if (head && head.length) {
      target.write(head)
    }
    target.pipe(socket)
    socket.pipe(target)
  })
  target.on('error', () => socket.destroy())
  socket.on('error', () => target.destroy())
})

server.listen(port, () => {
  console.log(
    `[client] 静态页 http://localhost:${port} （dist=${dist}${homeDir ? `，home=${homeDir}` : ''}，` +
      `/v1 → ${upstream.origin}，` +
      `/home-status → http://127.0.0.1:${statusPort}，` +
      `/home-config · /home-config/probe · /home-binding → http://127.0.0.1:${configPort}）`
  )
})
