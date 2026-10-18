// 用户主题的纯逻辑测试（刀 1：外观装载）。
//
// 这一刀把"外观"从**编进共享产物**改成**用户可装载的数据**，所以判据全落在
// 三件机械可查的事上：
//
//  1. **格式拦人、体检不拦人。** 格式不合就装载不了（拒收是对的）；体检的结论
//     从来不阻止装载 —— 但 `unknown` **绝不许并进 `ok`**（"我没检查"≠"没问题"）。
//  2. **保真：序列化不许改用户写下的意思。** 带后代选择器的规则必须原样保留，
//     否则 `[data-theme='x'] .dark { … }` 会被错当成日间令牌、丢掉 `.dark` 那一节。
//  3. **★ 官方目录是格式的一致性证明。** 十套官方主题**每一套**都要能被这个格式
//     完整表达、来回一趟逐项不变，并且体检**不许对它们误报 `fail`** ——
//     那十套是手工策展过的（`THEME_GUIDE.md`），它们要是红，红的是我的阈值。
//
// 运行：cd client && npm test
//
// ⚠ 本文件的色值写在夹具里，所以它**不能**放在 `src/`（`check-tokens.mjs` 扫 `src/`，
//    而色值只许出现在 `tokens.css`）。测试目录在扫描范围外，这是故意的。

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync, readdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import {
  asPreviewTheme,
  buildThemeFile,
  classifyToken,
  parseThemeCss,
  parseThemeFile,
  PREVIEW_SCOPE_ID,
  RESERVED_SCOPE_IDS,
  serializeThemeFile,
} from '../src/lib/themeFile.ts'
import {
  auditTheme,
  CONTRAST_PAIRS,
  contrastRatio,
  parseColor,
  perceptualDistance,
  summarizeFindings,
  type AuditBase,
} from '../src/lib/themeAudit.ts'
import {
  activeUserThemeIdSnapshot,
  hydrateUserThemes,
  importUserTheme,
  parseUserThemes,
  removeUserTheme,
  upsertUserTheme,
  userThemeLibraryCss,
  userThemesSnapshot,
  writeActiveUserThemeId,
  writeUserThemes,
  officialExportId,
  type UserThemeRecord,
} from '../src/lib/userTheme.ts'
import { THEME_PRESETS } from '../src/lib/themes.ts'
import { currentStorage, setStorage, type StoragePort } from '../src/api/storage.ts'

const read = (relative: string) => readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

const tokensCss = read('../src/styles/tokens.css')
/**
 * 官方主题**包**目录（刀 3）：一个包 = 一个文件 = 一个作用域。
 * 运行时由 `themeCatalog.ts` 的 `import.meta.glob` 急加载收进来；这里用 fs 读同一批文件
 * （Node 不认 `import.meta.glob`）。**两侧读的是同一个目录**，所以"包文件名 == 作用域名"
 * 这条不变式下面单钉了一条。
 */
const themesDir = fileURLToPath(new URL('../src/styles/themes/', import.meta.url))
const themePackages = readdirSync(themesDir)
  .filter((f) => f.endsWith('.css'))
  .sort()
  .map((f) => ({ id: f.replace(/\.css$/, ''), file: f, text: readFileSync(join(themesDir, f), 'utf8') }))

/** 基底（`''`，即默认主题 `celira` 的 `:root` / `.dark`）＋ 九个包。 */
const catalog = {
  ...parseThemeCss(tokensCss),
  ...Object.fromEntries(themePackages.map((p) => [p.id, parseThemeCss(p.text)[p.id] ?? { light: {}, dark: {} }])),
}
const scopeOf = (presetId: string) => (presetId === 'celira' ? '' : presetId)
const defaultBase: AuditBase = {
  light: catalog['']?.light ?? {},
  dark: catalog['']?.dark ?? {},
}

// ── 存储桩（2026-10-08 存储收尾第二刀：读写走 `api/storage.ts` 的窄口）─────────

/** 一个记账用的假窄口。`canWrite=false` 模拟写不进去（隐私模式 / 壳那头失败）。 */
function fakePort(store: Record<string, string> = {}, log: string[] = [], canWrite = true): StoragePort {
  return {
    async read(key) {
      log.push(`read:${key}`)
      return store[key] ?? null
    },
    async write(key, value) {
      log.push(`write:${key}`)
      if (!canWrite) return false
      store[key] = value
      return true
    },
    async remove(key) {
      log.push(`remove:${key}`)
      delete store[key]
    },
    async keys(prefix) {
      return Object.keys(store).filter((k) => k.startsWith(prefix))
    },
  }
}

/**
 * 换上一只空仓的假窄口，并把**快照**归零。
 *
 * 为什么不能只换窄口：补水是**幂等**的（第一个调用读过就不再读），而快照是模块级的。
 * 归零走的是公开写口（`writeUserThemes` / `writeActiveUserThemeId`），不是测试专用后门。
 */
