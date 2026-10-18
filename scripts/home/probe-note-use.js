#!/usr/bin/env node
'use strict'
/**
 * **判据 3 的真机读数**：她写下的字，后来被自己读过吗？
 *
 * ## 为什么要有它
 *
 * 判据 3（「**没被用过的存储不是记忆**」）到今天为止只是一句话 —— 没有任何一条能重跑的判据
 * 说得出"她写的东西到底有没有被用"。这一条把它变成一个**读数**：拿账本里的事实折出
 * 每条笔记的**创建时刻**，再看它**此后有没有被 `notes_read` 点名读过**。
 *
 * ## 它量什么（两个事实，都从账里折，不新写任何东西）
 *
 * 1. **创建时刻**：`notes_create` 的动作事实（`observed` 的 `created` 块带身份
 *    `note-<n>`，配 `action-selected` 的 `at-ms`）—— ⚠ **不能拿文件 mtime 冒充**：
 *    `notes_edit` / `notes_append` 会改写文件，mtime 是"最后动过"，不是"什么时候写的"。
 * 2. **此后有没有被点名读过**：`notes_read` 的**参数**里点到了哪个身份（`at-ms` 晚于创建）。
 *
 * ## 它**不**量什么（射程，必须说清）
 *
 * - 读数说的是「**从未被 `notes_read` 点名读过**」，**不是**"从未出现在她视野里" ——
 *   `notes_list` 会列身份与标题，她可能见过它。
 * - **不打印任何正文**：只有身份、时刻与计数。
 * - **不判"该不该删"**：判据 7 是「未用 → 可达性降级，**绝不** → 删」。那是裁决，不是探针的活。
 *
 * ## 用法
 *
 * ```text
 * node scripts/home/probe-note-use.js [--root <家目录>] [--agent <支>]
 * ```
 *
 * - 缺省读 `CELIRA_HOME_DIR` / `DEFAULT_HOME` 那个家（与 `home.js` 同一处取值）；
 * - 住户问权威那一支（`runtimeResidents` ＝ 新布局 ＋ 还没迁的老家）；一支都没有 ⇒ 不适用；
 *   要只看一支就用 `--agent <id>` 点名（**不猜**）。
 *
 * ## 退出码
 *
 * - **0** ＝ 算出来了；
 * - **2** ＝ **不适用**（没有住户 / 一支笔记都没有 / 账本还没建）—— 既不算通过也不算失败；
 * - **3** ＝ 账本在场但**读不出来**（整本一行都解析不了）；
 * - **1** ＝ 用法错。
 */

const fs = require('node:fs')
const path = require('node:path')

const P = require('./paths.js')
const { runtimeResidents } = require('./residents.js')

const EXIT = { OK: 0, USAGE: 1, NA: 2, UNREADABLE: 3 }
const PREFIX = '[probe-note-use]'

function parseArguments(argv) {
  const options = { root: null, agent: null }
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i]
    if (flag !== '--root' && flag !== '--agent') throw new Error(`认不得的选项 ${flag}`)
    const value = argv[i + 1]
    if (typeof value !== 'string' || value.trim() === '') throw new Error(`${flag} 后面要给一个非空取值`)
    const key = flag === '--root' ? 'root' : 'agent'
    if (options[key] !== null) throw new Error(`${flag} 给了不止一次`)
    options[key] = value.trim()
    i += 1
  }
  return options
}

/**
 * 读一本经历账本（活动段 ＋ 已封口的卷），按 `seq` 排好。
 *
 * **不静默跳过**：解析不了的行计数带回来（读数要能说"有多少行我没读懂"）。
 */
function readLedger(envDir) {
  const files = []
  const active = path.join(envDir, 'trace.ndjson')
  if (fs.existsSync(active)) files.push(active)
  const partsDir = path.join(envDir, 'trace.parts')
  if (fs.existsSync(partsDir)) {
    for (const name of fs.readdirSync(partsDir).sort()) {
      if (name.endsWith('.ndjson')) files.push(path.join(partsDir, name))
    }
  }
  const rows = []
  let unparsable = 0
  for (const file of files) {
    const text = fs.readFileSync(file, 'utf8')
    for (const line of text.split('\n')) {
      if (line === '') continue
      // 每一行是 `{json}\t<链值>`：链值不是 JSON 的一部分。
      const body = line.includes('\t') ? line.slice(0, line.lastIndexOf('\t')) : line
      try {
        rows.push(JSON.parse(body))
      } catch {
        unparsable += 1
      }
    }
  }
  rows.sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0))
  return { rows, unparsable, files: files.length }
}

/** 折出 `{ born, readsAfter, readTotal }`。**只碰身份与时刻，绝不碰正文。** */
function fold(rows) {
  const idsIn = (rec) => new Set(String(JSON.stringify(rec)).match(/note-\d+/g) || [])
  const selectedAt = new Map()
  for (const r of rows) {
    if (r.action === 'notes_create' && r.kind === 'action-selected') {
      selectedAt.set(`${r.run}|${r.turn}`, r['at-ms'])
    }
  }
  const born = new Map()
  for (const r of rows) {
    if (r.kind !== 'observed') continue
    const at = selectedAt.get(`${r.run}|${r.turn}`)
    if (at === undefined) continue
    for (const id of idsIn(r)) if (!born.has(id)) born.set(id, at)
  }
  const readTotal = new Map()
  const readsAfter = new Map()
  for (const r of rows) {
    if (r.action !== 'notes_read' || r.kind !== 'action-selected') continue
    const named = new Set(String(JSON.stringify(r.params || [])).match(/note-\d+/g) || [])
    for (const id of named) {
      readTotal.set(id, (readTotal.get(id) || 0) + 1)
      if (born.has(id) && typeof r['at-ms'] === 'number' && r['at-ms'] > born.get(id)) {
        readsAfter.set(id, (readsAfter.get(id) || 0) + 1)
      }
    }
  }
  return { born, readsAfter, readTotal }
}

