'use strict'
/**
 * 本机只读状态口（TASK_55）：**窗问 home.js，不直接问执行端**。
 *
 * 四条设计理由（这条是那个"腰"，别漂移）：
 *  1. **红线合规**：执行端私有状态（lane / credential / last_run 内部）绝不进 im 服务端、
 *     绝不经 Core API —— 这里问的是**环境账本**（admission / 活动收场），不是 Runtime 内部。
 *  2. **窗不直连执行端进程**：`agent_host_status` 是一次性 CLI 不是常驻服务，页面没法
 *     也不该直接对话它；经 home.js 的口，才有「谁能问、问什么」的门。
 *  3. **home.js 已经是这个家的单一编排者**（TASK_50/51）；状态口是它职责的自然延伸。
 *     注意它**只读**：编排纪律不变，这里没有任何"顺手重启"的能力（远程控制是未来另裁的事，
 *     预建即焊死）。
 *  4. **Host 线不动**：页面 Host 分支冻结原样，等删除刀。
 *
 * 端面**只有两个**：`GET /status` 与 `GET /healthz`。没有第三种 ——
 * 没有写、没有凭据、没有 im 代理（那是 `serve_client.mjs` 的职责，**不合并**：
 * 一个端页面、一个端读数，合并 = 一个进程两个身份）。
 *
 * **口上无秘密的物理论证**：只读、输出只有"本家读数"（admission / 活动收场 / [next]）；
 * spawn 子进程时用**白名单 env**，家的凭据（`DEEPSEEK_API_KEY` / `CELIRA_IM_BACKEND_AUTH` /
 * token）**一个字节都不经过它**。
 *
 * **默认仍然是 loopback、免钥匙**（`status_bind` 缺省 `127.0.0.1`）——那份论证在模式一下
 * 原样成立，一个字没松。家口远程访问刀加的**模式二**（绑非 loopback）是另一档：
 * 物理不可达这层防护没有了 ⇒ 进门要带**家钥匙**（`Authorization: Bearer`）+ TLS，
 * 两条 fail-closed 与钥匙纪律都在 `home-key.js`（那一层是这块的唯一权威）。
 */

const http = require('node:http')
const https = require('node:https')
const fs = require('node:fs')
const path = require('node:path')
const { spawn } = require('node:child_process')

const P = require('./paths')
const { confGet } = require('./conf')
const { EXIT } = require('./codes')
const proc = require('./proc')
const modelSources = require('./model-sources')
const residents = require('./residents')
const homeKey = require('./home-key')
const { parseHostStatus } = require('./host_status_parse')
const { ledgerBody } = require('./ledger_line')

/**
 * 状态口默认端口。**带默认值**（持久字段纪律）：老 `home.conf` 里没有 `status_port`
 * 这一键也要能起来——新键绝不能变成"必须手改配置才能跑"的门。
 *
 * 默认值本身住在 `paths.js`（与 `DEFAULT_LISTEN` / `DEFAULT_CLIENT_PORT` 同一个家——
 * 它们是同一类事实）；这里只转引，免得同一件事有两个权威。
 */
const DEFAULT_STATUS_PORT = P.DEFAULT_STATUS_PORT

/** 最近几条活动收场（页面要能一眼看完，不是把 32 条全倒过去）。 */
const RECENT_RUNS = 8

/**
 * **模式一**被来源守卫挡下时的稳定码（403）——与配置门**同一个词**（一个故障一个词：
 * 那一边是 `config-server.js` 的 `ERRORS.ORIGIN_NOT_ALLOWED`，两处逐字相同）。
 *
 * ⚠ 客户端**不解码**本口的错误串：状态那一族只按状态码分类（401 = 钥匙不对、
 * 其余非 2xx = `http`），所以这里不需要在 `client/` 配一份解码表——与配置门那条
 * "封闭表两侧对齐"的纪律不冲突（那一族的解码端在 `client/src/api/homeConfig.ts`）。
 */
const ORIGIN_NOT_ALLOWED = 'origin_not_allowed'

/**
 * **读数的短 TTL（TASK_72 ③）**：这段时间内的重复请求**直接用上一次那一份**。
 *
 * 为什么要有它：一次探测要 spawn `agent_host_status` 读整个账本再探一次 im 健康，实测 **3.7 秒**
 * ⇒ 页面每次进来都先显示"正在读取本机状态…"。TTL 内直接给上次那份，配合 `serve()` 的**开机预热**，
 * 页面第一眼就有东西看。
 *
 * 取 **15 秒**（裁决单要求 ≤30 秒）：它只是"别让同一次开机被反复读账本"的一档，不是缓存策略。
 *
 * ⚠️ **代价必须被标注，不许吞掉**：TTL 内的读数**可能不是这一轮的**。所以
 *   - `readAt` **原样带着那次真读数的时刻**（绝不改成"当下"）；
 *   - 响应多一个机械读数 `stale`：`age > ttlMs` 时是 `true`（页面据此说"这是缓存读数"）。
 */
const STATUS_TTL_MS = 15000

