'use strict'
/**
 * ★ **远程接上一支 Agent**（2026-10-15 主人裁的设计；命令行形态那一侧的唯一实现）。
 *
 * ## 一句原理
 *
 * **接上这件事由那台机器自己裁决、自己动手；外面的调用方只拿到一份读数。**
 * 不新增公共事实、不经过世界、不给"远程 HTTP"任何新权限面 —— 它只是那台机器**已经有的那只手**
 * （`node home.js up --agent <id>`）多开一个**默认关着**的入口。
 *
 * ## 四条边界（主人逐条裁过；改任何一条都是产品边界变更）
 *
 * 1. **钥匙粒度**：家钥匙回答"**能不能进这台机器**"（`home-key.js`：按机器发、落盘只有摘要、
 *    每请求校验、换一把即刻全失效）。而"**允许我替你起进程**"是**另一件事** ⇒ 同一个开关管它：
 *    `home.conf` 里的 `remote_control=on`，**只能在本地打开**（`node home.js remote-control on`）。
 *    准入 ≠ 授权 —— 拿到钥匙的人不该能自己授权自己。
 * 2. **TLS / 来源**：复用模式二既有那道闸（非 loopback ⇒ 必须钥匙 ＋ TLS，`home-key.js` 第 4 条
 *    fail-closed）；来源守卫仍只属模式一，两道闸**不叠加**。这一层不新增任何判据。
 * 3. **谁付钱**：这一步**永不选模型**（"选哪一条 = 花谁的钱，不替它决定"）：
 *    有绑定 ⇒ 起（文件档）；没绑定但部署者配了进程侧钥匙 ⇒ 起（环境档 —— 命令行形态的既有来路）；
 *    都没有 ⇒ **接上，但先不起它**（`homed-not-started`）。选模型在页面上做（那本来就能远程写）。
 * 4. **谁判起不起来**：机器是执行端，起不起是**它的**私有终态 ⇒ 这里只回**读数**，
 *    一个字的承诺都不给（`homed` / `homed-not-started` / `already-homed` / `failed`）。
 *
 * ## 两条不许松的纪律
 *
 * - **不写第二只手**：这一层不实现"建环境根 / 签凭据 / 起宿主"，它**跑那条既有的操作路径**
 *   （`node home.js up --agent <id> --attach-only`）—— 操作者在终端里敲的是同一条。
 * - **结局从账本读出来，不从人话文本猜**：跑完之后看那台机器**自己的事实**
 *   （住户登记在不在、宿主在不在跑 —— 与状态门 `buildResidentView` 同一把尺子：
 *   `residents.js` ＋ `proc.trustedLedgerPid` / `proc.hostOrphanPids`），
 *   再把 stdout 原样作为 `lines` 交出去。
 *
 * ## 留痕（必有的那一条）
 *
 * 每次远程接上都在那台机器上写一行（谁、什么时候、从哪儿、接了哪一支、结局）——
 * `logs/remote-control.log`。**家级日志，不是世界的事实**；**钥匙一个字节都不进它**。
 */

const fs = require('node:fs')
const path = require('node:path')
const { spawn } = require('node:child_process')

const conf = require('./conf')
const P = require('./paths')
const proc = require('./proc')
const residents = require('./residents')

/** `home.conf` 里那个开关的键名。 */
const SWITCH_KEY = 'remote_control'
/** 只认这一个值当"开着"（别的、以及缺省，一律当"没开" —— fail-closed）。 */
const SWITCH_ON = 'on'
/** 一次接上的预算（起宿主那一步本身就有报到窗口，这里给足）。 */
const ATTACH_TIMEOUT_MS = 180000
/** 交回去的原样读数最多几行（够看清，又不至于把整篇日志搬过网）。 */
const LINES_LIMIT = 60
/** 留痕文件（家级日志）。 */
const AUDIT_FILE = 'remote-control.log'

