'use strict'
/**
 * 绑定那两个写者的**保留判据**（2026-10-09 加宽：钥匙属机器，"用什么型号/什么档位"属她）。
 *
 * 为什么单独立一条：绑定里现在可以有 `model` / `reasoning_effort` 两格（支级事实，
 * 覆盖清单条目里的同名字段 ⇒ 换执行端时她还是她）。而**写者每次只拿到 `source`** ——
 * 漏了保留，页面上重新绑一次就把她那两格悄悄抹掉（与 `reasoning_effort` 那次
 * "页面保存抹掉清单那一格"是同族事故，实测吃过一次）。
 */

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const modelSources = require('../model-sources')

let cases = 0
function check(name, fn) {
  test(name, () => {
    cases += 1
    fn()
  })
}

function scratch(tag) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-ms-binding-${tag}-`))
}

function write(envDir, body) {
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(modelSources.bindingPath(envDir), `${body}\n`)
}

function read(envDir) {
  return JSON.parse(fs.readFileSync(modelSources.bindingPath(envDir), 'utf8'))
}

check('★ 重新绑定：那两格原样保留（不许被抹掉）', () => {
  const home = scratch('keep')
  const envDir = path.join(home, 'agents', 'a-1')
  write(envDir, JSON.stringify({ source: 'ms-1', model: 'her-model', reasoning_effort: 'max' }))

  modelSources.writeBinding(envDir, 'ms-2')
  const after = read(envDir)
  assert.equal(after.source, 'ms-2', '来源换成新那一条')
  assert.equal(after.model, 'her-model', '那两格不许被抹掉')
  assert.equal(after.reasoning_effort, 'max')
  // ⚠ **2026-10-14 起读侧也把这两格读出来**（原断言只比 `{state, source}`）：从前读侧看不见
  // 它们 ⇒ 页面说不出"具体用哪个型号"，而运行时**一直**认它们（`apply_binding_overrides`）
  // ⇒ 两处对同一份文件说两套话。总断言改为"连覆盖一起比"（**判据变强，没放宽**）。
  assert.deepEqual(modelSources.readBinding(envDir), {
    state: 'bound',
    source: 'ms-2',
    model: 'her-model',
    reasoningEffort: 'max',
  })
  fs.rmSync(home, { recursive: true, force: true })
})

// ── ★ 支级覆盖那两格的三态（2026-10-14）：不给=沿用、`null`=清掉、给=写成它 ──────────
//
// 判据来自运行时：`agent/src/model_source.rs` 的 `apply_binding_overrides`（绑定的型号/档位
// **赢过**条目里的）。门是它们**唯一**的写者 ⇒ 三态在这一层钉死，页面才敢只送一格。
check('★ 支级覆盖：显式写 / 显式清 / 不给就沿用（三态一个都不能含糊）', () => {
  const home = scratch('overrides')
  const envDir = path.join(home, 'agents', 'a-1')

  // ① 写：两格都给了 ⇒ 写出来就是它们。
  modelSources.writeBinding(envDir, 'ms-1', { model: 'deepseek-v4.1-flash', reasoningEffort: 'max' })
  assert.deepEqual(read(envDir), {
    source: 'ms-1',
    model: 'deepseek-v4.1-flash',
    reasoning_effort: 'max',
  })
  // ② 清：显式 `null` ⇒ 那一格**从文件里消失**（回到"用条目里的型号"）——另一格不动。
  modelSources.writeBinding(envDir, 'ms-1', { model: null })
  assert.deepEqual(read(envDir), { source: 'ms-1', reasoning_effort: 'max' })
  // ③ 不给（`undefined`）⇒ 沿用旧值（老行为：重新绑定不抹掉它）。
  modelSources.writeBinding(envDir, 'ms-2')
  assert.deepEqual(read(envDir), { source: 'ms-2', reasoning_effort: 'max' })
  // ④ 两格一起清 ⇒ 写出来就是最老的那个形状（一个字都不多）。
  modelSources.writeBinding(envDir, 'ms-2', { reasoningEffort: null })
  assert.deepEqual(read(envDir), { source: 'ms-2' })
  fs.rmSync(home, { recursive: true, force: true })
})

check('★ 读侧 fail-closed：型号/档位写坏了 ⇒ 整份绑定读成 unreadable（不当作"没写"）', () => {
  const home = scratch('badfields')
  const envDir = path.join(home, 'agents', 'a-1')
  // 运行时对这两种文件**都会拒绝启动** ⇒ 门必须说同一句话（说成"没覆盖"就是撒谎）。
  write(envDir, JSON.stringify({ source: 'ms-1', model: 42 }))
  assert.deepEqual(modelSources.readBinding(envDir), {
    state: 'unreadable',
    source: null,
    reason: 'bad_model',
    model: null,
    reasoningEffort: null,
  })
  write(envDir, JSON.stringify({ source: 'ms-1', reasoning_effort: 'turbo' }))
  assert.equal(modelSources.readBinding(envDir).reason, 'bad_reasoning_effort')
  // 空串 / 只有空白也算坏（运行时那条"非空字符串"同款）。
  write(envDir, JSON.stringify({ source: 'ms-1', model: '   ' }))
  assert.equal(modelSources.readBinding(envDir).reason, 'bad_model')
  // `null` 与"没写"同义 ⇒ **读得出来**（这就是清掉覆盖之后盘上的样子）。
  write(envDir, JSON.stringify({ source: 'ms-1', model: null, reasoning_effort: null }))
  assert.deepEqual(modelSources.readBinding(envDir), {
    state: 'bound',
    source: 'ms-1',
    model: null,
    reasoningEffort: null,
  })
  fs.rmSync(home, { recursive: true, force: true })
})

/**
 * ★ **思考档那张表在三处必须逐字相同**（2026-10-14）。
 *
 * 三处各有一份、都删不掉：**运行时**（`agent/src/model_source.rs`，权威：认不得的到不了下游）、
 * **家的门**（本目录的 `model-sources.js`，写侧预检）、**壳那份门**（`home_doors.rs`，同一格）。
 * 谁想改一档，三处一起改 —— 这条用例读**那两份 Rust 源文件**里的表逐字比对（读文本而已，
 * 不编译 Rust；与 `presets.test.ts` 那条"两份实现逐项比对"同一手法）。
 */
check('★ 思考档词表：运行时 / 家的门 / 壳的门 —— 三处逐字相同', () => {
  const repo = path.resolve(__dirname, '..', '..', '..')
  const readTable = (file, pattern) => {
    const text = fs.readFileSync(path.join(repo, file), 'utf8')
    const matched = pattern.exec(text)
    assert.ok(matched, `${file} 里找不到那张表（名字改了？那条纪律就要换地方钉）`)
    return matched[1]
      .split(',')
      .map((each) => each.trim().replace(/^"|"$/g, ''))
      .filter(Boolean)
  }
  const runtime = readTable('agent/src/model_source.rs', /REASONING_EFFORTS:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]+)\]/)
  // 壳那一份住在它的**形状层**（`app/src-tauri/src/model_sources.rs`），门本体引用它 ——
  // 一个 crate 一份（与"JS 那一份在 `scripts/home/model-sources.js`"同一条排布）。
  const shell = readTable('app/src-tauri/src/model_sources.rs', /REASONING_EFFORTS:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]+)\]/)
  assert.deepEqual(modelSources.REASONING_EFFORTS, runtime, '家的门那份必须等于运行时那份')
  assert.deepEqual(shell, runtime, '壳那份必须等于运行时那份')
  assert.ok(modelSources.REASONING_EFFORTS.length >= 3, '表不能是空的（那是空跑）')
})

/**
 * ★ **`vision` 那一格：两份写侧都要带上它**（2026-10-17）。
 *
 * 为什么单独立一条：这一格是**宿主声明的**（不写就是不支持），而两份实现各自**按名重建**条目
 * —— 谁漏了它，用户开着的视觉就在**那一种形态**里被静默关掉（**没有任何报错**）。
 *
 * 2026-10-17 实测：**壳那一份真漏了**（`app/src-tauri/src/model_sources.rs` 的条目结构里根本
 * 没有这一格，写一次就把它从盘上删掉），而**两个门禁都不会红** —— 因为两份门共用的那张表
 * （`client/scripts/door-cases.mjs`）量的是**来源与方法**，装不下"载荷里有哪几格"这件事。
 *
 * ⇒ 这一条读**那两份源码**逐处比对（读文本、**不编译 Rust**；与同文件那条"思考档三处逐字
 * 相同"同一手法）。四处缺一不可：结构里那一格 · 读进来 · 写出去 · 重建时的**三态**。
 */
check('★ `vision` 那一格：家的门 / 壳的门 —— 两处都要带上它（写侧按名重建的坑）', () => {
  const repo = path.resolve(__dirname, '..', '..', '..')
  // 读源码，**先摘注释再扫**（与 `client/tests/user-theme.test.ts` 那条同一手法）。
  //
  // ⚠ 为什么必须摘：这一条是"某段文字在不在"的守卫，而**一句解释就能骗过它** —— 实测（2026-10-17）：
  // 把"写盘那一行"注释掉，`includes` 仍然命中（注释里还留着那段文字），守卫当场变绿。
  // 只摘**整行**注释（行首的斜杠斜杠）：行内的斜杠斜杠可能是字符串里的地址（`https://…`），
  // 按它一刀切会把测试夹具那些行切断。
  const stripComments = (text) =>
    text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
  const sourceOf = (file) => stripComments(fs.readFileSync(path.join(repo, file), 'utf8'))
  const js = sourceOf('scripts/home/model-sources.js')
  const shell = sourceOf('app/src-tauri/src/model_sources.rs')
  const door = sourceOf('app/src-tauri/src/home_doors.rs')

  // 家的门（JS 那一份）：形状层收它、落盘写它、来件没说时从旧条目带过来。
  assert.ok(js.includes('vision'), '家的门那一份要认得这一格')
  assert.ok(
    /vision === undefined/.test(js),
    '家的门那份要认**三态**（`undefined` = 没说 ⇒ 从旧条目带过来，不许压成两态）',
  )

  // 壳那一份（Rust）：四处缺一不可。
  assert.ok(
    /pub vision: bool/.test(shell),
    '壳那份的条目结构里必须有 `vision` —— 缺了它，写一次就把这一格从盘上删掉',
  )
  assert.ok(/get\("vision"\)/.test(shell), '壳那份的归一化要**读**它（只有显式 true 才算支持）')
  assert.ok(/"vision"\.to_string\(\)/.test(shell), '壳那份的形状层要**带上**它（值表那条路）')
  assert.ok(
    shell.includes('\\"vision\\": {}'),
    '壳那份**写盘那一行**要带上它 —— 这一行才是真正决定盘上有没有这一格的地方',
  )
  assert.ok(
    /vision: Option<bool>/.test(shell),
    '壳那份的 upsert 要收**三态**（`None` = 没说 ⇒ 从旧条目原样带过来）',
  )
  assert.ok(
    /invalid_vision/.test(door),
    '壳那份的门要认 `invalid_vision` 这个拒码（与家的门**逐字相同**）',
  )
})