/**
 * **Agent 的经历的机械投影**（TASK_72 ④）：界面上只给最近这么多条。
 *
 * Agent 的经历是**私有世界**：`trace.ndjson` 里 3834 条中有 2796 条带正文或参数
 * （实测 2026-09-21）。所以这一面**只投影机械字段**，字段用**白名单**（见
 * [`EXPERIENCE_FIELDS`]）——白名单而不是黑名单：以后 trace 加字段时，
 * 默认是"不给 Agent 看"，而不是"顺手漏出去"。
 */
const EXPERIENCE_ROWS = 40

/**
 * 投影时**唯一允许过桥**的字段（逐条白名单）。
 *
 * ⚠️ 排除清单与客户端那份**逐字对齐**（`client/src/api/homeStatus.ts` 的同一段注释）——
 * 判据：**一个字段进来之前先问"它会不会带正文"**，会就不进。
 *
 * - **会带正文的**：`params`（参数值里就是 Agent 说的话/写的标题）、`blocks`（结果正文）、
 *   `text`（宿主输入正文）、`content` / `payload`（模型原文与推理）、`extra-actions`
 *   （动作清单，可能带参数）；
 * - **不带正文、但也不在白名单里**：`protocol` / `protocol-version`（实测取值
 *   `chat-completions` / `1`）—— 被丢掉是**白名单制的必然结果**，不是漏掉。
 *   点这两条名，是为了让这份清单**不再看起来像穷举**。
 */
const EXPERIENCE_FIELDS = [
  'seq',
  'run',
  'turn',
  'kind',
  'stage',
  'action',
  'index',
  'state',
  'decision',
  'subject',
  'system',
  'ending',
  'failure',
  // 刀2b C②：**派生**字段（不是 trace 里的一格）——那一场的起源，由 `projectOrigin` 从
  // `run-start` 注入行的 `activity-origin` 块**机械映射**出来（认不出⇒`unknown`，绝不猜）。
  // 它进来之前同样过了那一问："会不会带正文？"——不会：它是两三个稳定取值之一。
  'origin',
]

/**
 * 三种**起源定稿句**（与 `agent/src/os.rs` 的三个公开常量**逐字对齐**）→ 稳定取值。
 *
 * ⚠ 这是一份**闭集**：认不出的句子一律 `unknown`（**不猜、不塞进最近的那一档**）。
 * 逐字对齐的理由与"两侧词表对表"同款：那三句是**产品定稿**，改了就得同时改这里；
 * 有一条测试专门喂三句原文断言映射。
 */
const ORIGIN_SENTENCES = [
  ['bell', 'This activity was started by the doorbell: the backend event stream signaled that new events are available in the im app.'],
  ['timer', 'This activity was started by the timer: an alarm set in this environment has arrived.'],
  ['manual', 'This activity was started manually by the host. No external event triggered it.'],
]

/** 起源块在注入行里的 label（`os.rs` 的 `MANUAL_ORIGIN_LABEL`）。 */
const ORIGIN_LABEL = 'activity-origin'

/**
 * 那一场的**起源**（刀2b C②）：从它自己的 `run-start` 注入行里机械取出来。
 *
 * 判据（实测的形状，见刀2b 报告 §前置读数）：
 * - 只在 `kind === 'injection' && stage === 'run-start'` 这一行上取 —— 每场**恰好一条**；
 * - 那一行的 `blocks[]` 里 **label = `activity-origin`** 的那一块，`text` 就是起源定稿句；
 * - 三句**逐字比**；都不同 ⇒ `unknown`（认不出就是认不出）；
 * - ⚠ `text` 本身**不进口**（它是正文）：出去的只有**映射出来的稳定取值**。
 *
 * 返回 `string | null`：`null` = 这一行不是起源行（或没有那一块）⇒ **不加这个字段**。
 */
function projectOrigin(row) {
  if (row.kind !== 'injection' || row.stage !== 'run-start') return null
  const blocks = Array.isArray(row.blocks) ? row.blocks : []
  for (const block of blocks) {
    if (!block || typeof block !== 'object' || block.label !== ORIGIN_LABEL) continue
    const text = typeof block.text === 'string' ? block.text.trim() : ''
    for (const [origin, sentence] of ORIGIN_SENTENCES) {
      if (text === sentence) return origin
    }
    return 'unknown'
  }
  return null
}

/** 只读文件**尾部**最多这么多字节（经历文件可能几 MB，且我们只要最后几条）。 */
const EXPERIENCE_TAIL_BYTES = 262144

/**
 * 读 Agent 的经历的**尾部**（不整读：文件里含模型原文，几 MB 是常态）。
 *
 * 三种结局分开说（与 `agent` 同一套口径）：读不到文件 / 文件在但读不出来 / 读到了。
 * **"没有经历"与"读不到经历"是两件事**——前者不该在这条路上出现（读到了、是空的，
 * 才算"没有经历"）。
 */
