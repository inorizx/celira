// 配色变体策展辅助：把 tokens.css 各主题块的「色彩声音」令牌转成 oklch
// 读数（l/c/h），供人工挑选变体色相；并支持按目标色相生成覆盖层 CSS、
// 对最终 CSS 做 WCAG 对比度体检。
// 运行：node scripts/variant-lab.mjs        → 打印现状表
//       node scripts/variant-lab.mjs gen    → 由 VARIANT_PLAN 生成 CSS 到 stdout
//       node scripts/variant-lab.mjs check  → 体检 tokens.css 现状（含变体层）
import { readFileSync, readdirSync } from 'node:fs'

/**
 * 数据来源 = **基底 ＋ 官方主题包**（刀 3：官方主题从 `tokens.css` 搬进了
 * `src/styles/themes/*.css`，一个包 = 一个文件 = 一个作用域）。
 *
 * 为什么这样拼就够了：下面 `extractBlock` 按选择器找块、`check` 从「配色变体层」那行注释
 * 往后切 —— 把包文本**接在 `tokens.css` 之后**，两条路都照旧找得到（包里的变体块
 * 也都排在那一行注释之后）。
 */
const themesDir = new URL('../src/styles/themes/', import.meta.url)
const css = [
  readFileSync(new URL('../src/styles/tokens.css', import.meta.url), 'utf8'),
  ...readdirSync(themesDir)
    .filter((f) => f.endsWith('.css'))
    .sort()
    .map((f) => readFileSync(new URL(f, themesDir), 'utf8')),
].join('\n')

// 变体只接管「色彩声音」令牌；前景色（primary-foreground 等）与背景/表面
// 结构不动，保证对比度与主题气质稳定。
const VOICE_TOKENS = [
  '--primary',
  '--brand',
  '--ring',
  '--bubble-self-from',
  '--bubble-self-to',
  '--accent',
  '--accent-foreground',
]

/**
 * 对比度配对。第三格是这一对的**门槛**，省略即 AA 正文。
 *
 * ⚠ **与 `src/lib/themeAudit.ts` 的 `CONTRAST_PAIRS` 必须逐项一致（连门槛一起）** ——
 * `tests/user-theme.test.ts` 钉着这条同源；两份表分叉过就有一边在骗人。
 *
 * ⚠ `--border` / `--input` 这两族**曾经只被一次性审计脚本查过、从未进门禁**，
 * 于是 paper 的输入框描边一路低到 1.38 而没人拦（2026-10-17 记账）。
 */
const CONTRAST_PAIRS = [
  ['--primary-foreground', '--primary'],
  ['--bubble-self-foreground', '--bubble-self-from'],
  ['--bubble-self-foreground', '--bubble-self-to'],
  ['--accent-foreground', '--accent'],
  ['--foreground', '--background'],
  ['--card-foreground', '--card'],
  ['--popover-foreground', '--popover'],
  ['--bubble-peer-foreground', '--bubble-peer'],
  ['--muted-foreground', '--muted'],
  // 非文字（WCAG 1.4.11）：焦点环要在它可能出现的每个面上看得见
  ['--ring', '--card', 3],
  ['--ring', '--background', 3],
  ['--ring', '--muted', 3],
  ['--ring', '--accent', 3],
  // 表面分离：不靠发丝线分层时，至少得看得见那条线
  ['--border', '--card', 1.3],
  ['--border', '--background', 1.3],
  ['--input', '--card', 2],
  ['--input', '--background', 2],
]

/** AA 正文底线；浮点比较留一个极小的容差（`4.50:1` 这种刚好压线的值不许因舍入判红）。 */
const AA = 4.5
const EPS = 1e-9
/** WCAG 1.4.11 非文字那一档：焦点环、只当指示点用的状态色。 */
const GRAPHIC = 3.0

/**
 * 「文字令牌 × 表面」——**每一个文字令牌都可能压在任何一个表面上**。
 *
 * `CONTRAST_PAIRS` 那 9 对是「一个前景配一个它专属的底」（`--primary-foreground` 只压
 * `--primary`），但下面这些是**同一个令牌出现在好几个底上**：`--primary` 既是主按钮的底、
 * 又是选中态的文字色（`ConversationItem` 选中行 `bg-accent` ＋ `text-primary`）；
 * `--destructive` 既压卡片也压选中行；code 八色压在 `--muted`（`MarkdownText` 的代码块底）。
 * 只按白卡片解会漏 —— 实测漏过 `--agent` 压 accent 4.43、`--muted-foreground` 压 accent 4.38。
 */