async function resetThemes(store: Record<string, string> = {}, canWrite = true) {
  setStorage(fakePort(store, [], canWrite))
  await hydrateUserThemes()
  await writeUserThemes([])
  await writeActiveUserThemeId(null)
  return store
}

const GOOD_THEME = `/* celira-theme: 我的深空 */
[data-theme='user-mine'] {
  --background: #0e1015;
  --foreground: #e8eaf0;
}
.dark[data-theme='user-mine'] {
  --background: #05060a;
  --foreground: #eef0f6;
}
`

// ── 格式：拦人的三条 ────────────────────────────────────────────────────────

test('格式：无作用域的规则**拒收**（会污染别的主题，是绝对红线）', () => {
  const bad = `:root { --background: #000000; }\n.dark { --background: #111111; }\n`
  const result = parseThemeFile(bad)
  assert.equal(result.ok, false)
  assert.equal(result.ok === false && result.code, 'unscoped_rule')
})

test('格式：主题 id 形状不合法**拒收**（不许与官方预设撞名）', () => {
  for (const id of ['mine', 'user-', 'user-A', 'user-a b', 'paper']) {
    const bad = `[data-theme='${id}'] { --background: #000000; }\n.dark[data-theme='${id}'] { --background: #111111; }\n`
    const result = parseThemeFile(bad)
    assert.equal(result.ok, false, `${id} 本该被拒`)
    assert.equal(result.ok === false && result.code, 'bad_scope_id', `${id} 的错码不对`)
  }
})

test('格式：缺夜间那一档**拒收**（少一档会让日间令牌在夜间被误用）', () => {
  const bad = `[data-theme='user-mine'] { --background: #000000; }\n`
  const result = parseThemeFile(bad)
  assert.equal(result.ok, false)
  assert.equal(result.ok === false && result.code, 'missing_dark')
})

test('格式：外部引用一律**拒收**（本地优先；字体必须随仓库分发）', () => {
  const cases: [string, string][] = [
    ['@import', `@import url(https://example.com/x.css);\n${GOOD_THEME}`],
    ['外部 url', `[data-theme='user-mine'] { --background: url("https://example.com/a.png"); }\n.dark[data-theme='user-mine'] { --background: #111111; }\n`],
    ['@font-face', `@font-face { font-family: X; src: url(/x.woff2); }\n${GOOD_THEME}`],
  ]
  for (const [label, css] of cases) {
    const result = parseThemeFile(css)
    assert.equal(result.ok, false, `${label} 本该被拒`)
    assert.equal(result.ok === false && result.code, 'external_ref', `${label} 的错码不对`)
  }
})

test('格式：一份文件里两个主题 id ⇒ 拒收（不许悄悄只认一个）', () => {
  const bad = `${GOOD_THEME}[data-theme='user-other'] { --background: #000000; }\n`
  const result = parseThemeFile(bad)
  assert.equal(result.ok, false)
  assert.equal(result.ok === false && result.code, 'mixed_scope')
})

test('格式：认得的 at 规则（@media / @supports）放行，别的拒收', () => {
  const ok = `@media (min-width: 700px) {\n  [data-theme='user-mine'] { --background: #000000; }\n}\n.dark[data-theme='user-mine'] { --background: #111111; }\n`
  assert.equal(parseThemeFile(ok).ok, true)

  const bad = `@keyframes spin { from { opacity: 0 } }\n${GOOD_THEME}`
  const result = parseThemeFile(bad)
  assert.equal(result.ok, false)
  assert.equal(result.ok === false && result.code, 'unsupported_at_rule')
})

test('★ 保真：带后代选择器的规则原样保留，**不许被错当成令牌**', () => {
  // 反面样子：把它当令牌，序列化就丢掉 `.dark` 那一节 —— 等于替用户改了他写下的意思。
  const css = `[data-theme='user-mine'] { --background: #000000; }
[data-theme='user-mine'] .dark { --background: #ff0000; }
[data-theme='user-mine'] body { font-family: Georgia, serif; }
.dark[data-theme='user-mine'] { --background: #111111; }
`
  const parsed = parseThemeFile(css)
  assert.equal(parsed.ok, true)
  if (!parsed.ok) return
  assert.equal(parsed.theme.light['--background'], '#000000', '恰好等于作用域的那条才算令牌')
  assert.equal(parsed.theme.formRules.length, 2, '两条带后代的规则都该进形态层')
  const round = serializeThemeFile(parsed.theme)
  assert.match(round, /\[data-theme='user-mine'\] \.dark/, '后代选择器必须原样留下')
  assert.match(round, /\[data-theme='user-mine'\] body/, '形态层的字族必须原样留下')
  // 再解析一次，形态层不许在来回中丢失
  const again = parseThemeFile(round)
  assert.equal(again.ok, true)
  if (again.ok) assert.equal(again.theme.formRules.length, 2)
})