function readAgentExperience({ traceFile, limit = EXPERIENCE_ROWS }) {
  let raw
  try {
    const size = fs.statSync(traceFile).size
    const start = Math.max(0, size - EXPERIENCE_TAIL_BYTES)
    const handle = fs.openSync(traceFile, 'r')
    try {
      const buffer = Buffer.alloc(Math.min(EXPERIENCE_TAIL_BYTES, size - start))
      fs.readSync(handle, buffer, 0, buffer.length, start)
      raw = buffer.toString('utf8')
    } finally {
      fs.closeSync(handle)
    }
  } catch {
    // 文件不存在 ⇒ 这次"没有经历可读"（不是"Agent 没有经历"）。
    return { state: 'missing' }
  }

  // 尾部第一行可能是被截断的半行 ⇒ 只有**不在开头**的那一行才可能不完整，丢掉它。
  const lines = raw.split(/\r?\n/).filter((line) => line.trim() !== '')
  const rows = []
  for (const line of lines) {
    let row
    try {
      // ⚠ **行尾那一列不是正文**（2026-10-06 起每行带一个校验和，见 `ledger_line.js`）。
      // 从前这里对整行 `JSON.parse` ⇒ 对那之后的**每一行**都抛 ⇒ 读数成
      // `{state:'loaded', rows:[]}`：**"读不出来"被报成了"她没有经历"**，
      // 而本函数自己的注释写着那两件事必须分开。
      row = JSON.parse(ledgerBody(line))
    } catch {
      continue // 半行/坏行跳过（机械读数面不因一行坏数据整体失败）
    }
    rows.push(projectExperience(row))
  }
  // 尾部窗口可能不含全部经历 ⇒ `total` 只声称"这一窗口里有多少条"，并用 `windowed` 标注，
  // 免得把"窗口内的数"说成"Agent 经历的总数"（那是撒谎）。
  const shown = rows.slice(-limit)
  return {
    state: 'loaded',
    rows: shown,
    windowed: raw.length >= EXPERIENCE_TAIL_BYTES,
    truncated: rows.length > shown.length,
  }
}

/** 一条经历 → 机械读数（**只过白名单字段**；值不是字符串/数字的一律丢）。 */
function projectExperience(row) {
  const out = {}
  if (typeof row !== 'object' || row === null) return out
  for (const key of EXPERIENCE_FIELDS) {
    const value = row[key]
    if (typeof value === 'string' || typeof value === 'number') out[key] = value
  }
  // 起源是**派生**的（白名单里那一格 `origin` 在 trace 里并不存在）：
  // 从同一行的 `blocks[]` 里映射出来，只放稳定取值，正文不进。
  const origin = projectOrigin(row)
  if (origin !== null) out.origin = origin
  return out
}

/**
 * 读**环境根的身份**（刀2b C①）：`<环境根>/identity.json` 的 `agent`。
 *
 * 为什么要它：状态口从前是**机器级**读数，不带"这是哪个 Agent 的账本" ⇒ 前端判不了
 * "这个 Agent 是否在本机运行"（刀2a 登记的判据缺口）。有了这一格，那件事就能判了。
 *
 * 三条边界：
 * - **只给 id，不给名字**：实测 `identity.json` 里只有 `{agent: <id>}`（形状就这么大），
 *   而这个名字**不在本机**——它在 im 的公共世界（`GET /v1/backend/me`），而本口的进程环境是
 *   **白名单、不含任何凭据**（口上无秘密那条纪律）⇒ 本口**拿不到** display_name。
 *   名字用前端自己已有的那份（它本来就持有自己 Agent 的显示名）。
 * - **读不出来 ⇒ 不给这一格**（不编一个 id 出来）；
 * - 它是**文件里的记录**（环境根的权威），不是"调用方声明的那个"——两者不一致时以文件为准。
 */
function readEnvIdentity(envDir) {
  let text
  try {
    text = fs.readFileSync(path.join(envDir, 'identity.json'), 'utf8')
  } catch {
    return null
  }
  try {
    const parsed = JSON.parse(text)
    const agent = parsed && typeof parsed.agent === 'string' ? parsed.agent.trim() : ''
    return agent || null
  } catch {
    return null
  }
}

/**
 * spawn `agent_host_status` 时只给这些环境变量。
 *
 * 白名单而不是继承：那个二进制本来就不吐凭据，但"不吐"是它的属性，"不给"是我们的纪律 ——
 * 少一层依赖就少一个将来漏的机会。
 */
const ALLOWED_ENV_KEYS = [
  'SystemRoot',
  'windir',
  'PATH',
  'PATHEXT',
  'TEMP',
  'TMP',
  'NUMBER_OF_PROCESSORS',
  'OS',
  'PROCESSOR_ARCHITECTURE',
]

function minimalEnv(source) {
  const out = {}
  for (const key of ALLOWED_ENV_KEYS) {
    if (typeof source[key] === 'string') out[key] = source[key]
  }
  return out
}

