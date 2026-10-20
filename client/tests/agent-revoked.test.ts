// **已撤销的 Agent 不该有一张"可操作"的页**（`src/api/agentRevoked.ts`）——纯逻辑 ＋ 源级守卫
// （2026-10-20 T3）。
//
// ## 要钉的两件事
//
// 1. **判据**：`shouldShowAgentAdmin(agent)` —— 撤销之后什么都改不了（凭证作废、绑定作废、
//    房间成员关系作废、世界那一侧也不再接受它的事件）⇒ 管理面（接入 / 凭证 / 撤销 /
//    接上 / 模型）一件都不许渲染。判据在纯函数里，页面只消费它。
// 2. **那句后果说明只有一个字面量**：它现在出现在**两处**（列表那一行 ／ 深链接那一页），
//    而"同一件事两个说法"正是这一刀要治的病。⇒ 用例直接读那两个源文件：
//    - 必须引用常量 `AGENT_REVOKED_CONSEQUENCE`；
//    - **不许**自己再写一遍那句话（内联 = 第二个权威，改一处忘一处是必然的）。
//
// ⚠ 缺省那一档（老响应没有 `revoked` 这一格）按**活着**处理：把它当"已撤销"会让一次
//   响应缺字段就把整页关掉（与 `AgentsListPanel` 一直以来的读法一致）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { AGENT_REVOKED_CONSEQUENCE, shouldShowAgentAdmin } from '../src/api/agentRevoked.ts'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

test('判据：活着 ⇒ 渲染管理面；已撤销 ⇒ 不渲染；缺那一格按活着处理', () => {
  assert.equal(shouldShowAgentAdmin({ revoked: false }), true)
  assert.equal(shouldShowAgentAdmin({ revoked: true }), false)
  // 老响应没有这一格 ⇒ **不许**当成已撤销（一次缺字段不许把整页关掉）。
  assert.equal(shouldShowAgentAdmin({}), true)
})

test('那句后果说明是一个**完整句子**（不是半句、不是拼出来的）', () => {
  // 事实：撤销是软撤销 —— 它不再接收事件，房间成员关系与凭证一并作废。
  assert.ok(AGENT_REVOKED_CONSEQUENCE.includes('已撤销'), AGENT_REVOKED_CONSEQUENCE)
  assert.ok(AGENT_REVOKED_CONSEQUENCE.includes('不再接收事件'), AGENT_REVOKED_CONSEQUENCE)
  assert.ok(AGENT_REVOKED_CONSEQUENCE.includes('房间成员关系与凭证一并作废'), AGENT_REVOKED_CONSEQUENCE)
})

test('★ 两处渲染同一个常量：列表那一行与深链接那一页都不许内联那句话', () => {
  const panel = read('../src/components/celira/AgentsListPanel.tsx')
  const page = read('../src/pages/MyAgentsPage.tsx')
  for (const [name, source] of [
    ['AgentsListPanel.tsx', panel],
    ['MyAgentsPage.tsx', page],
  ] as const) {
    assert.ok(
      source.includes('AGENT_REVOKED_CONSEQUENCE'),
      `${name} 没有引用那句后果说明的常量（同一件事又要写第二遍）`,
    )
    assert.ok(
      !source.includes('不再接收事件，房间成员关系与凭证一并作废'),
      `${name} 把那句话**内联**了一遍 —— 第二个权威（改一处忘一处）`,
    )
  }
})

test('★ 深链接那一页用的是**判据**，不是就地写 `agent.revoked`', () => {
  // 原因：`revoked` 的缺省口径（缺格 = 活着）与"该不该渲染管理面"是两件事，
  // 判据要能被用例钉住（页面那一层进不了 `node --test`）。
  const page = read('../src/pages/MyAgentsPage.tsx')
  assert.ok(page.includes('shouldShowAgentAdmin(agent)'), '深链接那一页没有走那个判据')
})

test('★ 那一页只对"某一支那一页"成立：列表模式不许被它顶掉（`homeMode` 那道门）', () => {
  // 真机撞出来的形状：列表模式下 `agent` 也会被算出来（`agentList[0]` 那个兜底），
  // 于是少了 `homeMode` 这一道，**`/app/agents` 会被换成一页"已撤销"** —— 列表与「添加」
  // 一起消失（第一支刚好是撤销的那支时）。
  const page = read('../src/pages/MyAgentsPage.tsx')
  assert.ok(
    page.includes('if (homeMode && !shouldShowAgentAdmin(agent))'),
    '已撤销那一页没有 `homeMode` 这道门 —— 列表页会被它顶掉',
  )
})
