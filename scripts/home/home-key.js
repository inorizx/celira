'use strict'
/**
 * **家钥匙与家口的远程准入**（家口远程访问刀）——这一层的全部职责：
 * 「这个家在不在网络上、钥匙够不够、敲门的人手里那把对不对」。
 *
 * ## 一件事实、两个模式
 *
 * 两个照看口（只读状态口 `status-server.js` / 模型配置门 `config-server.js`）**按绑定地址**
 * 二选一，不混搭：
 *
 * - **模式一（默认）**：绑 loopback。行为与本刀之前**一字不变** —— 无钥匙、按
 *   `Host`/`Origin` 认来源（防 DNS rebinding）、页面走同源代转。走这条路的用户
 *   一个字节都不必改。这道来源守卫由**两口共用**（`localOriginAllowed`），
 *   不各写一份（同一件事两个实现，迟早只改一边）。
 * - **模式二（本刀新增）**：绑非 loopback（`0.0.0.0` / 局域网 IP / 主机名）。
 *   于是「物理不可达」这层防护消失了，**唯一新增的信任边界是『谁在敲门』** ⇒
 *   用一把机器级钥匙收口：`Authorization: Bearer <钥匙>` + TLS。
 *
 * ## 钥匙是什么（与模型 key 不是一类，别混淆）
 *
 * 它是**机器的准入凭据**：按机器发、一台执行端一把、由这台机器一次性产出
 * （`node home.js issue-home-key`）。它**不是身份**（不回答"你是谁"，只回答"能不能进这台
 * 机器"），与 Human token 同类（possession 证明），可以存在客户端本地。
 * 泄了怎么办：**换一把** —— 重跑签发命令覆盖摘要，所有已配的连接在下一次请求就失效。
 *
 * ## 五条纪律
 *
 * 1. **落盘的只有摘要**：钥匙原文只在签发那一刻打印一次，**永不落盘、不进日志、不进响应、
 *    不进 URL**。文件里那 64 个十六进制字符答不了"钥匙是什么"。
 * 2. **每请求校验、无会话缓存**：摘要**每个请求现读**（文件极小）⇒ 换钥匙＝立即全部失效，
 *    不需要任何"让旧会话过期"的机制。
 * 3. **比的是摘要，且定时长比较**（`crypto.timingSafeEqual`）：不拿字符串比钥匙。
 * 4. **fail-closed 两条**：绑非 loopback 而家里没有钥匙摘要、或没有 TLS 证书 ⇒
 *    **拒绝启动**（在 `resolveLaunch` 里，开口之前就拒），并说清缺什么。
 * 5. **错与缺说的是同一个码**（`UNAUTHORIZED`）：没带钥匙 / 钥匙不对 / 摘要没了 ——
 *    对敲门的人都是"进不去"；如实分开细节会把家口变成一个探测面。
 * 6. **两道闸各管一种模式，不叠加**：来源守卫（`Host`/`Origin`）只属于**模式一**
 *    （那才是防 DNS rebinding 的那一条）；模式二里浏览器**本来就是跨源的** ——
 *    二口的预检与 CORS 头正是为它而设（`preflight` / `attachCors`）。在模式二上
 *    再按 loopback 卡一遍 `Host`，等于把远程那条路**自己堵死**：请求永远来自
 *    `<远程地址>:<端口>`，而那是必然非 loopback 的。
 */

const crypto = require('node:crypto')
const fs = require('node:fs')
const net = require('node:net')
const path = require('node:path')

const P = require('./paths')
const { EXIT } = require('./codes')

/**
 * 摘要文件（落在家目录下）。内容**只有**一行 64 位小写十六进制 SHA-256 摘要。
 *
 * 为什么不放进 `home.conf`：那是给人手编的配置（端口、身份 id），而这是一份**凭据派生物**
 * —— 混进去会让"改配置"与"换钥匙"变成同一件事，也会让备份配置的人顺带备份一份可爆破的
 * 摘要。分开住，一件事一个文件。
 */
const KEY_FILE_NAME = 'home_key.sha256'

/** 钥匙原文的字节数：256 bit（裁决要求 ≥256 bit）。 */
const KEY_BYTES = 32