const TEXT_TOKENS = [
  '--primary',
  '--agent',
  '--destructive',
  '--presence-failed',
  '--presence-online',
  '--presence-timeout',
  '--presence-thinking',
  '--muted-foreground',
  '--accent-foreground',
]
const SURFACE_TOKENS = ['--card', '--background', '--muted', '--accent']
const CODE_TOKENS = [
  '--code-keyword',
  '--code-string',
  '--code-comment',
  '--code-number',
  '--code-function',
  '--code-type',
  '--code-variable',
  '--code-meta',
]
/** 只当指示点用、不作文字的状态色（`text-presence-silent` 全仓不存在）。 */
const GRAPHIC_TOKENS = ['--presence-silent']

/**
 * **全面覆盖**的主题（`主题/模式`）：除 `CONTRAST_PAIRS` 外，还要过
 * 「文字令牌 × 四个表面」「code 八色 × `--muted`」「非文字 × 四个表面」。
 *
 * ## 为什么按主题分档，而不是把上面这些直接塞进 `CONTRAST_PAIRS`
 *
 * 实测过：把这一整套当绝对底线压上去，**当时 10 套主题里有 8 套会新增几百条越线**。
 * 那几百条不是一张登记表，那是"把几套主题重做一遍"—— 压暗填充或翻深字都会改掉
 * 各主题的身份（sakura 的粉、clay 的陶土、pixel 的暖屏），逐主题收编是各自的视觉裁决。
 *
 * 所以这一档是**棘轮**：进了这张表的主题，从此在这些组合上不许再退；
 * 没进的仍按旧口径（只有 `CONTRAST_PAIRS`）跑，缺口如实记在下面，不假装已经覆盖。
 *
 * ## 缺口（2026-10-17 实测）
 *
 * **零。** 十套主题（`celira` `glass` `contrast` `paper` `comic` `dazzle`
 * `clay` `neon` `pixel` `sakura`）的日/夜两档已全部收编 —— 上面那几百条同类越线
 * 在 2026-10-17 这几刀里逐套修掉了（共 20 个块，约 230 个令牌值）。
 * **新主题一律先进这张表**：它是新的，没有存量债可欠。
 */
const FULL_COVERAGE = new Set([
  'celira/light',
  'celira/dark',
  'glass/light',
  'glass/dark',
  'contrast/light',
  'contrast/dark',
  'paper/light',
  'paper/dark',
  'comic/light',
  'comic/dark',
  'dazzle/light',
  'dazzle/dark',
  'clay/light',
  'clay/dark',
  'neon/light',
  'neon/dark',
  'pixel/light',
  'pixel/dark',
  'sakura/light',
  'sakura/dark',
])

/**
 * 已登记的存量债务：`主题/模式` -> `前景令牌|背景令牌` -> 登记时的实测比值。
 *
 * ## 为什么基色要进绝对门禁（2026-10-17 刀）
 *
 * 旧口径是**只**看「变体有没有比本主题基色劣化 0.25」，绝对值低于 4.5 只记 WARN。
 * 那条规则有个盲区：**基色自己永远免检**（拿自己比自己，永远不劣化）。实测代价 ——
 * 默认主题日间的 `--primary-foreground on --primary` 是 3.33:1、
 * 自己的气泡是 3.00:1，三条配色变体全都贴着它过关（3.30 / 3.33 / 3.46），
 * 门禁打印 99 条 WARN 之后**退出码 0**，于是整族问题活了下来。
 *
 * 而这两族**不受色相约束** —— 白字压不住就翻成深字（夜间就是这么做的：
 * 同一对令牌在夜间是 6.27:1），所以"前景色选错了"必须被拦住，
 * 不能当成"主题设计语言"放过。变体那一侧仍走相对规则，理由是成立的：
 * 同 oklch 明度下换色相，WCAG 亮度天然会变，绝对值该由基色那一行负责。
 *
 * ## 它是一条棘轮，不是豁免
 *
 * - 实测**低于**登记值 ⇒ 判红（只许变好，不许变坏）；
 * - 新出现的、没登记的越线 ⇒ 判红；
 * - 修好一条就**从表里删一条**（删空一个主题就删掉那一项）。
 *
 * 下表这些**本轮不动**：它们分属各主题包自己的视觉取舍（sakura 的粉、
 * clay 的陶土色、pixel 的 Game Boy 绿——压暗填充或翻深字都会改变该主题的身份），
 * 逐主题重做属于各自的视觉裁决，不在一刀里顺手改。
 */
