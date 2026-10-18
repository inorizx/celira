'use strict'
/**
 * **模型配置那两份文件的见证** —— 门改过它们，就记一条；读的时候拿"此刻"与"最后一条"比。
 *
 * ## 它回答的唯一问题
 *
 * **这一份最后是「从页面改的」吗？如果是，什么时候改的？**
 *
 * ⚠ 它**答不出"是谁"**：答得出的是**两档**，而且**两档必须分开说**——
 *
 * | 档 | 它知道什么 |
 * |---|---|
 * | **从页面那条路改的** | **有时刻**（写的那一刻就在门上，记下了） |
 * | **别处改的**（她走 terminal 写、直接改文件、别的工具） | **只知"变了"，不知是谁** |
 *
 * ⇒ 这一层的判据一句话：**它答的是「页面有没有写过」，不是「有没有人写过」。**
 *
 * ## 缝收在**门上**，不在 `writeAtomic` 上（这一条是承重的）
 *
 * `model-sources.js` 的 `writeAtomic` 也承载**不是页面**的写：`home.js up` 的迁移
 * （`migrateIfNeeded`）与布置时的绑定携带。在那一个函数上记账，会把**布置**说成
 * **"页面改的"** —— 而这一刀存在的全部理由就是不许把这两件事混成一件。
 *
 * ⇒ 记账的调用点只有两处：**这道门**（`config-server.js`）与**壳那道门**
 * （`app/src-tauri/src/home_doors.rs`）。存储层的形状与签名**一个字节不动**。
 *
 * ## 它与 `world_witness.js` 的关系：**形状照抄，不复用**
 *
 * 同一个形状（只追加 ＋ 链值 ＋ 四态 ＋ fail-closed 解码），但**不是同一个模块**：
 * `world_witness` 焊死在世界上（`im/core.db`、`im/media`、`clean-stop`），把它泛化
 * 要重构一个已提交已测的模块，还会把世界的公共事实与这台机器的配置混成一件事。
 *
 * ## 链值算在**一条规范行**上，不算在 JSON 序列化上
 *
 * 这本账有**两个写者、两种语言**（JS 的门与壳的门）。链值若算在各自的 `JSON.stringify`
 * 上，两边的转义必须逐字节相同 —— 那是一个没人守得住的约定。
 * ⇒ 链值 = `sha256(utf8("<at-ms>\n<file>\n<size>\n<sha256|''>\n<prev|''>\n"))`：
 * 一条显式的规范行，两边各自实现它，且两边钉**同一个测试向量**。
 *
 * ## 它抓的是「改动」，不是「篡改」（这一条必须说出口）
 *
 * 同机同用户 ⇒ 她够得着那两份文件，也够得着这本账。⇒ **本地见证对拥有整机全权的主体
 * 不是封印**：蓄意的伪造可以连账一起重写。它的价值在另一处：**无意的改动**（脚本、工具、
 * 误操作）会在.这里显形。⇒ 它是**改动的可见性**，**不是密码学保证**。不许把它说成后者。
 *
 * ## 它不拒绝任何东西，也**绝不**让一笔写失败
 *
 * 记不下就**如实报一行**（调用方打 `console.error`），那一笔配置写**照旧成功** ——
 * 拿一条日志去否决用户刚点的那次保存，是比"少一条见证"更坏的事。
 * 要不要因为"有人动过它"做点什么，是**另一次取舍**（不在本文件）。
 */

const crypto = require('node:crypto')
const fs = require('node:fs')
const path = require('node:path')

/** 见证账本的文件名（住在 `<家>/logs/`，与 `world-witness.ndjson` 同族）。 */
const WITNESS_NAME = 'model-witness.ndjson'

/** 记录里**恰好**这些键 —— 认不得的形状一律拒绝（fail-closed，与仓库里另两本账同规）。 */
const RECORD_KEYS = ['at-ms', 'file', 'prev', 'sha256', 'size']