test('序列化**幂等**：parse → serialize → parse 得到同一份', () => {
  const first = parseThemeFile(GOOD_THEME)
  assert.equal(first.ok, true)
  if (!first.ok) return
  const once = serializeThemeFile(first.theme)
  const second = parseThemeFile(once)
  assert.equal(second.ok, true)
  if (!second.ok) return
  assert.equal(serializeThemeFile(second.theme), once)
  assert.deepEqual(second.theme.light, first.theme.light)
  assert.deepEqual(second.theme.dark, first.theme.dark)
})

test('名字来自头注释；没有就用 id（不编名字）', () => {
  const named = parseThemeFile(GOOD_THEME)
  assert.equal(named.ok && named.theme.name, '我的深空')
  const anon = GOOD_THEME.replace(/\/\* celira-theme: .*?\*\/\n/, '')
  const parsed = parseThemeFile(anon)
  assert.equal(parsed.ok && parsed.theme.name, 'user-mine')
})

// ── ★ 官方目录：格式的一致性证明 ────────────────────────────────────────────

test('★ 十套官方主题**每一套**都能被这个格式表达，来回一趟逐项不变', () => {
  for (const preset of THEME_PRESETS) {
    const tokens = catalog[scopeOf(preset.id)]
    assert.ok(tokens, `${preset.id} 在 tokens.css 里找不到令牌块（作用域名字对不上？）`)
    assert.ok(
      Object.keys(tokens.light).length > 0,
      `${preset.id} 的日间令牌表是空的 —— 用它证明格式够用就没有意义了`,
    )
    const id = officialExportId(preset.id)
    const file = serializeThemeFile({
      id,
      name: preset.name,
      light: { ...tokens.light },
      dark: { ...tokens.dark },
      formRules: [],
    })
    const back = parseThemeFile(file)
    assert.equal(back.ok, true, `${preset.id} 导出后居然解析不回来：${back.ok === false ? back.code : ''}`)
    if (!back.ok) continue
    assert.deepEqual(back.theme.light, tokens.light, `${preset.id} 的日间令牌在来回中变了`)
    assert.deepEqual(back.theme.dark, tokens.dark, `${preset.id} 的夜间令牌在来回中变了`)
  }
})

test('★ 体检对官方目录每一套都产出**完整**结论：两档都查、零"算不出来"', () => {
  for (const preset of THEME_PRESETS) {
    const tokens = catalog[scopeOf(preset.id)]!
    const findings = auditTheme(tokens, defaultBase)
    assert.ok(findings.length > 0, `${preset.id} 一条结论都没有（体检没跑？）`)
    const unknown = findings.filter((f) => f.level === 'unknown')
    assert.equal(
      unknown.length,
      0,
      `${preset.id} 有算不出的项（体检对官方主题就成了打折的）：\n${unknown.map((f) => f.detail).join('\n')}`,
    )
    assert.deepEqual(
      [...new Set(findings.map((f) => f.mode))].sort(),
      ['dark', 'light'],
      `${preset.id} 只查了一档 —— 只查当前明暗是最容易犯的懒`,
    )
  }
})

// ⚠ **2026-10-08 实测读数**（本刀建体检时量到的，钉在这里当证据、**不当规则**）：
//    浅色档的"自己"气泡是**渐变**，白字压在渐变**远端**那一段上偏低 ——
//    clay 2.25 / sakura 2.27 / comic 2.72 / glass 2.88 / celira 3.00（`to` 那一端）。
//    这是**官方主题的真实现状**，不是本模块的毛病 ⇒ 本条**不**断言"官方目录零 fail"：
//    那等于用放宽阈值换绿灯（本仓禁止）。官方配色是主人的策展决定，本刀不动它 ——
//    体检照实报，改不改由人决定。
test('★ 对比度检查是醒着的：低对比必须判 `fail`（合成最坏情况，不依赖官方配色）', () => {
  const theme = { light: { '--primary-foreground': '#ffffff', '--primary': '#f0f0f0' }, dark: {} }
  const findings = auditTheme(theme, defaultBase)
  const pair = findings.find((f) => f.mode === 'light' && f.tokens[0] === '--primary-foreground')
  assert.ok(pair, '这一对必须被检查到')
  assert.equal(pair.level, 'fail', '白字压在白底上必须判 fail —— 否则这套体检等于没开')
})

