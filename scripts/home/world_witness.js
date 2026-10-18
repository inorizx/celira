'use strict'
/**
 * **世界那一份的见证** —— 停世界时记一份摘要，起世界**之前**核一次。
 *
 * ## 它回答的唯一问题
 *
 * **世界不在场的那段时间里，有没有人动过它。**
 *
 * ⚠ 不是"它有没有变过"：世界跑着的时候，任何一句话都会改它 —— 那是正常的，不该被报。
 * 所以见证只在一个窗口里成立：**上一次干净收场那一刻** ↔ **下一次开机之前**。那个窗口里
 * 世界没在跑，因此里面发生的任何改动都不该发生。
 *
 * ## 它抓的是「改动」，不是「篡改」（这一条必须说出口）
 *
 * 同机同用户 ⇒ 她够得着 `<家>/im/core.db`，也够得着这份见证本身。⇒ **本地见证对拥有整机
 * 全权的主体不是封印**：蓄意的伪造可以连见证一起重写。
 *
 * 它的价值在另一处：**无意的改动**（脚本、工具、迁移、误操作）是最常见的形状，而它们会在
 * 这里显形。⇒ 它是**改动的可见性**（判据 17 的镜像），**不是密码学保证**。不许把它说成后者。
 *
 * ## 它不拒绝任何东西
 *
 * 本层只**如实报一行**。要不要因此拒绝开机，是另一次取舍（不在本文件）。
 * 同理：报出「变了」**不等于**报出「被篡改了」—— 本层只报事实，不判性质。
 *
 * ## 形状照她自己那本账（不发明新东西）
 *
 * 只追加 ＋ **链值**：每条记录带着上一条的摘要 ⇒ **删掉中间一条会被发现**。
 * 与 `trace.parts.idx` 的链值同一个形状、同一条理由（「账只追加，版面可重写」）。
 *
 * ## 见证住在哪：`<家>/logs/`，**不进环境根**
 *
 * 环境根会跟着 Agent 换机器（「一支 Agent 换机器 ＝ 把它的环境根搬过去」），而见证是
 * **这台机器上的编排事实** —— 与 pid 文件、宿主日志同族。搬走的档案里不该夹着另一台
 * 机器上的见证。（同一条判据见 `paths.js` 的 `agentLogDir`。）
 */

const crypto = require('node:crypto')
const fs = require('node:fs')
const path = require('node:path')

/** 见证账本的文件名（住在 `<家>/logs/`）。 */
const WITNESS_NAME = 'world-witness.ndjson'

/** 世界的事实住哪（与 `backup.js` / `tools/home_portability.py` 同一处口径）。 */
const DB_REL = path.join('im', 'core.db')
const MEDIA_REL = path.join('im', 'media', 'attachments')

/**
 * **上一次没干净停机的标志**（SQLite 两种日志模式都覆盖）。
 *
 * ⚠ 这条判据的**权威在 `backup.js`**（`JOURNAL_SUFFIXES`，与 `tools/home_portability.py` 的
 * `db_unclean_stop` 同源），这里**跟着走**：三处必须逐字相同，改一处就要同时改另两处。
 */
const DB_SIDECARS = ['-wal', '-shm', '-journal']

/** 记录里**恰好**这些键 —— 认不得的形状一律拒绝（fail-closed，与仓库里另两本账同规）。 */
const RECORD_KEYS = [
  'at-ms',
  'reason',
  'home',
  'core-size',
  'core-sha256',
  'media-count',
  'media-bytes',
  'prev',
]

/** 今天只有一种来路。**不预建**第二种（没有消费者的枚举不写）。 */
const REASON_CLEAN_STOP = 'clean-stop'

/** 见证账本的路径。 */
function witnessPath(homeDir) {
  return path.join(homeDir, 'logs', WITNESS_NAME)
}

/** 世界那两份事实的路径。 */
function dbPath(homeDir) {
  return path.join(homeDir, DB_REL)
}
function mediaPath(homeDir) {
  return path.join(homeDir, MEDIA_REL)
}

// ---------------------------------------------------------------------------
// 摘要：算一份"事实长什么样"
// ---------------------------------------------------------------------------