const KNOWN_BELOW_AA = {
  // 2026-10-17：**空表**。十套主题的日/夜两档全部收编（见 FULL_COVERAGE），
  // 上面那几百条同类越线逐套修掉了。机制留着 —— 将来若某套主题确有
  // 「本轮不动、但已知越线」的条目，按 主题/模式 -> 前景|背景 -> 实测值 记在这里，
  // 判红规则见文件头：低于登记值判红、新越线判红、修好一条删一条。
}

// 表面轻染色：变体对中性表面做同明度（l 不动）的色相转向 + 彩度下限，
// 让整套氛围随配色走；彩度下限刻意很低（0.01 量级），只染气质不染结构。
// paper（米色纸面）与 contrast（纯黑白）的表面是主题本体，豁免染色。
const SURFACE_TINT = {
  '--background': 0.01,
  '--card': 0.012,
  '--popover': 0.012,
  '--secondary': 0.016,
  '--muted': 0.014,
  '--border': 0.012,
  '--input': 0.012,
  '--bubble-peer': 0.012,
}
const TINT_OFF = new Set(['paper', 'contrast'])

const BLOCKS = [
  { key: 'celira', mode: 'light', selector: ':root' },
  { key: 'celira', mode: 'dark', selector: '.dark' },
  { key: 'glass', mode: 'light', selector: "[data-theme='glass']" },
  { key: 'glass', mode: 'dark', selector: ".dark[data-theme='glass']" },
  { key: 'neon', mode: 'light', selector: "[data-theme='neon']" },
  { key: 'neon', mode: 'dark', selector: ".dark[data-theme='neon']" },
  { key: 'paper', mode: 'light', selector: "[data-theme='paper']" },
  { key: 'paper', mode: 'dark', selector: ".dark[data-theme='paper']" },
  { key: 'clay', mode: 'light', selector: "[data-theme='clay']" },
  { key: 'clay', mode: 'dark', selector: ".dark[data-theme='clay']" },
  { key: 'comic', mode: 'light', selector: "[data-theme='comic']" },
  { key: 'comic', mode: 'dark', selector: ".dark[data-theme='comic']" },
  { key: 'contrast', mode: 'light', selector: "[data-theme='contrast']" },
  { key: 'contrast', mode: 'dark', selector: ".dark[data-theme='contrast']" },
  { key: 'pixel', mode: 'light', selector: "[data-theme='pixel']" },
  { key: 'pixel', mode: 'dark', selector: ".dark[data-theme='pixel']" },
  { key: 'sakura', mode: 'light', selector: "[data-theme='sakura']" },
  { key: 'sakura', mode: 'dark', selector: ".dark[data-theme='sakura']" },
  { key: 'dazzle', mode: 'light', selector: "[data-theme='dazzle']" },
  { key: 'dazzle', mode: 'dark', selector: ".dark[data-theme='dazzle']" },
]

