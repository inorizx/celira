// **会话草稿**（`lib/messageDrafts.ts`）的判据用例（2026-10-08 存储收尾第二刀）。
//
// 这一格装的是**用户已经打了一半的话** —— 丢了它用户是看得见的（下次进那个会话框是空的）。
// 所以这里钉的不是"函数返回什么"，而是三件容易静默出错的事：
//   ① **宽容解析**：脏数据不许变成崩溃，也不许把好草稿一起丢掉；
//   ② **读改写不丢别人的**：落盘是整个 `{ [roomId]: body }` 表，一次过早的写会把盘上
//      别的会话的草稿整份覆盖掉（补水之前写，或两条写乱序，都会）；
//   ③ **同一个键的两次写按调用顺序落地**：窄口是异步的，后发的先落地 = 盘上留着上一版。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'

import { draftFor, hydrateDrafts, parseDrafts, serializeDrafts, setDraft } from '../src/lib/messageDrafts.ts'
import { currentStorage, setStorage, type StoragePort } from '../src/api/storage.ts'

const DRAFTS_KEY = 'celira_drafts'

/** 让在途的微任务与 `await` 链条跑完。 */
async function settle(turns = 6) {
  for (let i = 0; i < turns; i += 1) await new Promise((r) => setImmediate(r))
}

/** 真等一段时间（落地延迟是按毫秒造的，`setImmediate` 那几拍等不到它）。 */
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

// ── ① 宽容解析（纯函数）─────────────────────────────────────────────────────

test('parseDrafts：读不懂 / 空 / 非对象 / 数组 一律归空表，绝不抛', () => {
  for (const raw of [null, undefined, '', 'not json', '[]', '42', '"str"', 'null']) {
    assert.equal(parseDrafts(raw).size, 0, `输入 ${String(raw)} 该读成空表`)
  }
})

test('parseDrafts：只收非空字符串，别的（数字 / 空串 / null / 嵌套）一律丢掉', () => {
  const parsed = parseDrafts(JSON.stringify({ 'r-1': '在', 'r-2': '', 'r-3': 42, 'r-4': null, 'r-5': {} }))
  assert.deepEqual([...parsed.entries()], [['r-1', '在']])
})

test('serializeDrafts → parseDrafts 往返一致', () => {
  const drafts = new Map([
    ['r-1', '第一间'],
    ['r-2', '第二间'],
  ])
  assert.deepEqual([...parseDrafts(serializeDrafts(drafts)).entries()].sort(), [...drafts.entries()].sort())
})

// ── ②③ 补水与落盘（走窄口）─────────────────────────────────────────────────

function fakePort(store: Record<string, string>, log: string[], delays: number[] = []): StoragePort {
  let writes = 0
  return {
    async read(key) {
      log.push(`read:${key}`)
      return store[key] ?? null
    },
    async write(key, value) {
      const index = writes
      writes += 1
      // 头一次写的落地故意慢一点：乱序是最容易悄悄出错的那一种。
      const delay = delays[Math.min(index, delays.length - 1)] ?? 0
      if (delay > 0) await new Promise((r) => setTimeout(r, delay))
      store[key] = value
      log.push(`write:${key}`)
      return true
    },
    async remove(key) {
      delete store[key]
    },
    async keys(prefix) {
      return Object.keys(store).filter((k) => k.startsWith(prefix))
    },
  }
}

test('★ 补水：盘上那份进快照（切回那个会话，上次没发完的话就在框里）', async () => {
  const before = currentStorage()
  const store: Record<string, string> = { [DRAFTS_KEY]: JSON.stringify({ 'r-1': '没发完的话' }) }
  setStorage(fakePort(store, []))
  try {
    await hydrateDrafts()
    assert.equal(draftFor('r-1'), '没发完的话')
    assert.equal(draftFor('r-2'), '', '没有草稿的会话是空串（不是 undefined）')
  } finally {
    setStorage(before)
  }
})

test('★ 落盘是**读改写**：写一间不许把盘上别的会话的草稿覆盖掉（键名一字不改）', async () => {
  const before = currentStorage()
  const store: Record<string, string> = { [DRAFTS_KEY]: JSON.stringify({ 'r-1': '没发完的话' }) }
  setStorage(fakePort(store, []))
  try {
    setDraft('r-2', '第二间的话')
    await settle()
    assert.deepEqual(JSON.parse(store[DRAFTS_KEY]!), { 'r-1': '没发完的话', 'r-2': '第二间的话' })

    // 清空一间 = 删掉那一格（不是留个空串在盘上）
    setDraft('r-1', '')
    await settle()
    assert.deepEqual(JSON.parse(store[DRAFTS_KEY]!), { 'r-2': '第二间的话' })
    assert.equal(draftFor('r-1'), '')
  } finally {
    setStorage(before)
  }
})

test('★ 同一格的两次写按**调用顺序**落地（后发的先到 = 盘上留着上一版）', async () => {
  const before = currentStorage()
  const store: Record<string, string> = {}
  const log: string[] = []
  // 第一次写慢 20ms：没有那条写链的话，第二次（快）会先落地、然后被第一次覆盖回旧值。
  setStorage(fakePort(store, log, [20, 0]))
  try {
    setDraft('r-1', '第一版')
    await settle(1)
    setDraft('r-1', '第二版')
    await sleep(80)
    assert.equal(JSON.parse(store[DRAFTS_KEY]!)['r-1'], '第二版', '盘上该是最后那一版')
  } finally {
    setStorage(before)
  }
})

test('写不进去**不抛**（本次会话里草稿仍然由内存保证）', async () => {
  const before = currentStorage()
  const failing: StoragePort = {
    async read() {
      return null
    },
    async write() {
      return false
    },
    async remove() {},
    async keys() {
      return []
    },
  }
  setStorage(failing)
  try {
    setDraft('r-9', '写不进去也要留住')
    await settle()
    assert.equal(draftFor('r-9'), '写不进去也要留住')
  } finally {
    setStorage(before)
  }
})

test('★ 一次抛异常不许**毒死写链**（否则后面每一笔都静默不写）', async () => {
  const before = currentStorage()
  const store: Record<string, string> = {}
  const log: string[] = []
  let firstCall = true
  const throwing: StoragePort = {
    async read(key) {
      return store[key] ?? null
    },
    async write(key, value) {
      if (firstCall) {
        firstCall = false
        throw new Error('这一份实现违约抛了')
      }
      store[key] = value
      log.push(`write:${key}`)
      return true
    },
    async remove(key) {
      delete store[key]
    },
    async keys(prefix) {
      return Object.keys(store).filter((k) => k.startsWith(prefix))
    },
  }
  setStorage(throwing)
  try {
    setDraft('r-1', '第一笔会抛')
    await settle()
    setDraft('r-1', '第二笔必须照旧落盘')
    await settle()
    assert.deepEqual(log, [`write:${DRAFTS_KEY}`], '第二笔该照旧落盘（链条没被毒死）')
    assert.equal(JSON.parse(store[DRAFTS_KEY]!)['r-1'], '第二笔必须照旧落盘')
  } finally {
    setStorage(before)
  }
})
