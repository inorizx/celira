'use strict'
/**
 * `probe-loop.js` 的**判据**用例（真机部分不在这里跑 —— 那要真世界、真宿主、一次真模型调用）。
 *
 * 为什么这几条值得单独钉住（2026-10-05）：那天我第一次跑这条回路时，用 `sender.kind` 过滤
 * "谁说的"——而消息载荷里那一格叫 **`actor`** ⇒ 她已经回了，脚本却报"**没等到**"。
 * 判据（谁算她说的 / 这一轮算不算回了）从此落在这里，被用例钉死。
 */

const test = require('node:test')
const assert = require('node:assert/strict')

const {
  parseArguments,
  isHerMessage,
  herMessages,
  loopVerdict,
  describeMessage,
  chooseModelSource,
  describeModelSource,
} = require('../probe-loop')

let cases = 0
function check(name, fn) {
  test(name, () => {
    cases += 1
    fn()
  })
}

const AGENT = 'aaaaaaaa-1111-2222-3333-444444444444'
const HUMAN = 'hhhhhhhh-1111-2222-3333-444444444444'

/** 世界回的那条消息**真实形状**（`MessageView`：`actor: {id, kind, display_name, status}`）。 */
function message(actorId, kind, body) {
  return {
    id: `m-${Math.random().toString(16).slice(2)}`,
    room_id: 'r-1',
    actor: { id: actorId, kind, display_name: 'x', status: 'active' },
    body,
    created_at: '2026-10-05T14:36:59.181Z',
  }
}

check('参数：缺省 240 秒、不保留；--timeout / --keep 认；不认识的一律明确失败', () => {
  assert.deepEqual(parseArguments([]), {
    timeoutSeconds: 240,
    keep: false,
    modelSources: null,
    modelSourceId: null,
    message: null,
  })
  assert.deepEqual(parseArguments(['--timeout', '30']), {
    timeoutSeconds: 30,
    keep: false,
    modelSources: null,
    modelSourceId: null,
    message: null,
  })
  assert.deepEqual(parseArguments(['--keep']), {
    timeoutSeconds: 240,
    keep: true,
    modelSources: null,
    modelSourceId: null,
    message: null,
  })
  assert.deepEqual(parseArguments(['--timeout', '90', '--keep']), {
    timeoutSeconds: 90,
    keep: true,
    modelSources: null,
    modelSourceId: null,
    message: null,
  })
  // 静默忽略未知参数 = 让人以为门禁按他想的那样跑了。
  assert.throws(() => parseArguments(['--timeout']), /正整数/)
  assert.throws(() => parseArguments(['--timeout', '0']), /正整数/)
  assert.throws(() => parseArguments(['--timeout', 'abc']), /正整数/)
  assert.throws(() => parseArguments(['--all']), /不认识的参数/)
  // 模型来源那两个旗标（2026-10-09）：缺值 / 空值 / 后面跟另一个旗标，一律明确失败。
  assert.equal(parseArguments(['--model-sources', 'D:/x.json']).modelSources, 'D:/x.json')
  assert.equal(parseArguments(['--model-source-id', 'ms-2']).modelSourceId, 'ms-2')
  assert.throws(() => parseArguments(['--model-sources']), /非空取值/)
  assert.throws(() => parseArguments(['--model-sources', '   ']), /非空取值/)
  assert.throws(() => parseArguments(['--model-sources', '--keep']), /非空取值/)
  assert.throws(() => parseArguments(['--model-source-id', '--keep']), /非空取值/)
  // 自定义那一句（2026-10-11）：缺省仍是"看到就回我一句"（`null` ⇒ 调用处兜底），
  // 给了就用给的；缺值 / 空值 / 后面跟另一个旗标，一律明确失败（与上面同一条纪律）。
  assert.equal(parseArguments(['--message', '  读一下你的 orientation.md  ']).message, '读一下你的 orientation.md')
  assert.throws(() => parseArguments(['--message']), /非空的话/)
  assert.throws(() => parseArguments(['--message', '   ']), /非空的话/)
  assert.throws(() => parseArguments(['--message', '--keep']), /非空的话/)
})

check('她用什么想：清单里挑一条 —— 缺省第一条、指定 id 必须找得到、四件必须齐', () => {
  const full = (id, name, model) => ({
    id,
    name,
    endpoint: `https://${id}.example.invalid/v1/chat/completions`,
    api_key: 'sentinel-key',
    model,
    context_window: 65536,
  })
  const list = [full('ms-1', '甲家', 'model-a'), full('ms-2', '乙家', 'model-b')]

  // 缺省：第一条（并如实说选的是哪一条）。
  const first = chooseModelSource(list, null)
  assert.equal(first.ok, true)
  assert.equal(first.entry.id, 'ms-1')

  // 指定 id：找得到就用它。
  const second = chooseModelSource(list, 'ms-2')
  assert.equal(second.ok, true)
  assert.equal(second.entry.model, 'model-b')

  // ⚠ 指定了却找不到 = **用法错**，不许静默回落到第一条（那会让人以为跑的是他指的那条）。
  const missing = chooseModelSource(list, 'ms-9')
  assert.equal(missing.ok, false)
  assert.match(missing.reason, /没有 id 为 ms-9/)
  assert.match(missing.reason, /ms-1、ms-2/, '要把有的 id 列出来')

  // 空清单 / 不是数组：明确说出来。
  assert.equal(chooseModelSource([], null).ok, false)
  assert.equal(chooseModelSource(null, null).ok, false)

  // 四件缺一：如实点名缺的是哪几件（坏清单不许当成"能跑"）。
  const broken = chooseModelSource([{ id: 'ms-1', endpoint: 'https://x.invalid', model: '' }], null)
  assert.equal(broken.ok, false)
  assert.match(broken.reason, /api_key/)
  assert.match(broken.reason, /model/)
})

