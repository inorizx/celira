/**
 * `api/storage.ts`（窄口 ＋ 转发门面 ＋ 搬家读）与 `api/shellStorage.ts`（壳里那份实现）的判据用例。
 *
 * 真机那一半（壳真的把文件写出来）在 app 侧 —— 这里钉的是**形状**：换实现之后读写走新实现、
 * 失败如实返回、认不出的形状不许说成成功、**搬家读的三条判据**（2026-10-08 存储收尾第二刀）。
 */

import test from 'node:test'
import assert from 'node:assert/strict'

import {
  browserStorage,
  currentStorage,
  legacyKeysUnder,
  legacyRemove,
  preferStoredRaw,
  readThrough,
  setStorage,
  storage,
  type StoragePort,
} from '../src/api/storage.ts'
import { shellStorage, type Invoke } from '../src/api/shellStorage.ts'

let cases = 0
function check(name: string, fn: () => void | Promise<void>) {
  test(name, async () => {
    cases += 1
    await fn()
  })
}

/** 一个记账用的假实现：把"谁被调了"记下来。 */
function fakePort(log: string[]): StoragePort {
  return {
    async read(key) {
      log.push(`read:${key}`)
      return `值:${key}`
    },
    async write(key, value) {
      log.push(`write:${key}=${value}`)
      return true
    },
    async remove(key) {
      log.push(`remove:${key}`)
    },
    async keys(prefix) {
      log.push(`keys:${prefix}`)
      return [`${prefix}a`]
    },
  }
}

check('换实现：装配处一换，调用方（`storage` 这个名字）读写全走新实现', async () => {
  const before = currentStorage()
  const log: string[] = []
  setStorage(fakePort(log))
  try {
    assert.equal(await storage.read('k'), '值:k')
    assert.equal(await storage.write('k', 'v'), true)
    await storage.remove('k')
    assert.deepEqual(await storage.keys('p:'), ['p:a'])
    assert.deepEqual(log, ['read:k', 'write:k=v', 'remove:k', 'keys:p:'])
  } finally {
    setStorage(before)
  }
  // 换回来之后照旧是原来那一份（默认是浏览器实现）。
  assert.equal(currentStorage(), before)
})

check('浏览器实现：`localStorage` 缺席时读不到、写忽略，**不抛**', async () => {
  const port = browserStorage(undefined as unknown as Storage)
  assert.equal(await port.read('k'), null)
  assert.equal(await port.write('k', 'v'), false)
  await port.remove('k')
  assert.deepEqual(await port.keys('p:'), [])
})

check('壳里那份：四个动作各叫一条命令，参数照窄口的形状', async () => {
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
  const invoke: Invoke = async (command, args) => {
    calls.push({ command, args })
    if (command === 'local_store_read') return '存着的值'
    if (command === 'local_store_write') return true
    if (command === 'local_store_keys') return ['room:a', 'room:b']
    return undefined
  }
  const port = shellStorage(invoke)
  assert.equal(await port.read('theme'), '存着的值')
  assert.equal(await port.write('theme', 'dark'), true)
  await port.remove('theme')
  assert.deepEqual(await port.keys('room:'), ['room:a', 'room:b'])
  assert.deepEqual(calls, [
    { command: 'local_store_read', args: { key: 'theme' } },
    { command: 'local_store_write', args: { key: 'theme', value: 'dark' } },
    { command: 'local_store_remove', args: { key: 'theme' } },
    { command: 'local_store_keys', args: { prefix: 'room:' } },
  ])
})

check('壳里那份：叫不动壳（抛）时**如实返回**，一次都不许崩', async () => {
  const broken: Invoke = async () => {
    throw new Error('壳不在')
  }
  const port = shellStorage(broken)
  assert.equal(await port.read('k'), null, '读不到 ⇒ null')
  assert.equal(await port.write('k', 'v'), false, '写失败 ⇒ false')
  await port.remove('k')
  assert.deepEqual(await port.keys('p:'), [])
})

check('★ 认不出的形状不许说成成功/有值（缺省不等于事实）', async () => {
  const weird: Invoke = async (command) => {
    if (command === 'local_store_read') return { 值: '对象不是字符串' } as unknown as string
    if (command === 'local_store_write') return 'true' as unknown as boolean
    if (command === 'local_store_keys') return 'room:a' as unknown as string[]
    return undefined
  }
  const port = shellStorage(weird)
  assert.equal(await port.read('k'), null, '回的不是字符串 ⇒ 当读不到')
  assert.equal(await port.write('k', 'v'), false, '回的严格来说不是 true ⇒ 当没写进去')
  assert.deepEqual(await port.keys('p:'), [], '回的不是数组 ⇒ 当没有（与浏览器实现同一处置）')
  // `keys` 里混进非字符串项 ⇒ 只留字符串（不许把认不出的东西当键名交给清理逻辑）。
  const mixed: Invoke = async () => ['ok', 42, null] as unknown as string[]
  assert.deepEqual(await shellStorage(mixed).keys('p:'), ['ok'])
})

// ── 搬家读（老位置 ⇒ 窄口，2026-10-08 存储收尾第二刀）──────────────────────
//
// 这一组是"用户升上来值还在不在"的判据本体。三句话：
//   ① 窄口有值 ⇒ 用它，**一个字都不问老位置**（老位置只是历史残留，不是第二个权威）；
//   ② 窄口空、老位置有 ⇒ 用老那份，**并搬进窄口**（下次它就有值了）；
//   ③ 两处都空 ⇒ 没有值（首访）。

