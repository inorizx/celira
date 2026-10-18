'use strict'
/**
 * 进程与网络原语 —— 家的开/关/看全靠这几个函数，**一份实现**（三个 .sh 里各抄了一份）。
 *
 * 几条从真实事故里长出来的纪律，迁移时逐条带过来：
 *
 * 1. **两个 pid 世界**：bash 版里 `$!` 记下来的是 **MSYS pid**（`kill` 语义），而进程表
 *    查出来的是 **Windows pid**（`taskkill /F /PID` 语义），两者的数字会撞——混用就是误杀。
 *    Node 里只有一个世界：`spawn` 给出的 `child.pid` **就是** Windows pid（POSIX 上就是普通
 *    pid），pid 文件与进程表用同一套数。
 * 2. **pid 文件会说谎**：宿主是出站连接、**不占端口**，端口扫永远碰不到它；托管/重起过的
 *    宿主会脱离 pid 文件（报停实际在跑，还占住 `agent_host.exe` 让 cargo 删不掉）。所以
 *    「Agent 在不在」除了 pid 文件，还要按**可执行路径**对一遍进程表（路径判据不是名字判据：
 *    D 盘还有别的副本，同名 exe 不许牵连）。⚠ 2026-09-24 再补一维：同一份发布包可以服务
 *    **多个家**，光有路径认不出彼此 ⇒ 判据 =「种类 ＋ **家事实**」（见本文件「家事实」段）。
 * 3. **杀之前先验身份**：pid 只是个数。落在一个不是我们的进程上（陈旧账本、数字撞车）时，
 *    杀它就是误杀无关进程。所以 `describeProcess(pid)` + `ownsPid(...)` 是**杀的前置**。
 */

const fs = require('node:fs')
const http = require('node:http')
const net = require('node:net')
const { spawn, spawnSync } = require('node:child_process')

const IS_WINDOWS = process.platform === 'win32'
const IS_LINUX = process.platform === 'linux'

/**
 * Linux 的进程事实来源：`/proc`。本文件在 POSIX 上的两条读法都从这里取 ——
 * **exe 的真实路径**（`/proc/<pid>/exe`）与**精确 argv**（`/proc/<pid>/cmdline`）。
 * 为什么不按名字（`ps` 的 `comm`）读：Linux 的 `comm` 是**线程名**且被内核截断到 15 字符
 * （实测 node 26 是 `node-MainThread`、`agent_host_status` 变成 `agent_host_stat`）——
 * 按名字核验两头都会错，而判据的红线是"**路径判据不是名字判据**"。
 */
const PROC_ROOT = '/proc'

// ---------------------------------------------------------------------------
// pid 文件
// ---------------------------------------------------------------------------

function readPid(file) {
  let raw
  try {
    raw = fs.readFileSync(file, 'utf8')
  } catch {
    return null
  }
  const value = Number.parseInt(raw.trim(), 10)
  return Number.isInteger(value) && value > 0 ? value : null
}

function writePid(file, pid) {
  fs.writeFileSync(file, `${pid}\n`, 'utf8')
}

function clearPid(file) {
  try {
    fs.rmSync(file, { force: true })
  } catch {
    // 删不掉也别拦着关机
  }
}

/** 这个 pid 还活着吗（`kill -0` 等价物）。 */
function isAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    // EPERM = 进程在、只是不归我们管（也算活着）；ESRCH = 没了。
    return error && error.code === 'EPERM'
  }
}

// ---------------------------------------------------------------------------
// 进程表（Windows：PowerShell + CIM；POSIX：/proc —— 各读各的真实，一份判据）
// ---------------------------------------------------------------------------

const CIM_SELECT =
  '[Console]::OutputEncoding=[Text.Encoding]::UTF8; ' +
  'Get-CimInstance Win32_Process'

/**
 * 跑一段 PowerShell，返回 stdout 的 UTF-8 文本；**起不来/读不到返回 `null`**（不抛）。
 *
 * 为什么不抛：读进程表失败是**读数问题**，不是"那儿什么都没有"。调用方要能把
 * 「读不到」与「不在」分开（`ledgerPid` 就是这么做的），而不是让整条命令崩掉。
 */
function powershell(script) {
  let done
  try {
    done = spawnSync(
      'powershell.exe',
      ['-NoProfile', '-NonInteractive', '-Command', script],
      { encoding: 'utf8', windowsHide: true },
    )
  } catch {
    return null
  }
  if (done.error) return null
  return done.stdout ?? ''
}

function parseCimJson(stdout) {
  const text = String(stdout ?? '').trim()
  if (!text) return []
  let parsed
  try {
    parsed = JSON.parse(text)
  } catch {
    return []
  }
  const rows = Array.isArray(parsed) ? parsed : [parsed]
  return rows
    .filter((row) => row && typeof row === 'object')
    .map((row) => ({
      pid: Number(row.ProcessId),
      name: String(row.Name ?? ''),
      exePath: row.ExecutablePath ? String(row.ExecutablePath) : null,
      cmdline: row.CommandLine ? String(row.CommandLine) : null,
    }))
    .filter((row) => Number.isInteger(row.pid))
}

// —— POSIX 的进程事实（/proc）—— 每一条都区分「读不到」与「不在」——

/** 读 `/proc/<pid>` 下的一个文件；读不到 ⇒ `null`（**「读不到」不许伪装成「没有」**）。 */
function readProcFile(file) {
  try {
    return fs.readFileSync(file, 'utf8')
  } catch {
    return null
  }
}

/**
 * 进程**真实可执行件路径**（`/proc/<pid>/exe` 这个链接的目标）。
 * - 读得到 ⇒ 它（末尾的 ` (deleted)` 后缀去掉：可执行件被替换/删掉时内核会挂上它）；
 * - 读不到 ⇒ `null` —— 别人的进程（EACCES）或已经退场。
 */
function readProcExe(pid) {
  let target
  try {
    target = fs.readlinkSync(`${PROC_ROOT}/${pid}/exe`)
  } catch {
    return null
  }
  return target.endsWith(' (deleted)') ? target.slice(0, -' (deleted)'.length) : target
}

/**
 * 精确 argv（`/proc/<pid>/cmdline`，NUL 分隔）；读不到 ⇒ `null`。
 * 为什么单独要它（不只用拼出来的字符串）：**含空格的取值不靠引号也切得对** ——
 * 家目录带空格时，`--root` 那一维只有拿到真 argv 才核得准。
 */
function readProcArgv(pid) {
  const raw = readProcFile(`${PROC_ROOT}/${pid}/cmdline`)
  if (raw === null) return null
  return raw.split('\0').filter((part) => part !== '')
}

/** 进程名（`/proc/<pid>/comm`）；读不到 ⇒ `null`。 */
function readProcComm(pid) {
  const raw = readProcFile(`${PROC_ROOT}/${pid}/comm`)
  return raw === null ? null : raw.trim()
}