/** 四态（与 `world_witness.js` 同一组词：判不了必须与"没改过"分开）。 */
const STATES = ['same', 'changed', 'no-witness', 'unreadable']

/** 见证账本的路径。 */
function witnessPath(homeDir) {
  return path.join(homeDir, 'logs', WITNESS_NAME)
}

/**
 * **哪一份** —— 相对家的路径，`/` 分隔，**可打印 ASCII**。
 *
 * 为什么是相对路径而不是绝对路径：这本账住在家里，家里那两份文件的位置在家内是固定的
 * （`model_sources.json` / `agents/<id>/model_binding.json`）；写绝对路径等于把一台机器的
 * 目录形状钉进账里，而"家"本身是可以整份搬走的。
 *
 * 为什么要求 ASCII：两个写者两种语言，这一格要能**逐字节**对得上（见文件头那条规范行）。
 * 说不成（路径在家外 / 含非 ASCII）⇒ `null` ⇒ **这一笔不记**（fail-closed，不猜一个名字）。
 */
function relativeFile(homeDir, file) {
  const rel = path.relative(homeDir, file)
  if (rel === '' || path.isAbsolute(rel)) return null
  if (rel === '..' || rel.startsWith(`..${path.sep}`)) return null
  const normalized = rel.split(path.sep).join('/')
  if (!/^[\x20-\x7e]+$/.test(normalized)) return null
  return normalized
}

// ---------------------------------------------------------------------------
// 记录：造、编码、解码（手写，认不得就拒绝）
// ---------------------------------------------------------------------------

/** 一条记录（**纯函数**：喂摘要就够 ⇒ 可机械判错）。 */
function makeRecord({ atMs, file, size, sha256, prev }) {
  if (!Number.isInteger(atMs) || atMs < 0) throw new Error('model-witness: at-ms 要是非负整数')
  if (typeof file !== 'string' || !/^[\x20-\x7e]+$/.test(file)) {
    throw new Error('model-witness: file 要是可打印 ASCII 的相对路径')
  }
  if (!Number.isInteger(size) || size < 0) throw new Error('model-witness: size 要是非负整数')
  // `sha256 === null` 是一个**真答案**：那一刻这一份**不在**（页面摘掉了它）。
  if (sha256 !== null && (typeof sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(sha256))) {
    throw new Error('model-witness: sha256 要么是 64 位十六进制，要么是 null（那一刻这一份不在）')
  }
  if (prev !== null && (typeof prev !== 'string' || !/^[0-9a-f]{64}$/.test(prev))) {
    throw new Error('model-witness: prev 要么是 null（链的起点），要么是 64 位十六进制')
  }
  return { 'at-ms': atMs, file, size, sha256, prev }
}

/** 一行（JSON ＋ 换行）。链值是**记录里的 `prev` 字段**，不是行尾附加物。 */
function encode(record) {
  return `${JSON.stringify(record)}\n`
}

/**
 * 链值算在**这条规范行**上（不是 `JSON.stringify`）。
 *
 * 形状逐字钉死：`<at-ms>\n<file>\n<size>\n<sha256|''>\n<prev|''>\n`。
 * 两个写者（JS / Rust）各自实现它，`tests/model-witness.test.js` 与壳的用例**钉同一个向量**。
 */
function canonicalLine(record) {
  return `${record['at-ms']}\n${record.file}\n${record.size}\n${record.sha256 ?? ''}\n${record.prev ?? ''}\n`
}

/** 一条记录的摘要 —— 下一条的 `prev` 就是它。 */
function digestRecord(record) {
  return crypto.createHash('sha256').update(canonicalLine(record), 'utf8').digest('hex')
}