function extractBlock(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const m = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\}`, 'm'))
  return m ? m[1] : null
}

// sRGB hex → oklch（CSS Color 4 标准换算）
function hexToOklch(hex) {
  const n = parseInt(hex.slice(1), 16)
  let r = ((n >> 16) & 255) / 255
  let g = ((n >> 8) & 255) / 255
  let b = (n & 255) / 255
  const lin = (c) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4))
  r = lin(r)
  g = lin(g)
  b = lin(b)
  const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b
  const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b
  const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b
  const l_ = Math.cbrt(l)
  const m_ = Math.cbrt(m)
  const s_ = Math.cbrt(s)
  const L = 0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_
  const a = 1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_
  const bb = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_
  const C = Math.sqrt(a * a + bb * bb)
  let H = (Math.atan2(bb, a) * 180) / Math.PI
  if (H < 0) H += 360
  return { l: L, c: C, h: H }
}

function oklchToHex({ l: L, c: C, h: H }) {
  const hr = (H * Math.PI) / 180
  const a = C * Math.cos(hr)
  const bb = C * Math.sin(hr)
  const l_ = L + 0.3963377774 * a + 0.2158037573 * bb
  const m_ = L - 0.1055613458 * a - 0.0638541728 * bb
  const s_ = L - 0.0894841775 * a - 1.291485548 * bb
  const l3 = l_ ** 3
  const m3 = m_ ** 3
  const s3 = s_ ** 3
  let r = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3
  let g = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3
  let b = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3
  const gam = (c) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055)
  const to255 = (c) => Math.round(Math.min(1, Math.max(0, gam(c))) * 255)
  return '#' + [to255(r), to255(g), to255(b)].map((v) => v.toString(16).padStart(2, '0')).join('')
}

// WCAG 相对亮度与对比度
function luminance(hex) {
  const n = parseInt(hex.slice(1), 16)
  const lin = (c) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4))
  const r = lin(((n >> 16) & 255) / 255)
  const g = lin(((n >> 8) & 255) / 255)
  const b = lin((n & 255) / 255)
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(fg, bg) {
  const l1 = luminance(fg)
  const l2 = luminance(bg)
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05)
}

function readTokens(body, list) {
  const out = {}
  for (const t of list) {
    const m = body.match(new RegExp(`${t}\\s*:\\s*(#[0-9a-fA-F]{6})`))
    if (m) out[t] = m[1].toLowerCase()
  }
  return out
}

const mode = process.argv[2] ?? 'table'