/** 路径末段（不引 path 模块：按**本平台**的分隔符字面取，与 `path_join` 同一条口径）。 */
function baseName(value) {
  const at = value.lastIndexOf(IS_WINDOWS ? '\\' : '/')
  return at >= 0 ? value.slice(at + 1) : value
}

/**
 * POSIX：组装一个进程行 —— 与 Windows 的 CIM 行**同形**（`{pid,name,exePath,cmdline}`，
 * 多一个 `argv` 给精确切词用）。所有字段都可能读不到（`null`），但**行本身存在**
 * 就说明"这个 pid 在 `/proc` 里看得到"。
 */
function describeProcessPosix(pid) {
  const exePath = readProcExe(pid)
  const argv = readProcArgv(pid)
  const comm = readProcComm(pid)
  return {
    pid,
    // 名字优先取**可执行件末段**（`node` 而不是 `node-MainThread`）；读不到再退回 comm。
    name: exePath !== null ? baseName(exePath) : (comm ?? ''),
    exePath,
    cmdline: argv === null ? null : argv.join(' '),
    argv,
  }
}

/** 按可执行件名（`agent_host.exe`）查进程表，返回 `{pid,name,exePath,cmdline}`。 */
function listProcessesByName(name) {
  if (!IS_WINDOWS) return listProcessesByNamePosix(name)
  const script =
    `${CIM_SELECT} -Filter "name='${name}'" | ` +
    'Select-Object ProcessId,Name,ExecutablePath,CommandLine | ConvertTo-Json -Compress'
  return parseCimJson(powershell(script))
}

/** 查**指定 pid** 的进程（杀之前验身份用）；进程不在 ⇒ `null`。 */
function describeProcess(pid) {
  const target = Number(pid)
  if (!Number.isInteger(target) || target <= 0) return null
  if (!IS_WINDOWS) {
    if (!isAlive(target)) return null
    // 活着、但 `/proc` 里看不到它 ⇒ **读不到**（不是"不在"）：返回 `null`，由账本那一层
    // 按 `unverifiable` 处理 —— 与 Windows 上"进程表读不到（PowerShell 不可用）"同一条纪律。
    if (!fs.existsSync(`${PROC_ROOT}/${target}`)) return null
    return describeProcessPosix(target)
  }
  const script =
    `${CIM_SELECT} -Filter "ProcessId=${target}" | ` +
    'Select-Object ProcessId,Name,ExecutablePath,CommandLine | ConvertTo-Json -Compress'
  const rows = parseCimJson(powershell(script))
  return rows.length > 0 ? rows[0] : null
}

/**
 * POSIX 的进程表：扫 `/proc`，按**可执行件的真实路径**末段匹配。
 *
 * 为什么不再用 `ps -A -o pid=,comm=,args=` 那一版：`comm` 是线程名且被截断
 * （见 `PROC_ROOT` 那段），按它匹配就是"按名字核验"——判据里禁止的那一手。
 * 读不到 exe 的进程（别人的进程）**直接跳过**：它证明不了是本家的常驻件，不列出来
 * （"宁可不动手，也不误动别人的进程"）。
 */
function listProcessesByNamePosix(name) {
  let entries
  try {
    entries = fs.readdirSync(PROC_ROOT)
  } catch {
    return []
  }
  const rows = []
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue
    const pid = Number(entry)
    const exePath = readProcExe(pid)
    if (exePath === null) continue
    if (baseName(exePath) !== name) continue
    rows.push(describeProcessPosix(pid))
  }
  return rows
}

// ---------------------------------------------------------------------------
// 纯判据（可单测）
// ---------------------------------------------------------------------------

/**
 * 路径比较：**按本平台的语义**。
 * - Windows：大小写无关、斜杠方向无关（同一个文件有多种写法）—— 这一半**逐字未动**；
 * - POSIX：大小写敏感、`/` 分隔（`/a/B` 与 `/a/b` 是两个不同的东西，斜杠方向也不许互译）。
 *
 * 为什么必须分开（本刀）：迁移前那版把 Windows 语义当成了唯一语义（`/`→`\\` + `toLowerCase`），
 * 于是 Linux 上对 `/home/...` 永不匹配 ⇒「按路径核验」整条路在 Linux 上是哑的。
 *
 * ★ **同一个目录的"另一种写法"也算同一个目录**（2026-10-11 真撞上，代价很具体）：
 * `spawn` 出来的命令行里路径可能是 **8.3 短名**（实测 `--home D:\CE76E2~1\celira-temp-world-…`，
 * 而本家手里是长名），也可能带 `..`。逐字比认不出"这是我的门" ⇒ `home.js down`
 * **拒绝杀自己起的门** ⇒ 门留在盘上**占着 5190/5191** ⇒ 后面每一次产品形态门禁量的
 * 都是它（看起来像"代码坏了"）。判据：**写法不同不算不同的家** —— 两边各自交给文件系统
 * 解析一次（`realpath` 会解开短名与 `..`）。
 * ⚠ **解析不出来就不认领**（路径已经不在了 / 解不开）：**宁可杀不掉，也不误杀** ——
 * 这条判据的用处是"敢杀"，而它错一次的代价是杀掉别人的东西。
 */
function samePath(left, right) {
  if (!left || !right) return false
  const windows = (value) => value.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase()
  const posix = (value) => value.replace(/\/+$/, '')
  const norm = IS_WINDOWS ? windows : posix
  if (norm(left) === norm(right)) return true
  // 逐字比不出 ⇒ 再让文件系统说一遍（短名 / `..` / 符号链接）。
  const canonical = (value) => {
    try {
      const real = fs.realpathSync.native ? fs.realpathSync.native(value) : fs.realpathSync(value)
      return norm(real)
    } catch {
      return null
    }
  }
  const a = canonical(left)
  const b = canonical(right)
  return a !== null && a === b
}

/**
 * 命令列里有没有这个片段。
 * - Windows：大小写无关，并把片段里的 `/` 容错成 `\\`（同一份命令行的多种写法）——逐字未动；
 * - POSIX：按字面（大小写敏感；`/` 就是 `/`）。
 */