check('读数行：给 id / 名字 / 型号 / 端点主机 —— **永不打印钥匙**', () => {
  const entry = {
    id: 'ms-1',
    name: 'OpenCode Go 路 DeepSeek V4.1 Flash',
    endpoint: 'https://opencode.ai/zen/go/v1/chat/completions',
    api_key: 'sentinel-key-must-not-appear',
    model: 'deepseek-v4.1-flash',
    context_window: 1000000,
  }
  const line = describeModelSource(entry)
  assert.match(line, /ms-1/)
  assert.match(line, /deepseek-v4\.1-flash/)
  assert.match(line, /opencode\.ai/)
  // 负向判据：取值、以及端点后面那一段路径，都不许出现在读数里。
  assert.ok(!line.includes('sentinel-key-must-not-appear'), '钥匙不许出现在读数里')
  assert.ok(!line.includes('/v1/'), '端点只报到主机为止')
  assert.ok(!line.includes('1000000'), '窗口不是这一行的事')
  // 端点不是合法 URL 时如实说，不许抛。
  assert.match(describeModelSource({ id: 'ms-1', model: 'm', endpoint: 'not-a-url' }), /不是合法 URL/)
})

check('谁算她说的：认 `actor.id`（判据只看身份，不看正文）', () => {
  assert.equal(isHerMessage(message(AGENT, 'agent', '你好'), AGENT), true)
  assert.equal(isHerMessage(message(HUMAN, 'human', '你好'), AGENT), false, '主人说的不算她说的')
  assert.equal(isHerMessage(message('someone-else', 'agent', '你好'), AGENT), false, '别的 Agent 不算')
})

check('★ 回归：载荷里那一格是 `actor` —— 写成 `sender` 的一律不算（那天误报的根）', () => {
  const withSender = {
    id: 'm-1',
    room_id: 'r-1',
    sender: { id: AGENT, kind: 'agent' },
    body: '你好呀，我是小测',
    created_at: '2026-10-05T14:37:03.678Z',
  }
  assert.equal(isHerMessage(withSender, AGENT), false, '不许把不存在的字段当成判据')
  assert.equal(herMessages([withSender], AGENT).length, 0)
  // 而真实形状的同一句话**必须**认出来。
  assert.equal(herMessages([message(AGENT, 'agent', '你好呀，我是小测')], AGENT).length, 1)
})

check('形状坏掉的一律不算（不猜）：没有 actor / actor 不是对象 / agentId 空 / 根本不是对象', () => {
  assert.equal(isHerMessage({ id: 'm', body: 'x' }, AGENT), false)
  assert.equal(isHerMessage({ actor: 'agent', body: 'x' }, AGENT), false)
  assert.equal(isHerMessage({ actor: { id: AGENT } }, ''), false, '不知道她是谁 ⇒ 谁都不算')
  assert.equal(isHerMessage(null, AGENT), false)
  assert.equal(isHerMessage('消息', AGENT), false)
})

check('这一轮的判决：三态各自独立（回没回 / 还没回），不许互相顶替', () => {
  const human = message(HUMAN, 'human', '看到就回我一句。')
  const hers = message(AGENT, 'agent', '你好呀，我是小测，收到你的测试消息了。')

  assert.equal(loopVerdict({ messages: [], agentId: AGENT }).kind, 'not-yet')
  assert.equal(loopVerdict({ messages: [human], agentId: AGENT }).kind, 'not-yet', '主人自己那条不算回')
  const replied = loopVerdict({ messages: [human, hers], agentId: AGENT })
  assert.equal(replied.kind, 'replied')
  assert.equal(replied.hers.length, 1)
  assert.equal(replied.hers[0].body, '你好呀，我是小测，收到你的测试消息了。')
  // 读不出来（不是数组）也不许当成"回了"。
  assert.equal(loopVerdict({ messages: null, agentId: AGENT }).kind, 'not-yet')
})

check('读数那一行：谁 / 什么时候 / 说了什么（正文压成一行、有长度上限；缺字段不炸）', () => {
  const line = describeMessage(message(AGENT, 'agent', '第一行\n第二行'))
  assert.match(line, /2026-10-05T14:36:59\.181Z/)
  assert.match(line, /\[agent:aaaaaaaa\]/)
  assert.match(line, /第一行 第二行/)
  assert.ok(!describeMessage({}).includes('undefined'))
  assert.equal(typeof describeMessage(null), 'string')
  const long = describeMessage(message(AGENT, 'agent', 'x'.repeat(400)))
  assert.ok(long.length < 200, '读数行不许无限长')
})

test('用例数 > 0（空跑守卫）', () => {
  assert.ok(cases > 0, '这一片一条都没跑 —— 那是空跑，不是通过')
})