/**
 * **这台机器允不允许从外面接 Agent**（缺省 **不许**）。
 *
 * 读的是 `home.conf`（与家钥匙摘要分开：那个在 `home-key.js` 管的文件里）。
 * 每请求现读 ⇒ 改开关**立即生效**，不需要重启门（与钥匙"每请求校验、无会话缓存"同一条纪律）。
 */
function remoteControlEnabled(homeDir) {
  const file = path.join(homeDir, 'home.conf')
  return conf.confGet(file, SWITCH_KEY, '') === SWITCH_ON
}

/**
 * 开 / 关这个开关（**只能本地调**：CLI 命令，或那台机器上自己的页面）。
 *
 * 返回改完之后的状态；写失败往上抛（调用方如实说）。
 */
function setRemoteControl(homeDir, on) {
  const file = path.join(homeDir, 'home.conf')
  conf.confSet(file, SWITCH_KEY, on ? SWITCH_ON : 'off')
  return remoteControlEnabled(homeDir)
}

/**
 * 结局词表（**与壳那份逐字相同**：`app/src-tauri/src/anjia/mod.rs` 的 `AnjiaOutcome::kind()`）。
 *
 * 为什么必须同名：页面按它分流 —— 串了就是"把拒绝说成成功"那一类错。用例读两侧源文件对表。
 */
const ATTACH_OUTCOMES = ['homed', 'homed-not-started', 'already-homed', 'failed']

/**
 * 这条路上会出现的拒绝码（**进门的封闭表**：`config-server.js` 的 `ERRORS`；客户端那份逐字对账）。
 *
 * ⚠ **不新造近义词**：`bad_agent_id` 就是门那一侧既有的那个码（同一个判据，同一句话）。
 */
const ATTACH_ERRORS = {
  REMOTE_CONTROL_OFF: 'remote_control_off',
  BAD_AGENT_ID: 'bad_agent_id',
  ATTACH_FAILED: 'attach_failed',
}

/** 那一支现在的处境（**只读那台机器自己的事实**）。 */
function factsOf(homeDir, agentId, deps = {}) {
  const layout = (deps.detectLayout ?? P.detectLayout)()
  const scoped = { homeDir, envDirs: [P.agentEnvRoot(homeDir, agentId)] }
  const list = (deps.residents ?? residents).residents(homeDir)
  const resident = list.find((each) => each.agentId === agentId) ?? null
  const envRoot = P.agentEnvRoot(homeDir, agentId)
  const homed = resident !== null && fs.existsSync(path.join(envRoot, P.AGENT_CONF))
  let running = false
  let pid = null
  if (resident !== null) {
    const ledger = (deps.proc ?? proc).trustedLedgerPid(
      'agent',
      residents.residentPidFile(homeDir, resident),
      layout,
      scoped,
    )
    if (ledger !== null) {
      running = true
      pid = ledger
    } else {
      const orphan = (deps.proc ?? proc).hostOrphanPids(layout, scoped)[0] ?? null
      if (orphan !== null) {
        running = true
        pid = orphan
      }
    }
  }
  const binding = resident === null ? null : require('./model-sources').readBinding(envRoot)
  return { homed, running, pid, envRoot, resident, binding }
}

/** 那只手要跑的那条命令（**既有的操作路径**：操作者在终端里敲的是同一条）。 */
function upArgsFor(agentId) {
  return ['up', '--agent', agentId, '--attach-only']
}

/** 跑那条**既有的**操作路径（唯一那只手）。可注入（用例给替身，不真起进程）。 */
function runUp({ homeDir, agentId, timeoutMs }) {
  return new Promise((resolve) => {
    const homeJs = path.resolve(__dirname, '..', '..', 'home.js')
    const child = spawn(process.execPath, [homeJs, ...upArgsFor(agentId)], {
      cwd: homeDir,
      windowsHide: true,
      env: process.env,
    })
    let stdout = ''
    let stderr = ''
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      child.kill()
    }, timeoutMs)
    child.stdout.on('data', (chunk) => {
      stdout += String(chunk)
    })
    child.stderr.on('data', (chunk) => {
      stderr += String(chunk)
    })
    child.on('error', (error) => {
      clearTimeout(timer)
      resolve({ code: null, stdout, stderr: `${stderr}\n${error.message}`, timedOut })
    })
    child.on('close', (code) => {
      clearTimeout(timer)
      resolve({ code, stdout, stderr, timedOut })
    })
  })
}

