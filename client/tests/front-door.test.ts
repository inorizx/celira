// 正门（前门翻正）行为测试：启动探测读数 + 持久偏好 ⇒ 第一屏该是什么。
//
// 运行：cd client && npm test
// 覆盖：有可复用的直连凭据 ⇒ 直接连；没有 ⇒ 登录页；
//       **★ TASK_63 收窄**：Host 门已撤 ⇒ probe 的任何读数（host /
//       not-authenticated / host-not-ready / unavailable / legacy）都不再改变
//       第一屏归属；"探不到 ≠ 不是 Host" 这条判据本身没动（detectHost 未改）；
//       prefs 新字段 serverUrl 的默认值让旧 localStorage 数据开箱可用；
//       服务器地址归一化与基址拼接（同源 = 一个字符都不变）。

import assert from 'node:assert/strict'
import test from 'node:test'
import { decideFrontDoor } from '../src/api/entry.ts'
import {
  DEFAULT_PREFS,
  normalizeServerUrl,
  parsePrefs,
  serializePrefs,
  serverUrlError,
} from '../src/api/prefs.ts'
import type { ConnectionPrefs } from '../src/api/prefs.ts'
import { joinBaseUrl, webSocketBase } from '../src/api/client.ts'

/**
 * 门禁不许空跑（与家里那条 `e2e` 纪律同源）：每条用例跑完就登记，最后一条断言
 * 本文件真的执行了用例。空跑看着绿，其实什么都没验。
 *
 * ⚠ TASK_64 起本文件少 3 条用例：`decideFrontDoor` 的 `probe` 参数随 host 模块
 * 一起拔了（裁决 3），那 3 条钉的是"probe 读数不改变第一屏归属"——探测本身已经
 * 不存在，它们没有可钉的对象了（不是被跳过，是随对象删除）。
 */
let completed = 0
const DECLARED_CASES = 14

function frontDoorCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

const prefsOf = (over: Partial<ConnectionPrefs> = {}): ConnectionPrefs => ({
  ...DEFAULT_PREFS,
  ...over,
})

frontDoorCase('首访（无凭据）→ 第一屏是登录页', () => {
  assert.deepEqual(decideFrontDoor(prefsOf()), { screen: 'login' })
})

frontDoorCase('已存直连凭据 → 自动直连，不经过登录页', () => {
  assert.deepEqual(decideFrontDoor(prefsOf({ mode: 'core', token: 't-1' })), {
    screen: 'core-auto',
  })
})

frontDoorCase('没有可用凭据（空 token / 只有空白）→ 登录页', () => {
  assert.deepEqual(decideFrontDoor(prefsOf({ mode: 'core', token: '' })), {
    screen: 'login',
  })
  // 只有空白 token 也不算凭据。
  assert.deepEqual(decideFrontDoor(prefsOf({ mode: 'core', token: '   ' })), {
    screen: 'login',
  })
})

frontDoorCase('旧 localStorage 数据（没有 serverUrl 字段）→ 默认空串 = 同源，无需迁移', () => {
  const legacyRaw = JSON.stringify({ mode: 'core', token: 'legacy-token' })
  const parsed = parsePrefs(legacyRaw)
  assert.deepEqual(parsed, { mode: 'core', token: 'legacy-token', serverUrl: '', worldId: '' })
  assert.deepEqual(decideFrontDoor(parsed), { screen: 'core-auto' })
})

frontDoorCase('旧数据没有 `worldId`（2026-09-25 新增）⇒ 空串 = 还不知道，一切按凭据分键', () => {
  // 世界身份要问一句（`GET /v1/home`）才知道 ⇒ **空串是常态**，不是"缺字段"。
  assert.equal(parsePrefs(JSON.stringify({ mode: 'core', token: 't' })).worldId, '')
  assert.equal(DEFAULT_PREFS.worldId, '')
  assert.equal(parsePrefs(JSON.stringify({ mode: 'core', token: 't', worldId: 42 })).worldId, '')
  // 有了就原样带出来（往返不丢）。
  const withWorld = prefsOf({ mode: 'core', token: 't', worldId: 'h-1' })
  assert.equal(parsePrefs(serializePrefs(withWorld)).worldId, 'h-1')
})