function cmdlineIncludes(cmdline, needle) {
  if (!cmdline || !needle) return false
  if (IS_WINDOWS) return cmdline.toLowerCase().includes(needle.replace(/\//g, '\\').toLowerCase())
  return cmdline.includes(needle)
}

/**
 * 切词源：POSIX 的进程行带**精确 argv**（`/proc/<pid>/cmdline`）⇒ 用它；否则用命令行字符串
 * （Windows 的 CIM 只给一份字符串，按引号切）。
 */
function tokenSource(info) {
  if (!info) return null
  return Array.isArray(info.argv) ? info.argv : info.cmdline
}

/**
 * 命令列里 `--旗标 <值>` 的取值是不是这个（切词后**成对**比，路径按 `samePath`）。
 *
 * `line` 可以是命令行字符串，也可以是**切好的 argv**（POSIX 的 `/proc` 行给的是后者）。
 *
 * 为什么不是子串判据：`cmdlineIncludes(cmdline, '--root ' + envDir)` 会把 `…\home\env2`
 * 这种**更长**的路径也算中（前缀相同），`--mcp` 的取值里也可能碰巧含这个片段 —— 而这一维
 * 判错的方向是**跨家误杀**。只认 `--旗标 值` 这一种形状：我们自己的 `spawn` 只发这一种
 * （`=` 形状从没发过，不为它预建；真认不出时 `down` 的端口收尾还兜着一道）。
 */
function cmdlineFlagMatches(cmdline, flag, value) {
  if (!cmdline || !flag || !value) return false
  const tokens = parseTokens(cmdline)
  for (let index = 0; index + 1 < tokens.length; index += 1) {
    if (tokens[index] !== flag) continue
    if (samePath(tokens[index + 1], value)) return true
  }
  return false
}

/**
 * 命令列里 `--旗标 <值>` 的取值（切词后**成对**取；没有这个旗标 ⇒ `null`）。
 *
 * 与 `cmdlineFlagMatches` 同一套切词，只是"取出来"而不是"比一比" —— 家事实那一维要把
 * 声明的家目录**拿到手**（拿去和本家的比），而不是只判相等。
 */
function flagValue(cmdline, flag) {
  const tokens = parseTokens(cmdline)
  for (let index = 0; index + 1 < tokens.length; index += 1) {
    if (tokens[index] === flag) return tokens[index + 1]
  }
  return null
}

/**
 * **家事实的"声明"那一维**（2026-09-25）：命令行里有没有 `--home <家目录>`，它等不等于本家。
 *
 * 三态，与端口那一维（`portVerdict`）同一条纪律：
 *   · `'absent'`  命令行里没声明 ⇒ 这一维**缺席**，由老判据说话（手工直敲的常驻件就是这一档）；
 *   · `'ours'`    声明了、而且是本家 ⇒ 自己人；
 *   · `'foreign'` 声明了、但不是本家 ⇒ **铁证：不是本家的人**（同包 + 同端口也救不了它）。
 *
 * ⚠ 与 `requireFact` 同一条纪律：**"声明了但本家没给 homeDir"必须炸**，不许静默退回
 * "只看端口" —— 那正是 2026-09-24 那一刀堵过的洞（默认配置下两家同端口 ⇒ 判据分不出彼此）。
 */
function homeFlagVerdict(info, home) {
  const declared = flagValue(tokenSource(info), '--home')
  if (declared === null) return 'absent'
  if (!home || typeof home.homeDir !== 'string' || home.homeDir === '') {
    throw new Error(
      '家事实缺 homeDir：命令行列了 --home，判据就必须有本家的家目录可比（不许静默退回只看端口）',
    )
  }
  return samePath(declared, home.homeDir) ? 'ours' : 'foreign'
}

/** 是不是一个"壳"（我们起的常驻件都是被壳或 node 拉起来的）。 */
function isShellProcess(info) {
  const name = (info.name || '').toLowerCase()
  return name === 'node.exe' || name === 'cmd.exe' || name === 'npm.cmd' || name === 'node'
}

// ---------------------------------------------------------------------------
// 家事实（认领判据的第二维）
// ---------------------------------------------------------------------------

/**
 * **家事实**——「这支进程是**哪个家**的」那一半判据（`home` 对象）。
 *
 * 为什么必须有这一维（2026-09-24 真机实测）：同一个家启动的常驻件都来自**同一份发布包**
 * （`bin/*.exe`、`scripts/home/*.js`），所以"可执行件路径相等"这条判据**认不出两个家** ——
 * 真家的宿主（`--root D:\celira-home\env`）与临时家的宿主是同一个 `F:\celira-test\bin\agent_host.exe`：
 * `up` 把在跑的那支当成"本家已经在跑"（不起本家 ⇒ 家起不来，还把外来 pid 写进本家账本），
 * `down` 的孤儿清扫按同一条路径把**别家的宿主杀掉**（跨家误杀）。唯一能区分它们的事实是
 * **那个家自己的东西**：环境根 / 端口。
 *
 * 每支常驻件的家事实从哪读（**只有这一处说了算**）：
 *  - `agent`（宿主）：命令行里的 `--root <环境根>` —— 唯一的家**路径**事实，起 Agent 时就写进命令行；
 *  - `client`：命令行里的 `--port <client_port>`（发布/源码两形态都由 `up` 传）——命令行**常量**；
 *  - `im` / `status` / `config`：命令行里没有家事实（`celira-im.exe` 无参；那两个口是
 *    `home.js … --serve`，家事实在环境变量里、进程表读不到）⇒ 家事实是**端口**（见 `portVerdict`）。
 *
 * `home` 的形状（调用方从家目录与 `home.conf` 组装）：
 *   `{ envDir | envDirs, clientPort, imPort, statusPort, configPort, listening }`
 * （`envDirs` 是**一支一个环境根**那一组，2026-10-01 多支刀；`envDir` 是单支的老形状。）
 * `listening` 是一次 netstat 的**快照**（`listenerSnapshot()`）：pid → 它在听的端口集合 ——
 * 只有 im/status/config 三支用它。判据保持**纯函数**（可单测）：取快照是调用方的事。
 */
const HOME_FACT = {
  agent: { flag: '--root', key: 'envDir' },
  client: { flag: '--port', key: 'clientPort' },
  im: { portKey: 'imPort' },
  status: { portKey: 'statusPort' },
  config: { portKey: 'configPort' },
  // **家的门**（2026-10-01 第六刀）：一个进程、两个端面 ⇒ 家事实是**一组端口**
  // （读口 `status_port` ＋ 写口 `config_port`；听其中任一个就算本家的）。
  doors: { portsKey: 'doorsPorts' },
}

/**
 * 取一支 target 的家事实（**缺了必须炸**）。
 *
 * 为什么不许静默退回"只看可执行件路径"：那正是本刀堵的洞 —— 一个忘了传家事实的调用点
 * 会看起来一切正常，直到某天两个家并存时又一次跨家误杀。与 `FINGERPRINTS` 那条
 * "未登记的 target 不许静默"同一条纪律：**查无事实 ⇒ 报错**。
 * （"值来自用户配置、读不懂"是另一回事：那由 `portVerdict` 按"判不了"处理，不炸整条命令。）
 *
 * ⚠ **agent 那一维可以是一组**（2026-10-01 多支刀）：一个家可以住 N 支 Agent，每支有
 * 自己的环境根 ⇒ 判据是"`--root` 落在**本家的那一组**里"。`home.envDirs`（数组）优先，
 * 单支的老形状 `home.envDir`（字符串）继续收（单支调用点与既有用例照旧）。
 */
function requireFact(target, home) {
  const fact = HOME_FACT[target]
  if (!fact) {
    throw new Error(`未登记的进程 target：${String(target)}——每个 target 必须有自己的家事实`)
  }
  if (target === 'agent') {
    const many = home ? home.envDirs : undefined
    if (Array.isArray(many)) {
      const roots = many.filter((value) => typeof value === 'string' && value !== '')
      if (roots.length === 0) {
        throw new Error(
          'target agent 的家事实缺 --root 那一组（home.envDirs）——认领判据不许静默退回"只看可执行件"',
        )
      }
      return roots
    }
  }
  if (fact.portsKey) {
    const many = home ? home[fact.portsKey] : undefined
    const ports = Array.isArray(many) ? many.filter((value) => typeof value === 'string' && value !== '') : []
    if (ports.length === 0) {
      throw new Error(
        `target ${target} 的家事实缺端口那一组（home.${fact.portsKey}）——认领判据不许静默退回"只看可执行件"`,
      )
    }
    return ports
  }
  const key = fact.portKey ?? fact.key
  const value = home ? home[key] : undefined
  if (typeof value !== 'string' || value === '') {
    const what = fact.portKey ? `端口（home.${key}）` : `${fact.flag} 的值（home.${key}）`
    throw new Error(`target ${target} 的家事实缺 ${what}——认领判据不许静默退回"只看可执行件"`)
  }
  return value
}

/**
 * 端口这一维的裁决（`im` / `status` / `config` 三支）：**三态**（2026-09-24 裁）。
 *
 *   `'mine'`    它在听本家的那个端口 ⇒ 自己人；
 *   `'foreign'` 它在听**别的**端口 ⇒ 不是本家的人（别家的 im / 状态口 / 配置门就是这个形状）；
 *   `'unknown'` 它谁都没听（刚起还没绑上、或挂死了）⇒ **不能据此否认**，由老判据说话。
 *
 * 为什么第三态必须存在（而不是"严格要求正在听本家端口"）：`up` 把账本写进 pid 文件是在
 * **进程绑端口之前**；那一刻若按严格口径判"外人"，`up` 会以为本家的 im 不在而**双起**一支
 * （后起的撞端口即退，`up` 反而以"端口上的不是我们起的那个"收场），`down` 会**漏收**它。
 * 而真正的跨家误认（别家的 im 在跑）**一定**在听它自己那个端口 ⇒ 由 `'foreign'` 抓住，不漏。
 *
 * 端口值读不懂（`home.conf` 被人改坏）⇒ 也按 `'unknown'`：判不了的事不拿它当否认，
 * 那条路 `up` 自己有"读不懂就不动它"的读数。
 */
function portVerdict(pid, target, home) {
  const fact = requireFact(target, home)
  // 一支 target 可以**有多个口**（家的门：读口 ＋ 写口）⇒ 听其中任一个就算本家的。
  const mine = (Array.isArray(fact) ? fact : [fact]).map((value) => Number(value))
  const listening = home.listening
  if (!(listening instanceof Map)) {
    throw new Error('家事实缺端口快照（home.listening）——认领判据不许静默退回"只看可执行件"')
  }
  const want = mine.filter((value) => Number.isInteger(value))
  if (want.length === 0) return 'unknown'
  const ports = listening.get(Number(pid))
  if (!ports || ports.size === 0) return 'unknown'
  return want.some((value) => ports.has(value)) ? 'mine' : 'foreign'
}

/**
 * 「这个 target 监听哪个端口」—— `listenerOf` 要扫的那个口（与家事实同一个来源）。
 * `agent` 不在表里：宿主是**出站**进程，不占端口（端口扫永远碰不到它，这正是它需要
 * `--root` 那条命令行事实的原因）。
 */
const LISTEN_PORT_KEY = {
  im: 'imPort',
  client: 'clientPort',
  status: 'statusPort',
  config: 'configPort',
  // 家的门：**两个口都扫**（进程只起一个，绑的是两个口）——见 `listenerOf` 的数组分支。
  doors: 'doorsPorts',
}

// ---------------------------------------------------------------------------
// 认领判据（每支 target 的指纹）
// ---------------------------------------------------------------------------

/**
 * **每个 target 的指纹**——唯一判据 =「这支进程是不是**本家**的」，两维缺一不可：
 *   ① 种类（可执行件路径 / 命令行）——它是**哪一支**常驻件；
 *   ② 家事实（`--root` / `--port` / 端口）——它属于**哪个家**（见本文件上半段那段）。
 *
 * 三条形态纪律（前两条 TASK_59 立，都是踩出来的）：
 *  1. **指纹必须把这个 target 与别的 target 分开**。原来 `client` 那支收"命令行提到 repo"，
 *     而状态口是 `node <repo>/home.js status --serve` **也提 repo** ⇒ 两支**互相认领**；
 *  2. **查无指纹 ⇒ 报错**，不许静默 `return false`。静默会把"我们忘了登记"伪装成
 *     "这不是我们的进程"，而 `down` 会带着 **exit 0** 说一句"不是我们的——不杀" ⇒
 *     **最坏的假绿**（进程明明是我们的，却永远停不掉，看起来还像如实拒绝）。
 *  3. **查无家事实 ⇒ 报错**（同一条纪律的第二维）：缺了它，两个共用发布包的家会互相认领。
 */
const FINGERPRINTS = {
  im: (info, layout, home) => {
    const verdict = portVerdict(info.pid, 'im', home)
    return samePath(info.exePath, layout.imExe) && verdict !== 'foreign'
  },
  agent: (info, layout, home) => {
    const roots = requireFact('agent', home)
    const list = Array.isArray(roots) ? roots : [roots]
    return (
      samePath(info.exePath, layout.hostExe) &&
      list.some((root) => cmdlineFlagMatches(tokenSource(info), '--root', root))
    )
  },
  client: (info, layout, home) => {
    const port = requireFact('client', home)
    if (!isShellProcess(info)) return false
    // 发布形态：`node …/scripts/serve_client.mjs --dist … --port …`
    // 源码形态：`node …/client/node_modules/vite/bin/vite.js preview --port …`
    // ⚠ **不许再收"提到 repo"**（那一条正是把状态口认成 client 的入口）。
    const kind =
      cmdlineIncludes(info.cmdline, 'serve_client.mjs') ||
      cmdlineIncludes(info.cmdline, path_join(layout.clientDir, 'node_modules'))
    if (!kind) return false
    // 家事实第二维（2026-09-25）：声明了家就不是本家的 ⇒ 直接否认（同端口也救不了）。
    // （源码形态的 `vite preview` 带不了这个旗标 ⇒ 那一档只有端口这一维，见 `homeFlagVerdict`。）
    if (homeFlagVerdict(info, home) === 'foreign') return false
    return cmdlineFlagMatches(tokenSource(info), '--port', port)
  },
  // **家的门**（2026-10-01 第六刀）：`node …/home.js doors --home …` —— 一个进程、两个端面。
  //
  // 家事实是**一组端口**（读口或写口，任一听即本家）；第二维仍是 `--home`。
  // 跨词短语 `home.js doors`：单看 `home.js` 会认到 `up/down` 那些**一次性命令**上。
  //
  // ⇒ 这两条让 2026-10-01 之前那种"两个门各一个进程"的形状**也还能被认出来**
  //   （下面 status / config 两条旧指纹就是为此留的迁移桥：老家还在跑旧门时，
  //   `up`/`down` 要认得出它们、才能把它们换成这一支）。旧门不再有**新**用途。
  doors: (info, layout, home) => {
    const verdict = portVerdict(info.pid, 'doors', home)
    if (!isShellProcess(info)) return false
    if (homeFlagVerdict(info, home) === 'foreign') return false
    return cmdlineIncludes(info.cmdline, 'home.js doors') && verdict !== 'foreign'
  },
  status: (info, layout, home) => {
    const verdict = portVerdict(info.pid, 'status', home)
    if (!isShellProcess(info)) return false
    if (homeFlagVerdict(info, home) === 'foreign') return false
    // 只读状态口：`node …/home.js status --serve`。两个片段**同时**要求 ——
    // 单看 `home.js` 会认到 `node home.js up/down` 那些**一次性命令**上（它们不是常驻件）。
    //
    // ⚠ TASK_77 收紧：原来只要求 `home.js` ＋ `--serve`，而模型配置门是
    // `home.js config --serve` —— **同样满足那两条** ⇒ 两支会互相认领（新用例当场抓到了）。
    // 判据改成**跨词短语** `status --serve`（与 config 那一支对称）：认错进程会去杀错人。
    return (
      cmdlineIncludes(info.cmdline, 'home.js') &&
      cmdlineIncludes(info.cmdline, 'status --serve') &&
      verdict !== 'foreign'
    )
  },
  // **模型配置门**（TASK_77）：`node …/home.js config --serve`。
  //
  // 与状态口**同款、也必须分开**：两支都是 `home.js … --serve`，靠那个子命令词区分。
  // 这里要求的是**跨词短语** `config --serve` 而不是裸 `config`：路径或别的参数里碰巧含
  // 这个词时不该被认成它——认错了会去杀一个不相干的进程。
  config: (info, layout, home) => {
    const verdict = portVerdict(info.pid, 'config', home)
    if (!isShellProcess(info)) return false
    if (homeFlagVerdict(info, home) === 'foreign') return false
    return (
      cmdlineIncludes(info.cmdline, 'home.js') &&
      cmdlineIncludes(info.cmdline, 'config --serve') &&
      verdict !== 'foreign'
    )
  },
}

/**
 * 「这个 pid 真的是我们要停的那个吗」——两类判据（种类 ＋ 家事实，见 `FINGERPRINTS`）。
 * 认不出来就**不杀**（只清账本并如实报），这是"只停自己人"的执行体；
 * 但**"没登记"不是"认不出"** ⇒ 那种情况必须炸出来。
 */
function ownsPid(target, info, layout, home) {
  const fingerprint = FINGERPRINTS[target]
  if (typeof fingerprint !== 'function') {
    throw new Error(
      `未登记的进程 target：${String(target)}——每个 target 必须有自己的指纹（不许静默当成外人）`,
    )
  }
  if (!info) return false
  return fingerprint(info, layout, home)
}

/** 小工具：只在判据里用，避免再引 path（本文件刻意不依赖 path 模块的可用性）。 */
function path_join(dir, leaf) {
  return `${dir.replace(/[\\/]+$/, '')}${IS_WINDOWS ? '\\' : '/'}${leaf}`
}

/**
 * 按引号切词：`--mcp "名字=命令 带空格"` 要当成**一个**取值。
 * （与 status 那份 python 逐字同算法——它的理由仍然成立：宿主命令行里单双引号都有。）
 *
 * ⚠ 参数也可以直接是**切好的 argv**（POSIX 的 `/proc/<pid>/cmdline`）：那就原样交回 ——
 * 真 argv 里带空格的取值不靠引号，再切一次反而会把它切坏。
 */
function parseTokens(line) {
  if (Array.isArray(line)) return line
  const out = []
  let current = ''
  let quoted = false
  for (const char of line) {
    if (char === '"') quoted = !quoted
    else if (char === ' ' && !quoted) {
      if (current) {
        out.push(current)
        current = ''
      }
    } else current += char
  }
  if (current) out.push(current)
  return out
}

/** 从一条宿主命令行里取出 `--mcp` 的**名字段**（按出现顺序去重）。 */
function mcpNamesFromCommandLine(line) {
  const parts = parseTokens(line)
  const names = []
  for (let index = 0; index < parts.length; index += 1) {
    if (parts[index] !== '--mcp' || index + 1 >= parts.length) continue
    const name = parts[index + 1].split('=', 1)[0]
    if (name && !names.includes(name)) names.push(name)
  }
  return names
}

/**
 * 解析 `netstat -ano`：**在监听**这个端口的 pid 列表。
 * 端口按「本地地址的最后一个 `:端口` 精确匹配」判（`18080` 不会匹配 `8080`）。
 */
function parseNetstat(output, port) {
  const suffix = `:${port}`
  const pids = new Set()
  for (const line of String(output ?? '').split('\n')) {
    const tokens = line.trim().split(/\s+/)
    if (tokens.length < 4) continue
    if (!tokens.includes('LISTENING')) continue
    const local = tokens[1] ?? ''
    if (!local.endsWith(suffix)) continue
    const pid = Number.parseInt(tokens[tokens.length - 1], 10)
    if (Number.isInteger(pid) && pid > 0) pids.add(pid)
  }
  return [...pids].sort((a, b) => a - b)
}

/**
 * 反过来读同一份 `netstat -ano`：**每个 pid 在听哪些端口**（`Map<pid, Set<端口>>`）。
 *
 * 端口这一维的家事实（im/status/config）要的正是这个方向：一个 pid 在听**别的**端口，
 * 就说明它不是本家的那支（本家的三支各只占本家的一个口）。只收 `LISTENING` 行、
 * 端口取本地地址最后一段（`127.0.0.1:8080` / `[::]:5190` 都适用）。
 */
function listeningPortsByPid(output) {
  const map = new Map()
  for (const line of String(output ?? '').split('\n')) {
    const tokens = line.trim().split(/\s+/)
    if (tokens.length < 4) continue
    if (!tokens.includes('LISTENING')) continue
    const local = tokens[1] ?? ''
    const at = local.lastIndexOf(':')
    if (at < 0) continue
    const port = Number.parseInt(local.slice(at + 1), 10)
    const pid = Number.parseInt(tokens[tokens.length - 1], 10)
    if (!Number.isInteger(port) || !Number.isInteger(pid) || pid <= 0) continue
    if (!map.has(pid)) map.set(pid, new Set())
    map.get(pid).add(port)
  }
  return map
}

// ---------------------------------------------------------------------------
// 端口 / HTTP
// ---------------------------------------------------------------------------

/** Windows：`netstat -ano`（读不到返回 `''`，与迁移前逐字相同）。 */
function netstat() {
  const done = spawnSync('netstat', ['-ano'], {
    encoding: 'utf8',
    windowsHide: true,
  })
  if (done.error || !done.stdout) return ''
  return done.stdout
}

/**
 * 解析 `ss -ltnp`（Linux）的一行（纯函数 ⇒ 可单测）：
 *   `LISTEN 0 511 127.0.0.1:8080 0.0.0.0:* users:(("node",pid=30636,fd=20))`
 *
 * 端口取本地地址的**最后一段**（`[::1]:5190`、`127.0.0.53%lo:53` 都适用）；pid 取 `pid=` 的取值。
 * **看不到 pid** 的条目（别人的进程 / 内核的）也如实产出 `{ pid: null, port }` —— 那是
 * "知道有人在听、但看不到是谁"，与"没有"是两件事。
 */
function parseSsListeners(output) {
  const entries = []
  for (const line of String(output ?? '').split('\n')) {
    const tokens = line.trim().split(/\s+/)
    if (tokens[0] !== 'LISTEN' || tokens.length < 4) continue
    const local = tokens[3]
    const at = local.lastIndexOf(':')
    if (at < 0) continue
    const port = Number.parseInt(local.slice(at + 1), 10)
    if (!Number.isInteger(port)) continue
    const pids = [...line.matchAll(/pid=(\d+)/g)].map((match) => Number.parseInt(match[1], 10))
    if (pids.length === 0) entries.push({ pid: null, port })
    else for (const pid of pids) entries.push({ pid, port })
  }
  return entries
}

/**
 * POSIX：在听的 TCP 套接字 —— `ss -ltnp`（iproute2；`users:(("node",pid=…))` 那一列就是端口→pid）。
 *
 * 为什么不用 `netstat -ano`（迁移前那条路）：Linux 的 netstat（net-tools）根本没有 PID 列
 * （`-o` 是 timers）⇒ 端口→pid 永远读不到 ⇒ `down` 的端口收尾把"还占着"说成"已放"（本刀堵的洞）。
 *
 * **读不到返回 `null`**（`ss` 起不来 / 输出读不到）——「读不到」与「一个都没有」必须分开：
 * 前者不许被说成"端口已放"（与账本 `unverifiable` 同一条纪律）。
 */
function posixListeners() {
  const done = spawnSync('ss', ['-ltnp'], { encoding: 'utf8' })
  if (done.error || done.status !== 0 || typeof done.stdout !== 'string') return null
  return parseSsListeners(done.stdout)
}

function listenersOnPort(port) {
  if (IS_WINDOWS) return parseNetstat(netstat(), port)
  const entries = posixListeners()
  if (entries === null) return []
  const pids = new Set()
  for (const entry of entries) {
    if (entry.port === Number(port) && entry.pid !== null) pids.add(entry.pid)
  }
  return [...pids].sort((a, b) => a - b)
}

/**
 * **端口快照**：一次读「在听」的表 ⇒ `Map<pid, Set<端口>>`（家事实 `home.listening` 的那一项）。
 *
 * 为什么由调用方取、且一个命令只取一次：判据本身要**纯**（可单测），取快照是 I/O；
 * 一次命令里的世界本来就是一个快照（认领判据判的是"此刻在跑的那些是谁"）。快照**过期**
 * 的方向是安全的：本命令中途才起来的进程只会落进 `'unknown'`（判不了 ⇒ 不否认），
 * 而已经在跑的别家进程**一定**在快照里（跨家误认要抓的正是它们）。
 *
 * POSIX 上**看不到 pid** 的监听条目（别人的进程）不进 Map：它们进不了"认领"判据，
 * 但会在 `portInUse` 里被算作"有人"（见那里的纪律）。读不到整张表 ⇒ 空 Map（判不了 ⇒ 不否认）。
 */
function listenerSnapshot() {
  if (IS_WINDOWS) return listeningPortsByPid(netstat())
  const entries = posixListeners()
  if (entries === null) return new Map()
  const map = new Map()
  for (const entry of entries) {
    if (entry.pid === null) continue
    if (!map.has(entry.pid)) map.set(entry.pid, new Set())
    map.get(entry.pid).add(entry.port)
  }
  return map
}

/**
 * 端口上有东西在听吗（占用判据）。
 * - Windows：`netstat -ano` 的 LISTENING 行（读不到 ⇒ 与迁移前一致：按"没有"处理）；
 * - POSIX：`ss -ltnp` 的 LISTEN 行，**含看不到 pid 的那些**（别人的进程也算"占着"）；
 *   **整张表读不到 ⇒ `true`** —— "证明不了它空着"不许被说成"它空着"
 *   （本刀堵的"谎报端口已放"正是这一条：读不到时宁可说占着，也不说已放）。
 */
function portInUse(port) {
  if (IS_WINDOWS) return parseNetstat(netstat(), port).length > 0
  const entries = posixListeners()
  if (entries === null) return true
  return entries.some((entry) => entry.port === Number(port))
}

/**
 * 「这个端口上是谁」的查法 —— 按平台给**跑得通**的那条命令。
 * 读数里不许建议一条在本平台查不出 pid 的命令（Linux 的 `netstat -ano` 没有 PID 列）。
 */
function portLookupHint(port) {
  return IS_WINDOWS
    ? `netstat -ano | grep LISTENING | grep ":${port} "`
    : `ss -ltnp | grep ":${port} "`
}

/** 问一个 URL 的状态码；连不上/超时 ⇒ `'000'`（与 `curl -w '%{http_code}'` 同款读数）。 */
async function httpCode(url) {
  return (await httpProbe(url)).code
}

/**
 * 问一个 URL 的**回执**：状态码 ＋（有上限的）正文。连不上 / 超时 ⇒ `{ code: '000', body: '' }`。
 *
 * 为什么要有"正文"这一格（2026-09-30）：就绪探活要能区分「**这个门**起来了」与
 * 「**别的门**在应门」—— 光看"端口上有回执"会把另一个门应下的 404 当成本门就绪
 * （真机实测：两个门撞同一个端口时，`up` 打印了与事实不符的门读数）。
 * 正文只用于**判据**（如状态口 `/healthz` 的 `{ok:true}` 标记），有上限、不落盘。
 */
function httpProbe(url, { timeout = 3000, bodyLimit = 262144 } = {}) {
  return new Promise((resolve) => {
    let settled = false
    const done = (value) => {
      if (!settled) {
        settled = true
        resolve(value)
      }
    }
    let request
    try {
      request = http.get(url, { timeout }, (response) => {
        const chunks = []
        let size = 0
        response.on('data', (chunk) => {
          size += chunk.length
          if (size <= bodyLimit) chunks.push(chunk)
        })
        response.on('end', () =>
          done({
            code: String(response.statusCode ?? '000'),
            body: Buffer.concat(chunks).toString('utf8'),
          }),
        )
        response.on('error', () => done({ code: String(response.statusCode ?? '000'), body: '' }))
      })
    } catch {
      done({ code: '000', body: '' })
      return
    }
    request.on('timeout', () => {
      request.destroy()
      done({ code: '000', body: '' })
    })
    request.on('error', () => done({ code: '000', body: '' }))
  })
}

/**
 * 等一个（或几个）URL 起来：2xx/3xx/4xx 都算起来了（4xx 说明服务器在听）。
 * ⚠ 多个候选是**必须**的：curl 会按 Happy Eyeballs 把 `localhost` 解析出的地址都试一遍，
 * 而 `http.get` 只认一个解析结果（vite 可能只听 `::1`）——所以这里显式试 `localhost`
 * 与 `127.0.0.1` 两个候选，才算与 curl 行为等价。
 *
 * `options.match`（2026-09-30）：给了它 ⇒ 回执**还要对得上**才算"起来了" —— 就绪探活
 * 要能区分「这个门起来了」与「别的门在应门」（404 不再被当成本门就绪）。判据吃
 * `{code, body}`（见 `httpProbe`），由调用方按**本门自己的标记**写。不给 ⇒ 与从前逐字相同。
 * 返回 `{ ok, code }`：失败时 `code` 是**最后一次**看到的那个码（`'000'` = 一次都没答）。
 */
async function waitHttp(urls, attempts = 20, intervalMs = 500, options = {}) {
  const list = Array.isArray(urls) ? urls : [urls]
  const match = typeof options.match === 'function' ? options.match : null
  let lastCode = '000'
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    for (const url of list) {
      const probe = await httpProbe(url)
      if (probe.code !== '000') lastCode = probe.code
      if (/^[234]/.test(probe.code) && (!match || match(probe))) {
        return { ok: true, code: probe.code }
      }
    }
    if (attempt < attempts - 1) await sleep(intervalMs)
  }
  return { ok: false, code: lastCode }
}

/** 一个 TCP 连接探针（连上就断）：**不做 HTTP/TLS 握手**。 */
function tcpProbe(host, port, timeoutMs = 1500) {
  return new Promise((resolve) => {
    let settled = false
    const done = (value) => {
      if (settled) return
      settled = true
      socket.destroy()
      resolve(value)
    }
    const socket = net.connect({ host, port })
    socket.setTimeout(timeoutMs, () => done(false))
    socket.once('connect', () => done(true))
    socket.once('error', () => done(false))
  })
}

/**
 * 等一个 TCP 端口开始接受连接（**模式二的 `up` 用它等两口起来**）。
 *
 * 为什么不用 `waitHttp`：家口远程访问刀的模式二里，那两口在 **TLS** 上、而且**进门要带
 * 家钥匙** —— `waitHttp` 既说不来 TLS、也没法带钥匙（`up` 手里只有家的目录，没有钥匙原文）。
 * 而 `up` 要知道的只是"这个常驻件起来了没有" ⇒ **TCP 层能连上**就是这个事实的最小判据；
 * "它答不答、答什么"归页面那份读数管（两件事不许混成一件）。
 */
async function waitTcp(host, port, attempts = 20, intervalMs = 500) {
  const target = host === '0.0.0.0' || host === '::' ? '127.0.0.1' : host
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    if (await tcpProbe(target, port)) return { ok: true }
    if (attempt < attempts - 1) await sleep(intervalMs)
  }
  return { ok: false }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

// ---------------------------------------------------------------------------
// 杀
// ---------------------------------------------------------------------------

/**
 * 停一个 pid。返回是否真的停了。**调用方必须先用 `ownsPid` 验过身份**——本函数不下判断。
 *
 * ⚠ 为什么 Windows 上**直接 `/F`**（不先"请"一次）：这三个进程都是没有消息泵的控制台进程，
 * `taskkill /PID`（不带 `/F`）请不动它们，只会白等一轮再强停；而 `.sh` 版真正用的 MSYS
 * `kill` 对原生进程就是 TerminateProcess（立即生效）——所以"直接 /F"才是与旧版**读数等价**
 * 的做法（`[im] 已停（pid N）`，不会多出一行"请它退它没退"）。
 *
 * POSIX：`force` ⇒ 直接 SIGKILL（与 Windows 的 `/F` 同款语义，`down` 三个调用点都走这一档）；
 * 不带 `force` 才按惯例 TERM → KILL。**两种都要等它真的退场**再回话 —— 迁移前那版发完信号
 * 立刻返回，而调用方紧接着就问"还活着吗"：SIGKILL 的退场要几毫秒，那一问会**随机**得到
 * "停不下来"（本刀实测出来的一格：down 的读数会偶发假红）。等待窗口与 Windows 那支同款。
 */
async function killPid(pid, { force = false } = {}) {
  if (!isAlive(pid)) return true
  if (!IS_WINDOWS) {
    const signals = force ? ['SIGKILL'] : ['SIGTERM', 'SIGKILL']
    for (const signal of signals) {
      try {
        process.kill(pid, signal)
      } catch {
        return !isAlive(pid)
      }
      for (let attempt = 0; attempt < 20; attempt += 1) {
        if (!isAlive(pid)) return true
        await sleep(100)
      }
    }
    return !isAlive(pid)
  }
  spawnSync('taskkill', ['/F', '/PID', String(pid)], { encoding: 'utf8', windowsHide: true })
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (!isAlive(pid)) return true
    await sleep(250)
  }
  return !isAlive(pid)
}