/** 留痕：一行，追加写；**不含钥匙**（也不含任何凭据）。 */
function audit(homeDir, { at, from, agentId, outcome, detail }) {
  const file = path.join(homeDir, 'logs', AUDIT_FILE)
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true })
    const line = JSON.stringify({
      at,
      from: from ?? null,
      agentId,
      outcome,
      ...(detail ? { detail } : {}),
    })
    fs.appendFileSync(file, `${line}\n`, 'utf8')
    return file
  } catch {
    // 留痕写不进去**不许**把已经做成的接上说成失败（那是撒谎），但要在读数里说清。
    return null
  }
}

/** 交回去的原样读数（尾巴那几十行）。 */
function tailLines(stdout, stderr) {
  const all = `${stdout}\n${stderr}`
    .split('\n')
    .map((line) => line.trimEnd())
    .filter((line) => line.trim() !== '')
  return all.slice(-LINES_LIMIT)
}

/**
 * **接上一支**（远程那条路的全部逻辑；门与 CLI 共用）。
 *
 * 返回两种形状，**一种都不许假装成功**：
 * - `{ ok: true, outcome, agentId, envRoot, pid, lines, auditFile }` —— 接上了（起没起看 `outcome`）；
 * - `{ ok: false, error, lines }` —— 拒绝 / 失败（`error` 是封闭表里的码）。
 *
 * `from` 是**调用方**的来路（远程那条路给来源地址；CLI 给 `null`），只进留痕，不参与任何判断。
 */
async function attachRemote({
  homeDir,
  agentId,
  from = null,
  timeoutMs = ATTACH_TIMEOUT_MS,
  runner = runUp,
  clock = () => new Date().toISOString(),
  deps = {},
}) {
  if (!remoteControlEnabled(homeDir)) {
    return { ok: false, error: ATTACH_ERRORS.REMOTE_CONTROL_OFF, lines: [] }
  }
  const before = factsOf(homeDir, agentId, deps)
  const run = await runner({ homeDir, agentId, timeoutMs })
  const after = factsOf(homeDir, agentId, deps)
  const lines = tailLines(run.stdout, run.stderr)
  if (run.timedOut) {
    lines.push(`[远程] 那台机器在 ${Math.round(timeoutMs / 1000)} 秒内没跑完这一步 —— 停手（那边的账本可能已经在动，去它自己的读数里看）`)
  }
  let outcome
  if (!after.homed) {
    outcome = 'failed'
  } else if (after.running) {
    outcome = before.running ? 'already-homed' : 'homed'
  } else {
    outcome = 'homed-not-started'
  }
  const auditFile = audit(homeDir, {
    at: clock(),
    from,
    agentId,
    outcome,
    detail: run.code === 0 ? null : `up 退出码 ${run.code}`,
  })
  if (auditFile === null) {
    lines.push('[远程] ⚠ 这一次没留上痕（logs 写不进去）—— 接上这件事本身照旧算数，但要知道它没被记下来')
  }
  if (outcome === 'failed') {
    return { ok: false, error: ATTACH_ERRORS.ATTACH_FAILED, outcome, lines }
  }
  return {
    ok: true,
    outcome,
    agentId,
    envRoot: after.envRoot,
    pid: after.pid,
    lines,
    auditFile,
  }
}

module.exports = {
  SWITCH_KEY,
  SWITCH_ON,
  AUDIT_FILE,
  ATTACH_TIMEOUT_MS,
  ATTACH_OUTCOMES,
  ATTACH_ERRORS,
  remoteControlEnabled,
  setRemoteControl,
  upArgsFor,
  factsOf,
  attachRemote,
}