/**
 * **起状态口进程**时要给的环境（`up` 用）。
 *
 * 为什么要一份单独的：状态口是**只读**的，它自己的环境里也没有任何理由带家的凭据。
 * 给它的就是那条白名单（操作系统必需品）＋ `CELIRA_HOME_DIR`（它得知道看哪个家）。
 * ⇒ **`DEEPSEEK_API_KEY` / `CELIRA_IM_BACKEND_AUTH` / Human token 一个字节都不进去**。
 *
 * 需要的三样能力都在这条白名单覆盖的范围内：
 *   - 探 im 健康 = Node 内置 `http.get`（**不依赖 curl**）；
 *   - 查进程身份 = PowerShell（在 `WindowsPowerShell\v1.0`，随 PATH 进）；
 *   - 看端口 = `netstat`（在 System32，随 PATH 进）。
 */
function serveEnv(source, homeDir) {
  return { ...minimalEnv(source), CELIRA_HOME_DIR: homeDir }
}

/**
 * 跑一次 `agent_host_status`，把它的两股输出收全（**异步**）。
 *
 * ⚠ **为什么必须异步**（2026-09-24 收口刀）：这条链以前用 `spawnSync`，而 TTL 过期后那次
 * "后台刷新"走的就是它 ⇒ **它把 Node 事件循环按住**整个探测时长（临时家实测一次探测
 * 同步阻塞 **389ms**；源码自述真家 ~3.7 秒）。落在那个窗口里的**任何**请求都只能干等 ——
 * 实测：独立客户端在窗口内发的第二、三、四个请求分别等了 317 / 226 / 134ms，
 * 正好等于窗口剩下的时间。**一个只读口的并发等待时间不许等于别人的探测时长。**
 *
 * 读数语义**一字未动**：还是"忠实转发 `agent_host_status` 的输出"，只是不再按住循环。
 * 退出码照旧不参与判读（与同步版一致，判读全靠文本）——收下来只为让这条缝的形状与
 * `spawnSync` 的返回值对齐。
 */
function runStatusProcess({ statusExe, envDir, agentId, spawn: spawnFn = spawn }) {
  return new Promise((resolve) => {
    let child
    try {
      child = spawnFn(statusExe, [envDir, '--agent', agentId], {
        windowsHide: true,
        env: minimalEnv(process.env),
      })
    } catch (error) {
      // 起不来（参数形状不对）：与"读不出来"同一档，不抛给调用方（同步版也是这个结论）。
      resolve({ stdout: '', stderr: '', code: null, error })
      return
    }
    let stdout = ''
    let stderr = ''
    let settled = false
    const finish = (result) => {
      if (settled) return
      settled = true
      resolve(result)
    }
    child.stdout?.setEncoding('utf8')
    child.stderr?.setEncoding('utf8')
    child.stdout?.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr?.on('data', (chunk) => {
      stderr += chunk
    })
    // 起不来（'error'）与跑完了（'close'）各自是一次结论；谁先到算谁，只认一次。
    child.on('error', (error) => finish({ stdout, stderr, code: null, error }))
    child.on('close', (code) => finish({ stdout, stderr, code }))
  })
}

/** 读 Agent 的账本：忠实转发 `agent_host_status` 的结果，包括"Agent 正忙"那一态。 */
async function readAgentStatus({ statusExe, envDir, agentId, spawn: spawnFn = spawn }) {
  const done = await runStatusProcess({ statusExe, envDir, agentId, spawn: spawnFn })
  const text = `${done.stdout ?? ''}${done.stderr ?? ''}`
  const parsed = parseHostStatus(text)
  if (parsed.ok === 'busy') {
    return { state: 'busy', note: 'Agent 这一拍正在跑一次活动（环境同时只有一个写者）' }
  }
  if (parsed.ok !== true) {
    return { state: 'unreadable', reason: parsed.reason }
  }
  const value = parsed.value
  // 明细只给最近几条：这个口的职责是"看一眼 Agent 的处境"，不是导出账本。
  //
  // ⚠ **2026-09-29**：`agent_host_status` 从这天起把本页取自账本的**最近**那一头，
  //   所以 `slice(-N)` 取到的确实是最近 N 条。在此之前那一页是**最早**的一页 ——
  //   同一个 slice 取到的是最早那几条，而界面写着「最近」。
  const runs = value.runs.slice(-RECENT_RUNS)
  return {
    state: 'loaded',
    admission: value.admission,
    // 「活动次数」= **总数**。此前这里放的是**本页条数**（读数的名字叫 `activities`，
    // 读者自然当成总数）：账本一旦超过一页，界面上的活动次数就冻在 32 上不动
    // （真机实测 36 条报 32），而明细还永远停在最早那一页。
    activities: value.activities,
    // 明细被截断了没有 —— 总数比实际给出的条数多就是截断了。
    // 界面那句「总数照给，明细只给最近几条」靠它；从前它跟着一个会读错的布尔字段走。
    truncated: value.activities !== null && value.activities > runs.length,
    runs,
    // 一共几条活动（与 `activities` 同一个数；保留是因为它在载荷契约里）。
    runsTotal: value.activities ?? runs.length,
    next: value.next,
  }
}