frontDoorCase('prefs 读不懂 / 缺字段 → 一律归一到缺省（绝不抛错、绝不给半个对象）', () => {
  for (const raw of [null, undefined, '', 'not-json', '[]', 'null', '{"mode":123}']) {
    assert.deepEqual(parsePrefs(raw), DEFAULT_PREFS, `raw=${String(raw)}`)
  }
  assert.deepEqual(parsePrefs('{"token":42,"serverUrl":{}}'), DEFAULT_PREFS)
})

frontDoorCase('历史遗留 mode（host / mock 等未知值）→ 一律归一到 core', () => {
  assert.equal(parsePrefs('{"mode":"mock","token":"t"}').mode, 'core')
  assert.equal(parsePrefs('{"mode":"CORE","token":"t"}').mode, 'core')
  assert.equal(
    parsePrefs('{"mode":"host","token":"t"}').mode,
    'core',
    'Host 已退役 ⇒ 旧值就是残留，读进来按 core 处理',
  )
})

frontDoorCase('serialize ⇄ parse 往返保住服务器地址', () => {
  const prefs = prefsOf({ mode: 'core', token: 't-9', serverUrl: 'http://192.168.1.9:8080' })
  assert.deepEqual(parsePrefs(serializePrefs(prefs)), prefs)
})

frontDoorCase('服务器地址归一化：没写 scheme 补 http://、去尾斜杠、空串保持空（同源）', () => {
  assert.equal(normalizeServerUrl(''), '')
  assert.equal(normalizeServerUrl('   '), '')
  assert.equal(normalizeServerUrl('127.0.0.1:8080'), 'http://127.0.0.1:8080')
  assert.equal(normalizeServerUrl('  http://a.example.com/  '), 'http://a.example.com')
  assert.equal(normalizeServerUrl('https://a.example.com/base/'), 'https://a.example.com/base')
})

frontDoorCase('地址校验：空串合法（同源）、非 http/https 拒绝、读不懂报错', () => {
  assert.equal(serverUrlError(''), null)
  assert.equal(serverUrlError('127.0.0.1:8080'), null)
  assert.equal(serverUrlError('ftp://a.example.com'), '服务器地址只支持 http/https')
  assert.equal(serverUrlError('http://'), '服务器地址读不懂，示例：http://127.0.0.1:8080')
})

frontDoorCase('基址拼接：空地址 = 同源相对路径（家里托管路径一个字符都不变）', () => {
  assert.equal(joinBaseUrl('', '/v1/me'), '/v1/me')
  assert.equal(joinBaseUrl(undefined, '/v1/me'), '/v1/me')
  assert.equal(joinBaseUrl('http://127.0.0.1:8080', '/v1/me'), 'http://127.0.0.1:8080/v1/me')
  assert.equal(joinBaseUrl('http://127.0.0.1:8080/', '/v1/me'), 'http://127.0.0.1:8080/v1/me')
})

frontDoorCase('WebSocket 前缀：http→ws、https→wss、同源→null（交给页面来源推导）', () => {
  assert.equal(webSocketBase(''), null)
  assert.equal(webSocketBase(undefined), null)
  assert.equal(webSocketBase('http://127.0.0.1:8080'), 'ws://127.0.0.1:8080')
  assert.equal(webSocketBase('https://a.example.com/'), 'wss://a.example.com')
  // 非 http(s) 的怪东西不冒充同源：返回 null（调用方仍按页面来源走）。
  assert.equal(webSocketBase('ws://a.example.com'), null)
})

frontDoorCase('直连地址只改「去哪个世界」，不改任何持久层的形状', () => {
  // 凭据一律进同一份 prefs（localStorage），没有第二个存储面。
  // `worldId`（2026-09-25）也在这同一份里：它是**世界级事实的归档键**，不是第二个存储面。
  const prefs = prefsOf({ mode: 'core', token: 't-2', serverUrl: 'http://127.0.0.1:9000' })
  const keys = Object.keys(JSON.parse(serializePrefs(prefs)) as Record<string, unknown>).sort()
  assert.deepEqual(keys, ['mode', 'serverUrl', 'token', 'worldId'])
})

frontDoorCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(
    completed >= DECLARED_CASES - 1,
    `完成的用例数 ${completed} 少于声明的 ${DECLARED_CASES - 1}：有用例被静默跳过`,
  )
})
