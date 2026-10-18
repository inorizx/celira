/**
 * **壳里的本机世界**（`api/shellLocal.ts`）与正门那一格 —— 纯逻辑用例。
 *
 * 钉住的是四条判据（都不需要壳在场）：
 *
 * 1. **不插队**：页面手里已有凭据 ⇒ 一律 `passthrough`（用户上次连的是哪个世界就是哪个）；
 * 2. **问一次**：壳说"还没表态" ⇒ `ask`（§8.6-1）；表过态说不跑 ⇒ `passthrough`；
 * 3. **不假装成功**：`ok` 而没有原文 / 没起来 / 缺件 ⇒ 绝不 adopt（`reason` 如实）；
 * 4. **地址只认读得懂的两个口**：`localBaseOf` 缺一个、或两个口不同源 ⇒ `null`（宁可不装）。
 *
 * ⚠ 本文件**不 import 任何 Tauri 模块**（`shellLocal.ts` 只在真要 invoke 时才动态 import），
 * 所以 `node --test` 的类型剥离模式下能直接跑。
 */
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { DEFAULT_PREFS, type ConnectionPrefs } from '../src/api/prefs.ts'
import { decideFrontDoor } from '../src/api/entry.ts'
import {
  adoptDecision,
  localBaseOf,
  localWorldChoiceLabel,
  planLocalWorld,
  shouldStartLocalWorld,
  type LocalSessionView,
  type LocalWorldReadout,
} from '../src/api/shellLocal.ts'

function prefsOf(patch: Partial<ConnectionPrefs> = {}): ConnectionPrefs {
  return { ...DEFAULT_PREFS, ...patch }
}

function readoutOf(patch: Partial<LocalWorldReadout> = {}): LocalWorldReadout {
  return {
    state: 'running',
    address: 'http://127.0.0.1:8080',
    homeDir: 'D:/celira-home',
    humanId: 'h-1',
    statusBase: 'http://127.0.0.1:5190',
    configBase: 'http://127.0.0.1:5191',
    runLocalWorld: true,
    imExe: 'D:/celira/target/release/celira-im.exe',
    detail: null,
    ...patch,
  }
}

function sessionOf(patch: Partial<LocalSessionView> = {}): LocalSessionView {
  return {
    state: 'ok',
    address: 'http://127.0.0.1:8080',
    humanId: 'h-1',
    token: 'clr_hs_x',
    mintedHuman: false,
    detail: null,
    ...patch,
  }
}

// ── 1) 没壳：web 一字不变 ────────────────────────────────────────────────────

test('没壳 / 壳没答 ⇒ passthrough（web 行为一字不变）', () => {
  assert.deepEqual(planLocalWorld(null, prefsOf()), { kind: 'passthrough' })
})

// ── 2) 不插队：页面手里有凭据 ────────────────────────────────────────────────

test('页面手里已有凭据 ⇒ passthrough：本机那条不插队（哪怕壳说该跑）', () => {
  const prefs = prefsOf({ token: 'existing', serverUrl: 'https://my.world.example' })
  assert.deepEqual(planLocalWorld(readoutOf(), prefs), { kind: 'passthrough' })
  // 同一份判据：正门那边本来就会 core-auto（用户上次的选择说了算）。
  assert.equal(decideFrontDoor(prefs).screen, 'core-auto')
})

test('空白 token 不算凭据（与 decideFrontDoor 同一条判据）', () => {
  const prefs = prefsOf({ token: '   ' })
  assert.equal(decideFrontDoor(prefs).screen, 'login')
  assert.deepEqual(planLocalWorld(readoutOf({ runLocalWorld: null }), prefs), { kind: 'ask' })
})

// ── 2b) ★ 2026-10-08：**凭据已被证明没用之后**，本机世界可以接手 ──────────────
//
// 现场（`app/README.md` §5 第 15 行）：页面手里攥着上一次那个世界的凭据 ⇒ 正门走"直进"
// ⇒ 那个世界早就不在了 ⇒ 用户被丢进一张要手填地址与 token 的表单。
// 这一格就是那一刀的判据：**只放宽"不插队"这一条**，其余三态一字未变。

test('★ 直连失败之后 ⇒ 本机世界接手（有凭据也不再 passthrough）', () => {
  const prefs = prefsOf({ token: 'stale-token', serverUrl: 'http://127.0.0.1:8180' })
  // 第一轮：凭据在场 ⇒ 不插队（用户上次的选择优先）。
  assert.deepEqual(planLocalWorld(readoutOf(), prefs), { kind: 'passthrough' })
  // 第二轮：同一份读数、同一份偏好，**只多一条"凭据已被证明没用"** ⇒ 该进就进。
  assert.deepEqual(planLocalWorld(readoutOf(), prefs, { credentialsFailed: true }), {
    kind: 'enter',
    address: 'http://127.0.0.1:8080',
    statusBase: 'http://127.0.0.1:5190',
    configBase: 'http://127.0.0.1:5191',
  })
})