/** 一个文件的 `{size, sha256}`；读不出来 ⇒ `null`（**判不了 ≠ 没变**）。 */
function digestFile(p) {
  try {
    const bytes = fs.readFileSync(p)
    return { size: bytes.length, sha256: crypto.createHash('sha256').update(bytes).digest('hex') }
  } catch {
    return null
  }
}

/**
 * 附件目录的 `{count, bytes}` —— **只数数，不逐个哈希**。
 *
 * 为什么不到 sha256：附件是世界的**大件**（真机上是几十 MB 量级），逐个哈希会把"关机"
 * 变成一个随存量线性增长的动作 —— 而这一层的目的只是"世界不在场时有没有人动过它"。
 * 计数与总字节变了就一定有人动过；没变也不排除有人动过（如实说，不假装它是哈希）。
 *
 * ⚠ **缺省不等于零**（与 `skills_desktop.rs` 同一条纪律）：目录**不存在** ⇒ 那是
 * "世界还没有过附件"的合法形状 ⇒ `{count:0, bytes:0}`；目录**在但读不出来** ⇒ `null`
 * （判不了）。两者混成一档，会让"有人删掉了整份附件"看起来像"本来就没有"。
 */
function summarizeMedia(dir) {
  let count = 0
  let bytes = 0
  let names
  try {
    names = fs.readdirSync(dir)
  } catch (e) {
    if (e && e.code === 'ENOENT') return { count: 0, bytes: 0 }
    return null
  }
  for (const name of names) {
    try {
      const st = fs.statSync(path.join(dir, name))
      if (!st.isFile()) continue
      count += 1
      bytes += st.size
    } catch {
      return null
    }
  }
  return { count, bytes }
}

/** 此刻世界那两份事实的摘要；任一份读不出来 ⇒ `null`。 */
function snapshot(homeDir) {
  const core = digestFile(dbPath(homeDir))
  if (!core) return null
  const media = summarizeMedia(mediaPath(homeDir))
  if (!media) return null
  return { core, media }
}

/**
 * `core.db` 旁还留着日志兄弟吗？留着 ⇒ **上一次不是干净停的**。
 *
 * 为什么记基线之前必须问这一句：`home.js down` 走的是**强杀**，而强杀留下的库不是"那一刻
 * 的事实"（可能停在一半）。拿它当基线会造一个**假基准** —— 下一次开机就会报"有人动过它"，
 * 而动的其实是它自己没写完。⇒ **记不下就是记不下**，不许拿一个"可能"当基准。
 */
function isCleanlyStopped(homeDir) {
  return !DB_SIDECARS.some((suffix) => fs.existsSync(`${dbPath(homeDir)}${suffix}`))
}

// ---------------------------------------------------------------------------
// 记录：造、编码、解码（手写，认不得就拒绝）
// ---------------------------------------------------------------------------

/** 一条记录（**纯函数**：喂摘要就够 ⇒ 可机械判错）。 */
function makeRecord({ atMs, home = null, core, media, prev }) {
  if (!Number.isInteger(atMs) || atMs < 0) throw new Error('world-witness: at-ms 要是非负整数')
  if (home !== null && (typeof home !== 'string' || home === '')) {
    throw new Error('world-witness: home 要么是 null，要么是非空字符串')
  }
  if (!core || typeof core.sha256 !== 'string' || core.sha256.length !== 64) {
    throw new Error('world-witness: core-sha256 要是 64 位十六进制')
  }
  if (!Number.isInteger(core.size) || core.size < 0) throw new Error('world-witness: core-size 要是非负整数')
  if (!media || !Number.isInteger(media.count) || !Number.isInteger(media.bytes)) {
    throw new Error('world-witness: media 要是 {count, bytes}')
  }
  if (prev !== null && (typeof prev !== 'string' || prev.length !== 64)) {
    throw new Error('world-witness: prev 要么是 null（链的起点），要么是 64 位十六进制')
  }
  return {
    'at-ms': atMs,
    reason: REASON_CLEAN_STOP,
    home,
    'core-size': core.size,
    'core-sha256': core.sha256,
    'media-count': media.count,
    'media-bytes': media.bytes,
    prev,
  }
}

/** 一行（JSON ＋ 换行）。链值是**记录里的 `prev` 字段**，不是行尾附加物。 */
function encode(record) {
  return `${JSON.stringify(record)}\n`
}

