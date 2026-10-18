'use strict'
/**
 * `node home.js backup` —— **家的一次备份**（升级/回退那条生命线的另一半）。
 *
 * ## 为什么它必须是一个"带前置条件"的动作
 *
 * 回退唯一永远成立的办法是"升级前把家整个复制一份"（手册「升级与回退」）。而**"完整"只能由
 * 停机保证**：运行中复制出来的 SQLite 是"可能能用"的文件，不是"能用"的文件。所以这一层
 * 先验"没有任何一支在跑"，验不过就**拒绝备份**并说清怎么停 —— 不做"大概其"的复制。
 *
 * ## 备份的是**家**，不是"公共事实"
 *
 * 与 `tools/home_portability.py`（那是**公共事实**的离线备份/校验/恢复：`core.db` ＋ 媒体附件）
 * 分工明确、互不取代：
 *   · 那个工具**在仓库里**、有完整的校验与恢复链，管的是"世界的公共事实"；
 *   · 这一条**在包里**（`home.js` 是发布物的一部分），管的是"**整个家**"：她的环境与笔记、
 *     资料、家的配置 —— 恰恰是回退时最容易被忽略、又最不可再生的那一半。
 *
 * ## 一条必须说出口的事实
 *
 * 家里有 `<家>/model_sources.json`（模型清单，**含明文 api_key**；手册「模型配置」那一节
 * 写着）。⇒ **这份备份会带着模型 key**，要像对待 `env/` 一样对待它。清单本身只记
 * 路径/大小/sha256，**绝不含任何 key 原文**。
 *
 * ## 判据（与 `tools/home_portability.py` 的 `db_unclean_stop` 同一条）
 *
 * `core.db` 旁还留着 `-wal` / `-shm` / **`-journal`** ⇒ 上一次不是干净停的（本构建走 SQLite
 * 默认的 rollback journal，硬停留下的是 `-journal`）⇒ 拒绝备份：先开一次机让它自己收尾、
 * 再干净关一次。⚠ 这条判据在两处实现（Python 那份是权威、这里跟着走），因为两边不同语言；
 * **规则必须逐字相同**，改一处就要同时改另一处。
 */

const crypto = require('node:crypto')
const fs = require('node:fs')
const path = require('node:path')

const { EXIT } = require('./codes')
const P = require('./paths')
const { confGet, portOf } = require('./conf')
const proc = require('./proc')
const residents = require('./residents')

/** 备份清单（机器可校验；人读的摘要打在 stdout）。 */
const MANIFEST_NAME = 'BACKUP_MANIFEST.json'

/**
 * **不进备份的东西**：全是运行产物 —— 进程号、日志、写到一半的临时文件。
 *
 * 白名单 vs 黑名单：这里用**黑名单**（默认全备）。理由：备份漏掉一样东西的代价是
 * "回退之后发现少了点什么"，而多带上一份日志的代价只是几十 MB。家的形状会长（今天就有
 * `model_sources.json`、以后还可能有别的），**默认全备**才跟得上。
 */
const SKIP_DIRS = new Set(['logs'])
const SKIP_SUFFIXES = ['.pid']
const SKIP_NAMES = new Set([MANIFEST_NAME])

/** 上一次没干净停机的标志（三种后缀：SQLite 的两种日志模式都覆盖）。 */
const JOURNAL_SUFFIXES = ['-wal', '-shm', '-journal']

/** 默认目的地：家目录的**兄弟**（绝不放进家里 —— 那会自己套自己）。 */
function destinationFor(homeDir, now = new Date()) {
  const stamp = now.toISOString().replace(/[-:]/g, '').replace(/\..+$/, '').replace('T', '-')
  const base = path.basename(path.resolve(homeDir)) || 'celira-home'
  return path.join(path.dirname(path.resolve(homeDir)), `${base}-backup-${stamp}`)
}

/** 这个相对路径要不要跳过（纯函数 ⇒ 可判错）。 */
function shouldSkip(relative) {
  const parts = String(relative).split(/[\\/]/).filter(Boolean)
  if (parts.length === 0) return true
  if (parts.some((part) => SKIP_DIRS.has(part))) return true
  if (parts.some((part) => SKIP_NAMES.has(part))) return true
  const name = parts[parts.length - 1]
  return SKIP_SUFFIXES.some((suffix) => name.endsWith(suffix))
}

/** 家的库里还留着日志兄弟吗（留着一个就说明没干净停机）。 */
function uncleanStopMarker(dbPath) {
  for (const suffix of JOURNAL_SUFFIXES) {
    if (fs.existsSync(dbPath + suffix)) return suffix
  }
  return null
}

/** 枚举要备份的文件（相对路径 + 字节数），按字典序（清单确定性）。 */
function listFiles(homeDir) {
  const out = []
  const walk = (dir, prefix) => {
    let entries
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true })
    } catch (error) {
      if (error && error.code === 'ENOENT') return
      throw error
    }
    for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const rel = prefix ? `${prefix}/${entry.name}` : entry.name
      if (shouldSkip(rel)) continue
      const full = path.join(dir, entry.name)
      if (entry.isDirectory()) walk(full, rel)
      else if (entry.isFile()) out.push({ rel, size: fs.statSync(full).size })
      // 符号链接/其它一律不进：备份里出现链接，恢复时会指向别处（与工具同一纪律）。
    }
  }
  walk(homeDir, '')
  return out
}