// ---------------------------------------------------------------------------
// 起常驻进程
// ---------------------------------------------------------------------------

/**
 * 起一个**脱离调用方**的常驻进程，日志直接落文件。
 *
 * 与 bash 版最重要的差别：日志用 `fs.openSync` 拿到的**文件描述符**交给子进程
 * （不是管道）⇒ 不存在"后台子壳握着调用方的 stdout，调用方永远等不到 EOF"那一类事故
 * （MSYS 上实测过：脚本早跑完了，调用方挂十几分钟）。`detached` + `unref` 保证
 * node 退出后它继续活着（这正是"家里那三个进程不随命令结束而死"的机制）。
 */
function spawnDetached(file, args, { cwd, env, logFile }) {
  const fd = fs.openSync(logFile, 'a')
  try {
    const child = spawn(file, args, {
      cwd,
      env,
      detached: true,
      stdio: ['ignore', fd, fd],
      windowsHide: false,
    })
    child.unref()
    return child.pid
  } finally {
    fs.closeSync(fd)
  }
}

/**
 * 开新一份日志：上一份留档成 `<名字>.1`，原文件清空。
 *
 * 为什么不是纯追加：`up` 之后要读 `[listening]` 行判断"Agent 起来了吗"，日志累积时读到的
 * 可能是**上一次**那一行 —— 那是假绿（本仓为"看起来绿"栽过跟头）。
 * 为什么不是纯清空：宿主独有的读数（`drives` / `notification` / `model-failure` / 启动参数）
 * **只落在这一份日志里**，清掉就等于"重启即失"，出事的人回头查不到上一轮。
 * **留一份**是这两件事的最小公约数：读的那一侧看到的本轮仍然从空开始，上一轮还在盘上。
 *
 * 返回 `{ archived, lostBytes }`：`lostBytes > 0` 表示**盘上原本有内容、却没留下来** ——
 * 调用方应当把这件事说出来。静默丢掉上一轮，正是这条规矩要防的事。
 */
