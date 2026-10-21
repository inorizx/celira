/**
 * **有没有新版本**（`api/appUpdate.ts`）：关于页那一颗「检查更新」。
 *
 * 判据三条，都是这一格存在的理由：
 *  1. `latest` / `newer` / `unknown` **三态各说各的话**，不合并；
 *  2. 「有新版但没有可下载的包」**不许**被说成"已是最新"；
 *  3. 「读不到」带上渠道给的原因，不翻译成乐观说法。
 *
 * ⚠ 与 `app-version.test.ts` 同款：本文件不 import 任何 Tauri 模块（`appUpdate.ts` 只在
 * 真要 invoke 时才动态 import），所以 `node --test` 的类型剥离模式能直接跑。
 */
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { sizeLine, updateLine, type AppUpdateReadout } from '../src/api/appUpdate.ts'

function readout(overrides: Partial<AppUpdateReadout> = {}): AppUpdateReadout {
  return {
    state: 'latest',
    current: '0.1.3',
    latest: '0.1.3',
    channel: 'github',
    asset: null,
    note: null,
    ...overrides,
  }
}

test('updateLine：已是最新 ⇒ 说号，不说"可能是"', () => {
  assert.equal(updateLine(readout()), '已是最新（v0.1.3）')
})

test('updateLine：有新版 ⇒ 说版本与大小', () => {
  const line = updateLine(
    readout({
      state: 'newer',
      latest: '0.1.4',
      asset: {
        name: 'celira-0.1.4-win-x64.zip',
        url: 'https://example/celira-0.1.4-win-x64.zip',
        size: 29804949,
        sha256: 'e2b56326702af450dcfba5bd9fbb5a3d6df9caf4cc25bdae0d93d0bbf9954d81',
      },
    }),
  )
  assert.equal(line, '有新版 v0.1.4（28.4 MiB）')
})

test('updateLine：有新版但没有包 ⇒ 明说，不许说成"已是最新"', () => {
  const line = updateLine(readout({ state: 'newer', latest: '0.1.4', asset: null }))
  assert.match(line, /有新版 v0\.1\.4/)
  assert.match(line, /没有可下载的包/)
})

test('updateLine：读不到 ⇒ 把渠道的原因原样带上', () => {
  const line = updateLine(
    readout({ state: 'unknown', latest: null, channel: null, note: 'github: HTTP 502；gitee: 连不上' }),
  )
  assert.equal(line, '读不到：github: HTTP 502；gitee: 连不上')
})

test('updateLine：没有 note 的读不到也不编一个原因', () => {
  assert.equal(updateLine(readout({ state: 'unknown', latest: null, note: null })), '读不到')
})

test('updateLine：null（还没问过）⇒ 空串，由调用方决定怎么提示', () => {
  assert.equal(updateLine(null), '')
})

test('sizeLine：未知就是未知，不写 0', () => {
  assert.equal(sizeLine(null), '大小未知')
  assert.equal(sizeLine(0), '大小未知')
  assert.equal(sizeLine(1048576), '1.0 MiB')
  assert.equal(sizeLine(29804949), '28.4 MiB')
})

test('checkAppUpdate：浏览器里 ⇒ null（这条路整条不存在）', async () => {
  const { checkAppUpdate } = await import('../src/api/appUpdate.ts')
  assert.equal(await checkAppUpdate(), null)
})

// ── 能不能点「下载并更新」：三条拒绝，最后一条是 fail-closed ──────────────────

test('installBlocker：没有新版 ⇒ 拒绝', async () => {
  const { installBlocker } = await import('../src/api/appUpdate.ts')
  assert.equal(installBlocker(readout({ state: 'latest' })), '没有新版本')
})

test('installBlocker：有新版但没有包 ⇒ 拒绝', async () => {
  const { installBlocker } = await import('../src/api/appUpdate.ts')
  assert.equal(
    installBlocker(readout({ state: 'newer', latest: '0.1.4', asset: null })),
    '渠道没有给出可下载的包',
  )
})

test('installBlocker：没有 sha256 ⇒ 拒绝（字节无法证明，就不换）', async () => {
  const { installBlocker } = await import('../src/api/appUpdate.ts')
  const line = installBlocker(
    readout({
      state: 'newer',
      latest: '0.1.4',
      asset: { name: 'celira-0.1.4-win-x64.zip', url: 'https://example/x.zip', size: 1, sha256: null },
    }),
  )
  assert.match(String(line), /sha256/)
})

test('installBlocker：三样齐 ⇒ null（可以换）', async () => {
  const { installBlocker } = await import('../src/api/appUpdate.ts')
  assert.equal(
    installBlocker(
      readout({
        state: 'newer',
        latest: '0.1.4',
        asset: {
          name: 'celira-0.1.4-win-x64.zip',
          url: 'https://example/x.zip',
          size: 1,
          sha256: 'e2b56326702af450dcfba5bd9fbb5a3d6df9caf4cc25bdae0d93d0bbf9954d81',
        },
      }),
    ),
    null,
  )
})

test('downloadAppUpdate：浏览器里 ⇒ 抛错，不假装下载', async () => {
  const { downloadAppUpdate } = await import('../src/api/appUpdate.ts')
  await assert.rejects(
    () =>
      downloadAppUpdate(
        readout({
          state: 'newer',
          latest: '0.1.4',
          asset: {
            name: 'celira-0.1.4-win-x64.zip',
            url: 'https://example/x.zip',
            size: 1,
            sha256: 'e2b56326702af450dcfba5bd9fbb5a3d6df9caf4cc25bdae0d93d0bbf9954d81',
          },
        }),
      ),
    /这个形态不能更新自己/,
  )
})
