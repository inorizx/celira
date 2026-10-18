// **打包形态门禁（`scripts/verify-ui-app.mjs`）自己的守卫**（2026-10-08 第二刀）。
//
// 为什么给一条**脚本**写用例：这一刀修的两处都是"门禁自己会骗人"的形状，而它们**没有别的守卫**——
//   ① **收尾只扫世界、不扫住户** ⇒ 孤儿住户留在盘上，下一次冷启动起不来（实测第二次跑 13/18，
//      红的全是"读不到本机服务"，**看着像代码坏了**）；
//   ② **面板读数还在路上就判** ⇒ 「本机服务 / 世界」在说明文字里就有、「退出时：读不到」在加载态
//      也成立 ⇒ 那两条**假绿**，只有 Agent 那一行如实红（实测冷启动 17/18）。
// 两处都不会让 `npm run verify` 变红（那一条门禁要真机），所以判据必须**在源码上**钉着，
// 否则下一次有人顺手改一行，它又会静默地骗人。
//
// 与 `nav-groups.test.ts` 守 `verify-ui.mjs` 的期望入口清单同规：**读源码、钉判据，不起浏览器**。

import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'

const REPO = path.resolve(import.meta.dirname, '..')
const GATE = fs.readFileSync(path.join(REPO, 'scripts', 'verify-ui-app.mjs'), 'utf8')

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）。 */
let completed = 0
const DECLARED_CASES = 7

function gateCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

// ── ① 面板：读数落定才算数 ──────────────────────────────────────────────────

gateCase('★ 面板那三条断言**每一条**都并上了 `panelSettled`（漏一条就有一条能靠加载态假绿）', () => {
  // 按**顶层** `check(` 切段（每条断言一段）：段内只该有这一条断言的判据本体。
  const chunks = GATE.split(/\ncheck\(/).slice(1)
  assert.ok(chunks.length > 10, `只切出 ${chunks.length} 段 —— 切法本身失效了`)
  for (const anchor of [
    '★ 壳专属读数：本机服务面板真的读得到',
    '本机服务面板：Agent 那一行读得到',
    '本机服务面板：退出行为那一行读得到',
  ]) {
    const chunk = chunks.find((each) => each.includes(anchor))
    assert.ok(chunk, `门禁里找不到那条断言：${anchor}`)
    // ⚠ 判据落在**谓词那一行**（`panelSettled && …`），不是整段：这一段里"没落定就换一句说明"
    //   那个三元表达式也提到 `panelSettled` ⇒ 只看整段的话，谓词被摘掉它照样绿（实测过一次）。
    assert.match(
      chunk,
      /^\s*panelSettled && /m,
      `${anchor} 的**谓词**没并上 panelSettled —— 加载态能蒙过它`,
    )
  }
})

gateCase('★ "落定"的判据只有一处，且那行过渡态文字在**过渡态清单**里（清单是唯一权威）', () => {
  assert.match(
    GATE,
    /const panelSettled = !\/正在读本机服务\/\.test\(machineText\)/,
    'panelSettled 的判据该是"面板那行过渡态文字不在了"',
  )
  const list = GATE.match(/const TRANSIENT_TEXTS = \[([^\]]*)\]/)
  assert.ok(list, '找不到过渡态清单')
  assert.ok(
    list[1].includes('正在读本机服务'),
    '「正在读本机服务」必须在 TRANSIENT_TEXTS 里 —— 否则整页级等待会把它当终态',
  )
})

gateCase('★ 面板那条等待等的是"面板在 **且** 读数落定"，不是"面板出现就判"', () => {
  assert.match(
    GATE,
    /if \(\/本机服务\/\.test\(machineText\) && !\/正在读本机服务\/\.test\(machineText\)\) break/,
    '等待条件该是"面板挂上来了 **且** 读数不在路上"',
  )
})

// ── ② 收尾：住户也归门禁收 ──────────────────────────────────────────────────

gateCase('★ 住户收尾三条判据齐：**差集**（基线）＋ **种类**（壳自己那份 sidecar）＋ **家事实**', () => {
  assert.match(GATE, /let hostPidsBefore = \[\]/, '要有基线这一格')
  assert.match(GATE, /hostPidsBefore = residentPidsNow\(shellLayout\)/, '基线要在壳起来之前取一次')
  assert.match(
    GATE,
    /hostExe: path\.join\(path\.dirname\(path\.resolve\(exe\)\), 'bin', 'agent_host\.exe'\)/,
    '种类判据要落在**壳自己那份** sidecar（`<壳目录>/bin/agent_host.exe`），不是源码形态那一份',
  )
  assert.match(
    GATE,
    /envDirs: list\.map\(\(resident\) => resident\.envDir\)/,
    '家事实（`envDirs`）要从 `residents.js` 的住户表来 —— 缺了它 proc.js 会炸（那是刻意的）',
  )
  assert.match(GATE, /const doomed = rows\.filter\(\(row\) => !before\.includes\(row\.pid\)\)/, '只收差集')
  assert.match(GATE, /await homeProc\.killPid\(row\.pid, \{ force: true \}\)/, '收尾要真的杀')
})

gateCase('★ 顺序：基线在 `spawn` **之前**取，收尾在**世界之后**（先停世界，再停住户）', () => {
  const baselineAt = GATE.indexOf('hostPidsBefore = residentPidsNow(shellLayout)')
  const spawnAt = GATE.indexOf('shellProcess = spawn(exe, [], {')
  assert.ok(baselineAt > 0 && spawnAt > 0, '两处都要在')
  assert.ok(baselineAt < spawnAt, '基线晚于 spawn ⇒ 壳带起的住户会被算进基线 ⇒ 永远收不掉')

  const worldSweepAt = GATE.indexOf('await sweepWorldOrphan()')
  const residentSweepAt = GATE.indexOf('await sweepResidentOrphans()')
  assert.ok(worldSweepAt > 0 && residentSweepAt > 0, '两处收尾都要在')
  assert.ok(worldSweepAt < residentSweepAt, '先收世界、再收住户（住户还在时世界会被它们连回来）')
})

gateCase('★ 认不出就不杀：基线读不到 / 住户清单读不出来 ⇒ 一支都不杀（并如实说）', () => {
  assert.match(GATE, /if \(hostPidsBefore === null\) return/, '基线读不到 ⇒ 不杀（不是"当成一支都没有"）')
  // `residentPidsNow` 里那条"读不到"的分支：必须返回 `null`，且**如实说** ——
  // 返回空表就等于"这个家一支住户都没有"，收尾于是以为没有孤儿（这一格最容易出的假绿）。
  const readNow = GATE.slice(GATE.indexOf('function residentPidsNow'), GATE.indexOf('async function sweepResidentOrphans'))
  assert.ok(readNow.length > 0, '找不到 `residentPidsNow`')
  assert.match(readNow, /catch \(error\) \{[\s\S]*?return null/, '读不到要返回 `null`')
  assert.match(readNow, /⚠ 住户清单读不出来/, '读不出来要如实说，不许静默')
  const sweep = GATE.slice(GATE.indexOf('async function sweepResidentOrphans'))
  assert.match(sweep, /catch \(error\) \{[\s\S]*?没敢动手/, '收尾时清单读不出来 ⇒ 认不出就不杀')
})

gateCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