/** 解码一行；形状认不得 ⇒ **明确拒绝**（不猜、不降级）。 */
function decode(line) {
  let rec
  try {
    rec = JSON.parse(line)
  } catch (e) {
    throw new Error(`model-witness: 这一行不是 JSON（${e.message}）`)
  }
  if (!rec || typeof rec !== 'object' || Array.isArray(rec)) {
    throw new Error('model-witness: 这一行不是对象')
  }
  const keys = Object.keys(rec).sort()
  if (keys.length !== RECORD_KEYS.length || keys.some((k, i) => k !== RECORD_KEYS[i])) {
    throw new Error(`model-witness: 认不得的键（${keys.join(',')}）`)
  }
  return makeRecord({
    atMs: rec['at-ms'],
    file: rec.file,
    size: rec.size,
    sha256: rec.sha256,
    prev: rec.prev,
  })
}

// ---------------------------------------------------------------------------
// 读：整本读回来（读不出来 = 判不了，与"没有见证"分开）
// ---------------------------------------------------------------------------

/**
 * 读回全部记录。文件不存在 ⇒ `[]`（**第一次**，不是"坏了"）。
 *
 * ⚠ **一行读不出来 ⇒ 抛**（调用方 `open` 把它收成"整本判不了"）：账是**证据**，
 * 一本读不全的账不能拿来回答"这一份有没有被人动过" —— 那正是"判不了"的形状。
 */
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

/**
 * 打开这本账（**只读**，一次读全）：`{ records, chain, last, broken }`。
 *
 * - `last`：`哪一份 ⇒ 最后一条记录`（`Map`）。**按份看**，不是按整本看；
 * - `chain`：链的第一个断点（`null` = 完整）；
 * - `broken`：账读不出来的原因（`null` = 读得出来）。**它与"账是空的"不是一回事**。
 */
function open(homeDir) {
  let records
  try {
    records = readRecords(homeDir)
  } catch (e) {
    return { records: [], chain: null, last: new Map(), broken: e.message }
  }
  const last = new Map()
  for (const record of records) last.set(record.file, record)
  return { records, chain: chainBreak(records), last, broken: null }
}

/** 一个文件的 `{size, sha256}`；读不出来 ⇒ `null`（**判不了 ≠ 没变**）。 */
function digestFile(file) {
  try {
    const bytes = fs.readFileSync(file)
    return { size: bytes.length, sha256: crypto.createHash('sha256').update(bytes).digest('hex') }
  } catch {
    return null
  }
}

// ---------------------------------------------------------------------------
// 判：四态（判不了必须与"没改过"分开 —— 与 `world_witness.js` 同规）
// ---------------------------------------------------------------------------

/**
 * 拿"这一份最后一次被门写过那一刻"与"此刻"比。
 *
 * ⚠ **三件事必须分开**：
 * - **没有上一条见证** ⇒ `no-witness`（判不了：它可能被别处改过，也可能从来没被门碰过）；
 * - **在，而内容对不上** ⇒ `changed`（这是大事，不是"判不了"）；
 * - **读不出来**（这一份读不出来 / 这本账读不出来）⇒ `unreadable`，`detail` 分开说是哪一半。
 *
 * @param {{last: Map<string, object>, broken: string|null}} ledger
 * @param {string} homeDir
 * @param {string} file 绝对路径
 * @returns {{state:'same'|'changed'|'no-witness'|'unreadable', atMs:number|null,
 *            detail:string|null, deltaBytes:number|null}}
 */
