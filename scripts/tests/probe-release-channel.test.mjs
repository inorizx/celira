// **发布渠道探针：那三格读得对吗**（2026-10-21，跟 Gitee 镜像一起加）。
//
// 这一族钉两件事：
//  1. 哈希那一格**不是猜的**：GitHub 印在附件页，Gitee 印在发布说明里；读不到就回 null，
//     绝不用大小或版本号冒充（`update.ps1` 的哈希策略是 fail-closed）。
//  2. 渠道答不全时**如实说答不全**：没有附件 ⇒ ok:false 且把答上的那几格放 partial，
//     不是"差不多能用"。
//
// 运行： node --test scripts/tests/probe-release-channel.test.mjs

import test from 'node:test'
import assert from 'node:assert/strict'

import { compareVersions, hashFromBody, readChannel } from '../probe-release-channel.mjs'

const HEX = '977a3367a2f1bff395bb311a80a8fb07f369170a3af6eda1bf0d5ddd1efd7652'

function giteeRelease(overrides = {}) {
  return {
    tag_name: 'v0.1.3',
    name: 'Celira v0.1.3',
    created_at: '2026-10-21T10:00:00+08:00',
    body: `# Celira 0.1.3\n\n- fix: something\n\nsha256: ${HEX}\n`,
    assets: [
      {
        name: 'celira-0.1.3-win-x64.zip',
        size: 30_000_000,
        browser_download_url: 'https://gitee.com/x/y/releases/download/v0.1.3/celira-0.1.3-win-x64.zip',
      },
    ],
    ...overrides,
  }
}

test('hashFromBody: reads the digest line the packager writes', () => {
  assert.equal(hashFromBody(`changes\n\nsha256: ${HEX}`), HEX)
})

test('hashFromBody: lowercases, because two channels may print it differently', () => {
  assert.equal(hashFromBody(`sha256: ${HEX.toUpperCase()}`), HEX)
})

test('hashFromBody: absent answers null (never a made-up digest)', () => {
  assert.equal(hashFromBody('# Celira 0.1.3\n\nno digest here'), null)
  assert.equal(hashFromBody(''), null)
  assert.equal(hashFromBody(undefined), null)
})

test('hashFromBody: a short hex is not a digest', () => {
  assert.equal(hashFromBody('sha256: deadbeef'), null)
})

test('readChannel: Gitee body carries the digest, so the channel answers all three cells', () => {
  const read = readChannel([giteeRelease()])
  assert.equal(read.ok, true)
  assert.equal(read.entry.version, '0.1.3')
  assert.equal(read.entry.hash, HEX)
  assert.equal(read.entry.asset.name, 'celira-0.1.3-win-x64.zip')
})

test('readChannel: no digest anywhere answers hash:null and says so at the call site', () => {
  const read = readChannel([giteeRelease({ body: 'no digest' })])
  assert.equal(read.ok, true)
  assert.equal(read.entry.hash, null)
})

test('readChannel: a release with no assets is not usable, and keeps what it did answer', () => {
  const read = readChannel([giteeRelease({ assets: [] })])
  assert.equal(read.ok, false)
  assert.equal(read.partial.version, '0.1.3')
  assert.equal(read.partial.asset, null)
})

test('readChannel: the newest release is picked by created_at, not by array order', () => {
  const older = giteeRelease({ tag_name: 'v0.1.2', created_at: '2026-10-20T00:00:00Z' })
  const newer = giteeRelease({ tag_name: 'v0.1.3', created_at: '2026-10-21T00:00:00Z' })
  assert.equal(readChannel([older, newer]).entry.version, '0.1.3')
  assert.equal(readChannel([newer, older]).entry.version, '0.1.3')
})

test('compareVersions: 0.1.2 is older than 0.1.3 (hasUpdate is this sign)', () => {
  assert.equal(compareVersions('0.1.2', '0.1.3'), -1)
  assert.equal(compareVersions('0.1.3', '0.1.3'), 0)
  assert.equal(compareVersions('0.1.4', '0.1.3'), 1)
})