test('★ 那一条放宽**只管"不插队"**：没壳 / 表过态不跑 / 没表态，三态一字未变', () => {
  const prefs = prefsOf({ token: 'stale-token' })
  // 没壳：第二轮照样没得接手（浏览器里这条整段不存在）。
  assert.deepEqual(planLocalWorld(null, prefs, { credentialsFailed: true }), {
    kind: 'passthrough',
  })
  // 用户表过态说"这台机器上不跑世界" ⇒ 第二轮也**不许**替他改主意。
  assert.deepEqual(planLocalWorld(readoutOf({ runLocalWorld: false }), prefs, { credentialsFailed: true }), {
    kind: 'passthrough',
  })
  // 还没表态 ⇒ 还是问一次（不替用户起世界）。
  assert.deepEqual(planLocalWorld(readoutOf({ runLocalWorld: null }), prefs, { credentialsFailed: true }), {
    kind: 'ask',
  })
  // 说要跑、而它没在跑 ⇒ blocked（**绝不假装成功**：起它是下一步的事，由 `shouldStartLocalWorld` 说）。
  assert.deepEqual(
    planLocalWorld(readoutOf({ state: 'not-running' }), prefs, { credentialsFailed: true }),
    { kind: 'blocked', state: 'not-running', detail: null },
  )
})

test('★ 「本机世界」那一格三态：还没表态不许说成"你选过"；读不到也不许落进那一档', () => {
  // 2026-10-08：把"问一次"的答案做成**可以再改**的开关之后，这一行必须说准它现在是哪一态。
  assert.equal(localWorldChoiceLabel(readoutOf({ runLocalWorld: true })), '本机世界：开着')
  assert.equal(
    localWorldChoiceLabel(readoutOf({ runLocalWorld: false })),
    '本机世界：不跑（打开时直接去连别的世界）',
  )
  assert.equal(
    localWorldChoiceLabel(readoutOf({ runLocalWorld: null })),
    '本机世界：还没表态（下次打开时问你一次）',
  )
  // ⚠ 缺省不等于零，也不等于"没答过"：读不到要自己说读不到。
  assert.equal(localWorldChoiceLabel(null), '本机世界：读不到')
})

// ── 3) 问一次（§8.6-1）──────────────────────────────────────────────────────

test('壳说还没表态 ⇒ ask（问一次；不替用户起世界）', () => {
  assert.deepEqual(planLocalWorld(readoutOf({ runLocalWorld: null }), prefsOf()), { kind: 'ask' })
  // 哪怕世界已经在跑：没表态就还是问一次（"要不要在这台机器上跑"是一个表态，不是状态）。
  assert.deepEqual(
    planLocalWorld(readoutOf({ runLocalWorld: null, state: 'running' }), prefsOf()),
    { kind: 'ask' },
  )
})

test('表过态说不跑 ⇒ passthrough（第一屏回登录页，去连别的世界）', () => {
  assert.deepEqual(
    planLocalWorld(readoutOf({ runLocalWorld: false }), prefsOf()),
    { kind: 'passthrough' },
  )
  assert.equal(decideFrontDoor(prefsOf()).screen, 'login')
})

// ── 4) 说要跑：在跑就进，没跑就如实说 ────────────────────────────────────────

test('表态要跑且世界在跑 ⇒ enter，带上两个口的地址', () => {
  const plan = planLocalWorld(readoutOf(), prefsOf())
  assert.deepEqual(plan, {
    kind: 'enter',
    address: 'http://127.0.0.1:8080',
    statusBase: 'http://127.0.0.1:5190',
    configBase: 'http://127.0.0.1:5191',
  })
})

test('表态要跑但世界没起来 / 缺件 ⇒ blocked，原因原样带出来', () => {
  const down = planLocalWorld(readoutOf({ state: 'not-running', detail: '起不来（30 秒没回应）' }), prefsOf())
  assert.deepEqual(down, { kind: 'blocked', state: 'not-running', detail: '起不来（30 秒没回应）' })
  const noCore = planLocalWorld(readoutOf({ state: 'no-core', detail: null }), prefsOf())
  assert.deepEqual(noCore, { kind: 'blocked', state: 'no-core', detail: null })
})