test('★ 体检与门侧同源：`CONTRAST_PAIRS` 与 `variant-lab.mjs` 逐项一致（**连门槛一起**，口径不许漂）', () => {
  const lab = read('../scripts/variant-lab.mjs')
  const start = lab.indexOf('const CONTRAST_PAIRS')
  // 外层数组的收尾括号在行首（内层每一行都以 `],` 结尾）⇒ 用 `\n]` 定位才切得对。
  const end = lab.indexOf('\n]', start)
  assert.ok(start >= 0 && end > start, '没能在 variant-lab.mjs 里定位 CONTRAST_PAIRS')
  const block = lab.slice(start, end)
  // 第三格（门槛）是可选的 ⇒ 正则要能吃下两种形状。
  const pairs = [...block.matchAll(/\[\s*'(--[a-z-]+)'\s*,\s*'(--[a-z-]+)'\s*(?:,\s*([\d.]+)\s*)?\]/g)].map(
    (m) => [m[1]!, m[2]!, m[3] === undefined ? undefined : Number(m[3])],
  )
  assert.ok(pairs.length > 0, '没能从 variant-lab.mjs 里解析出对比度配对（形状变了？）')
  // ⚠ 门槛也要比：只比令牌名的话，一边把 4.5 悄悄改成 2.0 也照样"同源"。
  assert.deepEqual(
    CONTRAST_PAIRS.map((p) => [p[0], p[1], p[2]]),
    pairs,
    '两侧的对比度配对（含门槛）漂了：门那边是权威（真算对比度的是它），改一处必须改两处',
  )
  assert.ok(
    pairs.some((p) => p[2] !== undefined && p[2] !== 4.5),
    '一对带门槛的都没有 —— 门槛那一格等于没在测（形状改了？）',
  )
})

test('★ 色彩声音分类与 `variant-lab.mjs` 的 VOICE_TOKENS 对齐', () => {
  const lab = read('../scripts/variant-lab.mjs')
  const start = lab.indexOf('const VOICE_TOKENS')
  const block = lab.slice(start, lab.indexOf(']', start))
  const voices = [...block.matchAll(/'(--[a-z-]+)'/g)].map((m) => m[1]!)
  assert.ok(voices.length > 0, '没能解析出 VOICE_TOKENS')
  for (const voice of voices) {
    assert.equal(
      classifyToken(voice),
      'voice',
      `${voice} 在 variant-lab 里属"色彩声音"，这里却归了别类 —— 两处的分类必须同源`,
    )
  }
})

// ── 体检：unknown 不许并进 ok ───────────────────────────────────────────────

test('★ 算不出的值报 `unknown`，且**不并进 ok**（"我没检查"≠"没问题"）', () => {
  const theme = { light: { '--primary': 'color-mix(in oklab, red, blue)' }, dark: {} }
  const findings = auditTheme(theme, defaultBase)
  const pair = findings.find((f) => f.tokens.includes('--primary') && f.kind === 'unresolved')
  assert.ok(pair, '--primary 是 color-mix ⇒ 必须有一条"没算出来"')
  assert.equal(pair.level, 'unknown')
  const summary = summarizeFindings(findings)
  assert.ok(summary.unknown > 0, 'unknown 要被单独计数')
  assert.equal(
    summary.ok + summary.weak + summary.fail + summary.unknown,
    findings.length,
    '四档计数必须守恒（不许把 unknown 悄悄并进某一档）',
  )
})

test('★ 五态撞成同一个颜色 ⇒ 判 fail（语义分离被抹平）', () => {
  const same = '#808080'
  const theme = {
    light: {
      '--presence-online': same,
      '--presence-thinking': same,
      '--presence-silent': same,
      '--presence-timeout': same,
      '--presence-failed': same,
    },
    dark: {},
  }
  const findings = auditTheme(theme, defaultBase)
  const presence = findings.filter((f) => f.kind === 'presence')
  assert.ok(presence.length >= 1, '五态必须有一条结论')
  assert.ok(
    presence.some((f) => f.level === 'fail'),
    '五个状态同色必须判 fail，不能因为"设了值"就算通过',
  )
})

test('对比度与感知距离的数学：黑白 21:1、同色 1:1、同色距离 0', () => {
  const black = parseColor('#000000')
  const white = parseColor('#ffffff')
  assert.equal(black.ok && white.ok, true)
  if (!black.ok || !white.ok) return
  assert.ok(Math.abs(contrastRatio(black.rgb, white.rgb) - 21) < 0.01, '黑白应当是 21:1')
  assert.equal(contrastRatio(black.rgb, black.rgb), 1, '同色是 1:1')
  assert.equal(perceptualDistance(white.rgb, white.rgb), 0, '同色距离是 0')
})

test('颜色解析：认得十六进制（三/四/六/八位）、rgb()、hsl()、oklch()', () => {
  assert.equal(parseColor('#abc').ok, true)
  assert.equal(parseColor('#abcd').ok, false, '四位带 alpha ⇒ 不合成，如实说读不到')
  assert.equal(parseColor('#aabbcc').ok, true)
  assert.equal(parseColor('#aabbccdd').ok, false, '八位带 alpha ⇒ 同上')
  assert.equal(parseColor('rgb(1, 2, 3)').ok, true)
  assert.equal(parseColor('rgb(1 2 3)').ok, true)
  assert.equal(parseColor('rgb(1 2 3 / 0.5)').ok, false, '带 alpha ⇒ unknown')
  assert.equal(parseColor('hsl(120 50% 50%)').ok, true)
  assert.equal(parseColor('oklch(0.7 0.1 250)').ok, true)
  assert.equal(parseColor('oklch(70% 0.1 250)').ok, true)
  assert.equal(parseColor('var(--x)').ok, false)
  assert.equal(parseColor('currentColor').ok, false)
  assert.equal(parseColor('').ok, false)
})

