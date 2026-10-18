import assert from 'node:assert/strict'
import test from 'node:test'
import {
  attachOutcomeLine,
  machineCandidates,
  ONE_MACHINE_NOTE,
  type MachineReading,
} from '../src/api/remoteAttach.ts'
import { remoteAttachLine, remoteAttachReadout, type HomeConfigView } from '../src/api/homeConfig.ts'
import type { HomeEntry } from '../src/api/homes.ts'

/**
 * ★ **"让它去哪台机器上跑"**（2026-10-15 远程接上那一刀）。
 *
 * 这一组守的是主人当次点的那三样 —— **归属 / 边界 / 直觉**：
 *
 * - **归属**：它现在在哪台跑，由**各台自己报**（那台的 `residents`）；"哪台愿意让它去"由
 *   **那台自己报的那一格**（`remoteControl`）；本页不替任何一台说话。
 * - **边界**：一次只接**那一台**（点名 entry）；那台没允许/不提供/读不到，**三档三句话**，
 *   一个都不许压成"不能"（它们要用户做的事完全不同）。
 * - **直觉**：按钮上的字**带机器名**；「接到**这台**机器上」只留给本机那一档。
 */

const ENTRY = (over: Partial<HomeEntry> & { id: string }): HomeEntry =>
  ({ local: false, name: over.id, ...over }) as HomeEntry

/** 一份最小可用的配置读数（只放这一组用得着的那一格）。 */
function view(remoteControl: 'on' | 'off' | null | undefined): HomeConfigView {
  const base = { readAt: '2026-10-15T00:00:00Z', entries: [] } as unknown as HomeConfigView
  return remoteControl === undefined ? base : { ...base, remoteControl }
}

const machine = (over: Partial<MachineReading> & { entry: HomeEntry }): MachineReading => ({
  config: view('on'),
  residents: [],
  ...over,
})

test('★ 归属：它现在在哪台跑 ⇒ 说"在这台跑"、**不摆按钮**（摆一个无意义的按钮会被读成"再开一份"）', () => {
  const [row] = machineCandidates({
    agentId: 'a-1',
    machines: [
      machine({ entry: ENTRY({ id: 'remote', name: '外地那台' }), residents: ['a-1'] }),
    ],
  })
  assert.equal(row!.runningThere, true)
  assert.match(row!.line, /在这台机器上跑/)
  assert.equal(row!.action, null)
})

test('★ 归属：本机那一档 —— 按钮写「接到这台机器上」（"这台"只许留给本机）', () => {
  const [row] = machineCandidates({
    agentId: 'a-1',
    machines: [
      machine({ entry: ENTRY({ id: 'local', name: '本机', local: true }), residents: [] }),
    ],
  })
  assert.equal(row!.allowed, true)
  assert.equal(row!.action?.label, '接到这台机器上')
})

test('★ 直觉/边界：外地那台允许 ⇒ 按钮上**带机器名**（不许只写"接上"）', () => {
  const [row] = machineCandidates({
    agentId: 'a-1',
    machines: [
      machine({ entry: ENTRY({ id: 'remote', name: '办公室那台' }), residents: ['别的支'] }),
    ],
  })
  assert.equal(row!.allowed, true)
  assert.equal(row!.action?.label, '接到 办公室那台 上')
  assert.match(row!.line, /办公室那台/)
})

test('★ 边界：三档"不能"各说各的（能开的 / 开不了的 / 读不到的，一个都不许压成"不能"）', () => {
  const rows = machineCandidates({
    agentId: 'a-1',
    machines: [
      machine({ entry: ENTRY({ id: 'r1', name: '没开的那台' }), config: view('off') }),
      machine({ entry: ENTRY({ id: 'r2', name: '壳那台' }), config: view(null) }),
      machine({ entry: ENTRY({ id: 'r3', name: '旧版那台' }), config: view(undefined) }),
    ],
  })
  assert.equal(rows[0]!.action, null)
  assert.match(rows[0]!.line, /remote-control on/, '没开那一档要指得出开的命令')
  assert.equal(rows[0]!.allowed, false)
  assert.match(rows[1]!.line, /只在本机接 Agent/, '壳那一档是"这条路不存在"，不是"没开"')
  assert.match(rows[2]!.line, /没报这一格/, '老门那一档是"看不清"，不许当成没开')
  // 三句话**互不相同**（压成一句就是把三种下一步说成一种）。
  assert.equal(new Set(rows.map((row) => row.line)).size, 3)
})