// ── 5) 进本机世界之前：先确保这个家跑起来（世界 ＋ 住户） ─────────────────────

test('enter 与 not-running 都要 ensure；缺件/起失败/没表态/表态说不跑都不要', () => {
  // ⚠ 2026-10-03 真机发现的缺口：从前只在"世界没在跑"时才 ensure，于是**世界在跑、而她不在跑**
  // 这一态没人管 —— 那正是托盘语义下的常态（关窗口/退出 app 时本地服务照旧跑）。所以
  // `enter` 也必须走 ensure（幂等），"打开 app ⇒ 她就在"才由正门保证。
  assert.equal(
    shouldStartLocalWorld(planLocalWorld(readoutOf(), prefsOf())),
    true,
    '世界已经在跑 ⇒ 也要确认住户在跑',
  )
  assert.equal(
    shouldStartLocalWorld(planLocalWorld(readoutOf({ state: 'not-running' }), prefsOf())),
    true,
    '世界没在跑 ⇒ 起它',
  )
  // 缺可执行件 / 起失败 ⇒ **不偷着重试**（那是要人看的两态，重试是用户按一下的事）。
  assert.equal(
    shouldStartLocalWorld(planLocalWorld(readoutOf({ state: 'no-core' }), prefsOf())),
    false,
  )
  assert.equal(
    shouldStartLocalWorld(planLocalWorld(readoutOf({ runLocalWorld: null }), prefsOf())),
    false,
    '还没表态 ⇒ 先问，绝不替他起',
  )
  assert.equal(
    shouldStartLocalWorld(planLocalWorld(readoutOf({ runLocalWorld: false }), prefsOf())),
    false,
    '表态说不跑 ⇒ 放行去连别的世界，不起本机的',
  )
})

// ── 6) 采用会话：只在真的拿到原文时 ──────────────────────────────────────────

test('ok 且有原文才 adopt；ok 而没有原文是一个读不懂的回执 ⇒ 绝不 adopt', () => {
  assert.deepEqual(adoptDecision(sessionOf()), { adopt: true, reason: null })
  assert.deepEqual(adoptDecision(sessionOf({ token: null })), { adopt: false, reason: 'no-token' })
  assert.deepEqual(adoptDecision(null), { adopt: false, reason: 'unreachable' })
  for (const state of ['world-down', 'no-core', 'failed']) {
    assert.deepEqual(adoptDecision(sessionOf({ state, token: null })), { adopt: false, reason: state })
  }
})

// ── 6) 本机家的地址：两个口都读得懂才算数 ────────────────────────────────────

test('localBaseOf：两个口同源且端口读得出 ⇒ 装上 loopback 基址', () => {
  assert.deepEqual(localBaseOf(readoutOf()), {
    host: 'http://127.0.0.1',
    statusPort: 5190,
    configPort: 5191,
  })
  // 端口是覆盖过的（home.conf 里写了）⇒ 照壳给的用。
  assert.deepEqual(
    localBaseOf(readoutOf({ statusBase: 'http://127.0.0.1:6190', configBase: 'http://127.0.0.1:6191' })),
    { host: 'http://127.0.0.1', statusPort: 6190, configPort: 6191 },
  )
})

test('localBaseOf：缺一个口 / 端口读不懂 / 两个口不同源 ⇒ null（宁可不装）', () => {
  assert.equal(localBaseOf(null), null)
  assert.equal(localBaseOf(readoutOf({ statusBase: '' })), null)
  assert.equal(localBaseOf(readoutOf({ configBase: 'http://127.0.0.1' })), null)
  assert.equal(localBaseOf(readoutOf({ statusBase: 'not a url' })), null)
  assert.equal(
    localBaseOf(readoutOf({ configBase: 'http://192.168.1.9:5191' })),
    null,
    '两个口不同源 ⇒ 不装（半个地址只会让一半请求打到编出来的地方）',
  )
})

// ── 7) 正门那一格：唯一分派点仍然只有一处 ───────────────────────────────────

test('decideFrontDoor 的第三格只在"没凭据 + 壳说要问"时出现', () => {
  assert.deepEqual(decideFrontDoor(prefsOf(), true), { screen: 'local-ask' })
  assert.deepEqual(decideFrontDoor(prefsOf(), false), { screen: 'login' })
  assert.deepEqual(decideFrontDoor(prefsOf(), null), { screen: 'login' })
  // 手里有凭据 ⇒ 用户的选择优先，连问都不问。
  assert.deepEqual(decideFrontDoor(prefsOf({ token: 't' }), true), { screen: 'core-auto' })
})