test('hsl 与 oklch 换算到 sRGB 后与已知取值吻合（容差 1/255 级别）', () => {
  const hsl = parseColor('hsl(0 100% 50%)')
  assert.equal(hsl.ok, true)
  if (hsl.ok) {
    assert.ok(hsl.rgb.r > 250 && hsl.rgb.g < 5 && hsl.rgb.b < 5, `红不对：${JSON.stringify(hsl.rgb)}`)
  }
  const oklch = parseColor('oklch(1 0 0)')
  assert.equal(oklch.ok, true)
  if (oklch.ok) {
    assert.ok(oklch.rgb.r > 250 && oklch.rgb.g > 250 && oklch.rgb.b > 250, `白不对：${JSON.stringify(oklch.rgb)}`)
  }
  const dark = parseColor('oklch(0 0 0)')
  assert.equal(dark.ok, true)
  if (dark.ok) assert.ok(dark.rgb.r < 3 && dark.rgb.g < 3 && dark.rgb.b < 3, '黑不对')
})

// ── 存储：脏数据诚实丢弃 ＋ 读改写走窄口 ────────────────────────────────────

test('读用户主题（纯解析）：非 JSON / 非数组 / 缺字段 / 解析不回来的一律丢弃', () => {
  const cases: [string, string | null][] = [
    ['没有存储', null],
    ['坏 JSON', '{broken'],
    ['不是数组', '{"id":"user-a"}'],
    ['缺 css', '[{"id":"user-a","name":"A"}]'],
    ['css 解析不回来', '[{"id":"user-a","name":"A","css":":root{}"}]'],
  ]
  for (const [label, raw] of cases) {
    assert.deepEqual(parseUserThemes(raw), [], `${label} 时本该读成空列表`)
  }
  // 形状对的那一条读得回来（正例：免得上面那几条只是"什么都读不出来"）
  const good = parseUserThemes(JSON.stringify([{ id: 'user-a', name: '甲', css: GOOD_THEME }]))
  assert.equal(good.length, 1)
  assert.equal(good[0]!.id, 'user-mine')
})