check('没有那两格 ⇒ 写出来的仍是老形状（只有 source，盘上不多一个字）', () => {
  const home = scratch('old')
  const envDir = path.join(home, 'agents', 'a-1')
  modelSources.writeBinding(envDir, 'ms-1')
  assert.deepEqual(read(envDir), { source: 'ms-1' })
  fs.rmSync(home, { recursive: true, force: true })
})

check('旧的读不懂 ⇒ 照旧只写 source（不炸、也不猜）', () => {
  const home = scratch('broken')
  const envDir = path.join(home, 'agents', 'a-1')
  write(envDir, '{ 这不是 JSON')
  modelSources.writeBinding(envDir, 'ms-3')
  assert.deepEqual(read(envDir), { source: 'ms-3' })
  fs.rmSync(home, { recursive: true, force: true })
})

/**
 * ★ **还没住户化、但已经先选好了**的那几支（2026-10-14「选择先于接上」）。
 *
 * 判据三条（每一条都能被一个反例打红）：
 * 1. **住户不算**在里面（住户的绑定走 `boundPerResident` 那一格 —— 混进来机器页就会把
 *    一支还没接上机器的 Agent 画成住在这里）；
 * 2. **没有绑定文件的目录不算**（空目录 = 一次写失败的遗迹，不是"选好了"）；
 * 3. 顺序稳定（按 id），与目录的创建次序无关。
 */