/** 组装 `GET /status` 的载荷。**只读读数，不含凭据。** */
async function buildStatusPayload(ctx) {
  // **形状 fail-closed**：`residents` 只收**函数**（每次现问）。传数组进来会在下面被静默当成
  // "没给"（⇒ 合成一支旧形状的住户）—— 而"拿一份**会过期的快照**当住户表"正是 2026-10-12
  // 治的那个病（门把开口那一刻的世界当成现在）⇒ 当场说清，不悄悄读错。
  if (ctx.residents !== undefined && typeof ctx.residents !== 'function') {
    throw new Error('ctx.residents 必须是函数（每次现问的住户表）；数组是一种会过期的快照')
  }
  const { layout, homeDir } = ctx
  // **这一格只有一个事实**：探活问到的那个状态码（`null` ＝ 它没答话）。
  //
  // ⚠ 从前这里还有一个**派生**的 `listening`（`code === 200`），而页面拿它当主词 ⇒
  // 「世界活着、只是它自己报 500」被写成「没在跑（HTTP 500）」——**自相矛盾的字面**。
  // 判据（`client/src/api/homeStatus.ts` 那一族）：**没有 / 读不到 / 没在跑 必须分开**：
  // **有应答（任何码）⇒ 那个口上有人在 ⇒ 它在跑**；`null` ⇒ **它没答话**（读不到，
  // 不等于它停了 —— 与 `proc.portInUse` 那句"证明不了它空着，不许说它空着"同一把尺子）。
  // ⇒ 派生字段删掉：一个事实一个权威（2026-10-11）。
  let im = { health: null }
  try {
    // `proc.httpCode` 给的是**字符串**（它内部 `String(statusCode ?? '000')`）——必须显式转数字再比，
    // 用 `=== 200` 会永远 false（实测踩过：`listening:false` 配 `health:"200"` 自相矛盾）。
    const code = Number.parseInt(String(await ctx.proc.httpCode(`http://${ctx.imListen}/health`)), 10)
    // 连不上时 `curl` 给的是 `000` ⇒ 转出来是 **0**。0 不是 HTTP 状态码，原样透出去
    // 会让页面写「没在跑（HTTP 0）」——把"读不到"说成了一个具体读数（实测踩过）。
    im = { health: Number.isInteger(code) && code > 0 ? code : null }
  } catch {
    im = { health: null }
  }

  // —— **住户表**：一个家可以住 N 支（2026-10-01 多支刀）——
  //
  // `ctx.residents()` 给得出住户 ⇒ 逐支一份读数（`residents[]`）；旧调用点只给了 `agentId`
  // ⇒ 合成一支**旧形状**的住户（环境根 `<家>/env`、账本 `logs/host.pid` / `logs/host.out`）
  // —— 与从前一字不差。
  //
  // ★ **2026-10-12：住户表每次现问**（`ctx.residents` 从前是一个**数组**，开口那一刻就冻住了）
  //   —— 门起来之后再安的那一支会**读成不存在**、搬走的那一支**读成还在**，两者都是把边界
  //   说成事实。权威是磁盘上那份住户表（`residents.runtimeResidents`），门只是通道 ——
  //   与 `config-server.js` 那一处同一条纪律。
  //   ⚠ **`() => []` 是一个真答案**（这个家还没有住户 ⇒ `no-resident`），不是"没给"。
  const list =
    typeof ctx.residents === 'function'
      ? ctx.residents()
      : [
          {
            agentId: ctx.agentId ?? null,
            envDir: P.legacyEnvRoot(homeDir),
            slot: 1,
            roomId: '',
            legacy: true,
            mismatch: null,
          },
        ]
  // **逐支并行**（2026-10-01 第六刀）：每支视图都要 spawn 一次 `agent_host_status`（真家一次
  // ~3.7 秒）⇒ 串行会让"页面一次刷新"的等待随住户数**相加**（三支 ≈ 三次探测）。并行之后
  // 总等待 ≈ **最慢那一支**。顺序照旧（`Promise.all` 保序）：读数还是按住户表给。
  //
  // 这条与同一份文件里既有的那条纪律是同一句：**"一个只读口的并发等待时间不许等于别人的探测
  // 时长"**（2026-09-24 收口刀把 `spawnSync` 换成异步 `spawn` 时写下的）。多支只是它的第二个面。
  const views = await Promise.all(list.map((resident) => buildResidentView({ ctx, resident })))
  // **第一支**（住户 1）的读数同时抬到顶层：冻结的前端照旧读 `agent` / `lastWake` / `experience`
  // （"多一个字段不管"是它的形状守卫写明的）——本刀**只加不改**，页面一个字节都不用动。
  const primary = views[0] ?? {
    // **这个家里一支住户都没有**（2026-10-01 多支刀之后才可能出现）——它与"二进制不在"
    // 是**两件事**，2026-10-05 起分开说：从前两者共用 `missing-binary`，于是刚建好世界的新用户
    // 在「运行」那一格读到「本机缺少必要的程序」（真机撞到）—— 那是一句**假警报**：
    // 机器好好的，只是还没有 Agent 住进来。判据：**缺省不等于故障**。
    agent: { state: 'no-resident' },
    lastWake: null,
    experience: { state: 'missing' },
  }

  return {
    readAt: new Date().toISOString(),
    im,
    agent: primary.agent,
    lastWake: primary.lastWake,
    // ★ **宿主那一格也抬到顶层**（2026-10-08）：与 `agent` / `lastWake` 同一条规矩（第一支抬顶层，
    //   冻结前端照旧读）。⚠ 壳那一边的门是**另一个生产者**，两边的 `host` 必须**同形**
    //   （同一刀在 `app/src-tauri/src/home_doors.rs` 的 `host_view` 里补上）。
    //   一支住户都没有时**整格不给**（缺省 = 这一面没读数，**不编一个"没在跑"**）。
    ...(primary.host ? { host: primary.host } : {}),
    // Agent 的经历（机械投影；TASK_72 ④）。**只给机械字段**：动作 / 轮次 / 状态 / 条数，
    // 没有正文、没有参数值（白名单在 `projectExperience` 里）。
    experience: primary.experience,
    // **每支一份**（本刀新增）：身份 / 住户序号 / 房 / 宿主 / 模型绑定 / 处境 / 经历。
    residents: views,
  }
}