function rotate(file) {
  let pending = 0
  try {
    if (fs.existsSync(file)) {
      pending = fs.statSync(file).size
      fs.copyFileSync(file, `${file}.1`)
      fs.writeFileSync(file, '', 'utf8')
      return { archived: true, lostBytes: 0 }
    }
  } catch {
    // 落到下面：哪怕留档失败也必须清空，否则本轮读数会被旧内容污染。
  }
  fs.writeFileSync(file, '', 'utf8')
  return { archived: false, lostBytes: pending }
}

/**
 * 家里那支宿主（`agent_host.exe`）的进程行 —— **本家的**那些（种类 ＋ 家事实，两手都过）。
 * 三个 verb 共用这一份（up 的接管、down 的对账、status 的装配面读数都在问它）。
 *
 * ⚠ 2026-09-24 起这里必须按家事实筛：同一份发布包服务多个家时，"可执行件路径相等"
 * 会把**别家的宿主**也列进来（status 的对账会读到别家的 `--mcp` 装配面，down 会杀掉它）。
 */
function describeHostProcesses(layout, home) {
  const name = require('node:path').basename(layout.hostExe)
  return listProcessesByName(name).filter((row) => ownsPid('agent', row, layout, home))
}

function hostOrphanPids(layout, home) {
  return describeHostProcesses(layout, home).map((row) => row.pid)
}

