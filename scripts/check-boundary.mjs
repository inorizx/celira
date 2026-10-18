#!/usr/bin/env node
'use strict'
/**
 * **边界守卫**：客户端/执行端「要调的端点」必须真的在 im 服务端「注册了的端点」里。
 *
 * ## 这个守卫补的是哪一类洞
 *
 * 2026-09-29 真机实测抓到的形状（`im_my_profile` 恒 405 那条）：
 * 契约写着读面是 `GET /v1/backend/profile`，执行端却对**只注册了 PUT** 的
 * `/v1/backend/me/profile` 发 GET。后果不是"读不到"那么轻——写资料前必须先读
 * `base_version`，于是**Agent 连自己的资料都写不了**，而整次活动以
 * `system:indeterminate` 收场。
 *
 * 为什么三道防线都没拦住：
 * - **契约文档**：一直是对的，但没有任何机械判据拿它跟实现比；
 * - **im 门禁**：服务端没做错——它就没注册那个 GET；
 * - **执行端门禁**：假服务器**按实现里的常量应答**，实现与测试共享同一个错误假设，
 *   那条用例甚至把错的路径**钉在了断言里**（`im_app.rs` 的 `GET /v1/backend/me/profile`）。
 *
 * ⇒ 两个组件各自全绿，**边界上零覆盖**。本脚本就把边界变成一条会红的判据。
 *
 * ## 判据方向（写死，别反过来）
 *
 * **调用点 ⊆ 服务端路由**。反方向（服务端有、调用点没有）**不报红**——那是
 * "服务端领先客户端"，是安全的（`/v1/blocks*`、`/v1/calls*` 今天就是这种处境）。
 *
 * ## 它量得到什么、量不到什么（如实登记）
 *
 * - 量得到：路径打错、方法打错、段数对不上（`me` / `profile` 这类）；
 * - 量**不到**：请求体/响应体的字段形状、状态码语义、可见性裁决——那些要两边的真机用例。
 *   本脚本只做"端点存在性与方法"这一层，不假装管更多。
 *
 * 用法：
 *   node scripts/check-boundary.mjs            # 检查
 *   node scripts/check-boundary.mjs --verbose  # 连每个调用点一起列出来
 *
 * 退出码：0 = 全通过；1 = 有调用点对不上（或自检没抓住故意埋的违例）。
 */

import fs from 'node:fs'
import path from 'node:path'

const REPO = path.resolve(import.meta.dirname, '..')
const HTTP_DIR = path.join(REPO, 'im', 'server', 'src', 'adapters', 'http')
const CLIENT_FILES = [path.join(REPO, 'client', 'src', 'api', 'core.ts')]
const AGENT_SRC_DIR = path.join(REPO, 'agent', 'src')

/**
 * 射程内的执行端文件，以及**它为什么在射程里**（2026-10-08 扩射程那一刀）。
 *
 * 判据由主人点出：**什么会被注册成工具**——**模型侧能调的动词就是"工具"**，而动词打在世界的
 * 哪个端点上，是**字符串**（路径常量）：打错了两边各自编译通过、各自测试全绿，真机上只表现为
 * "她照目录去调，撞 405 / 404"（2026-09-29 那次就是这样）。
 *
 * - 三个 App 文件 = **工具来源**：`impl AppPort` 的 `declared_capabilities()` 就是**注册到模型侧的
 *   动词面**（通信面 / 社交面 / 资料面各一份声明，三个 App 共用 `im_channel` 那条传输）；
 * - `doorbell.rs` = **不是工具**（模型永远不会调它）——它是**唤醒通道**：路径打错不是"她调不动"，
 *   是**她永远醒不过来**。⇒ 它的目标是**一条常量**，因此**不按调用点量**，直接对路由表
 *   （见 `readRingPath`）。理由不同，所以**分开登记**，不混进同一条断言。
 */
const AGENT_FILES = [
  {
    file: 'im_app.rs',
    kind: 'tool',
    why: '工具来源：通信面（房间 / 消息 / 历史 / 呈现 / live / typing）',
  },
  { file: 'im_social_app.rs', kind: 'tool', why: '工具来源：社交面（通讯录 / 请求 / 拉黑）' },
  {
    file: 'im_profile_app.rs',
    kind: 'tool',
    why: '工具来源：资料面（读资料 / 写资料 / 头像）',
  },
  {
    file: 'doorbell.rs',
    kind: 'channel',
    why: '不是工具，是唤醒通道（模型不会调它；路径错 = 她永远醒不过来）',
  },
]

/**
 * 执行端里**打世界端点**的源文件：凡含 `"/v1/` 字面量的 `.rs`（递归扫 `agent/src`）。
 *
 * 它是**派生**的，不是手抄的——新增一个"打世界的文件"会**自动**出现在这里，于是下面那条
 * "每个射程内文件都要有理由"的断言会**当场要一个理由**（新增模块必须被人看见）。
 */
