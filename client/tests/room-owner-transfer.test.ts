// **群主转让给 Agent**（客户端侧契约；2026-10-26）。
//
// 产品裁决：owner 是房内的治理角色，不是参与者 kind。Agent 能建房并成为
// owner，人类群主也必须能把群交给房里的 Agent。这个文件钉住客户端最容易
// 退回去的一步：转让按钮只出现在「人」分组里，Agent 分组没有入口。
//
// 这一层证明的是**源码里的权限与调用形状**；真正的 Core 权限矩阵由
// `im/server/tests/room_profile_blackbox.rs` 与既有群治理黑盒证明。

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const DRAWER = readFileSync(
  fileURLToPath(new URL('../src/components/celira/RoomInfoDrawer.tsx', import.meta.url)),
  'utf8',
)

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})

test('★ 人类群主能把群转让给 Agent 成员（判据是 role，不看 kind）', () => {
  const agentsStart = DRAWER.indexOf('{agents.map')
  const humansStart = DRAWER.indexOf('{humans.map')
  assert.ok(agentsStart >= 0, '必须找得到 Agent 成员分组')
  assert.ok(humansStart > agentsStart, 'Agent 分组必须在人类分组之前')

  const agentsBlock = DRAWER.slice(agentsStart, humansStart)
  const humansBlock = DRAWER.slice(humansStart)

  // 两处都走同一条 action：人类群主 → 目标成员（m.id）。
  assert.ok(
    agentsBlock.includes('transferOwnership(room.id, m.id, undefined)'),
    'Agent 分组的成员行必须有转让入口',
  )
  assert.ok(
    humansBlock.includes('transferOwnership(room.id, m.id, undefined)'),
    '人类分组的既有转让入口不能丢',
  )

  // Agent 这一处不能又被 kind 挡住：owner 判据是 role。
  assert.ok(
    !agentsBlock.includes("m.kind === 'human'"),
    'Agent 转让不应再被 kind === human 拦截',
  )
  assert.ok(
    agentsBlock.includes('转让群主给'),
    '转让入口要有可读的无障碍名称/提示',
  )
})