/**
 * 端口上那支**是不是我们的人**（种类 ＋ 家事实，**不看 pid 文件**）。
 *
 * 为什么需要它：pid 文件会说谎——跨实现迁移的过渡期最明显（.sh 起的那三个进程把 **MSYS pid**
 * 写进账本，Node 读出来对不上任何 Windows 进程）。判据落在"端口上的那个进程是谁"，
 * 就与"账本里写了什么"无关了。找到就返回它的 pid，否则 null。
 *
 * 端口不再由调用方单独传：**哪个 target 扫哪个口**由家事实一处说了算（`LISTEN_PORT_KEY`）——
 * 分开传会让"扫 5290、却拿 5190 当判据"这种错配悄悄退化成"不是我们的"。
 */
function listenerOf(target, layout, home) {
  const portKey = LISTEN_PORT_KEY[target]
  if (!portKey && !FINGERPRINTS[target]) {
    throw new Error(`未登记的进程 target：${String(target)}——每个 target 必须有自己的家事实`)
  }
  if (!portKey) return null // 家里只有宿主（agent）不监听端口
  const fact = requireFact(target, home)
  const ports = Array.isArray(fact) ? fact : [fact]
  for (const port of ports) {
    for (const pid of listenersOnPort(port)) {
      if (ownsPid(target, describeProcess(pid), layout, home)) return pid
    }
  }
  return null
}