function agentEndpointFiles() {
  const found = []
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name)
      if (entry.isDirectory()) walk(full)
      else if (entry.name.endsWith('.rs') && fs.readFileSync(full, 'utf8').includes('"/v1/')) {
        found.push(path.relative(AGENT_SRC_DIR, full).split(path.sep).join('/'))
      }
    }
  }
  walk(AGENT_SRC_DIR)
  return found.sort()
}

/** 壳的装配入口（`generate_handler![…]` 就在这儿）与客户端源码根（扫 `invoke('…')` 用）。 */
const SHELL_LIB = path.join(REPO, 'app', 'src-tauri', 'src', 'lib.rs')
const CLIENT_SRC_DIR = path.join(REPO, 'client', 'src')

// ---------------------------------------------------------------------------
// 路径形状
// ---------------------------------------------------------------------------

/** 一个动态段：`{room_id}` / `${roomId}` / `{}` / `{room}` 全归一到这里。 */
const DYN = '{x}'

/**
 * 把一条路径拆成段（`{text, absorbing}`）：
 *
 * - 查询串一律丢掉（`?since=…`）：判据管的是端点，不管参数；
 * - 含插值的段**整段**算动态（`text = '{x}'`）；
 * - `absorbing` = 这一段是**两个及以上相邻插值、中间没有任何字面字符**拼出来的
 *   （Rust 侧的 `{participant}{suffix}`，`suffix` 自己是 `/accept` / `/decline`）。
 *   **只有这种段**才允许"吃掉"路由后面的段——单个 `{room}` 不许（它不可能含 `/`，
 *   否则 `/v1/backend/rooms/{room}` 就会被误配到 `.../messages` 上；自检里埋了这一条）。
 */
function segmentsOf(raw) {
  const withoutQuery = String(raw).split('?')[0]
  return withoutQuery
    .split('/')
    .filter((s) => s !== '')
    .map((rawSeg) => {
      const interpolations = (rawSeg.match(/\$\{[^}]*\}|\{[A-Za-z0-9_]*\}/g) ?? []).length
      const dynamic = interpolations > 0
      const onlyInterpolations = rawSeg.replace(/\$\{[^}]*\}|\{[A-Za-z0-9_]*\}/g, '').length === 0
      return {
        text: dynamic ? DYN : rawSeg,
        absorbing: dynamic && onlyInterpolations && interpolations >= 2,
      }
    })
}

/**
 * 调用点能不能落到某条路由上。
 *
 * - 段数：
 *   - 调用点比路由**长** ⇒ 不匹配；
 *   - **相等** ⇒ 只有当尾段**不是** absorbing 时才匹配（吸收型尾段意味着那个变量自带
 *     `/`，所以它必然还要吃掉至少一段 —— 否则 `…/contacts/{participant}{suffix}` 会误配到
 *     段数相同的 `…/contacts/requests` 上；自检里埋了这一条）；
 *   - 比路由**短** ⇒ 只有尾段是 absorbing 才匹配；
 * - 逐段比：调用点的字面段必须与路由的对应段逐字相等（路由那段是占位则任意）。
 */
function matches(callSegs, routeSegs) {
  const last = callSegs[callSegs.length - 1]
  if (callSegs.length > routeSegs.length) return false
  if (callSegs.length === routeSegs.length) {
    if (last.absorbing) return false
  } else if (!last.absorbing) {
    return false
  }
  for (let i = 0; i < callSegs.length; i += 1) {
    if (callSegs[i].text === DYN) continue
    if (routeSegs[i].text === DYN) continue
    if (callSegs[i].text !== routeSegs[i].text) return false
  }
  return true
}

// ---------------------------------------------------------------------------
// 服务端：注册了哪些端点
// ---------------------------------------------------------------------------

const METHODS = ['get', 'post', 'put', 'delete', 'patch']

function readServerRoutes(dir = HTTP_DIR) {
  const routes = []
  const files = fs
    .readdirSync(dir)
    .filter((f) => f.endsWith('.rs') && !f.endsWith('_tests.rs'))
    .sort()
  for (const file of files) {
    const text = fs.readFileSync(path.join(dir, file), 'utf8')
    // `.route("PATH", <handler chain>)` —— 允许链式调用跨行。
    const re = /\.route\(\s*"([^"]+)"\s*,\s*([\s\S]{0,200}?)\)\s*\n/g
    let m
    while ((m = re.exec(text)) !== null) {
      const methods = METHODS.filter((name) => new RegExp(`\\b${name}\\(`).test(m[2])).map((s) =>
        s.toUpperCase(),
      )
      routes.push({ path: m[1], methods, file })
    }
  }
  return routes
}