/**
 * **一支住户**在状态口载荷里的那一份。
 *
 * 与 CLI 的 `node home.js status` 同一批事实（身份 / 房 / 宿主 / 模型绑定 / 处境 / 经历），
 * 少**凭据**那一格 —— 理由有两条，都不是口味：
 *   · 端面纪律：这个口**不含凭据**（"口上无秘密"的物理论证）；
 *   · 连"有没有"也不报：凭据目录按平台算（Windows 要 `LOCALAPPDATA`、还认
 *     `CELIRA_CREDENTIALS_DIR`），而**这个进程的环境是白名单**（那两条都不在里面）⇒
 *     算出来的是**另一个目录**，报一个算错的"没有"比不报更坏。
 *     "凭据在不在"这一格在 CLI（`node home.js status`）里，那里算得准。
 */
async function buildResidentView({ ctx, resident }) {
  const { layout, homeDir } = ctx
  const envDir = resident.envDir
  let agent = { state: 'missing-binary' }
  if (P.isFile(layout.statusExe)) {
    agent = await readAgentStatus({
      statusExe: layout.statusExe,
      envDir,
      agentId: resident.agentId,
      // 注入点：生产给默认的**异步** `spawn`；用例给一个计数替身，把"这次请求到底 spawn 了几次"
      // 变成可判据。⚠ 替身必须是 `spawn` 的形状（交回一个带 stdout/stderr 流与 close 事件的孩子）
      // —— 本刀把探测从 `spawnSync` 换成 `spawn` 之后，同步形状的替身喂进来只会得到假读数。
      ...(ctx.spawn ? { spawn: ctx.spawn } : {}),
    })
  }

  // 刀2b C①：**Agent 的身份**（环境根 identity.json 的记录）——刻意放在这一层、与 Agent 的状态**无关**：
  // 「Agent 这一轮在忙」「二进制缺」「账本读不出来」都不该让页面答不出"这是哪个 Agent 的账本"。
  const envAgentId = readEnvIdentity(envDir)
  if (envAgentId) agent = { ...agent, agentId: envAgentId }

  // 宿主那一格：**按支**判（一支一个进程，各自的账本 ＋ 各自的环境根）。
  // 没有 `proc` 那两把（用例的替身常常只给 httpCode）⇒ 如实给 `unknown`，**不猜**。
  const hostLog = residents.residentHostLog(homeDir, resident)
  const hostLogLabel = path.relative(homeDir, hostLog) || hostLog
  let host = { state: 'unknown', pid: null, log: hostLogLabel }
  if (typeof ctx.proc?.trustedLedgerPid === 'function' && typeof ctx.proc?.hostOrphanPids === 'function') {
    try {
      const scoped = { homeDir, envDirs: [envDir] }
      const pid = ctx.proc.trustedLedgerPid('agent', residents.residentPidFile(homeDir, resident), layout, scoped)
      if (pid !== null) host = { state: 'running', pid, log: hostLogLabel }
      else {
        const orphan = ctx.proc.hostOrphanPids(layout, scoped)[0] ?? null
        host = orphan !== null ? { state: 'running-orphan', pid: orphan, log: hostLogLabel } : { state: 'stopped', pid: null, log: hostLogLabel }
      }
    } catch {
      host = { state: 'unknown', pid: null, log: hostLogLabel }
    }
  }

  // 模型绑定那一格：绑定文件里只有**清单条目的 id**（`{source: "ms-1"}`），没有钥匙 ⇒ 可以报。
  const binding = modelSources.readBinding(envDir)

  // **这一支的手**（2026-10-01 第五刀）：外部 server 与终端档，**按支**给（来源也一起给，
  // 页面才知道"这台 server 是跟家走的还是这一支自己的"）。只报**名字与数量**：命令不进载荷。
  const hands = residents.residentHands({ homeDir, resident, homeConfFile: ctx.confFile ?? path.join(homeDir, 'home.conf') })
  const terminal = residents.resolveTerminal(hands, { isWindows: proc.IS_WINDOWS, isLinux: proc.IS_LINUX })
  const handsView = {
    mcp: {
      count: hands.mcp.length,
      names: hands.mcp.map((declaration) => declaration.split('=', 1)[0]),
      source: hands.mcpSource,
    },
    terminal: { mode: terminal.mode, source: terminal.source, unavailable: terminal.unavailable },
  }

  // Agent 最近一段在场（宿主日志的最后一条 `[run]` 读数）：`busy` 时这是唯一的即时读数。
  //
  // ⚠ **键名 `lastWake` 是**冻结的**（前端在读它，见 `client/src/api/agentRowStatus.ts`），
  // 所以它一个字都不改**；改的只是它取的那一行——2026-10-10 在场那一刀把 `[wake]` 那一块
  // 改成了 `[run]`（一段在场会吸收后来的每一声音铃，一次唤醒不再是"一段"），
  // 内容行也从 `wake run=…` 变成 `run=…`。
  // 前端解时间用的是 `run=(\S+)`，那一格两版都在，因此它照旧读得出来。
  let lastWake = null
  try {
    if (P.isFile(hostLog)) {
      const lines = fs.readFileSync(hostLog, 'utf8').split(/\r?\n/)
      lastWake = lines.filter((line) => line.startsWith('run=')).pop() ?? null
    }
  } catch {
    lastWake = null
  }

  return {
    agentId: resident.agentId ?? null,
    slot: resident.slot ?? null,
    roomId: resident.roomId || null,
    // 还没住户化的老家那一支（环境根 `<家>/env`）：如实标出来，不混进新布局的读数里。
    legacy: Boolean(resident.legacy),
    host,
    modelBinding: { state: binding.state, source: binding.source ?? null },
    hands: handsView,
    agent,
    lastWake,
    experience: readAgentExperience({ traceFile: path.join(envDir, 'trace.ndjson') }),
  }
}

