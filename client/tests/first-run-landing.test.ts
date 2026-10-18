// **第一次那条路：落点与"下一步做什么"**（2026-10-11 立）。
//
// ## 它治的是什么
//
// 第一次那条路（本机世界刚立起来、这台机器上还没有模型源）的落点是
// `/app/runtimes/local#model` —— 靠**滚动**把人送到「模型」那一间。而真机量下来：
//
// ```
// 模型 top=641px · 运行 top=100px · 容器 scrollTop=171 = max（1179/348、768/768 三档同形）
// ```
//
// ⇒ **它滚到了上限**，只是「模型」是**最后一间**、下面没有内容可滚 ⇒ 顶不到最上面。
// **注入"滚一次就收工"的旧实现再量，读数一模一样** ⇒ "滚一次不够"这个缺陷**没有被量出来**。
// 所以这一刀改的不是滚动，而是**把动作从折叠线下面搬到读数之前**（机器页那张「第一次」卡）。
// 本文件同时钉住这两件（滚动只留"一份实现 + 不再静默失败"那点确定的东西）。
//
// ## 为什么是源级断言
//
// 这个仓库的 client 测试是 `node --test` + TS 类型剥离，**没有 jsdom / 组件渲染** ⇒
// "接线还在不在、顺序对不对"只能按源码判（与 `no-silent-red-path.test.ts` 同一口径）；
// **纯判据**（锚点名 → 元素 id）直接调函数。
// 真机那一条在 `client/scripts/verify-ui.mjs`（几何判据：那一间真的进了视野/对齐）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readdirSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { ANCHOR_SCROLL_WAIT_MS, resolveAnchorId } from '../src/lib/anchorScroll.ts'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

const HELPER = read('../src/lib/anchorScroll.ts')
const MACHINE_PAGE = read('../src/pages/RuntimesPage.tsx')
const AGENT_PAGE = read('../src/pages/MyAgentsPage.tsx')
/** 三个房间（含 `id="model"` 那一间）住在这个组件里，不在页面里。 */
const MACHINE_ROOMS = read('../src/components/celira/Runtimes.tsx')

/** 机器页那张旧锚点表（判据在页面里，这里**照抄**它来测纯函数 —— 值不一致就该红）。 */
const LEGACY = { home: 'run', days: 'activity', mind: 'model' }

test('门禁不许空跑：四份源码都读到了，且它们各自该有的锚点都在', () => {
  assert.ok(HELPER.length > 800, `共用实现只有 ${HELPER.length} 字节 —— 读错文件了？`)
  assert.ok(MACHINE_PAGE.includes('scrollToAnchor('), '机器页没有接共用实现')
  assert.ok(AGENT_PAGE.includes('scrollToAnchor('), 'Agent 主页没有接共用实现')
  assert.ok(MACHINE_PAGE.includes('<RuntimeModelSection'), '机器页没有渲染「模型」那一间')
  assert.ok(MACHINE_ROOMS.includes('id="model"'), '「模型」那一间没有锚点 id —— 滚过去会静默落空')
})

test('★ resolveAnchorId：空 / 只有 # / 空白 ⇒ 不滚；旧名映射；认不出的原样交出去', () => {
  assert.equal(resolveAnchorId(''), null, '空锚点不许滚')
  assert.equal(resolveAnchorId('#'), null, '只有一个 # 不许滚')
  assert.equal(resolveAnchorId('   '), null, '空白不许滚')
  assert.equal(resolveAnchorId('#model'), 'model')
  assert.equal(resolveAnchorId('#home', LEGACY), 'run', '旧名「家」该映射到「运行」')
  assert.equal(resolveAnchorId('#days', LEGACY), 'activity', '旧名「日子」该映射到「活动」')
  assert.equal(resolveAnchorId('#mind', LEGACY), 'model', '旧名「脑子」该映射到「模型」')
  // 认不出的名字**原样交出去**（元素不在就滚不动 —— 不报错、也不跳走）。
  assert.equal(resolveAnchorId('#nope', LEGACY), 'nope')
  // 不给映射表时旧名不映射（映射表归各自页面所有，这一层不替它猜）。
  assert.equal(resolveAnchorId('#home'), 'home')
})