test('★ 存储生命周期走**窄口**：补水 → 导入 → 覆盖 → 生效 → 删除（键名一字不改）', async () => {
  const before = currentStorage()
  const store: Record<string, string> = {}
  try {
    await resetThemes(store, true)
    // ① 导入一条：落进窄口，快照跟着变（同一个 id 再导入 = 覆盖）
    const first = importUserTheme(GOOD_THEME, defaultBase)
    assert.equal(first.ok, true)
    if (!first.ok) return
    await upsertUserTheme(first.record)
    assert.equal(userThemesSnapshot().length, 1)
    assert.deepEqual(JSON.parse(store['celira-user-themes']!).length, 1)

    const edited = GOOD_THEME.replace('#0e1015', '#12141a')
    const second = importUserTheme(edited, defaultBase)
    assert.equal(second.ok, true)
    if (!second.ok) return
    await upsertUserTheme(second.record)
    const list = userThemesSnapshot()
    assert.equal(list.length, 1, '同一个 id 只该有一条')
    assert.match(list[0]!.css, /#12141a/, '留下的该是后来那一份')

    // ② 生效：写进 `celira-theme-user`，快照同步
    await writeActiveUserThemeId(list[0]!.id)
    assert.equal(activeUserThemeIdSnapshot(), 'user-mine')
    assert.equal(store['celira-theme-user'], 'user-mine')

    // ③ 删除生效中的那条：一并从"当前生效"上摘下来（不留悬空 id），窄口那一格也删掉
    await removeUserTheme('user-mine')
    assert.equal(activeUserThemeIdSnapshot(), null, '删掉的主题不许继续当"当前生效"')
    assert.deepEqual(userThemesSnapshot(), [])
    assert.equal('celira-theme-user' in store, false, '当前生效那一格该被删掉（不是留个空串）')

    // ④ 写不进去（隐私模式 / 壳那头失败）：**不抛**，快照仍然更新（本次会话里照样生效）
    setStorage(fakePort({}, [], false))
    assert.equal(await writeUserThemes([{ id: 'user-x', name: 'X', css: GOOD_THEME }]), false)
    assert.equal(userThemesSnapshot().length, 1, '写失败只影响"下次还在不在"，不影响这一次')
  } finally {
    setStorage(before)
  }
})

test('导入失败时**不改动任何已存的东西**（拒收是拒收，不是半装）', async () => {
  const before = currentStorage()
  const store: Record<string, string> = {}
  try {
    await resetThemes(store, true)
    const ok = importUserTheme(GOOD_THEME, defaultBase)
    assert.equal(ok.ok, true)
    if (!ok.ok) return
    await upsertUserTheme(ok.record)
    const snapshot = JSON.stringify(store)

    const bad = importUserTheme(':root { --background: #000000; }', defaultBase)
    assert.equal(bad.ok, false)
    assert.equal(JSON.stringify(store), snapshot, '被拒的导入不许动存储')
  } finally {
    setStorage(before)
  }
})

// ── 导出 ────────────────────────────────────────────────────────────────────

test('★ 导出官方预设 = 一份**合法且逐项等同**的用户主题文件', () => {
  for (const preset of THEME_PRESETS) {
    const tokens = catalog[scopeOf(preset.id)]!
    const file = buildThemeFile(officialExportId(preset.id), `${preset.name}（副本）`, tokens)
    const parsed = parseThemeFile(serializeThemeFile(file))
    assert.equal(parsed.ok, true, `${preset.id} 导出的文件自己解析不过`)
    if (!parsed.ok) continue
    assert.equal(parsed.theme.id, officialExportId(preset.id))
    assert.deepEqual(parsed.theme.light, tokens.light, `${preset.id} 的日间令牌在导出中变了`)
    assert.deepEqual(parsed.theme.dark, tokens.dark, `${preset.id} 的夜间令牌在导出中变了`)
  }
})

test('官方预设 id → 导出用的用户主题 id 稳定且合法', () => {
  assert.equal(officialExportId('paper'), 'user-paper')
  assert.equal(officialExportId(''), 'user-celira', '默认主题没有作用域名，得有自己的 slug')
  for (const preset of THEME_PRESETS) {
    const id = officialExportId(preset.id)
    const probe = `[data-theme='${id}'] { --background: #000000; }\n.dark[data-theme='${id}'] { --background: #111111; }\n`
    assert.equal(parseThemeFile(probe).ok, true, `${id} 不是合法的作用域 id`)
  }
})

// ── 边界：外观不外传（源码级）───────────────────────────────────────────────

test('★ 外观层零网络、零凭据，且**纯逻辑不碰打包器特性**', () => {  const source = read('../src/lib/userTheme.ts')
  for (const forbidden of ['fetch(', 'XMLHttpRequest', 'WebSocket', 'Authorization', 'Bearer']) {
    assert.ok(!source.includes(forbidden), `外观层出现了「${forbidden}」—— 外观只落本机，不跨任何协议`)
  }
  assert.ok(!/https?:\/\//.test(source), '外观层出现了对外地址')
  // ⚠ 2026-10-08 真的炸过一次：`?raw` 是**打包器**写法，Node 解析到 `…/tokens.css?raw`
  //    直接报 `Unknown file extension ".css"`。它只许出现在 `themeCatalog.ts`；
  //    一旦漏进纯逻辑层，整套离线用例当场红 —— 而且红的原因看着像"代码坏了"。
  //    判据落在**import 的形状**上（注释里提到 ?raw 不算违规，那是解释）。
  assert.ok(
    !/import\s*\([^)]*\.css/.test(source) && !/from\s+'[^']*\.css/.test(source),
    'userTheme.ts 不许 import 任何 .css（那是打包器写法，Node 解析不了）',
  )
  const catalogSource = read('../src/lib/themeCatalog.ts')
  // ⚠ 包必须是**急加载**的（刀 3）：`main.tsx` 在 `createRoot` **之前**注入全部包，
  //    靠的就是它同步可得。改成惰性加载 ⇒ 样式晚一帧到 ⇒ 首帧先出基底配色再跳。
  //    这条判据钉的就是那个决定（顺带：`?raw` 只许出现在这个模块）。
  assert.match(
    catalogSource,
    /import\.meta\.glob\([^)]*eager:\s*true/s,
    '主题包必须急加载（惰性会让首帧闪，且启动那一处注入不到）',
  )
  assert.match(
    catalogSource,
    /import tokensRaw from '\.\.\/styles\/tokens\.css\?raw'/,
    '基底的文本读入在这里（`tokens.css` 现在只剩基底，几 KB，静态读入即可）',
  )
  assert.ok(!/https?:\/\//.test(catalogSource), '目录层出现了对外地址')
})

// ── 刀 2：装之前先看（小样）─────────────────────────────────────────────────

test('★ 保留 id 被拒收（否则"文件自己就叫它"会让小样漏到整个应用）', () => {
  for (const id of RESERVED_SCOPE_IDS) {
    const css = `[data-theme='${id}'] { --background: #000000; }\n.dark[data-theme='${id}'] { --background: #111111; }\n`
    const result = parseThemeFile(css)
    assert.equal(result.ok, false, `${id} 是保留 id，本该被拒`)
    assert.equal(result.ok === false && result.code, 'reserved_id')
  }
  // 只差一个字的不算保留（别把正常 id 一起拦了）
  const ok = parseThemeFile(`[data-theme='${PREVIEW_SCOPE_ID}-2'] { --background: #000000; }\n.dark[data-theme='${PREVIEW_SCOPE_ID}-2'] { --background: #111111; }\n`)
  assert.equal(ok.ok, true)
})

test('★ 小样**不外泄**：每条开块的选择器都带保留作用域，原 id 一处不留', () => {
  const parsed = parseThemeFile(GOOD_THEME)
  assert.equal(parsed.ok, true)
  if (!parsed.ok) return
  const preview = asPreviewTheme(parsed.theme)
  const css = serializeThemeFile(preview)

  // 原 id 一处不留 —— 这条是"预览正在生效中的那套主题不会改到全应用"的直接判据。
  assert.ok(!css.includes("'user-mine'"), `小样里还留着原 id：\n${css}`)
  // 结构判据：形态层每一条都换了作用域
  for (const rule of preview.formRules) {
    assert.match(rule.selector, new RegExp(`data-theme='${PREVIEW_SCOPE_ID}'`), `形态层没换作用域：${rule.selector}`)
  }
  // 文本判据（覆盖序列化器自己产出的那两个令牌块）：**每条开块的选择器**都必须带保留作用域。
  // 这是"只有那张卡能命中"的机械证明 —— `<html>` 上永不写保留 id（保留 id 那条纪律挡住了最后一条路）。
  for (const line of css.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed.endsWith('{')) continue
    assert.ok(
      trimmed.startsWith('/*') || trimmed.includes(`data-theme='${PREVIEW_SCOPE_ID}'`),
      `这条选择器没带保留作用域（小样会漏到全局）：${trimmed}`,
    )
  }
  // ⚠ 小样文本**过不了** `parseThemeFile` —— 保留 id 正是它拒收的。
  //    这不是缺陷，是**一条真性质**：小样是一次性的，**永远不可能被当成一份主题再装一次**。
  //    （判据故意钉成"它必须被拒"，哪天有人给 parseThemeFile 开了后门，这条就红。）
  const back = parseThemeFile(css)
  assert.equal(back.ok, false, '小样文本不该是可导入的主题文件')
  assert.equal(back.ok === false && back.code, 'reserved_id')
  // 令牌逐项不变（小样是同一套，只换作用域）
  assert.deepEqual(preview.light, parsed.theme.light)
  assert.deepEqual(preview.dark, parsed.theme.dark)
  // 纯函数：不许改到传进来的那一份
  assert.equal(parsed.theme.id, 'user-mine')
})

test('★ 小样的形态层也换了作用域（带后代选择器的规则照旧原样保留形状）', () => {
  const withForm = GOOD_THEME.replace(
    ".dark[data-theme='user-mine'] {",
    "[data-theme='user-mine'] body { font-family: Georgia, serif; }\n.dark[data-theme='user-mine'] {",
  )
  const parsed = parseThemeFile(withForm)
  assert.equal(parsed.ok, true)
  if (!parsed.ok) return
  assert.equal(parsed.theme.formRules.length, 1, '夹具本身要带一条形态层规则')

  const css = serializeThemeFile(asPreviewTheme(parsed.theme))
  assert.match(css, new RegExp(`\\[data-theme='${PREVIEW_SCOPE_ID}'\\] body`), '形态层要换成保留作用域')
  assert.ok(!css.includes('user-mine'), '形态层里也不许留原 id')
})

test('★ 小样**不落任何持久面**，且与另外两个 `<style>` 各是各的元素（各有唯一写者）', () => {
  const section = read('../src/components/celira/UserThemeSection.tsx')
  for (const forbidden of ['localStorage', 'sessionStorage', 'indexedDB', 'fetch(', 'https://']) {
    assert.ok(!section.includes(forbidden), `预览这一区出现了「${forbidden}」—— 小样是瞬时 DOM 状态，不落任何持久面`)
  }
  // 主题库由 use-theme 写；小样由本区写。判据落在**赋值**上（不落裸子串 ——
  // 注释里合法地提到对方的名字不该误报，这与守卫那条"不用裸子串误报中立词汇"同规）。
  assert.match(section, /PREVIEW_STYLE_ID = 'celira-theme-preview'/, '小样要有自己的 <style> id')
  assert.ok(!/=\s*'celira-user-themes'/.test(section), '小样不许去写主题库那个元素')
  const useTheme = read('../src/hooks/use-theme.tsx')
  assert.match(useTheme, /USER_STYLE_ID = 'celira-user-themes'/, '主题库那个 id 归 use-theme')
  assert.ok(!/=\s*'celira-theme-preview'/.test(useTheme), 'use-theme 不许碰小样那个元素')
  assert.ok(!/=\s*'celira-theme-packages'/.test(useTheme), '官方包那个元素归启动那一处，use-theme 不许碰')
})

test('★ 主题库装的是**全部已装**（不是只有生效那份）—— 已装列表的真样张靠它', () => {
  const a: UserThemeRecord = { id: 'user-a', name: '甲', css: "[data-theme='user-a'] { --background: #010203; }" }
  const b: UserThemeRecord = { id: 'user-b', name: '乙', css: "[data-theme='user-b'] { --background: #040506; }" }
  const library = userThemeLibraryCss([a, b])
  // 两份都在，且**各自带着自己的作用域**（这才是"没生效的也永不命中"的根据）
  for (const record of [a, b]) {
    assert.ok(library.includes(record.css.trim()), `${record.id} 的样式没进库`)
    assert.ok(library.includes(`[data-theme='${record.id}']`), `${record.id} 在库里丢了作用域`)
  }
  // 空库给**空串**（不是 undefined/null）：装载器写空串而不是删元素，
  // 于是"全删光了"与"从没装过"落到同一个状态。
  assert.equal(userThemeLibraryCss([]), '')
})

test('★ 交互之后才出现的那几句也不许有内行词（sweep 只看默认渲染，结构上看不到它们）', () => {
  // 为什么需要这一条：内行词表本来就有执行者 —— `probe.mjs` 的 `sweep` 随 `npm run verify`
  // 逐条路由扫**渲染后的正文**。但它只看**默认渲染**。本区的检查结果、小样、撞名提示
  // **都要先点一下才出现** ⇒ 那几句在 sweep 眼里**不存在**。
  // 2026-10-08 实测：刀 1 就在这儿漏过一个「读数」（`JARGON` 里明令禁止），verify 一路全绿。
  //
  // 词表**直接取权威那份**（`scripts/jargon.mjs` 文件头：抄回去就是两个权威，一定会漂）。
  const { JARGON } = createRequire(import.meta.url)('../scripts/jargon.mjs') as { JARGON: string[] }
  const source = read('../src/components/celira/UserThemeSection.tsx')
  // 注释里照旧合法（`DESIGN_SYSTEM.md` 明说那是给开发者读的）⇒ **先摘注释再扫**，
  // 否则这条守卫会因为一句解释性注释而误报（与"不用裸子串误报中立词汇"同规）。
  const code = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
  const leaked = JARGON.filter((word) => code.includes(word))
  assert.deepEqual(leaked, [], `用户可见文案里出现了内行词：${leaked.join(' / ')}`)
})

// ── 刀 3：官方主题包（一个包 = 一个文件 = 一个作用域）────────────────────────

test('★ 包目录的不变式：**文件名就是作用域名**，且每个包都过形状校验', () => {
  assert.ok(themePackages.length >= 9, `包目录里只有 ${themePackages.length} 个包`)
  for (const pkg of themePackages) {
    const parsed = parseThemeFile(pkg.text, { requireUserPrefix: false })
    assert.equal(parsed.ok, true, `${pkg.file} 过不了形状校验：${parsed.ok === false ? parsed.code : ''}`)
    if (!parsed.ok) continue
    assert.equal(parsed.theme.id, pkg.id, `${pkg.file} 的作用域名与文件名不一致（运行时按文件名找它）`)
    // 形状校验通过 ⇒ **每条规则都带作用域** ⇒ 这个包不可能污染别的主题（那是格式的第一条硬要求）
    assert.ok(Object.keys(parsed.theme.light).length > 0, `${pkg.file} 的日间令牌表是空的`)
    assert.ok(Object.keys(parsed.theme.dark).length > 0, `${pkg.file} 缺夜间那一档`)
  }
})

test('★ 来源政策那一层确实在起作用：同一个文件"当包能过、当导入过不了"', () => {
  const pkg = themePackages.find((p) => p.id === 'glass')!
  assert.equal(parseThemeFile(pkg.text, { requireUserPrefix: false }).ok, true, '当包（只看形状）应当成立')
  const asImport = parseThemeFile(pkg.text)
  assert.equal(asImport.ok, false, '当导入（要求 user- 前缀）应当被拒')
  assert.equal(asImport.ok === false && asImport.code, 'bad_scope_id')
  // 而**保留 id 两边都不许占用** —— 那不是来源政策，是安全性质（小样会漏到全应用）
  const reserved = `[data-theme='${PREVIEW_SCOPE_ID}'] { --background: #000000; }\n.dark[data-theme='${PREVIEW_SCOPE_ID}'] { --background: #111111; }\n`
  assert.equal(parseThemeFile(reserved, { requireUserPrefix: false }).ok, false, '包也不许占用保留 id')
})

test('★ 十套官方主题在"基底 ＋ 包"里都找得到，且没有孤儿包', () => {
  for (const preset of THEME_PRESETS) {
    const tokens = catalog[scopeOf(preset.id)]
    assert.ok(
      tokens && Object.keys(tokens.light).length > 0,
      `${preset.id} 既不在基底（tokens.css）也不在包目录里 —— 它掉在两处之外了`,
    )
  }
  // 反向：包目录里不许有"没登记进目录"的孤儿包（加了包忘登记，或删了登记没删包）
  const known = new Set(THEME_PRESETS.map((p) => scopeOf(p.id)))
  for (const pkg of themePackages) {
    assert.ok(known.has(pkg.id), `${pkg.file} 是个孤儿包：THEME_PRESETS 里没有 ${pkg.id}`)
  }
})