/** 一条记录的摘要 —— 下一条的 `prev` 就是它。 */
function digestRecord(record) {
  return crypto.createHash('sha256').update(JSON.stringify(record)).digest('hex')
}

/** 解码一行；形状认不得 ⇒ **明确拒绝**（不猜、不降级）。 */
function decode(line) {
  let rec
  try {
    rec = JSON.parse(line)
  } catch (e) {
    throw new Error(`world-witness: 这一行不是 JSON（${e.message}）`)
  }
  if (!rec || typeof rec !== 'object' || Array.isArray(rec)) {
    throw new Error('world-witness: 这一行不是对象')
  }
  const keys = Object.keys(rec).sort()
  if (keys.length !== RECORD_KEYS.length || keys.some((k, i) => k !== [...RECORD_KEYS].sort()[i])) {
    throw new Error(`world-witness: 认不得的键（${keys.join(',')}）`)
  }
  if (rec.reason !== REASON_CLEAN_STOP) throw new Error(`world-witness: 认不得的 reason（${rec.reason}）`)
  return makeRecord({
    atMs: rec['at-ms'],
    home: rec.home,
    core: { size: rec['core-size'], sha256: rec['core-sha256'] },
    media: { count: rec['media-count'], bytes: rec['media-bytes'] },
    prev: rec.prev,
  })
}

// ---------------------------------------------------------------------------
// 读：整本读回来（读不出来 = 判不了，与"没有见证"分开）
// ---------------------------------------------------------------------------

/** 读回全部记录。文件不存在 ⇒ `[]`（**第一次**，不是"坏了"）。 */
function readRecords(homeDir) {
  const p = witnessPath(homeDir)
  let text
  try {
    text = fs.readFileSync(p, 'utf8')
  } catch {
    return []
  }
  const out = []
  for (const line of text.split('\n')) {
    if (line === '') continue
    out.push(decode(line))
  }
  return out
}

/** 链的第一个断点（`null` = 链完整）。**删掉中间一条 ⇒ 断点就在那里。** */
function chainBreak(records) {
  for (let i = 0; i < records.length; i += 1) {
    const expect = i === 0 ? null : digestRecord(records[i - 1])
    if (records[i].prev !== expect) return { index: i, expected: expect, found: records[i].prev }
  }
  return null
}

// ---------------------------------------------------------------------------
// 判：四条读数（判不了必须与"没改过"分开 —— 与 `world.js` 的三态、`proc.ledgerPid` 的四态同规）
// ---------------------------------------------------------------------------

/**
 * 拿"上一次干净收场那一刻"与"此刻"比。
 *
 * ⚠ **三件事必须分开**（与"缺省不等于零"同一条纪律）：
 * - **世界还没建过**（`core.db` 不存在、也没有上一条见证）⇒ `no-witness`（第一次）；
 * - **世界不见了**（`core.db` 不存在，**而上一条见证说它还在**）⇒ `changed`（这是大事，不是"判不了"）；
 * - **读不出来**（在，但读不动）⇒ `unreadable`。
 *
 * @param {{last: object|null, current: {core:object, media:object}|null,
 *          home?: string|null, coreExists?: boolean}} input
 * @returns {{state:'same'|'changed'|'no-witness'|'unreadable'|'other-home', detail:string|null}}
 */
function classify({ last, current, home = null, coreExists = true }) {
  if (!current && !coreExists) {
    if (last === null) return { state: 'no-witness', detail: null }
    return { state: 'changed', detail: 'core-missing' }
  }
  if (!current) return { state: 'unreadable', detail: 'core-or-media' }
  if (!last) return { state: 'no-witness', detail: null }
  // **两个家共用一份见证 ⇒ 误报**（与"两个家共用一组端口"同一个形状的爆炸半径）。
  // 记着别的家 id ⇒ 这一条不是这个家的 ⇒ 判不了，而且要点名它记的是谁。
  if (home !== null && last.home !== null && last.home !== home) {
    return { state: 'other-home', detail: last.home }
  }
  if (last['core-sha256'] !== current.core.sha256 || last['core-size'] !== current.core.size) {
    return { state: 'changed', detail: 'core', delta: current.core.size - last['core-size'] }
  }
  if (last['media-count'] !== current.media.count || last['media-bytes'] !== current.media.bytes) {
    return { state: 'changed', detail: 'media', delta: current.media.bytes - last['media-bytes'] }
  }
  return { state: 'same', detail: null }
}