test('★ 边界：那台整份读数都读不到 ⇒ 如实说读不到，按钮不摆（不指一条走不通的路）', () => {
  const [row] = machineCandidates({
    agentId: 'a-1',
    machines: [
      machine({
        entry: ENTRY({ id: 'r1', name: '关机那台' }),
        config: null,
        residents: null,
      }),
    ],
  })
  assert.equal(row!.action, null)
  assert.match(row!.line, /读不到/)
})

test('★ 边界：住户那一格读不到（`residents: null`）**不许**当成"它不在这儿"', () => {
  const [row] = machineCandidates({
    agentId: 'a-1',
    machines: [
      machine({
        entry: ENTRY({ id: 'r1', name: '那台' }),
        config: view('on'),
        residents: null,
      }),
    ],
  })
  assert.equal(row!.runningThere, null, '读不到就是读不到')
  // 配置读得到 + 允许 ⇒ 按钮照摆（"住户读不到"是另一格的事，不牵连这一格）——
  // 真接上去时那台自己会答 `already-homed`（幂等），页面不必替它猜。
  assert.equal(row!.action?.label, '接到 那台 上')
})

test('★ 归属：还没读到它是谁 ⇒ 一行动作都不摆', () => {
  const rows = machineCandidates({
    agentId: null,
    machines: [machine({ entry: ENTRY({ id: 'local', name: '本机', local: true }) })],
  })
  assert.equal(rows[0]!.action, null)
  assert.match(rows[0]!.line, /读不到/)
})

test('★ 结局长相：三档各一句（`homed-not-started` 既不许说成失败、也不许说成"跑起来了"）', () => {
  assert.match(attachOutcomeLine({ outcome: 'homed', pid: 42 }), /跑起来了/)
  const pending = attachOutcomeLine({ outcome: 'homed-not-started', pid: null })
  assert.match(pending, /先没起它/)
  assert.ok(!pending.includes('跑起来了'), pending)
  assert.match(attachOutcomeLine({ outcome: 'already-homed', pid: 42 }), /什么都没动/)
  // 不认识的结局**不许**当成功（那一档的调用方本来就会抛，这只是兜底那句）。
  assert.match(attachOutcomeLine({ outcome: 'something-new', pid: null }), /没接上/)
})

test('★ 三态读数：`on` / `off` / `null`（不提供） / 缺场（老门）—— 四档四句话', () => {
  assert.equal(remoteAttachReadout(view('on')).kind, 'on')
  assert.equal(remoteAttachReadout(view('off')).kind, 'off')
  assert.equal(remoteAttachReadout(view(null)).kind, 'unsupported')
  assert.equal(remoteAttachReadout(view(undefined)).kind, 'unknown')
  const lines = [
    remoteAttachLine(remoteAttachReadout(view('on'))),
    remoteAttachLine(remoteAttachReadout(view('off'))),
    remoteAttachLine(remoteAttachReadout(view(null))),
    remoteAttachLine(remoteAttachReadout(view(undefined))),
  ]
  assert.equal(new Set(lines).size, 4, '四档四句话，一个都不许省')
})

test('★ 那句硬事实写着"搬家"（不然会被读成"多开一份"）', () => {
  assert.match(ONE_MACHINE_NOTE, /同时只在一台机器上跑/)
  assert.match(ONE_MACHINE_NOTE, /搬/)
})