const stamp = (ms) =>
  new Date(ms).toISOString().slice(5, 16).replace('T', ' ')

function reportResident(resident) {
  const notesDir = path.join(resident.envDir, 'notes')
  if (!fs.existsSync(notesDir)) {
    console.log(`${PREFIX}   ${resident.label}: 没有 notes/ ⇒ 不适用`)
    return { applicable: false, notes: 0 }
  }
  const onDisk = fs
    .readdirSync(notesDir)
    .filter((n) => n.endsWith('.note'))
    .map((n) => (n.match(/note-\d+/) || [null])[0])
    .filter(Boolean)
    .sort((a, b) => Number(a.split('-')[1]) - Number(b.split('-')[1]))
  if (onDisk.length === 0) {
    console.log(`${PREFIX}   ${resident.label}: notes/ 是空的 ⇒ 不适用`)
    return { applicable: false, notes: 0 }
  }

  const { rows, unparsable, files } = readLedger(resident.envDir)
  if (files === 0) {
    console.log(`${PREFIX}   ${resident.label}: 账本还没建 ⇒ 不适用`)
    return { applicable: false, notes: onDisk.length }
  }
  if (rows.length === 0) {
    console.log(`${PREFIX}   ${resident.label}: 账本在场但一行都读不出来（${unparsable} 行）⇒ 判不了`)
    return { applicable: false, notes: onDisk.length, unreadable: true }
  }

  const { born, readsAfter, readTotal } = fold(rows)
  const have = onDisk.filter((id) => born.has(id))
  const missing = onDisk.filter((id) => !born.has(id))
  const used = onDisk.filter((id) => (readsAfter.get(id) || 0) > 0)
  const never = onDisk.filter((id) => (readsAfter.get(id) || 0) === 0)
  const total = [...readTotal.values()].reduce((a, b) => a + b, 0)

  console.log(`${PREFIX}   ${resident.label}:`)
  console.log(`${PREFIX}     账本 ${files} 份 / ${rows.length} 条记录（读不懂 ${unparsable} 行）`)
  console.log(`${PREFIX}     笔记 ${onDisk.length} 条 | 折得到创建时刻 ${have.length} | 折不到 ${missing.length}`)
  if (have.length > 0) {
    const times = have.map((id) => born.get(id))
    console.log(`${PREFIX}     创建时刻 ${stamp(Math.min(...times))} -> ${stamp(Math.max(...times))}`)
  }
  const pct = Math.round((100 * used.length) / onDisk.length)
  console.log(
    `${PREFIX}     ★ 写下之后被 notes_read 点名读过至少一次：${used.length} / ${onDisk.length} = ${pct}%`
  )
  if (never.length > 0) {
    const window = never.filter((id) => born.has(id)).map((id) => born.get(id))
    const span =
      window.length > 1 ? `（创建于 ${stamp(Math.min(...window))} -> ${stamp(Math.max(...window))}）` : ''
    console.log(`${PREFIX}     从未被点名：${never.join(' ')} ${span}`)
  }
  if (total > 0) {
    console.log(
      `${PREFIX}     点名读总次数 ${total} | 平均每条 ${(total / readTotal.size).toFixed(1)}`
    )
  }
  console.log(
    `${PREFIX}     ⚠ 不判：读数说的是「从未被 notes_read 点名读过」，不是「从未出现在她视野里」` +
      `（notes_list 会列身份与标题）；也不判"该不该删"（判据 7：未用 → 可达性降级，绝不 → 删）`
  )
  return { applicable: true, notes: onDisk.length, used: used.length, never: never.length }
}

function main(argv) {
  let options
  try {
    options = parseArguments(argv)
  } catch (error) {
    console.error(`${PREFIX} ${error.message}`)
    console.error('用法: node scripts/home/probe-note-use.js [--root <家目录>] [--agent <支>]')
    return EXIT.USAGE
  }

  const homeDir = P.homeDir(options.root)
  const confFile = path.join(homeDir, 'home.conf')
  let list = runtimeResidents(homeDir, confFile)
  if (options.agent !== null) {
    list = list.filter((resident) => resident.agentId === options.agent)
    if (list.length === 0) {
      console.error(`${PREFIX} 这个家里没有住户 ${options.agent}`)
      return EXIT.NA
    }
  }

  console.log(`${PREFIX} 家 = ${homeDir}`)
  if (list.length === 0) {
    console.log(`${PREFIX} 这个家一支住户都没有 ⇒ 不适用（既不算通过也不算失败）`)
    return EXIT.NA
  }

  const results = list.map((resident) =>
    reportResident({
      ...resident,
      label: `${resident.displayName || '(无名)'} ${resident.agentId}`,
    })
  )
  if (results.some((r) => r.unreadable)) return EXIT.UNREADABLE
  if (!results.some((r) => r.applicable)) {
    console.log(`${PREFIX} 没有任何一支有可算的笔记 ⇒ 不适用`)
    return EXIT.NA
  }
  return EXIT.OK
}

if (require.main === module) {
  process.exitCode = main(process.argv.slice(2))
}

module.exports = { readLedger, fold, main, EXIT }