/**
 * **账本可不可信**：pid 文件里那个数，究竟是我们的进程、别人的进程，还是根本读不出来？
 *
 * 为什么必须有这层：`isAlive(账本里的数)` 单独用会**假阳性**——MSYS pid 与 Windows pid 的
 * 数字空间不重叠，一个陈旧账本里的数完全可能撞上另一个无关进程（实测撞到过：status 报
 * 「活着（pid 20303）」，而真 im 的 Windows pid 是 26544）。所以判据是**两步**：
 * 活着 **且** 身份对上（种类 ＋ **家事实**，见 `FINGERPRINTS`）才算"我们的"。
 *
 * 四态（**"读不到"必须与"不是"分开**——读不到时退回 bash 版的语义：只看活着与否）：
 *   `{ state: 'gone' }`      账本没写、或那支已经退了
 *   `{ state: 'ours' }`      活着且验明是**本家**的（种类 ＋ 家事实，见 `FINGERPRINTS`）
 *   `{ state: 'not-ours' }`  活着但**不是**本家的（陈旧/撞号/**别家的**）——绝不拿它去杀、也别当"已经在跑"
 *   `{ state: 'unverifiable' }` 活着但进程表读不到（PowerShell 不可用）⇒ 当"我们的"处理（与 .sh 同）
 *
 * ⚠ 2026-09-24：判据加了家事实这一维 ⇒ 别家的 pid（哪怕可执行件路径一致）落进 `not-ours`。
 * 处置按既有语义：**只清账本、不杀**（裁决于 2026-09-24：被清的那支若在跑，它属于别家）。
 */