/**
 * 起状态口。返回 `{ server, port }`（调用方负责 log 与生命周期）。
 *
 * 路由是**穷举**的：`/status`、`/healthz`，其余一律 404；非 GET 一律 405。
 * 穷举而不是"加个前缀随便走"，是为了让"这个口能干什么"在一屏之内看完。
 *
 * **没有 CORS 面**（TASK_61 删）：页面不再直连这个端口，而是问**同源**的 `/home-status/*`
 * 由代理代转（发布形态 `serve_client.mjs` / 源码形态 vite preview），同源就不存在跨源。
 * 那一层响应头白名单是"端口不同"时代的产物——少一个面比多一份配置好。
 * 口子**仍只绑 `127.0.0.1`**，可达面没有变大（这一点由绑定决定，不由响应头决定）。
 *
 * **短 TTL 缓存**（TASK_72 ③，见 [`STATUS_TTL_MS`]）：
 * - 同一时刻来的多个请求共享同一次探测（并发合并，既有行为）；
 * - `ttlMs` 内的请求直接用上次那一份（**`readAt` 原样带着**，响应里 `stale: false`）；
 * - 过期后的那一次请求**立刻**给出旧的那份（`stale: true`）**并在后台刷新** ——
 *   既不让页面再等 3.7 秒，也不把"这是缓存"说成"这是当下"。
 *
 * @param {object} ctx 生产由 `serve()` 装配；用例可以注入 `spawn`（把"这次请求 spawn 了几次"变成判据）。
 * @param {{ttlMs?: number, now?: () => number, access?: {mode: 'local'} | {mode: 'remote', homeDir: string}, tls?: {cert: Buffer, key: Buffer} | null}} [options]
 *   用例注入：时钟与 TTL；**准入**（缺省 `{mode:'local'}` = 与从前一字不差：无钥匙、
 *   无 CORS、无预检）；`tls` 给了就用 `https` 起监听（模式二由 `serve()` 装配）。
 */