/** 钥匙的字面前缀：让它一眼看得出是"家钥匙"（与 Human token / 模型 key 区分开）。 */
const KEY_PREFIX = 'clrhk_'

/** 摘要的形状（唯一认的形状；别的都当"读不出来"）。 */
const DIGEST_SHAPE = /^[0-9a-f]{64}$/

/**
 * 远程模式**唯一**的拒绝码（401）。
 *
 * 它是家口对外的稳定词表里新增的那一格 —— `config-server.js` 的 `ERRORS` 引用它、
 * 客户端解码端（`client/src/api/homeConfig.ts`）配同一份。**一个码一处权威**。
 */
const UNAUTHORIZED = 'unauthorized'

/** 两口共用的默认绑定：现状（mode 一）。 */
const DEFAULT_BIND = '127.0.0.1'

function keyFilePath(homeDir) {
  return path.join(homeDir, KEY_FILE_NAME)
}

/** 生成一把钥匙（≥256 bit CSPRNG；base64url ⇒ 不含引号/空白等需要在 URL 或头里转义的字符）。 */
function generateKey() {
  return `${KEY_PREFIX}${crypto.randomBytes(KEY_BYTES).toString('base64url')}`
}

/** 摘要算法**只有这一处**：签发与校验都走它（两处实现迟早漂移）。 */
function hashKey(token) {
  return crypto.createHash('sha256').update(String(token), 'utf8').digest('hex')
}

/**
 * 原子写摘要（临时文件 + rename：断电/并发下不会留下一份半写的摘要）。
 * 覆盖旧文件＝换钥匙（旧钥匙下一请求即失效）。
 */
function writeKeyDigest(homeDir, digest) {
  const file = keyFilePath(homeDir)
  const temp = path.join(homeDir, `.${KEY_FILE_NAME}.${process.pid}.tmp`)
  fs.writeFileSync(temp, `${digest}\n`, 'utf8')
  try {
    fs.renameSync(temp, file)
  } catch (error) {
    try {
      fs.rmSync(temp, { force: true })
    } catch {
      // 清不掉临时文件不改变结论（要报的是 rename 失败的因）
    }
    throw error
  }
}

/**
 * 读摘要。四种结局分开说（**"没有钥匙"与"读不出来"不是一回事**）：
 * `present`（有且合形状）/ `missing`（文件不在）/ `unreadable`（在但读不动或形状不对）。
 */
function readKeyDigest(homeDir) {
  const file = keyFilePath(homeDir)
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch (error) {
    if (error && error.code === 'ENOENT') return { state: 'missing', file }
    return { state: 'unreadable', file, reason: 'read-failed' }
  }
  const digest = text.trim()
  if (!DIGEST_SHAPE.test(digest)) return { state: 'unreadable', file, reason: 'malformed' }
  return { state: 'present', file, digest }
}

/**
 * 敲门的人手里那把对不对：**先各自哈希、再定时长比较**（比的是摘要，不是原文；
 * 长度固定 ⇒ `timingSafeEqual` 不会因长度不同抛）。
 */
function keyMatches(digest, token) {
  if (!DIGEST_SHAPE.test(String(digest ?? ''))) return false
  if (typeof token !== 'string' || token === '') return false
  const expected = Buffer.from(String(digest), 'hex')
  const presented = Buffer.from(hashKey(token), 'hex')
  return expected.length === presented.length && crypto.timingSafeEqual(expected, presented)
}

/**
 * 从 `Authorization` 头里取出 Bearer 值；没有/不是 Bearer ⇒ `''`（调用方按"没钥匙"处置）。
 * 方案名大小写不敏感（RFC 7235），值**原样**取（不 trim 内部字符，只去首尾空白）。
 */
function bearerOf(header) {
  if (typeof header !== 'string') return ''
  const match = /^\s*Bearer\s+(.+?)\s*$/i.exec(header)
  return match ? match[1] : ''
}

/**
 * 这个绑定地址算不算 loopback（模式一/模式二的分界）。
 *
 * 判据按**地址的物理含义**，不是按字符串白名单：`127.0.0.0/8` 整段都是本机
 * （Windows 上也一样），`::1`、`localhost` 同理。
 */