test('★ 「滚一次就收工」这种写法全仓只许有一处（共用实现里那一处）', () => {
  const root = new URL('../src/', import.meta.url)
  const files: string[] = []
  const walk = (dir: URL) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) walk(new URL(`${entry.name}/`, dir))
      else if (/\.tsx?$/.test(entry.name)) files.push(fileURLToPath(new URL(entry.name, dir)))
    }
  }
  walk(root)
  assert.ok(files.length > 50, `扫描面太小（${files.length} 个文件）—— 这段 walk 自己坏了`)

  const offenders = files
    .filter((file) => readFileSync(file, 'utf8').includes('scrollIntoView'))
    .map((file) => file.slice(fileURLToPath(root).length).replace(/\\/g, '/'))
  assert.deepEqual(
    offenders.sort(),
    ['lib/anchorScroll.ts'],
    '带锚点滚动只许有**一份**实现（`lib/anchorScroll.ts`）—— 各页自己写一份就是"滚一次就收工"，\n' +
      '而真机量过它滚不到（上面几节分批长高会把目标顶走）。',
  )
})

test('★ 共用实现的两条硬纪律：有界等待 ＋ 出现即收工（不盯、不抢用户的滚动）', () => {
  const ms = ANCHOR_SCROLL_WAIT_MS
  assert.ok(Number.isFinite(ms), '等待窗口必须是有限数')
  assert.ok(
    ms > 0 && ms <= 3000,
    `等待窗口 ${ms}ms 不合理：它只盖"数据比页面晚一拍"这一档（真机上元素通常当帧就在）`,
  )
  assert.match(HELPER, /Date\.now\(\) >= deadline/, '没有"到期即停"的判据 ⇒ 会无限重试')
  assert.ok(!/while\s*\(\s*true\s*\)/.test(HELPER), '不许无限循环')
  assert.match(HELPER, /scrollIntoView[\s\S]{0,240}stopped = true/, '滚完必须立刻收工')
  // **不接管用户的滚动**（2026-10-11 定）：曾经写过"盯住 6 秒、用户一动就交还"那一版，
  // 后来**注入旧实现实测：读数一模一样** ⇒ "滚一次不够"这个缺陷没有被量出来 ⇒
  // 按仓库那条"不许为想象中的未来建设"撤掉了。这一条守着它别偷偷回来。
  for (const event of ['wheel', 'touchstart', 'keydown']) {
    assert.ok(
      !HELPER.includes(`'${event}'`),
      `不该监听 ${event}：这一层只负责"深链接说到哪就到哪"，不接管用户的滚动`,
    )
  }
})

test('★ 第一次那张卡：判据只有一处（modelSourceAbsent），且**读不到不摆**', () => {
  assert.ok(
    MACHINE_PAGE.includes('modelSourceAbsent(configView)'),
    '在场判据必须调 `api/homeConfig.ts` 的 `modelSourceAbsent`（纯函数、有离线用例），不许就地写一个 if',
  )
  assert.ok(
    MACHINE_PAGE.includes('configView !== null &&'),
    '读不到（`view === null`）时不许摆这张卡 ——「读不到 ≠ 没有模型源」是这一族的头号病',
  )
  // 判据不许新开请求：它必须取自页面上已有的那份读数（`useHomeEntries` 的 `config`）。
  assert.ok(
    MACHINE_PAGE.includes('const configView = reading?.config.view ?? null'),
    '判据该取自已有的读数（`reading.config.view`），不许为它再打一次口子',
  )
})

test('★ 第一次那张卡排在**读数之前**，且两个出口都在（配模型 / 建一支 Agent）', () => {
  const card = MACHINE_PAGE.indexOf('<FirstModelStep')
  const machineCard = MACHINE_PAGE.indexOf('<MachineCard')
  const runRoom = MACHINE_PAGE.indexOf('<RuntimeRunSection')
  assert.ok(card > 0 && machineCard > 0 && runRoom > 0, '三处锚点有一个找不到 —— 改名了？这条守卫要跟着改')
  assert.ok(
    card < machineCard && machineCard < runRoom,
    '顺序错了：**动作必须排在读数之前** —— 三个房间回答的是"现在怎么样"，' +
      '而第一次来的人要的是"我该做什么"（2026-10-11 主人裁）',
  )
  assert.ok(
    MACHINE_PAGE.includes('`/app/runtimes/${encodeURIComponent(entry.id)}/config`'),
    '第一步的出口该指向**按机器寻址**的配置页（不是"设置 → 模型配置"）',
  )
  assert.ok(
    MACHINE_PAGE.includes("navigate('/app/agents')"),
    '第二步（建一支 Agent）的出口在世界侧 —— 不写出来，这条路走到一半就断了',
  )
  // 真机门禁的稳定钩子（`client/scripts/verify-ui.mjs` 按它找这张卡，不靠正文正则）。
  assert.ok(MACHINE_PAGE.includes('data-first-model-step'), '缺给真机门禁用的稳定钩子')
})