function sha256File(file) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256')
    const stream = fs.createReadStream(file)
    stream.on('data', (chunk) => hash.update(chunk))
    stream.on('error', reject)
    stream.on('end', () => resolve(hash.digest('hex')))
  })
}

/** 复制一个文件并**双侧校验**（源与落地各算一次 sha256，且大小必须一致）。 */
async function copyVerified(from, to) {
  fs.mkdirSync(path.dirname(to), { recursive: true })
  await new Promise((resolve, reject) => {
    const read = fs.createReadStream(from)
    const write = fs.createWriteStream(to)
    read.on('error', reject)
    write.on('error', reject)
    write.on('close', resolve)
    read.pipe(write)
  })
  const [sourceHash, destHash] = [await sha256File(from), await sha256File(to)]
  const sourceSize = fs.statSync(from).size
  const destSize = fs.statSync(to).size
  if (sourceSize !== destSize || sourceHash !== destHash) {
    return { ok: false, reason: `${path.basename(from)} 复制后对不上（${sourceSize}→${destSize}）` }
  }
  return { ok: true, size: sourceSize, sha256: sourceHash }
}

/**
 * 解析 `node home.js backup [--to <目录>]` 的参数。
 *
 * ⚠ **`--to` 缺值一律是用法错**，绝不回落成默认目的地：`backup --to`（手滑/脚本里变量为空）
 * 会被读成"备份到家旁边那个默认目录"—— 那是一次**没人要的整份复制**（真机上踩到过：
 * 半成品落在磁盘根上）。缺值就报用法错，并说清正确形状。
 *
 * @returns {{ok: true, to?: string} | {ok: false, message: string}}
 */
function parseArgs(argv) {
  const args = Array.isArray(argv) ? argv : []
  let to
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i]
    if (arg !== '--to') return { ok: false, message: `认不得的参数：${arg}` }
    const value = args[i + 1]
    if (typeof value !== 'string' || value.trim() === '') {
      return { ok: false, message: '`--to` 后面要跟一个目录，例如： node home.js backup --to D:/celira-home-before-upgrade' }
    }
    // 以 `--` 开头的下一个词**当成"另一个旗标"**（`--to --force` 是笔误，不是"存到名叫
    // `--force` 的目录里去"；真要那么叫就写 `--to ./--force`）。
    if (value.trim().startsWith('--')) {
      return { ok: false, message: `\`--to\` 后面看起来是另一个旗标（${value.trim()}）—— 目的地要写一个目录` }
    }
    to = value.trim()
    i += 1
  }
  return to === undefined ? { ok: true } : { ok: true, to }
}

/**
 * 备份入口。
 *
 * @param {{to?: string}} options `--to` 给目的地（不给就用 `destinationFor`）
 * @returns {Promise<number>} 退出码（与家那一套同族：0 成／1 用法／2 环境／3 需要你动手）
 */
