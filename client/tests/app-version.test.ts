/**
 * **这一份 app 自己的身份**（`api/appVersion.ts`）：关于页那一行版本号。
 *
 * 判据只有一条，但它正是这一格存在的理由：**号从壳里来，读不到就说读不到 —— 不编一个号**。
 * 从前那一行是页面上写死的 `v0.1.0`：发一版就撒一次谎。
 *
 * ⚠ 与 `shell-local.test.ts` 同款：本文件**不 import 任何 Tauri 模块**
 * （`appVersion.ts` 只在真要 invoke 时才动态 import），所以 `node --test` 的类型剥离模式下能直接跑。
 */
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { appVersionLine, type AppVersionReadout } from '../src/api/appVersion.ts'

test('appVersionLine：发布包直接写号', () => {
  const readout: AppVersionReadout = { version: '0.1.0', source: 'package' }
  assert.equal(appVersionLine(readout), 'v0.1.0')
})

test('appVersionLine：源码形态那个号是编译期的，必须标出来', () => {
  // 它未必等于你手里那个包的名字 —— 不标，读者会以为这就是包的名字。
  assert.equal(appVersionLine({ version: '0.1.0', source: 'build' }), 'v0.1.0（编译期）')
})

test('appVersionLine：壳里读不到 ⇒ 说读不到，**不编一个号**', () => {
  assert.equal(appVersionLine(null), '版本读不到')
})

// ── 桥那一半：不在壳里（浏览器）⇒ 整条不存在，一个字节都不多做 ────────────────

test('readAppVersion：浏览器里 ⇒ null（这条路整条不存在）', async () => {
  const { readAppVersion } = await import('../src/api/appVersion.ts')
  // `node --test` 的环境没有壳的内部注入 ⇒ `isTauriRuntime()` 为假 ⇒ 不 import Tauri、直接 null。
  assert.equal(await readAppVersion(), null)
})