check('★ 先选好的那几支：住户不算、没绑定的不算、按 id 排', () => {
  const home = scratch('unhomed')
  const residents = ['a-1', 'b-2']
  // 住户两支（有 agent.conf）——它们**不许**出现在这一格里。
  for (const id of residents) {
    const envDir = path.join(home, 'agents', id)
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(path.join(envDir, 'agent.conf'), `agent_id=${id}\n`)
    modelSources.writeBinding(envDir, 'ms-1')
  }
  // 先选好的两支（只有绑定，什么都没有）：故意按**倒序**创建，看它排不排。
  modelSources.writeBinding(path.join(home, 'agents', 'z-9'), 'ms-2')
  modelSources.writeBinding(path.join(home, 'agents', 'c-3'), 'ms-2')
  // 空目录（写失败的遗迹）与杂物：都不算。
  fs.mkdirSync(path.join(home, 'agents', 'empty-1'), { recursive: true })
  fs.writeFileSync(path.join(home, 'agents', 'a-note.txt'), 'x')

  const rows = modelSources.unhomedBindings(home, residents)
  assert.deepEqual(
    rows.map((row) => [row.agentId, row.binding.state, row.binding.source]),
    [
      ['c-3', 'bound', 'ms-2'],
      ['z-9', 'bound', 'ms-2'],
    ],
    '住户与空目录都不在这一格里；先选好的按 id 排',
  )
  fs.rmSync(home, { recursive: true, force: true })
})

check('没有 agents 目录 ⇒ 空（空是一个真答案，不是读不到）', () => {
  const home = scratch('no-agents')
  assert.deepEqual(modelSources.unhomedBindings(home, []), [])
  fs.rmSync(home, { recursive: true, force: true })
})

check('用例数 > 0（空跑守卫）', () => {
  assert.ok(cases > 0, '这一份必须真的跑过用例')
})