function ledgerPid(target, pidFile, layout, home) {
  const pid = readPid(pidFile)
  if (!isAlive(pid)) return { pid: null, state: 'gone' }
  const info = describeProcess(pid)
  if (info === null) return { pid, state: 'unverifiable' }
  return { pid, state: ownsPid(target, info, layout, home) ? 'ours' : 'not-ours' }
}

/** 账本可信（我们的 / 读不出来）⇒ 返回 pid；否则 null。 */
function trustedLedgerPid(target, pidFile, layout, home) {
  const ledger = ledgerPid(target, pidFile, layout, home)
  return ledger.state === 'ours' || ledger.state === 'unverifiable' ? ledger.pid : null
}

/** 从命令行里取出某个旗标的值（`--root <路径>` / `--agent <id>`）：对账时用来**把宿主认到住户身上**。 */
function flagValueFromCommandLine(line, flag) {
  const parts = parseTokens(line)
  for (let index = 0; index < parts.length - 1; index += 1) {
    if (parts[index] === flag) return parts[index + 1]
  }
  return null
}

module.exports = {
  flagValueFromCommandLine,
  IS_WINDOWS,
  IS_LINUX,
  // pid 文件
  readPid,
  writePid,
  clearPid,
  isAlive,
  // 进程表
  listProcessesByName,
  describeProcess,
  describeHostProcesses,
  hostOrphanPids,
  listenerOf,
  ledgerPid,
  trustedLedgerPid,
  // 纯判据
  samePath,
  cmdlineIncludes,
  cmdlineFlagMatches,
  flagValue,
  tokenSource,
  homeFlagVerdict,
  ownsPid,
  listeningPortsByPid,
  parseTokens,
  mcpNamesFromCommandLine,
  parseNetstat,
  parseSsListeners,
  // 端口 / HTTP
  listenersOnPort,
  listenerSnapshot,
  portInUse,
  portLookupHint,
  httpCode,
  httpProbe,
  waitHttp,
  tcpProbe,
  waitTcp,
  sleep,
  // 杀 / 起
  killPid,
  spawnDetached,
  rotate,
}