function readingFor(ledger, homeDir, file) {
  const none = { state: 'no-witness', atMs: null, detail: null, deltaBytes: null }
  if (ledger.broken !== null) {
    return { state: 'unreadable', atMs: null, detail: 'ledger', deltaBytes: null }
  }
  const rel = relativeFile(homeDir, file)
  // 说不成"哪一份"⇒ 查不到它的见证 ⇒ **判不了**（不是"没改过"）。
  if (rel === null) return none
  const last = ledger.last.get(rel) ?? null
  const exists = fs.existsSync(file)
  const current = exists ? digestFile(file) : null
  if (current === null) {
    if (!exists) {
      // 不在：见证记着"门写过它" ⇒ 有人拿走了；见证记着"门摘掉过它" ⇒ 与此刻一致。
      if (last === null) return none
      if (last.sha256 === null) return { state: 'same', atMs: last['at-ms'], detail: null, deltaBytes: null }
      return { state: 'changed', atMs: last['at-ms'], detail: 'missing', deltaBytes: null }
    }
    return { state: 'unreadable', atMs: last === null ? null : last['at-ms'], detail: 'file', deltaBytes: null }
  }
  if (last === null) return none
  if (last.sha256 === null) {
    // 见证记着"门摘掉过它"，而它现在在 ⇒ 有人放回来了。
    return { state: 'changed', atMs: last['at-ms'], detail: 'appeared', deltaBytes: null }
  }
  if (last.sha256 !== current.sha256) {
    return {
      state: 'changed',
      atMs: last['at-ms'],
      detail: 'content',
      deltaBytes: current.size - last.size,
    }
  }
  return { state: 'same', atMs: last['at-ms'], detail: null, deltaBytes: null }
}

// ---------------------------------------------------------------------------
// 写：只在门上记（**绝不抛** —— 记不下就如实回一个 reason）
// ---------------------------------------------------------------------------

/** 追加一条（调用方已经过形状判断）。 */
function append(homeDir, record) {
  const p = witnessPath(homeDir)
  fs.mkdirSync(path.dirname(p), { recursive: true })
  fs.appendFileSync(p, encode(record))
  return { ok: true, record }
}

/**
 * 记一条"**门写过这一份**"（那一刻它在，摘要就是它此刻的字节）。
 *
 * @returns {{ok:true, record:object}|{ok:false, reason:string}} **绝不抛**。
 */
function recordWrite(homeDir, file, atMs = Date.now()) {
  const rel = relativeFile(homeDir, file)
  if (rel === null) return { ok: false, reason: 'unnamed' }
  const digest = digestFile(file)
  if (digest === null) return { ok: false, reason: 'unreadable' }
  let ledger
  try {
    ledger = readRecords(homeDir)
  } catch (e) {
    return { ok: false, reason: `ledger:${e.message}` }
  }
  const prev = ledger.length === 0 ? null : digestRecord(ledger[ledger.length - 1])
  try {
    return append(homeDir, makeRecord({ atMs, file: rel, size: digest.size, sha256: digest.sha256, prev }))
  } catch (e) {
    return { ok: false, reason: `append:${e.message}` }
  }
}

/**
 * 记一条"**门摘掉了这一份**"（那一刻它不在 ⇒ `sha256: null`）。
 *
 * 为什么"清"也算一笔写：它决定她下一拍还醒不醒得过来（退回进程侧那一路）。不记它，
 * 页面上的"解除指定"就会在读数里变成"别处有人动过它" —— 把第一档报成第二档。
 */
function recordClear(homeDir, file, atMs = Date.now()) {
  const rel = relativeFile(homeDir, file)
  if (rel === null) return { ok: false, reason: 'unnamed' }
  let ledger
  try {
    ledger = readRecords(homeDir)
  } catch (e) {
    return { ok: false, reason: `ledger:${e.message}` }
  }
  const prev = ledger.length === 0 ? null : digestRecord(ledger[ledger.length - 1])
  try {
    return append(homeDir, makeRecord({ atMs, file: rel, size: 0, sha256: null, prev }))
  } catch (e) {
    return { ok: false, reason: `append:${e.message}` }
  }
}

module.exports = {
  WITNESS_NAME,
  RECORD_KEYS,
  STATES,
  witnessPath,
  relativeFile,
  makeRecord,
  encode,
  canonicalLine,
  digestRecord,
  decode,
  readRecords,
  chainBreak,
  open,
  digestFile,
  readingFor,
  recordWrite,
  recordClear,
}