function isLoopbackBind(bind) {
  const value = String(bind ?? '').trim()
  if (value === '') return true
  if (value.toLowerCase() === 'localhost') return true
  const family = net.isIP(value)
  if (family === 4) return value.split('.')[0] === '127'
  if (family === 6) return value === '::1'
  return false
}

/**
 * **开口之前的那一关**：按绑定地址算出这是哪个模式，并把 fail-closed 的两条核完。
 *
 * 返回 `{ ok: true, access, tls }` 或 `{ ok: false, message }`（message 是给人看的一句，
 * **说清缺什么**；`serve()` 打印它并以"需要你动手"退出）。
 *
 * - mode 一（bind 是 loopback）：`access = { mode: 'local' }`，`tls = null`
 *   —— 与从前一字不差（不读钥匙、不读证书，配了也当没配）。
 * - mode 二（bind 非 loopback）：**先要钥匙摘要**（缺 / 读不出来都拒），
 *   **再要 TLS 证书与私钥**（缺 / 读不出来都拒）。两样都齐才算数。
 */
function resolveLaunch({ bind, homeDir, tlsCert, tlsKey }) {
  const value = String(bind ?? '').trim() || DEFAULT_BIND
  if (isLoopbackBind(value)) return { ok: true, access: { mode: 'local' }, tls: null }

  const digest = readKeyDigest(homeDir)
  if (digest.state === 'missing') {
    return {
      ok: false,
      message:
        `绑的是非本机地址（${value}），但家里还没有家钥匙 —— 远程的家必须带钥匙。` +
        '先跑 node home.js issue-home-key 发一把。',
    }
  }
  if (digest.state === 'unreadable') {
    return {
      ok: false,
      message:
        `家钥匙摘要读不出来（${digest.file}${digest.reason === 'malformed' ? '：内容不是一行 sha256 摘要' : ''}）—— ` +
        '修好它，或重跑 node home.js issue-home-key 换一把新的。',
    }
  }

  const certPath = String(tlsCert ?? '').trim()
  const keyPath = String(tlsKey ?? '').trim()
  // 缺哪份说哪份（"缺证书"与"缺私钥"是两件事，只修一半照样起不来）。
  const missing = []
  if (!certPath) missing.push('tls_cert')
  if (!keyPath) missing.push('tls_key')
  if (missing.length > 0) {
    return {
      ok: false,
      message:
        `绑的是非本机地址（${value}），远程必须走 TLS —— 但 home.conf 里没有 ${missing.join(' 与 ')}。` +
        '把证书与私钥的路径填上（自签证书也行），或者先把绑定改回 127.0.0.1。',
    }
  }
  const unreadable = []
  for (const [label, file] of [
    ['tls_cert', certPath],
    ['tls_key', keyPath],
  ]) {
    try {
      fs.statSync(file)
    } catch {
      unreadable.push(`${label}（${file}）`)
    }
  }
  if (unreadable.length > 0) {
    return {
      ok: false,
      message: `TLS 证书文件读不到：${unreadable.join(' 与 ')} —— 修好路径，或先把绑定改回 127.0.0.1。`,
    }
  }

  let tls
  try {
    tls = { cert: fs.readFileSync(certPath), key: fs.readFileSync(keyPath) }
  } catch (error) {
    return {
      ok: false,
      message: `TLS 证书读不出来（${error && error.message ? error.message : error}）—— 检查文件的读取权限。`,
    }
  }
  return { ok: true, access: { mode: 'remote', homeDir }, tls }
}

/** 只认这些来源（模式一：`Origin` 与 `Host` 各判一次）。 */
const LOOPBACK_HOSTS = ['127.0.0.1', 'localhost', '[::1]']