function createStatusServer(ctx, options = {}) {
  const ttlMs = Number.isFinite(options.ttlMs) ? options.ttlMs : STATUS_TTL_MS
  const now = options.now ?? (() => Date.now())
  // **准入**：缺省模式一（和从前一字不差）。模式二由 `serve()` 按绑定地址装配。
  const access = options.access ?? { mode: 'local' }
  const tls = options.tls ?? null

  // **并发合并**：一次探测要 3~4 秒（spawn `agent_host_status` 读账本 + 探 im 健康），
  // 同一时刻来的多个请求共享同一次 —— 否则页面刷新一下就是几倍的重复探测。
  // ⚠ 探测本身必须是**异步**的（见 `runStatusProcess`）：合并只省掉重复，不解决阻塞。
  let inFlight = null
  const probe = () => {
    if (inFlight === null) {
      inFlight = buildStatusPayload(ctx).finally(() => {
        inFlight = null
      })
    }
    return inFlight
  }

  /** 上一次**真读数**：`payload.readAt` 是它被取回的时刻，**永不改写**。 */
  let cached = null

  /**
   * 取一份读数：**只有冷启动**才 await 探测；TTL 内直接给缓存；过期则给缓存的**同时**后台刷新。
   *
   * `stale` 只在这里加（缓存里那份不带它）：它是"这一份是不是过了窗口"的判据，与读数本身无关。
   */
  const payloadOnce = async () => {
    if (cached === null) {
      const payload = await probe()
      cached = { payload, atMs: now() }
      return { ...payload, stale: false }
    }
    const age = Math.max(0, now() - cached.atMs)
    if (age <= ttlMs) {
      return { ...cached.payload, stale: false }
    }
    // 过期：先把旧的那份给 Agent（并标注），刷新在后台跑，不挡这一次请求。
    void probe()
      .then((payload) => {
        cached = { payload, atMs: now() }
      })
      .catch(() => {})
    return { ...cached.payload, stale: true }
  }

  // **开机预热**（`serve()` 调它）：让页面第一次问时已经有一份暖读数可给。
  const warmStatus = () => {
    void payloadOnce().catch(() => {})
  }

  const handler = (req, res) => {
    // 家口远程访问刀：模式二下这三个动作按序发生（预检在认证**之前**——预检请求按规范
    // 不带 Authorization；模式一下三行全是空操作，行为与从前一字不差）。
    homeKey.attachCors(res, access)
    if (homeKey.preflight(req, res, access, ['GET', 'HEAD'])) return
    if (!homeKey.authorize(req, res, access)) return
    // **模式一的来源守卫**（2026-09-25 补；与配置门同一道、同一份实现）。
    //
    // 为什么只读口也要它：这一口的读数里有**是谁、Agent 的准入、Agent 的经历投影**（无正文，
    // 但是 Agent 的私有世界的一角）。"只绑 loopback"挡不住 DNS rebinding —— 那种页面在浏览器
    // 眼里是**同源**的，`Host` 才是判据（这正是 `home-key.js` 文件头把"按 Host/Origin
    // 认来源"写成两个口共同纪律的原因；从前只有配置门真做了这道）。
    //
    // 模式二**不调用它**：那一边的闸是家钥匙 + TLS，而浏览器必然跨源（见 home-key.js 纪律 6）。
    // 本机两种形态的页面都走同源代转，代转把 `Host` 改写成 loopback（`serve_client.mjs`
    // 显式设 `changeOrigin`）⇒ 正常路径一个字节都不受影响。
    if (access.mode !== 'remote' && !homeKey.localOriginAllowed(req)) {
      res.writeHead(403, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' })
      res.end(JSON.stringify({ error: ORIGIN_NOT_ALLOWED }))
      return
    }
    const url = (req.url ?? '').split('?')[0]
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.writeHead(405, { 'content-type': 'application/json; charset=utf-8', allow: 'GET' })
      res.end(JSON.stringify({ error: 'method_not_allowed' }))
      return
    }
    if (url === '/healthz') {
      // `/healthz` 不碰账本：它只回答"口子活着吗"，所以是即时的。
      res.writeHead(200, { 'content-type': 'application/json; charset=utf-8' })
      res.end(JSON.stringify({ ok: true }))
      return
    }
    if (url !== '/status') {
      res.writeHead(404, { 'content-type': 'application/json; charset=utf-8' })
      res.end(JSON.stringify({ error: 'not_found' }))
      return
    }
    void payloadOnce()
      .then((payload) => {
        res.writeHead(200, {
          'content-type': 'application/json; charset=utf-8',
          'cache-control': 'no-store',
        })
        res.end(JSON.stringify(payload))
      })
      .catch(() => {
        res.writeHead(500, { 'content-type': 'application/json; charset=utf-8' })
        res.end(JSON.stringify({ error: 'status_unavailable' }))
      })
  }
  // 传输层按准入走：模式二给了 `tls` ⇒ HTTPS（裁决：非 TLS 不开远程）；模式一照旧 HTTP。
  const server = tls ? https.createServer(tls, handler) : http.createServer(handler)
  // 开机预热用的钩子（挂在 server 上而不是改返回形状：调用方一处分都不必动）。
  server.warmStatus = warmStatus
  return server
}

module.exports = {
  createStatusServer,
  buildStatusPayload,
  buildResidentView,
  readAgentStatus,
  readAgentExperience,
  projectExperience,
  minimalEnv,
  serveEnv,
  RECENT_RUNS,
  STATUS_TTL_MS,
  EXPERIENCE_ROWS,
  EXPERIENCE_FIELDS,
  ORIGIN_NOT_ALLOWED,
  // 刀2b：起源映射与身份读取都单独导出（用例直接喂三句原文断言，不必起服务）。
  ORIGIN_SENTENCES,
  ORIGIN_LABEL,
  projectOrigin,
  readEnvIdentity,
  ALLOWED_ENV_KEYS,
  DEFAULT_STATUS_PORT,
}