// ---------------------------------------------------------------------------
// 客户端：要调哪些端点
// ---------------------------------------------------------------------------

/** 每个 `/v1/` 字面量 + 它的方法。方法只看**紧跟其后、下一个 v1 字面量之前**那一段。 */
function readClientCalls(files = CLIENT_FILES) {
  const calls = []
  for (const file of files) {
    const text = fs.readFileSync(file, 'utf8')
    const re = /[`'"](\/v1\/[^`'"]*)[`'"]/g
    let m
    while ((m = re.exec(text)) !== null) {
      const raw = m[1]
      const lineStart = text.lastIndexOf('\n', m.index) + 1
      const line = text.slice(lineStart, m.index)
      if (/^\s*(\/\/|\*|\/\*)/.test(line)) continue // 注释里提到的端点不算调用点
      const toward = text.slice(m.index + m[0].length)
      const nextPath = toward.search(/[`'"]\/v1\//)
      const window = toward.slice(0, nextPath === -1 ? 160 : Math.min(nextPath, 160))
      let method = null
      const found = /method:\s*'(\w+)'/.exec(window)
      if (found) method = found[1].toUpperCase()
      else {
        const before = text.slice(Math.max(0, m.index - 80), m.index)
        method = /uploadMultipart\(\s*$/.test(before) ? 'POST' : 'GET'
      }
      calls.push({ side: 'client', file: path.basename(file), line: lineNumberAt(text, m.index), method, raw })
    }
  }
  return calls
}

// ---------------------------------------------------------------------------
// 执行端：要调哪些端点
// ---------------------------------------------------------------------------

/** `const NAME: &str = "…";` —— 路径常量与路径片段（`/messages` 这类后缀）都在这里。 */
function readAgentConsts(text, file) {
  const consts = new Map()
  const re = /const\s+([A-Z][A-Z0-9_]*)\s*:\s*&str\s*=\s*"([^"]*)";/g
  let m
  while ((m = re.exec(text)) !== null) consts.set(m[1], m[2])
  return consts
}

/**
 * 把一段 Rust 表达式（`format!(…)` / `CONST.replace(…)` / 本地小函数调用）解析成一条路径。
 * 解析不出来返回 null —— 调用方**必须如实报"未解析"**，不许静默跳过。
 */
function resolveAgentExpr(expr, consts, fnBodies) {
  // 形如 `MY_PRESENTATION_PATH.replace("{}", room)` / `ROOMS_PATH.to_string()`
  const direct = /^\s*([A-Z][A-Z0-9_]*)\s*(?:\.replace\([^;]*\)|\.to_string\(\))?\s*$/.exec(expr)
  if (direct && consts.has(direct[1])) return consts.get(direct[1])

  // 形如 `history_path(room, …)`：读那个本地小函数的体，按格式串解析（同一套规则）。
  const call = /\b([a-z_][a-z0-9_]*)\s*\(/.exec(expr)
  if (call && fnBodies.has(call[1])) return resolveAgentExpr(fnBodies.get(call[1]), consts, new Map())

  // `format!("{ROOMS_PATH}/{room}…")` 这类：先做具名捕获替换，再取里面那条 /v1/ 字面量。
  const s = expr.replace(/\{([A-Z][A-Z0-9_]*)\}/g, (whole, name) =>
    consts.has(name) ? consts.get(name) : whole,
  )
  const literals = [...s.matchAll(/"([^"]*)"/g)].map((x) => x[1])
  return literals.find((lit) => lit.startsWith('/v1/')) ?? null
}

/** 本地返回 String 的小函数（`fn history_path(…) -> String { … }`）：取它的体。 */
function readLocalFns(text) {
  const bodies = new Map()
  const re = /\bfn\s+([a-z_][a-z0-9_]*)\s*\([^)]*\)\s*->\s*String\s*\{/g
  let m
  while ((m = re.exec(text)) !== null) {
    bodies.set(m[1], text.slice(m.index, m.index + 600))
  }
  return bodies
}

/**
 * 传输层自己的那几个 helper（它们的 `path` 是**参数**，不是调用点）。
 *
 * ⚠ 三个 App 的调用形状**不一样**（2026-10-08 扩射程时实测，不是推断）：`im_app` 走自己那几个
 * 包装（`self.read(…)` / `self.exchange(…)`），而**社交面与资料面直接走传输通道**
 * （`self.channel.exchange(…)` / `self.channel.put_bytes(…)`）。两种接收者都必须认——否则
 * "扩射程"就只是把文件加进名单，**实际一条都扫不到**，而它照样报绿。
 */
const TRANSPORT_FNS = ['exchange', 'read', 'fetch_bytes', 'upload_multipart', 'put_bytes']

/**
 * 请求调用点认的接收者：`self.…` 或 `self.channel.…`，**而且链上允许换行**。
 *
 * ⚠ 资料面那两条实测是**三段链**（`self` ↵ `.channel` ↵ `.exchange(`）：只认两段会**静默漏掉**
 * 它们，而那个文件里还有别的调用点（头像），所以"每个文件至少一条"的下界**抓不住**这种
 * 部分漏扫 —— 2026-10-08 扩射程时踩到：总数 **72**，而按端点清单应当是 **74**。
 */
const RECEIVER = String.raw`self\s*(?:\.\s*channel)?\s*\.\s*`

/** 没有显式方法名时，这个方法名默认意味着什么方法。 */
const IMPLIED_METHOD = {
  exchange: 'GET',
  read: 'GET',
  fetch_bytes: 'GET',
  upload_multipart: 'POST',
  put_bytes: 'PUT',
}

/**
 * **唤醒通道**那一条（`doorbell.rs`）：它不是工具，因此**不按调用点量**——它只有一个目标，
 * 就是 `RING_PATH` 那条常量。这里直接把它对到路由表上：**一条常量对一条路由**。
 *
 * 为什么不能"反正它不是工具，就不管了"：路径打错时它不会报 405 给谁看，而是**连接一直失败**，
 * 表现为"她永远醒不过来"——那比一次调用失败更难查。
 */
function readRingPath() {
  const file = path.join(AGENT_SRC_DIR, 'doorbell.rs')
  const text = fs.readFileSync(file, 'utf8')
  const consts = readAgentConsts(text, file)
  const raw = consts.get('RING_PATH')
  if (!raw) throw new Error('doorbell.rs 里找不到 RING_PATH —— 唤醒通道的路径常量不见了')
  return { side: 'agent', file: 'doorbell.rs', method: 'GET', raw }
}

function readAgentCalls(file, text = fs.readFileSync(file, 'utf8')) {
  const consts = readAgentConsts(text, file)
  const fnBodies = readLocalFns(text)
  const calls = []
  const unresolved = []

  // 1) `let path = <expr>;`
  const pathVars = []
  const letRe = /let\s+path\s*=\s*([\s\S]{0,400}?);\s*\n/g
  let lm
  while ((lm = letRe.exec(text)) !== null) pathVars.push({ at: lm.index, expr: lm[1] })
  const nearestPathExpr = (at) => {
    let best = null
    for (const pv of pathVars) if (pv.at < at && (best === null || pv.at > best.at)) best = pv
    return best ? best.expr : null
  }
  const enclosingFnName = (at) => {
    const head = text.slice(0, at)
    const m = [...head.matchAll(/\bfn\s+([a-z_][a-z0-9_]*)\s*[(<]/g)].pop()
    return m ? m[1] : null
  }

  // 2) 请求调用点：exchange(方法, 目标, …) / read(目标) / fetch_bytes(目标, …) /
  //    upload_multipart(操作, **目标**, …) / put_bytes(**目标**, …) —— 注意 upload_multipart
  //    与 put_bytes 的目标位置不同（前者第二个实参，后者第一个）。
  const patterns = [
    { fn: 'exchange', re: new RegExp(`${RECEIVER}exchange\\(\\s*"(\\w+)"\\s*,\\s*([^,]+)`, 'g') },
    { fn: 'read', re: new RegExp(`${RECEIVER}read\\(\\s*([^,)]+)`, 'g') },
    { fn: 'fetch_bytes', re: new RegExp(`${RECEIVER}fetch_bytes\\(\\s*([^,)]+)`, 'g') },
    {
      fn: 'upload_multipart',
      re: new RegExp(`${RECEIVER}upload_multipart\\(\\s*[^,]+,\\s*([^,]+)`, 'g'),
    },
    // 头像那条路（`im_profile_app`）走 `put_bytes`：2026-10-08 补进来——它从前不在已知的
    // 四种传输方法里，因此**射程里也没有它**。
    { fn: 'put_bytes', re: new RegExp(`${RECEIVER}put_bytes\\(\\s*([^,)]+)`, 'g') },
  ]
  for (const { fn, re } of patterns) {
    let cm
    while ((cm = re.exec(text)) !== null) {
      // 跳过这几个 helper **自己的定义**（那里的 `path` 是形参）。
      if (TRANSPORT_FNS.includes(enclosingFnName(cm.index))) continue
      const explicit = cm.length > 2 ? cm[1] : null
      const target = cm[cm.length - 1].trim()
      const method = explicit ? explicit.toUpperCase() : (IMPLIED_METHOD[fn] ?? 'GET')

      let raw = null
      if (consts.has(target)) raw = consts.get(target)
      else if (/^&?path$/.test(target)) {
        const expr = nearestPathExpr(cm.index)
        if (expr) raw = resolveAgentExpr(expr, consts, fnBodies)
      } else if (/^[A-Z][A-Z0-9_]*$/.test(target)) raw = consts.get(target) ?? null

      if (raw === null || !String(raw).startsWith('/v1/')) {
        unresolved.push({ file: path.basename(file), line: lineNumberAt(text, cm.index), fn, target })
        continue
      }
      calls.push({
        side: 'agent',
        file: path.basename(file),
        line: lineNumberAt(text, cm.index),
        method,
        raw,
      })
    }
  }
  return { calls, unresolved }
}

/**
 * **壳注册了哪些命令**（`generate_handler![ a::b, … ]` 里每一项的**最后一段**就是命令名）。
 *
 * 找不到 `generate_handler!` ⇒ **抛**（不许静默返回空表：那会让"注册表里什么都没有"看起来像
 * "客户端什么都没叫"，于是这条检查变成装饰）。
 */
function readShellCommands(file = SHELL_LIB) {
  const text = fs.readFileSync(file, 'utf8')
  const block = /generate_handler!\s*\[([\s\S]*?)\]/.exec(text)
  if (!block) throw new Error(`壳里找不到 generate_handler!（${file}）—— 守卫无从对账`)
  // ⚠ 先去掉行注释（列表里可以写注释 —— 我第一版就被自己写的注释坑了），再按逗号切；
  //   **最后一项后面没有逗号**，所以不能拿"逗号"当项的边界（第一版漏掉了最后一条命令）。
  const body = block[1].replace(/\/\/[^\n]*/g, '')
  const names = []
  for (const piece of body.split(',')) {
    const trimmed = piece.trim()
    if (trimmed === '') continue
    if (!/^[A-Za-z_][A-Za-z0-9_:]*$/.test(trimmed)) {
      throw new Error(`generate_handler! 里这一项认不出来：${JSON.stringify(trimmed)}（守卫不许猜）`)
    }
    names.push(trimmed.split('::').pop())
  }
  return names
}

/**
 * **客户端叫了哪些壳命令**（`invoke('名字', …)`）。
 *
 * ⚠ 只扫 `client/src/**`（产品源码）—— 用例里会有**假命令**（夹具故意叫一个不存在的名字来验
 * "叫不动壳时怎么办"），那不是接缝上的调用点。
 * ⚠ 解析不出来的（`invoke(变量)`）**也报**，不静默跳过：静默跳过等于这块边界少盖一块，
 * 而少盖的那块永远不会有人知道。
 * ⚠ **一个例外**（2026-10-05，写在明处）：`api.invoke(command, args)` 那种**方法**调用是
 * 通用桥本身（`shellLocal.ts` 的 `shellInvokeOrNull` 返回的就是它），实参本来就该是变量 ——
 * 命令名在它的**调用点**上（那些是字面量，照样逐个对账）。所以"实参不是字面量"只对
 * **裸调用** `invoke(…)` 报红。
 */
function readClientInvokes(dir = CLIENT_SRC_DIR) {
  const files = []
  const walk = (each) => {
    for (const entry of fs.readdirSync(each, { withFileTypes: true })) {
      const full = path.join(each, entry.name)
      if (entry.isDirectory()) walk(full)
      else if (/\.tsx?$/.test(entry.name)) files.push(full)
    }
  }
  walk(dir)

  const calls = []
  const unresolved = []
  // `(?<![.\w])` ⇒ 不把 `api.invoke(…)` 当成接缝调用（那是通用桥，见上面那条例外）。
  const QUOTED = /(?<![.\w])invoke\s*(?:<[^>]*>)?\s*\(\s*(['"`])([A-Za-z_][A-Za-z0-9_]*)\1/g
  const BARE_ANY = /(?<![.\w])invoke\s*(?:<[^>]*>)?\s*\(/g
  const METHOD_QUOTED = /\.invoke\s*(?:<[^>]*>)?\s*\(\s*(['"`])([A-Za-z_][A-Za-z0-9_]*)\1/g
  for (const file of files) {
    const text = fs.readFileSync(file, 'utf8')
    const found = new Set()
    for (const match of text.matchAll(QUOTED)) {
      found.add(match.index)
      calls.push({
        name: match[2],
        file: path.relative(REPO, file),
        line: lineNumberAt(text, match.index),
      })
    }
    // 方法形式只要实参是字面量，也照样对账（`api.invoke('名字')` 一样会打错）。
    for (const match of text.matchAll(METHOD_QUOTED)) {
      calls.push({
        name: match[2],
        file: path.relative(REPO, file),
        line: lineNumberAt(text, match.index),
      })
    }
    for (const match of text.matchAll(BARE_ANY)) {
      if (!found.has(match.index)) {
        unresolved.push({
          file: path.relative(REPO, file),
          line: lineNumberAt(text, match.index),
        })
      }
    }
  }
  return { calls, unresolved }
}

function lineNumberAt(text, index) {  let n = 1
  for (let i = 0; i < index; i += 1) if (text[i] === '\n') n += 1
  return n
}

// ---------------------------------------------------------------------------
// 自检：故意埋的违例**必须**被抓到（否则这个守卫就是装饰）
// ---------------------------------------------------------------------------

/**
 * **壳命令那一条接缝**（2026-10-05 加）：客户端叫的壳命令 ⊆ 壳注册的命令。
 *
 * 为什么要有它：客户端叫壳是**字符串**（`invoke('local_store_read')`），壳注册也是字符串
 * （`generate_handler![local_store::local_store_read]`）—— 两边各自编译通过、各自测试全绿，
 * 而名字打错一个字母**谁都不会知道**，真机上表现为"这个功能没反应"。这与上面那条
 * HTTP 接缝是**同一个判据**（调用点 ⊆ 提供方），只是换了一条边界，所以挂在这个守卫里。
 *
 * 判据方向与上面一致：**壳领先客户端不算错**（壳可以多注册几个命令）。
 */
function commandSeamMissed(calls, registered) {
  const have = new Set(registered)
  return calls.filter((each) => !have.has(each.name))
}

function selfCheck() {
  const routes = [
    { path: '/v1/backend/profile', methods: ['GET'] },
    { path: '/v1/backend/me/profile', methods: ['PUT'] },
    { path: '/v1/backend/rooms/{room_id}/messages', methods: ['GET', 'POST'] },
    { path: '/v1/backend/contacts/requests', methods: ['POST'] },
    { path: '/v1/backend/contacts/{participant_id}/accept', methods: ['POST'] },
  ].map((r) => ({ ...r, segs: segmentsOf(r.path) }))

  const landsOn = (method, callPath, routePath) => {
    const route = routes.find((r) => r.path === routePath)
    if (!route) throw new Error(`自检夹具里没有这条路由：${routePath}`)
    return route.methods.includes(method) && matches(segmentsOf(callPath), route.segs)
  }

  const cases = [
    // [期望, 方法, 调用点, 目标路由, 说明]
    [true, 'GET', '/v1/backend/profile', '/v1/backend/profile', '正对照：契约 §2.2 那条读路径'],
    [false, 'GET', '/v1/backend/me/profile', '/v1/backend/me/profile', '★ 真机那条缺陷：读打到了只注册 PUT 的写路径'],
    [false, 'PUT', '/v1/backend/profile', '/v1/backend/profile', '方法反了也要红'],
    [true, 'POST', '/v1/backend/rooms/{room}/messages', '/v1/backend/rooms/{room_id}/messages', '占位段互相匹配'],
    [false, 'GET', '/v1/backend/rooms/{room}/mesages', '/v1/backend/rooms/{room_id}/messages', '字面段打错要红'],
    [false, 'GET', '/v1/backend/rooms/{room}', '/v1/backend/rooms/{room_id}/messages', '单个占位尾段不许吃掉后面的段'],
    [true, 'POST', '/v1/backend/contacts/{participant}/accept', '/v1/backend/contacts/{participant_id}/accept', '末段是字面量、段数相等'],
    [false, 'POST', '/v1/backend/contacts/{participant}/accep', '/v1/backend/contacts/{participant_id}/accept', '末段字面量打错要红'],
    // 吸收型尾段（Rust 侧 `{participant}{suffix}`，suffix 自带 `/`）：它必然还要吃掉至少一段。
    [true, 'POST', '/v1/backend/contacts/{participant}{suffix}', '/v1/backend/contacts/{participant_id}/accept', '吸收型尾段吃到 accept'],
    [false, 'POST', '/v1/backend/contacts/{participant}{suffix}', '/v1/backend/contacts/requests', '★ 吸收型尾段不许误配到段数相同的路由'],
  ]

  const missed = []
  for (const [expected, method, callPath, routePath, why] of cases) {
    if (landsOn(method, callPath, routePath) !== expected) {
      missed.push(`${expected ? '该通过却红了' : '该红却过了'}：${method} ${callPath} → ${routePath}（${why}）`)
    }
  }

  // 壳命令接缝的正反例（同样的纪律：故意埋的违例必须被抓到）。
  const seamCases = [
    [0, [{ name: 'local_store_read' }], ['local_store_read', 'local_store_write'], '正对照：叫的就在注册表里'],
    [1, [{ name: 'local_store_readd' }], ['local_store_read'], '★ 名字打错一个字母要红'],
    [1, [{ name: 'anjia_start' }], [], '注册表里没有 ⇒ 红'],
    [0, [{ name: 'local_store_read' }], ['local_store_read', 'shell_extra'], '壳多注册不算错（方向是 ⊆）'],
  ]
  for (const [expected, calls, registered, why] of seamCases) {
    const got = commandSeamMissed(calls, registered).length
    if (got !== expected) {
      missed.push(`壳命令接缝自检：${why} —— 期望 ${expected} 处违例，实得 ${got}`)
    }
  }

  // 解析形状的正反例（2026-10-08 扩射程那一刀）：**接收者链**与**传输方法**都必须认得出，
  // 认不出的形状**不许被当成 0 条**悄悄放过去（那正是"扩了射程却什么都没盖住"的形状）。
  const parseCases = [
    [1, 'im_app 自己的包装', 'self.exchange("PUT", ROOMS_PATH, None, None)'],
    [1, '两段链（社交面）', 'self.channel.exchange("GET", CONTACTS_PATH, None, None)'],
    [
      1,
      '三段链（资料面实测形状：self ↵ .channel ↵ .exchange）',
      'self\n            .channel\n            .exchange("GET", MY_PROFILE_READ_PATH, None, None)',
    ],
    [1, 'put_bytes（头像那条路）', 'self.channel.put_bytes(MY_AVATAR_PATH, &header, body, &key)'],
    [0, '不认识的方法名不算调用点（由"声明了的路径必须有人量"那条兜住）', 'self.channel.ask("GET", CONTACTS_PATH, None, None)'],
  ]
  const fixtures = [
    'const ROOMS_PATH: &str = "/v1/backend/rooms";',
    'const CONTACTS_PATH: &str = "/v1/backend/contacts";',
    'const MY_PROFILE_READ_PATH: &str = "/v1/backend/profile";',
    'const MY_AVATAR_PATH: &str = "/v1/backend/me/avatar";',
  ].join('\n')
  for (const [expected, why, snippet] of parseCases) {
    const got = readAgentCalls('<self-check>', `${fixtures}\n${snippet}`).calls
    if (got.length !== expected) {
      missed.push(`调用点解析自检：${why} —— 期望 ${expected} 条，实得 ${got.length} 条`)
    }
  }
  // `put_bytes` 隐含的方法必须是 PUT（不是默认的 GET）。
  const avatar = readAgentCalls(
    '<self-check>',
    `${fixtures}\nself.channel.put_bytes(MY_AVATAR_PATH, &header, body, &key)`,
  ).calls
  if (avatar[0]?.method !== 'PUT') {
    missed.push(`调用点解析自检：put_bytes 隐含的方法应当是 PUT，实得 ${avatar[0]?.method}`)
  }

  // 总数**从用例数组算**，不写常量 —— 手写的数字迟早跟用例对不上，那时它会替守卫说谎。
  return { missed, total: cases.length + seamCases.length + parseCases.length + 1 }
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

function main() {
  const verbose = process.argv.includes('--verbose')

  const { missed: selfMissed, total: selfTotal } = selfCheck()
  if (selfMissed.length > 0) {
    console.error('✗ 自检未通过 —— 这个守卫抓不住它自己埋的违例，先修它：')
    for (const line of selfMissed) console.error(`    ${line}`)
    process.exit(1)
  }

  const routes = readServerRoutes().map((r) => ({ ...r, segs: segmentsOf(r.path) }))
  const clientCalls = readClientCalls()

  // ---- 执行端射程：**派生** ＋ 每个文件要有理由 ＋ 每个"工具来源"文件要有调用点 ----------
  const endpointFiles = agentEndpointFiles()
  const rangeProblems = []
  for (const file of endpointFiles) {
    if (!AGENT_FILES.some((each) => each.file === file)) {
      rangeProblems.push(
        `${file} 里有 "/v1/ 字面量（它在打世界端点），却没有登记理由 —— 新增模块必须被人看见`,
      )
    }
  }
  for (const { file, kind, why } of AGENT_FILES) {
    if (!endpointFiles.includes(file)) {
      rangeProblems.push(
        `${file} 登记在射程里（${kind}：${why}），但它今天不含 "/v1/ 字面量 —— 名单陈旧了`,
      )
    }
  }

  const agentCalls = []
  const agentUnresolved = []
  const ringCalls = []
  for (const { file, kind } of AGENT_FILES) {
    // 唤醒通道不按调用点量（它不是工具，只有一个目标）⇒ 直接拿那条常量对路由表。
    if (kind === 'channel') {
      ringCalls.push(readRingPath())
      continue
    }
    const one = readAgentCalls(path.join(AGENT_SRC_DIR, file))
    // **下界**：射程里的"工具来源"文件必须至少解析出一条调用点。没有它，"扩射程"可以只是
    // 把文件加进名单而**一条都扫不到**——那是最坏的一种绿（看着盖住了，其实没盖）。
    if (one.calls.length === 0) {
      rangeProblems.push(
        `${file} 在射程里却一条调用点都没解析出来 —— 扩射程变成了空跑（它照样会报绿）`,
      )
    }
    // **声明了的路径必须有人量**：文件里每条**完整路径常量**（值以 `/v1/` 开头）都必须出现在
    // 某一条已解析调用点里（相等，或是它的前缀——`ROOM_HISTORY_PREFIX` 那种拼路径的常量）。
    // 片段常量（`/messages` 这类不以 `/v1/` 开头的）不算。
    //
    // 为什么单要这一条：**部分漏扫**骗得过上面那个下界——文件里只要还有别的调用点，下界就过。
    // 2026-10-08 实测踩到：资料面那两条是**三段链**（`self` ↵ `.channel` ↵ `.exchange(`），
    // 被漏掉两条，总数只从 74 变成 72，肉眼看不出来。这一条会当场点名那个常量。
    const consts = readAgentConsts(fs.readFileSync(path.join(AGENT_SRC_DIR, file), 'utf8'), file)
    for (const [name, value] of consts) {
      if (!value.startsWith('/v1/')) continue
      if (!one.calls.some((call) => call.raw === value || call.raw.startsWith(value))) {
        rangeProblems.push(
          `${file}：路径常量 ${name}（${value}）没有被任何已解析的调用点用到 —— 要么它是死常量，要么解析漏了`,
        )
      }
    }
    agentCalls.push(...one.calls)
    agentUnresolved.push(...one.unresolved)
  }
  const calls = [...clientCalls, ...agentCalls, ...ringCalls]

  console.log(
    `边界守卫：服务端 ${routes.length} 条路由 ｜ 客户端 ${clientCalls.length} 个调用点 ｜ 执行端 ${agentCalls.length} 个调用点 ＋ 唤醒通道 ${ringCalls.length} 条`,
  )

  let failed = 0
  for (const call of calls) {
    const segs = segmentsOf(call.raw)
    const hit = routes.find((r) => r.methods.includes(call.method) && matches(segs, r.segs))
    if (!hit) {
      failed += 1
      const samePath = routes.filter((r) => matches(segs, r.segs))
      const why =
        samePath.length === 0
          ? '服务端没有这条路径'
          : `路径在，但它只答 ${samePath.flatMap((r) => r.methods).join('/')}`
      console.error(`  ✗ ${call.method} ${call.raw}  ← ${call.side}:${call.file}:${call.line}`)
      console.error(`      ${why}`)
    } else if (verbose) {
      console.log(`  ✓ ${call.method} ${call.raw}  → ${hit.path}`)
    }
  }

  if (agentUnresolved.length > 0) {
    // **未解析也要红**：静默跳过等于这个守卫少盖一块，而少盖的那块永远不会有人知道。
    failed += agentUnresolved.length
    console.error('  ✗ 有调用点解析不出来（守卫必须如实报，不许静默跳过）：')
    for (const u of agentUnresolved) {
      console.error(`      agent:${u.file}:${u.line}  ${u.fn}(…) 目标=${u.target}`)
    }
  }

  if (rangeProblems.length > 0) {
    failed += rangeProblems.length
    console.error('  ✗ 执行端射程有问题：')
    for (const line of rangeProblems) console.error(`      ${line}`)
  }

  // ---- 第二条接缝：客户端叫的壳命令 ⊆ 壳注册的命令 --------------------------
  const registered = readShellCommands()
  const invokes = readClientInvokes()
  const seamMissed = commandSeamMissed(invokes.calls, registered)
  console.log(
    `壳命令接缝：壳注册 ${registered.length} 条 ｜ 客户端叫 ${invokes.calls.length} 处`,
  )
  for (const miss of seamMissed) {
    failed += 1
    console.error(`  ✗ invoke('${miss.name}')  ← ${miss.file}:${miss.line}`)
    console.error('      壳里没有注册这条命令（名字打错？还是忘了 generate_handler! 那一行？）')
  }
  if (invokes.unresolved.length > 0) {
    failed += invokes.unresolved.length
    console.error('  ✗ 有 invoke 解析不出来（守卫必须如实报，不许静默跳过）：')
    for (const u of invokes.unresolved) {
      console.error(`      client:${u.file}:${u.line}  invoke(…) 的实参不是字面量`)
    }
  }

  if (failed > 0) {
    console.error(`\n边界守卫未通过：${failed} 处调用点在提供方那边对不上。`)
    console.error('判据方向是「调用点 ⊆ 提供方」——提供方领先调用方不算错。')
    process.exit(1)
  }
  console.log(
    `边界守卫通过：${calls.length} 个调用点全部落在服务端路由表里，且方法匹配；` +
      `客户端叫的 ${invokes.calls.length} 处壳命令全部已注册（自检 ${selfTotal}/${selfTotal}）。`,
  )
}
main()