/**
 * **壳自己的来源**（Tauri 的自定义协议）—— 2026-10-05 主人裁：它也算本机。
 *
 * 为什么非认它不可：app 里页面走的是**内嵌产物**（`tauri build` 那一形态），它的来源是
 * `http://tauri.localhost`（Windows/Linux 的 webview）或 `tauri://localhost`（macOS），
 * **不是 loopback** ⇒ 配置门的来源守卫会把自己的页面拒掉：真机实测，打包形态里机器页
 * 「模型」那一间显示"读不到模型配置 ／ 这个来源不被允许（只允许从本机访问）"，
 * 而同一台机器上 dev 形态（页面在 `localhost:5173`）一直是绿的 —— 所以这一条**只在产品
 * 形态上成立**（`app/README.md` §5 第 10 行）。
 *
 * ⚠ **它没有把口子放宽**（判据，不是感觉）：模式一**本来就认整族 loopback 来源**
 * （`http://localhost:<任何端口>`、`http://127.0.0.1:<任何端口>` —— 本机任何一个页面都算），
 * 而 `tauri.localhost` 是 **Tauri 自定义协议**的名字：普通浏览器里它由 `.localhost` 规则
 * 解析回 127.0.0.1，能"以这个来源供内容"的只有壳自己（别的进程要在 5191 上供内容，
 * 那它已经就是这道门了）。⇒ 认它，等于把"本机的东西"这条判据**说全**，不是开新口子。
 * **不是**"哪个域名解析到 127.0.0.1 都放行"——那正是 `Host` 那一道在挡的东西，一个字没动。
 */
const SHELL_ORIGINS = [
  // Windows / Linux 的 webview：自定义协议挂在 `tauri.localhost` 这个名字上。
  { protocol: 'http:', hostname: 'tauri.localhost' },
  { protocol: 'https:', hostname: 'tauri.localhost' },
  // macOS 的 webview：自定义协议是 `tauri://localhost`。
  { protocol: 'tauri:', hostname: 'localhost' },
]

/**
 * `Origin` 是不是**壳自己**（见 `SHELL_ORIGINS` 的理由）。
 *
 * ⚠ **协议与主机成对匹配**（不是"协议对就行"）：只判协议会把 `tauri://evil.example`
 * 放进来 —— 那是一条**由别人供内容**的来源（2026-10-05 写这条时被自己的反例打回来过）。
 * 端口一律不看，与 loopback 那一条同款。
 */
function shellOriginAllowed(origin) {
  try {
    const url = new URL(origin)
    return SHELL_ORIGINS.some(
      (each) => each.protocol === url.protocol && each.hostname === url.hostname.toLowerCase(),
    )
  } catch {
    return false
  }
}

/**
 * `Host` / `Origin` 头里的主机名是不是 loopback（端口随便）。
 *
 * 端口一律不看：同一个口可以被 `127.0.0.1:5190`、`localhost:5190` 两种写法敲到，
 * 而判据是"它是不是本机"，不是"敲的哪个端口"。
 */
function loopbackHostHeader(header) {
  if (typeof header !== 'string' || header === '') return false
  const host = header.startsWith('[')
    ? header.slice(0, header.indexOf(']') + 1)
    : header.split(':')[0]
  return LOOPBACK_HOSTS.includes(host)
}

/**
 * **模式一的来源守卫**（两个口共用这一份实现）。
 *
 * 两道，判的都是"敲门的是不是本机的东西"：
 *   · `Host` 必须是 loopback —— **这一条才是防 DNS rebinding 的那一条**
 *     （浏览器带着攻击者的域名发请求时，`Host` 就是那个域名，而页面自己以为同源）；
 *   · `Origin` **有就必须**是 loopback（浏览器的跨源写会带它；同源 fetch 也带），
 *     **或者壳自己的来源**（见 `SHELL_ORIGIN_HOSTS`）。
 *     没有 `Origin` 的（命令行 `curl`、同源表单）放行 —— **可达面由绑定决定，
 *     不由响应头决定**。
 *
 * ⚠ **模式二不调用本函数**：那一边的闸是家钥匙 + TLS，且浏览器必然跨源
 * （见文件头纪律 6）。两道闸是"按模式二选一"，不是叠加。
 */
function localOriginAllowed(req) {
  if (!loopbackHostHeader(req.headers.host)) return false
  const origin = req.headers.origin
  if (origin === undefined) return true
  // 壳自己的来源（Tauri 自定义协议）也算本机 —— 见 `SHELL_ORIGIN_HOSTS` 那段理由。
  if (shellOriginAllowed(origin)) return true
  try {
    return loopbackHostHeader(new URL(origin).host)
  } catch {
    return false
  }
}

