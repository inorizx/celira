// 「人话词表」的纯逻辑测试（人话刀，2026-09-22）。
//
// 这一层只做换词，所以判据也简单，但有一条**最要紧**的：
// **认不出的取值必须原样交出来**（不许编一个"看起来对"的词）——
// 编了就等于替下一刀宣布"这一格已经翻译过了"，而它其实没有。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  legacyHomeNote,
  machineMatchLabel,
  nextLabel,
  resultLabel,
  runStateLabel,
  stateLabel,
} from '../src/lib/plain-language.ts'

test('活动结果：`subject:` 那一族四个取值各有中文，其余**原样**（不猜）', () => {
  assert.equal(resultLabel('completed'), '完成')
  assert.equal(resultLabel('silent'), '未回复')
  assert.equal(resultLabel('failed'), '失败')
  // 后端给的是 `subject:` 前缀那一族：前缀在这里剥掉（一处权威）。
  assert.equal(resultLabel('subject:completed'), '完成')
  assert.equal(resultLabel('subject:silent'), '未回复')
  // 2026-10-06 补齐 `SubjectEnding` 的另外两个：这一格从前只译了三个，
  // 于是「主体选择拒绝」原样露出 `declined` 这个英文词。
  assert.equal(resultLabel('subject:declined'), '拒绝')
  assert.equal(resultLabel('subject:waiting'), '等外部结果')
  // ⚠ 系统那一族**不在这张表里**（"没正常收场"归机器页的「活动」房间）⇒ 原样带出去。
  assert.equal(resultLabel('system:model-failed'), 'system:model-failed')
  // 没见过的取值原样交出来 —— 露出一个生词，也比编一个词好。
  assert.equal(resultLabel('subject:something-new'), 'something-new')
  assert.equal(resultLabel('weird'), 'weird')
  // 没有这一格就是没有（不是空串）。
  assert.equal(resultLabel(null), null)
})

test('当前状态：准入那几个机械值各自有人话；认不出的原样', () => {
  assert.equal(stateLabel('ready'), '待命')
  assert.equal(stateLabel('paused'), '已暂停')
  assert.equal(stateLabel('paused(reason=disk)'), '已暂停')
  assert.equal(stateLabel('unfinished'), '上一次没做完')
  assert.equal(stateLabel('unfinished(run=host-1-2-3)'), '上一次没做完')
  assert.equal(stateLabel(undefined), '状态未知')
  assert.equal(stateLabel(''), '状态未知')
  assert.equal(stateLabel('something-new'), 'something-new')
})

// ★ 2026-10-05：**四类判定不许被压成同一句"这次没有读到结果"**。
// `unknown` 那一句尤其要紧：它的真义是"**已派发却没有结果：外部效果可能已经发生**"
// （`agent/README.md` §3.10）—— 对用户是"它可能已经做了"，不是"我们没读到"。
test('活动判定：五类各自有人话（`unknown` 必须说得出"可能已经做了"）；认不出的原样', () => {
  assert.equal(runStateLabel('ended'), '已收场')
  assert.equal(runStateLabel('unfinished'), '还没结束')
  assert.equal(runStateLabel('blocked'), '被挡住了')
  assert.equal(runStateLabel('unknown'), '不明 —— 它可能已经做了')
  assert.match(runStateLabel('unknown') ?? '', /可能已经做了/, '这一句不许退化成"没读到"')
  assert.equal(runStateLabel('refused'), '这一场的记录读不出来')
  // 没有这一格就是没有（不是空串）—— 页面据此走"封存了没有"那一档。
  assert.equal(runStateLabel(null), null)
  assert.equal(runStateLabel(undefined), null)
  assert.equal(runStateLabel(''), null)
  // 没见过的取值原样交出来（与 `resultLabel` 同一条纪律）。
  assert.equal(runStateLabel('something-new'), 'something-new')
})

// ★ 2026-10-05：**老家形态那一句要按原因码说准**（三种原因的下一步完全不同）。
// 壳从这一天起在开机时自己搬 ⇒ 这一格出现 = "这一轮没搬成"，而"为什么"由壳给稳定码说。
test('老家形态：三种原因码各说各的下一步；认不出的码走兜底（不编一个原因）', () => {
  const running = legacyHomeNote('host-running')
  assert.match(running, /正跑着/)
  assert.match(running, /下次开机 app 自己就搬/, '这一档的下一步是"停掉它"')

  const worldKey = legacyHomeNote('world-key')
  assert.match(worldKey, /只有 CLI 能做/)
  assert.match(worldKey, /node home\.js up/, '这一档的下一步是 CLI（真路径）')

  const failed = legacyHomeNote('move-failed')
  assert.match(failed, /什么都没改/)
  assert.match(failed, /重试/, '这一档的下一步是再开一次')

  // 三句话**必须互不相同**（否则"说准"是假的）。
  assert.equal(new Set([running, worldKey, failed]).size, 3)

  // 认不出 / 没给 ⇒ 兜底那句：说清"这一轮没搬成"，**不编一个具体原因**。
  for (const unknown of [undefined, null, '', 'something-new']) {
    const fallback = legacyHomeNote(unknown)
    assert.match(fallback, /这一轮没搬成/)
    assert.ok(!fallback.includes('只有 CLI 能做'), '兜底不许挑一个原因来说')
    assert.ok(!fallback.includes('下次开机 app 自己就搬'), '兜底不许挑一个原因来说')
  }
})

test('「接下来」那一行：待命说"当前空闲"，其余如实描述（不搬后端原话）', () => {
  assert.match(nextLabel('ready'), /当前空闲/)
  assert.match(nextLabel('unfinished(run=host-1-2-3)'), /没做完/)
  assert.match(nextLabel('paused(reason=x)'), /暂停/)
  // 认不出就不编一句话（给一个占位符，让"这一格没读懂"看得见）。
  assert.equal(nextLabel('something-new'), '——')
  assert.equal(nextLabel(null), '——')
})

test('★ 「Agent 在不在本机」：三态各有话，且 `unknown` **不冒充**「未接入」', () => {
  // 前端填肉刀（2026-09-23）：列表行在线那一格用它。
  assert.equal(machineMatchLabel('same'), '在本机运行')
  assert.equal(machineMatchLabel('different'), '未接入')
  assert.equal(machineMatchLabel('unknown'), '状态未知')
  // 这一条是这一格的**全部意义**：读不到 ≠ Agent 在这台机器上没跑。
  assert.notEqual(machineMatchLabel('unknown'), machineMatchLabel('different'))
})