check('★ 判据：窄口有值 ⇒ 用它，且**不迁移**（老位置有值也不搬）', () => {
  assert.deepEqual(preferStoredRaw('新', '老'), { raw: '新', migrate: false })
})

check('★ 判据：窄口空、老位置有值 ⇒ 用老那份**并迁移**（用户不该因为换存储丢东西）', () => {
  assert.deepEqual(preferStoredRaw(null, '老'), { raw: '老', migrate: true })
})

check('★ 判据："读到空"与"读不到"分开：窄口回空串也算**有值**（不许拿老值盖掉它）', () => {
  assert.deepEqual(preferStoredRaw('', '老'), { raw: '', migrate: false })
})

check('判据：两处都空 ⇒ 没有值、不迁移（首访）', () => {
  assert.deepEqual(preferStoredRaw(null, null), { raw: null, migrate: false })
})

/** 一个记账用的假窄口：读回 `portRaw`，把每次写记下来。 */
function readThroughPort(portRaw: string | null, log: string[]): StoragePort {
  return {
    async read(key) {
      log.push(`read:${key}`)
      return portRaw
    },
    async write(key, value) {
      log.push(`write:${key}=${value}`)
      return true
    },
    async remove() {},
    async keys() {
      return []
    },
  }
}

/** 老位置（webview 的 `localStorage`）最小桩；返回一个"事后清干净"的收尾函数。 */
function stubLegacy(store: Record<string, string>): () => void {
  const had = 'localStorage' in globalThis
  const before = (globalThis as Record<string, unknown>).localStorage
  ;(globalThis as Record<string, unknown>).localStorage = {
    length: Object.keys(store).length,
    key: (i: number) => Object.keys(store)[i] ?? null,
    getItem: (k: string) => store[k] ?? null,
    removeItem: (k: string) => {
      delete store[k]
    },
  }
  return () => {
    if (had) (globalThis as Record<string, unknown>).localStorage = before
    else delete (globalThis as Record<string, unknown>).localStorage
  }
}

check('★ 搬家读：窄口有值 ⇒ 只读窄口一次，老位置一个字都不问、更不写', async () => {
  const log: string[] = []
  const before = currentStorage()
  const restore = stubLegacy({ 'k': '老' })
  setStorage(readThroughPort('新', log))
  try {
    assert.equal(await readThrough('k'), '新')
    assert.deepEqual(log, ['read:k'], '只该有那一次窄口读')
  } finally {
    setStorage(before)
    restore()
  }
})

check('★ 搬家读：窄口空、老位置有 ⇒ 返回老值**并把老值写进窄口**', async () => {
  const log: string[] = []
  const before = currentStorage()
  const restore = stubLegacy({ 'k': '老' })
  setStorage(readThroughPort(null, log))
  try {
    assert.equal(await readThrough('k'), '老')
    assert.deepEqual(log, ['read:k', 'write:k=老'], '老值该被搬进窄口')
  } finally {
    setStorage(before)
    restore()
  }
})

check('搬家读：两处都没有 ⇒ `null`，一次写都不发生（首访不留痕）', async () => {
  const log: string[] = []
  const before = currentStorage()
  const restore = stubLegacy({})
  setStorage(readThroughPort(null, log))
  try {
    assert.equal(await readThrough('k'), null)
    assert.deepEqual(log, ['read:k'])
  } finally {
    setStorage(before)
    restore()
  }
})

check('搬家读：老位置读不到（非浏览器 / 隐私模式）⇒ 当它没有，**不抛**', async () => {
  const log: string[] = []
  const before = currentStorage()
  const had = 'localStorage' in globalThis
  const old = (globalThis as Record<string, unknown>).localStorage
  ;(globalThis as Record<string, unknown>).localStorage = {
    get length(): number {
      throw new Error('隐私模式')
    },
    key: () => {
      throw new Error('隐私模式')
    },
    getItem: () => {
      throw new Error('隐私模式')
    },
    removeItem: () => {
      throw new Error('隐私模式')
    },
  }
  setStorage(readThroughPort(null, log))
  try {
    assert.equal(await readThrough('k'), null)
    assert.deepEqual(legacyKeysUnder('k'), [], '列不出来 ⇒ 空表，不抛')
    legacyRemove(['k']) // 不许抛
  } finally {
    setStorage(before)
    if (had) (globalThis as Record<string, unknown>).localStorage = old
    else delete (globalThis as Record<string, unknown>).localStorage
  }
})

check('老位置清理：按前缀列出并删掉（登出清残留用），别的键不碰', () => {
  const store: Record<string, string> = {
    'celira.world:h-1:cursor': 'a',
    'celira.world:h-2:cursor': 'b',
    'celira-theme': 'dark',
  }
  const restore = stubLegacy(store)
  try {
    const doomed = legacyKeysUnder('celira.world:h-1:')
    assert.deepEqual(doomed, ['celira.world:h-1:cursor'])
    legacyRemove(doomed)
    assert.deepEqual(Object.keys(store).sort(), ['celira-theme', 'celira.world:h-2:cursor'])
  } finally {
    restore()
  }
})

test('用例数 > 0（空跑守卫）', () => {
  assert.ok(cases > 0, '这一片一条都没跑 —— 那是空跑，不是通过')
})
