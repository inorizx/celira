// 聊天首屏读数（`src/api/uiPolicy.ts` 的 `chatFeedState`）的行为测试。
//
// 核心判据一条：**只有"消息真的到了、而且确认是空的"那一支，才允许说「还没有消息」。**
// 收口前的判据是 `roomMessages.length === 0`（把"还没到 / 读失败 / 连接不在"三种处境
// 一并说成"没有消息"），下面把它抄成负对照，并配一条能让它真红的输入。

import assert from 'node:assert/strict'
import test from 'node:test'

import { chatFeedState } from '../src/api/uiPolicy.ts'

/** 收口前的判据（`ChatPage` 原文）：只看条数。 */
function legacyEmptyClaim(count: number): string | null {
  return count === 0 ? '这个会话还没有消息，说一句开始吧。' : null
}

const EMPTY_CLAIM = '这个会话还没有消息，说一句开始吧。'

test('消息到了且有内容 ⇒ `messages`（这一支不显示任何占位）', () => {
  assert.deepEqual(
    chatFeedState({ arrived: true, count: 3, error: null, canRead: true }),
    { kind: 'messages' },
  )
})

test('消息到了且确认是空的 ⇒ `empty`，文案就是那句「还没有消息」', () => {
  const feed = chatFeedState({ arrived: true, count: 0, error: null, canRead: true })
  assert.equal(feed.kind, 'empty')
  assert.equal(feed.kind === 'empty' ? feed.text : '', EMPTY_CLAIM)
})

test('★ 还没到 ⇒ `reading`；旧判据在同一输入下已经说了「还没有消息」（假话）', () => {
  // 负对照：条数为 0 时旧判据立刻宣布"这个会话还没有消息"。
  assert.equal(legacyEmptyClaim(0), EMPTY_CLAIM)
  const feed = chatFeedState({ arrived: false, count: 0, error: null, canRead: true })
  assert.equal(feed.kind, 'reading')
  assert.ok(!feed.text.includes('还没有消息'), `首屏没到不许说成空：${feed.text}`)
})

test('★ 首屏读失败 ⇒ `failed`，带出那句人话；旧判据在**同一输入**下说空', () => {
  assert.equal(legacyEmptyClaim(0), EMPTY_CLAIM)
  const feed = chatFeedState({ arrived: false, count: 0, error: '连接不上服务器', canRead: true })
  assert.equal(feed.kind, 'failed')
  assert.ok(feed.text.includes('连接不上服务器'), feed.text)
  assert.ok(!feed.text.includes('还没有消息'), `读不到不许说成空：${feed.text}`)
})

test('★ 连接不在（根本没发起读取）⇒ `unreachable`：既不是"正在读取"也不是"没有消息"', () => {
  const feed = chatFeedState({ arrived: false, count: 0, error: null, canRead: false })
  assert.equal(feed.kind, 'unreachable')
  assert.ok(!feed.text.includes('正在读取'), `没在读取就不许说正在读取：${feed.text}`)
  assert.ok(!feed.text.includes('还没有消息'), feed.text)
})

test('优先级：到了就是到了（旧错误属于上一次尝试，不夺走已到的内容）', () => {
  assert.deepEqual(
    chatFeedState({ arrived: true, count: 2, error: '上一次刷新失败', canRead: true }),
    { kind: 'messages' },
  )
  // 到了、确认是空、且**这一次**也没报错 ⇒ 空态。
  assert.equal(chatFeedState({ arrived: true, count: 0, error: null, canRead: false }).kind, 'empty')
})

test('四态文案两两不同（不许把两种处境压成同一句话）', () => {
  const texts = [
    chatFeedState({ arrived: true, count: 0, error: null, canRead: true }),
    chatFeedState({ arrived: false, count: 0, error: null, canRead: true }),
    chatFeedState({ arrived: false, count: 0, error: '连接不上服务器', canRead: true }),
    chatFeedState({ arrived: false, count: 0, error: null, canRead: false }),
  ].map((feed) => (feed.kind === 'messages' ? '' : feed.text))
  assert.equal(new Set(texts).size, texts.length, `四态文案必须各不相同：${texts.join(' | ')}`)
})