if (mode === 'table') {
  for (const b of BLOCKS) {
    const body = extractBlock(b.selector)
    if (!body) {
      console.log(`${b.key}/${b.mode}: BLOCK NOT FOUND`)
      continue
    }
    const tokens = readTokens(body, VOICE_TOKENS)
    console.log(`\n== ${b.key} (${b.mode}) ==`)
    for (const t of VOICE_TOKENS) {
      const hex = tokens[t]
      if (!hex) continue
      const { l, c, h } = hexToOklch(hex)
      console.log(`  ${t.padEnd(24)} ${hex}  l=${l.toFixed(3)} c=${c.toFixed(3)} h=${h.toFixed(1)}`)
    }
  }
} else if (mode === 'check') {
  // ===== 对比度体检：基色 + 变体层最终生效值 =====
  // ⚠ 这份名单必须覆盖**所有会被查的令牌**：`readTokens` 只读名单里的键，
  // 漏一个那一项就被静默跳过 —— 门禁会显示"绿"，而它一项都没查。
  const CHECK_TOKENS = [
    ...new Set([
      ...VOICE_TOKENS,
      ...CONTRAST_PAIRS.flat().filter((x) => typeof x === 'string'),
      ...TEXT_TOKENS,
      ...SURFACE_TOKENS,
      ...CODE_TOKENS,
      ...GRAPHIC_TOKENS,
    ]),
  ]
  const layerStart = css.indexOf('===== 配色变体层 =====')
  const layer = layerStart >= 0 ? css.slice(layerStart) : ''
  const variantBlocks = new Map()
  const blockRe = /^(\.dark)?(?::root|\[data-theme='(\w+)'\])(?::not\(\.dark\))?\[data-variant='(\w+)'\](?:,\s*\n(?:\.dark)?\[data-theme='\w+'\](?::not\(\.dark\))?\[data-variant='\3'\])?\s*\{([\s\S]*?)\}/gm
  let m
  while ((m = blockRe.exec(layer)) !== null) {
    const theme = m[2] ?? 'celira'
    const variant = m[3]
    const vmode = m[1] ? 'dark' : 'light'
    variantBlocks.set(`${theme}/${variant}/${vmode}`, readTokens(m[4], CHECK_TOKENS))
  }
  // 高对比（contrast）夜景基色是纯黄 #ffff00——sRGB 亮度天花板，任何
  // 换色都无法与之等比；该主题夜景的变体门禁改为绝对底线 AAA 7:1。
  const GATE_OVERRIDES = { 'contrast/dark': { absolute: 7 } }
  let failures = 0
  let debt = 0
  for (const b of BLOCKS) {
    const body = extractBlock(b.selector)
    if (!body) continue
    const base = readTokens(body, CHECK_TOKENS)
    const rows = [{ label: 'standard', tokens: {} }]
    for (const [k, tokens] of variantBlocks) {
      const [theme, variant, vmode] = k.split('/')
      if (theme === b.key && vmode === b.mode) rows.push({ label: variant, tokens })
    }
    // 这一档要查哪些对：`FULL_COVERAGE` 里的块加上「文字 × 表面 / code / 非文字」。
    const pairs = CONTRAST_PAIRS.map(([f, g, floor]) => [f, g, floor ?? AA])
    if (FULL_COVERAGE.has(`${b.key}/${b.mode}`)) {
      for (const t of TEXT_TOKENS) for (const s of SURFACE_TOKENS) pairs.push([t, s, AA])
      for (const c of CODE_TOKENS) pairs.push([c, '--muted', AA])
      for (const t of GRAPHIC_TOKENS) for (const s of SURFACE_TOKENS) pairs.push([t, s, GRAPHIC])
    }
    const seen = new Set()
    const deduped = pairs.filter(([f, g]) => {
      const k = `${f}|${g}`
      if (seen.has(k)) return false
      seen.add(k)
      return true
    })
    for (const row of rows) {
      const eff = { ...base, ...row.tokens }
      for (const [fgT, bgT, floor] of deduped) {
        if (!eff[fgT] || !eff[bgT]) continue
        const ratio = contrast(eff[fgT], eff[bgT])
        const baseRatio = contrast(base[fgT], base[bgT])
        const registered = KNOWN_BELOW_AA[`${b.key}/${b.mode}`]?.[`${fgT}|${bgT}`]
        let level
        if (row.label === 'standard') {
          // **基色走绝对底线**：见 KNOWN_BELOW_AA 的文件头说明。
          // 登记值按两位小数记（与打印一致）⇒ 比较留同样量级的容差，
          // 否则 `4.11` 这种登记值会被自己打印出来的那个数判红。
          if (ratio + EPS >= floor) level = 'ok'
          else if (registered === undefined) level = 'FAIL'
          else if (ratio + 0.005 < registered) level = 'FAIL'
          else level = 'DEBT'
        } else {
          // 变体走**相对规则，但以绝对底线封顶**：`min(基色 - 0.25, 底线)`。
          //
          // 为什么是 min：相对规则当初的理由是「色相在同等 oklch 明度下的 WCAG 亮度
          // 天然不同，绝对值基线归主题设计语言」（`THEME_GUIDE.md`「可读性」）。
          // 那条理由只在**基色自己没到标准**时才有意义 —— 基色在标准之上时，
          // 变体只要也过线就没有"劣化"可言。实测代价：`contrast` 主题日间的
          // amber / emerald 变体在 6.47–9.45（远在 AA 之上）被判红，
          // 只因为基色是 7.03/9.92。那是噪音，不是缺陷。
          // 基色低于标准时（`KNOWN_BELOW_AA` 里那些），min 取的就是旧口径，
          // 变体仍被钉在「不比基色劣化 0.25」上 —— 老行为一字不变。
          //
          // `contrast/dark` 的 AAA 提档同理要**以基色封顶**：那条规则是为纯黄基色
          // （亮度天花板、相对比较无意义）写的，但一条「变体必须 ≥7」的规则
          // 会在基色自己只有 4.94 时要求变体超过基色 —— 那是自相矛盾的。
          const override = GATE_OVERRIDES[`${b.key}/${b.mode}`]
          const limit = override
            ? Math.min(override.absolute, baseRatio)
            : Math.min(baseRatio - 0.25, floor)
          if (ratio + EPS < limit) level = 'FAIL'
          else if (ratio + EPS < floor) level = 'WARN'
          else level = 'ok'
        }
        if (level === 'FAIL') failures++
        if (level === 'DEBT') debt++
        const tag = { ok: ' ok ', WARN: 'WARN', FAIL: 'FAIL', DEBT: 'DEBT' }[level]
        const note =
          level === 'DEBT'
            ? `（已登记 ${registered.toFixed(2)}，不许再低）`
            : `（基色 ${baseRatio.toFixed(2)}:1）`
        console.log(`${tag} ${b.key}/${b.mode}/${row.label}  ${fgT} on ${bgT}: ${ratio.toFixed(2)}:1${note}`)
      }
    }
  }
  if (debt > 0) {
    console.log(`\n${debt} 项已登记的存量债务（低于 AA 但本轮不动，只许变好不许变坏）`)
  }
  console.log(failures === 0 ? '\n全部通过' : `\n${failures} 项未达标`)
  process.exit(failures === 0 ? 0 : 1)
} else if (mode === 'gen') {
  // ===== 策展计划：每主题每变体的目标色相（oklch h，日/夜分开）=====
  // 注：近中性灰阶（彩度过低）的色相旋转无意义，会被 SKIP；这类主题
  // 如需变体，应在 tokens.css 配色变体层手工调色，不经本脚本生成。
  // 规则：默认保留原令牌明度 l / 彩度 c，只把 h 拧到策展目标；
  // 每变体可按模式微调——lightLScale/darkLScale 乘明度（contrast 夜景
  // 顶格明度换色时必须降明度保彩度）、lightCScale/darkCScale 调彩度浓淡、
  // lightDl/darkDl 平移明度；bubble-self-to 保持原渐变方向（h 偏移量继承）。
  // paper 的色彩声音本来就是印章色，只覆盖 brand/ring/accent-foreground。
  const VARIANT_PLAN = {
    celira: {
      azure: { light: 250, dark: 250, lightCScale: 1.06, darkCScale: 1.06 },
      // 蔷薇夜景不染色表面：低明度下 348° 的染色是"脏粉棕"，脏；
      // 正确做法是中性石板底 + 让粉色行动色独自发光（暗夜玫瑰）。
      rose: { light: 348, dark: 348, lightCScale: 1.04, darkCScale: 1.04, darkTint: false },
    },
    glass: {
      aurora: { light: 196, dark: 198, lightCScale: 1.12, darkCScale: 1.1 },
      dusk: { light: 40, dark: 42, lightCScale: 1.06, darkCScale: 1.06 },
    },
    neon: {
      // 夜景气泡是白字（主按钮是深字），青/绿转亮后需单独压深气泡底。
      cyber: {
        light: 192,
        dark: 192,
        darkTokenDl: { '--bubble-self-from': -0.06, '--bubble-self-to': -0.05 },
      },
      acid: {
        light: 126,
        dark: 124,
        lightCScale: 1.04,
        darkCScale: 1.04,
        darkTokenDl: { '--bubble-self-from': -0.06, '--bubble-self-to': -0.05 },
      },
    },
    paper: {
      // 印章色只换 brand/ring/accent-foreground；accent 底是米色不转，
      // 靛青/松绿在同等明度下偏亮，前景单独压深一档保对比。
      indigo: {
        light: 265,
        dark: 265,
        tokens: ['--brand', '--ring', '--accent-foreground'],
        lightTokenDl: { '--accent-foreground': -0.045 },
      },
      pine: {
        light: 158,
        dark: 158,
        tokens: ['--brand', '--ring', '--accent-foreground'],
        lightTokenDl: { '--accent-foreground': -0.045 },
      },
    },
    clay: {
      sage: { light: 150, dark: 152, lightCScale: 0.94, darkCScale: 0.96 },
      terracotta: { light: 46, dark: 44, lightCScale: 1.05, darkCScale: 1.05 },
    },
    comic: {
      hero: { light: 250, dark: 250 },
      bubblegum: { light: 336, dark: 336, lightCScale: 1.03, darkCScale: 1.03 },
    },
    // contrast 夜景原色是纯黄（l≈0.97 顶格明度），换色必须同步降明度，
    // 否则色域收敛会把彩度磨光、颜色退成近白。
    contrast: {
      amber: {
        light: 66,
        dark: 76,
        darkLScale: 0.85,
        lightTokenDl: { '--accent-foreground': -0.02 },
      },
      emerald: {
        light: 154,
        dark: 152,
        darkLScale: 0.87,
        lightTokenDl: { '--accent-foreground': -0.02 },
      },
    },
    pixel: {
      arcade: { light: 250, dark: 252 },
      gameboy: { light: 126, dark: 124, lightCScale: 0.92, darkCScale: 0.94 },
    },
    sakura: {
      matcha: {
        light: 132,
        dark: 134,
        lightCScale: 0.96,
        darkCScale: 0.98,
        lightTokenDl: { '--accent-foreground': -0.03 },
      },
      wisteria: { light: 298, dark: 300, lightCScale: 1.02, darkCScale: 1.02 },
    },
    // 炫光：灯带的另一半色相——品红更艳、青柠更电；彩度都提一档保持张扬。
    dazzle: {
      flare: {
        light: 322,
        dark: 322,
        lightCScale: 1.06,
        darkCScale: 1.06,
        lightTokenDl: { '--bubble-self-to': 0.015 },
      },
      pulse: {
        light: 145,
        dark: 148,
        lightCScale: 1.05,
        darkCScale: 1.05,
        darkTint: false,
        lightTokenDl: { '--bubble-self-to': 0.055 },
        darkTokenDl: { '--bubble-self-to': 0.02 },
      },
    },
  }

  const CHROMA_FLOOR = 0.03

  function fitsGamut({ l, c, h }) {
    const hr = (h * Math.PI) / 180
    const a = c * Math.cos(hr)
    const bb = c * Math.sin(hr)
    const l_ = l + 0.3963377774 * a + 0.2158037573 * bb
    const m_ = l - 0.1055613458 * a - 0.0638541728 * bb
    const s_ = l - 0.0894841775 * a - 1.291485548 * bb
    const l3 = l_ ** 3
    const m3 = m_ ** 3
    const s3 = s_ ** 3
    const r = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3
    const g = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3
    const b = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3
    return r >= 0 && r <= 1 && g >= 0 && g <= 1 && b >= 0 && b <= 1
  }

  // 目标色相下若超出 sRGB 色域，逐步降彩度直到放得下（保明度/色相优先）；
  // lScale/dl 用于顶格明度或需刻意压深/提亮的策展微调。
  function placeHue(orig, hTarget, cScale = 1, lScale = 1, dl = 0) {
    const l = Math.min(1, Math.max(0, orig.l * lScale + dl))
    let c = orig.c * cScale
    while (c > 0.001 && !fitsGamut({ l, c, h: hTarget })) c *= 0.92
    return oklchToHex({ l, c, h: hTarget })
  }

  const lines = []
  lines.push('/* ===== 配色变体层 =====')
  lines.push('   按主题手工策展的整套配色（目录见 src/lib/themes.ts 各 preset.variants）：')
  lines.push('   覆盖「色彩声音」令牌（primary/brand/ring/bubble-self/accent 系），')
  lines.push('   并对背景/卡片/气泡等中性表面做同明度轻染色，让氛围随配色整体转向；')
  lines.push('   明度档位一律保持 → 对比度不劣化；agent 系、presence 五态与')
  lines.push('   destructive/code 语义色永不触碰；paper/contrast 表面是主题本体，不染。')
  lines.push("   'standard' 不写 data-variant。特异性高于主题块，日/夜各一档。")
  lines.push('   数值由 scripts/variant-lab.mjs gen 生成（oklch 保 l 拧 h，色域内收敛），')
  lines.push('   生成后必须跑 node scripts/variant-lab.mjs check 过对比度体检。 */')
  lines.push('')

  for (const b of BLOCKS) {
    const plan = VARIANT_PLAN[b.key]
    if (!plan) continue
    const body = extractBlock(b.selector)
    const tokens = readTokens(body, [
      ...VOICE_TOKENS,
      '--primary-foreground',
      ...Object.keys(SURFACE_TINT),
    ])
    const baseFg = tokens['--primary-foreground']
    const basePrimaryHex = tokens['--primary']
    for (const [variantId, v] of Object.entries(plan)) {
      const hue = v[b.mode]
      const tokenList = v.tokens ?? VOICE_TOKENS
      const lScale = b.mode === 'dark' ? (v.darkLScale ?? 1) : (v.lightLScale ?? 1)
      const cScale = b.mode === 'dark' ? (v.darkCScale ?? 1) : (v.lightCScale ?? 1)
      const dlManual = b.mode === 'dark' ? (v.darkDl ?? 0) : (v.lightDl ?? 0)
      // 对比度自动均衡：等 oklch 明度下不同色相的 WCAG 感知亮度天然不同
      // （蓝/青/绿系偏亮、品红/红系偏暗），直接旋转会让白字按钮可读性漂移。
      // 以基色 primary 对白字前景的对比度为靶，双向扫描明度平移量 dl，
      // 取达到靶值（容忍 -0.05）且最接近 0 的解；同一变体所有令牌共用该 dl。
      let dlAuto = 0
      if (v.matchContrast !== false && baseFg && basePrimaryHex && tokenList.includes('--primary')) {
        const target = contrast(baseFg, basePrimaryHex)
        const orig = hexToOklch(basePrimaryHex)
        let best = { dl: 0, ratio: contrast(baseFg, placeHue(orig, hue, cScale, lScale, 0)) }
        for (let d = -0.25; d <= 0.12; d += 0.005) {
          const ratio = contrast(baseFg, placeHue(orig, hue, cScale, lScale, d))
          const okHit = ratio >= target - 0.05
          const bestHit = best.ratio >= target - 0.05
          if ((okHit && !bestHit) || (okHit === bestHit && Math.abs(d) < Math.abs(best.dl))) {
            best = { dl: d, ratio }
          }
        }
        dlAuto = best.dl
      }
      const dl = dlManual + dlAuto
      // 单令牌微调：个别令牌的前景/背景明暗方向与 primary 不一致时
      // （如 neon 夜景气泡是白字、主按钮是深字），按令牌追加明度平移。
      const tokenDl = (b.mode === 'dark' ? v.darkTokenDl : v.lightTokenDl) ?? {}
      const entries = []
      for (const t of tokenList) {
        const hex = tokens[t]
        if (!hex) continue
        const orig = hexToOklch(hex)
        if (orig.c < CHROMA_FLOOR) continue
        let hTarget = hue
        if (t === '--bubble-self-to' && tokenList.includes('--bubble-self-from')) {
          const fromHex = tokens['--bubble-self-from']
          if (fromHex) {
            const fromH = hexToOklch(fromHex).h
            const toH = orig.h
            hTarget = hue + (toH - fromH)
          }
        }
        entries.push(`  ${t}: ${placeHue(orig, hTarget, cScale, lScale, dl + (tokenDl[t] ?? 0))};`)
      }
      // 表面轻染色：同明度转向变体色相；原彩度高于下限的表面（如 clay 暖陶底）
      // 保留自身彩度只转向，近中性表面取下限彩度。
      // 单变体可按模式豁免（如 celira/rose 夜景：低明度暖色染色显脏，
      // 豁免后保持中性石板底，只让行动色发光）。
      const tintOff =
        TINT_OFF.has(b.key) || (b.mode === 'dark' ? v.darkTint === false : v.lightTint === false)
      if (!tintOff) {
        for (const [t, floor] of Object.entries(SURFACE_TINT)) {
          const hex = tokens[t]
          if (!hex) continue
          const orig = hexToOklch(hex)
          entries.push(`  ${t}: ${placeHue({ ...orig, c: Math.max(orig.c, floor) }, hue)};`)
        }
      }
      if (entries.length === 0) continue
      // celira 是默认预设：html 不写 data-theme，需同时覆盖 :root 用法与
      // 嵌套预览（预览卡写 data-theme='celira'）两种命中路径。
      // 所有日景选择器都必须排除 .dark——否则夜景下日景变体块依然命中，
      // 一旦该变体夜景豁免表面染色（darkTint:false），日景表面会泄漏进夜景。
      const sel =
        b.key === 'celira'
          ? b.mode === 'light'
            ? `:root:not(.dark)[data-variant='${variantId}'],\n[data-theme='celira']:not(.dark)[data-variant='${variantId}']`
            : `.dark[data-variant='${variantId}'],\n.dark[data-theme='celira'][data-variant='${variantId}']`
          : b.mode === 'light'
            ? `[data-theme='${b.key}']:not(.dark)[data-variant='${variantId}']`
            : `.dark[data-theme='${b.key}'][data-variant='${variantId}']`
      lines.push(`/* ${b.key} · ${variantId}（${b.mode === 'light' ? '日' : '夜'}） */`)
      lines.push(`${sel} {`)
      lines.push(...entries)
      lines.push('}')
      lines.push('')
    }
  }
  console.log(lines.join('\n'))
}