/** 一句人话（**只说事实，不判性质**）。 */
function describe(result) {
  switch (result.state) {
    case 'same':
      return '世界的事实与上一次干净收场时逐字节相同'
    case 'changed': {
      if (result.detail === 'core-missing') {
        return '世界那一份**不在了**（上一次收场时它还在）——世界不在场的那段时间里有人动过它'
      }
      const sign = typeof result.delta === 'number' && result.delta >= 0 ? '+' : ''
      const size = typeof result.delta === 'number' ? `，${sign}${result.delta} 字节` : ''
      return `世界的事实与上一次干净收场时**不一样**（${result.detail}${size}）——世界不在场的那段时间里有人动过它`
    }
    case 'no-witness':
      return '没有上一条见证 ⇒ **判不了**（不是"没改过"）'
    case 'other-home':
      return `这份见证**不是这个家的**（它记的是 ${result.detail}）⇒ **判不了**（不是"没改过"）`
    default:
      return '此刻的事实读不出来 ⇒ **判不了**（不是"没改过"）'
  }
}

// ---------------------------------------------------------------------------
// 写：只在**干净收场**之后追加一条
// ---------------------------------------------------------------------------

/**
 * 记一条（追加，绝不改写已有的）。
 *
 * @param {string} homeDir
 * @param {{atMs?: number, homeId?: string|null}} options `homeId` = 这个家的 id（`home.conf`
 *        里的 `home_id`）。**记下来是为了让"两个家共用一份见证"当场显形** —— 与
 *        "两个家共用一组端口"同一个形状的爆炸半径（见 `world.js` 头注释）。
 * @returns {{record: object, path: string}|null} 事实读不出来 ⇒ `null`（不写半条）；
 *          **上一次不是干净停的** ⇒ 也 `null`（见 [`isCleanlyStopped`]：强杀留下的库不是
 *          "那一刻的事实"，拿它当基线会造一个假基准）
 */
function recordCleanStop(homeDir, { atMs = Date.now(), homeId = null } = {}) {
  if (!isCleanlyStopped(homeDir)) return null
  const current = snapshot(homeDir)
  if (!current) return null
  const records = readRecords(homeDir)
  const prev = records.length === 0 ? null : digestRecord(records[records.length - 1])
  const record = makeRecord({ atMs, home: homeId, core: current.core, media: current.media, prev })
  const p = witnessPath(homeDir)
  fs.mkdirSync(path.dirname(p), { recursive: true })
  fs.appendFileSync(p, encode(record))
  return { record, path: p }
}

/**
 * 起世界**之前**核一次（**只读**：本函数不写任何东西）。
 *
 * 顺序是承重的：**先核，后起**。世界一旦跑起来，任何一句话都会改它，那个窗口就没了。
 *
 * @param {string} homeDir
 * @param {{homeId?: string|null}} options 给了 `homeId` ⇒ 顺带判"这份见证是不是这个家的"
 */
function checkBeforeStart(homeDir, { homeId = null } = {}) {
  const records = readRecords(homeDir)
  const brk = chainBreak(records)
  const last = records.length === 0 ? null : records[records.length - 1]
  const result = classify({
    last,
    current: snapshot(homeDir),
    home: homeId,
    coreExists: fs.existsSync(dbPath(homeDir)),
  })
  return { ...result, records: records.length, chain: brk, text: describe(result) }
}

module.exports = {
  WITNESS_NAME,
  DB_REL,
  MEDIA_REL,
  DB_SIDECARS,
  REASON_CLEAN_STOP,
  RECORD_KEYS,
  witnessPath,
  dbPath,
  mediaPath,
  digestFile,
  summarizeMedia,
  snapshot,
  isCleanlyStopped,
  makeRecord,
  encode,
  digestRecord,
  decode,
  readRecords,
  chainBreak,
  classify,
  describe,
  recordCleanStop,
  checkBeforeStart,
}