/** 模式二下每个响应（含 4xx/5xx）都要带 CORS 头，否则浏览器读不到那份拒绝。模式一：一个都不加。 */
function corsHeaders(access) {
  if (!access || access.mode !== 'remote') return {}
  return { 'access-control-allow-origin': '*' }
}

/** 把 CORS 头挂到这次响应上（写在 `writeHead` 之前 ⇒ 两个口各自的 `respond()` 都不用改）。 */
function attachCors(res, access) {
  for (const [name, value] of Object.entries(corsHeaders(access))) res.setHeader(name, value)
}

/**
 * OPTIONS 预检（模式二）。**放在认证之前**：预检请求按规范不带 `Authorization`，
 * 若先认证，浏览器连"可以带钥匙"这一步都学不到。
 *
 * 放行的头**只列实际会用的两个**（`authorization` / `content-type`），不写通配 ——
 * 通配会让将来"多一个头"这件事静悄悄发生。
 */
function preflight(req, res, access, methods) {
  if (!access || access.mode !== 'remote') return false
  if (req.method !== 'OPTIONS') return false
  res.writeHead(204, {
    ...corsHeaders(access),
    'access-control-allow-methods': methods.join(', '),
    'access-control-allow-headers': 'authorization, content-type',
    'access-control-max-age': '600',
  })
  res.end()
  return true
}

/**
 * 认证（模式二）。摘要**每个请求现读**（无会话缓存 ⇒ 换钥匙下一请求即失效）。
 *
 * 拒的时候一律 401 + 同一个稳定码，**不区分"没带"与"不对"**（对敲门的人它们是一件事；
 * 分开说会给家口加一个探测面）。`WWW-Authenticate` 不发：钥匙不是给人手敲的，
 * 弹一个浏览器登录框只会误导。
 */
function authorize(req, res, access) {
  if (!access || access.mode !== 'remote') return true
  const digest = readKeyDigest(access.homeDir)
  const ok = digest.state === 'present' && keyMatches(digest.digest, bearerOf(req.headers.authorization))
  if (ok) return true
  res.writeHead(401, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    ...corsHeaders(access),
  })
  res.end(JSON.stringify({ error: UNAUTHORIZED }))
  return false
}

/**
 * `node home.js issue-home-key` —— 发一把家钥匙。
 *
 * 打印**一次**（照 `up --token` 的 `[token]` 行先例），随即把**摘要**原子写入
 * `<家>/home_key.sha256`。重跑＝覆盖摘要＝**旧钥匙立即全部失效**（打印里明说这一句）。
 * 钥匙原文不落盘、不进日志。
 */
function issue() {
  const homeDir = P.homeDir()
  try {
    fs.mkdirSync(homeDir, { recursive: true })
  } catch (error) {
    console.log(
      `[home-key] 家目录建不出来（${homeDir}）：${error && error.message ? error.message : error}`,
    )
    return EXIT.ENV_UNAVAILABLE
  }
  const token = generateKey()
  try {
    writeKeyDigest(homeDir, hashKey(token))
  } catch (error) {
    console.log(
      `[home-key] 摘要写不进去（${keyFilePath(homeDir)}）：${error && error.message ? error.message : error}`,
    )
    return EXIT.ENV_UNAVAILABLE
  }
  console.log(`[home-key] ${token}`)
  console.log('[home-key] 这一串只显示这一次（家里只留它的摘要）；重跑本命令 = 换一把新的，旧钥匙立刻全部失效')
  console.log(`[home-key] 摘要已落 ${keyFilePath(homeDir)}；怎么用见 scripts/HOME_MANUAL.md`)
  return EXIT.OK
}

module.exports = {
  KEY_FILE_NAME,
  KEY_BYTES,
  KEY_PREFIX,
  DIGEST_SHAPE,
  UNAUTHORIZED,
  DEFAULT_BIND,
  LOOPBACK_HOSTS,
  keyFilePath,
  generateKey,
  hashKey,
  writeKeyDigest,
  readKeyDigest,
  keyMatches,
  bearerOf,
  isLoopbackBind,
  loopbackHostHeader,
  localOriginAllowed,
  resolveLaunch,
  corsHeaders,
  attachCors,
  preflight,
  authorize,
  issue,
}