// **「已读 / 未读」那一格的文案**（契约 `CORE_V1_READ_RECEIPT.md` §2）。
//
// 为什么值得钉在源码上：它说的是**我这条消息的结局**（对方读到没读到），是主人 2026-10-10
// 点名的那个读数 ——「agent 是否已读消息，才是最关键的」。而这个仓库为它用过两套词：
// 门禁 `verify-ui.mjs` 通篇叫「已读 / 未读」，屏幕上却印着「读到这儿 / 还没读」。同一件事
// 两套词，谁都以为对方那套才是权威。
//
// ★ 2026-10-20 主人裁决（原话）：「比如那个还没读，希望这个应该是已读未读这种」⇒ 屏幕上
// 说「已读 / 未读」。这是**主人自己在契约里给这个事实用的词**，也是所有人一眼就懂的那两个
// 状态；而契约 §2 那条措辞纪律禁的是"**看到**"（读到是位置，看到是判断）—— 换词没有越过它。
//
// ⚠ 真机那半由 `verify-ui.mjs` 的探针量（`data-peer-read` 钩子 + 文案与取值一致）；这一条
// 只挡"文案漂回去"—— 探针要真世界才跑得起来，源码守卫在任何一次 `npm test` 都在场。
import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const BUBBLE = readFileSync(
  fileURLToPath(new URL('../src/components/celira/MessageBubble.tsx', import.meta.url)),
  'utf8',
)
const PROBE = readFileSync(
  fileURLToPath(new URL('../scripts/verify-ui.mjs', import.meta.url)),
  'utf8',
)

test('★ 气泡下那一格：`read` ⇒「已读」、`unread` ⇒「未读」', () => {
  assert.match(
    BUBBLE,
    /peerRead === 'read' \? '已读' : '未读'/,
    '★ 屏幕上必须是「已读 / 未读」（2026-10-20 主人裁决）',
  )
  for (const old of ['读到这儿', '还没读']) {
    assert.ok(
      !BUBBLE.includes(old),
      `★ 旧文案「${old}」不许再出现：同一格两套词，门禁与屏幕迟早各说各的`,
    )
  }
})

test('★ 但那两格都不许写成"看到"（契约 §2：读到是位置，看到是判断）', () => {
  // 只判**渲染出来的字面量**那一段，不判注释（注释里正写着这条纪律本身）。
  const cell = BUBBLE.slice(BUBBLE.indexOf('data-peer-read'))
  assert.ok(!cell.includes('看到'), '★ 这一格说的是读位置，不是"她看到了"')
})

test('★ 真机门禁与屏幕上那句话是**同一套词**（探针按文案断言，取的是同两个字）', () => {
  assert.match(
    PROBE,
    /cellText === \(cellValue === 'read' \? '已读' : '未读'\)/,
    '★ `verify-ui.mjs` 断言的就是这两个词 —— 改了 UI 忘了改探针，那边会假红',
  )
  assert.match(PROBE, /afterText2 === '已读'/, '★ 「对方真读了 ⇒ 当场变已读」那一半也要对得上')
})