async function backup(options = {}) {
  if (options.to !== undefined && (typeof options.to !== 'string' || options.to.trim() === '')) {
    console.log('[backup] `--to` 后面要跟一个目录（不给就用 <家>-backup-<时间戳>）')
    return EXIT.USAGE
  }
  const homeDir = P.homeDir()
  const layout = P.detectLayout()
  if (!fs.existsSync(homeDir)) {
    console.log(`[backup] 没有这个家：${homeDir}`)
    return EXIT.ENV_UNAVAILABLE
  }
  const confFile = path.join(homeDir, 'home.conf')
  const logDir = path.join(homeDir, 'logs')
  const listen = confGet(confFile, 'listen_addr', P.DEFAULT_LISTEN)

  // ── 前置条件一：没有任何一支在跑（含"账本丢了但人还在"的孤儿）────────────────
  //
  // **一支一个宿主**（2026-10-01 多支刀）：`home` 的家事实带**每一支**的环境根（`envDirs`），
  // 账本逐支看（`logs/agents/<支>/host.pid`；还没住户化的老家那一支仍是 `logs/host.pid`）。
  const residentList = residents.runtimeResidents(homeDir, confFile)
  const home = {
    homeDir,
    envDirs: residentList.map((resident) => resident.envDir),
    clientPort: confGet(confFile, 'client_port', P.DEFAULT_CLIENT_PORT),
    imPort: portOf(listen),
    statusPort: confGet(confFile, 'status_port', P.DEFAULT_STATUS_PORT),
    configPort: confGet(confFile, 'config_port', P.DEFAULT_CONFIG_PORT),
    // **家的门**（2026-10-01 第六刀）：一个进程两个端面（读口 ＋ 写口）。
    doorsPorts: [
      confGet(confFile, 'status_port', P.DEFAULT_STATUS_PORT),
      confGet(confFile, 'config_port', P.DEFAULT_CONFIG_PORT),
    ],
    listening: proc.listenerSnapshot(),
  }
  const running = []
  for (const [name, pidFile] of [
    ['im', 'im.pid'],
    ['client', 'client.pid'],
    ['status', 'status.pid'],
    ['config', 'config.pid'],
  ]) {
    // 与 `down` 认领进程用的是同一套**家事实**（账本 pid ＋ `--root/--port/--home`）：备份
    // 判"还有没有在跑"必须与关机判"该停哪一支"同源，否则会出现"down 说停干净了、backup 说还在跑"。
    const pid = proc.trustedLedgerPid(name, path.join(logDir, pidFile), layout, home)
    if (pid !== null) running.push(`${name}（pid ${pid}）`)
  }
  for (const resident of residentList) {
    const scoped = { ...home, envDirs: [resident.envDir] }
    const pid = proc.trustedLedgerPid(
      'agent',
      residents.residentPidFile(homeDir, resident),
      layout,
      scoped,
    )
    if (pid !== null) running.push(`agent ${resident.agentId}（pid ${pid}）`)
  }
  // 账本可能被删/损坏而进程还在 ⇒ 再看一眼端口：im 的口上有监听就一定不算停机。
  if (proc.listenersOnPort(home.imPort).length > 0) {
    running.push(`端口 ${home.imPort} 上有一个世界在听`)
  }
  if (running.length > 0) {
    console.log(`[backup] 家里还有东西在跑：${running.join('、')}`)
    console.log('[backup] 运行中复制出来的库是"可能能用"的文件，不是"能用"的文件 —— 先关机： node home.js down')
    return EXIT.NEEDS_YOU
  }

  // ── 前置条件二：上一次是干净停的吗（与 tools/home_portability.py 同一条判据）──
  const dbPath = path.join(homeDir, 'im', 'core.db')
  const marker = uncleanStopMarker(dbPath)
  if (marker !== null) {
    console.log(`[backup] 上次不是干净停的（${path.basename(dbPath)}${marker} 还在）`)
    console.log('[backup] 先开一次机让它自己收尾（node home.js up），再干净关一次（node home.js down），然后备份')
    return EXIT.NEEDS_YOU
  }

  // ── 目的地 ────────────────────────────────────────────────────────────────
  const dest = path.resolve(options.to || destinationFor(homeDir))
  if (dest === path.resolve(homeDir) || dest.startsWith(path.resolve(homeDir) + path.sep)) {
    console.log(`[backup] 目的地不能在家里（会自己套自己）：${dest}`)
    return EXIT.USAGE
  }
  if (fs.existsSync(dest)) {
    console.log(`[backup] 目的地已经存在（绝不覆盖）：${dest}`)
    return EXIT.ENV_UNAVAILABLE
  }

  const files = listFiles(homeDir)
  console.log(`[backup] 家 = ${homeDir}`)
  console.log(`[backup] 目的地 = ${dest}（${files.length} 个文件）`)
  fs.mkdirSync(dest, { recursive: true })

  const entries = []
  let bytes = 0
  try {
    for (const file of files) {
      const done = await copyVerified(path.join(homeDir, file.rel), path.join(dest, file.rel))
      if (!done.ok) {
        console.log(`[backup] ✗ ${done.reason}`)
        return EXIT.PARTIAL
      }
      entries.push({ path: file.rel, size: done.size, sha256: done.sha256 })
      bytes += done.size
    }
    // 清单**最后**写：半成品没有清单，一眼看得出它不完整。
    const manifest = {
      version: 1,
      created_at: new Date().toISOString(),
      source_home: homeDir,
      files: entries,
      totals: { files: entries.length, bytes },
      excluded: { dirs: [...SKIP_DIRS], suffixes: SKIP_SUFFIXES },
      note: '整个家（im/ env/ materials/ 配置）；不含 logs/ 与进程号。可能含 <家>/model_sources.json（模型清单，含 key）。',
    }
    const temp = path.join(dest, `.${MANIFEST_NAME}.tmp`)
    fs.writeFileSync(temp, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')
    fs.renameSync(temp, path.join(dest, MANIFEST_NAME))
  } catch (error) {
    console.log(`[backup] ✗ 备份没做完：${error && error.message ? error.message : error}`)
    return EXIT.PARTIAL
  }

  console.log(`[backup] ✓ 备份完成：${entries.length} 个文件、${bytes} 字节，每个都过了双侧 sha256`)
  console.log(`[backup]   清单：${path.join(dest, MANIFEST_NAME)}`)
  console.log('[backup]   不含：logs/ 与进程号（运行产物）。含 model_sources.json ⇒ **这份备份带着模型 key**，像 env/ 一样保管它')
  console.log('[backup]   回退：node home.js down 之后，把这份内容放回家目录')
  return EXIT.OK
}

module.exports = {
  MANIFEST_NAME,
  SKIP_DIRS,
  SKIP_SUFFIXES,
  JOURNAL_SUFFIXES,
  parseArgs,
  destinationFor,
  shouldSkip,
  uncleanStopMarker,
  listFiles,
  copyVerified,
  backup,
}
