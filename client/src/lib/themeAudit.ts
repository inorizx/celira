/**
 * 主题体检：**照实报，不拦人**。纯逻辑，零 DOM。
 *
 * ## 为什么"不拦人"是设计而不是宽容
 *
 * 格式错误（`themeFile.ts`）拦人，因为那种文件**装载不了**；体检的结论拦不了人，
 * 因为"好不好看"没法机械判定。它只回答**机械可算**的那几件事：
 *
 *  1. **对比度**（WCAG）：正文/前景与它所在的表面；
 *  2. **presence 五态两两可辨**：回复、明确沉默、超时、失败必须语义分离，
 *     撞成同一个颜色就把语义抹平了；
 *  3. **Agent 身份色**与主行动色、背景可区分（Human 与 Agent 的视觉分离）。
 *
 * 这三条都是**既有裁决的机械落点**（`THEME_GUIDE.md` 硬规则 4 与 §可访问性），
 * 不是本模块新立的规矩。
 *
 * ## 诚实优先于完整
 *
 * 算不出来的值（`color-mix()`、`var()`、带 alpha、渐变、认不出的函数）一律报
 * **`unknown`**，并且说清是哪一项读不到 —— **绝不当成通过**。判据一句话：
 * **"我没检查这一项"和"这一项没问题"是两个不同的结论，不许混。**
 *
 * ## 阈值是选择，写在这里给人看
 *
 *  - 对比度：≥ 4.5 通过（AA 正文），3–4.5 留意，< 3 不行；
 *  - 可辨性：用 OKLab 距离（感知均匀空间）。同一屏上要分辨，取 0.02 为通过、
 *    0.01–0.02 留意、< 0.01 不行 —— OKLab 的恰可察觉差约在 0.01 量级，
 *    而状态色是小尺寸指示点，所以要求两倍于它。
 *    ⚠ 这**不是**唯一防线：`DESIGN_SYSTEM.md` 要求"状态不得只用颜色表达"
 *    （presence 一律带文字或图标），颜色可辨性只是第二道。
 */

import type { TokenMap } from './themeFile.ts'

export interface Rgb {
  r: number
  g: number
  b: number
}

export type ColorParse = { ok: true; rgb: Rgb } | { ok: false; reason: 'unresolved' | 'syntax' }

/**
 * 对比度配对：**与 `scripts/variant-lab.mjs` 的 `CONTRAST_PAIRS` 必须逐项一致**
 * （`tests/user-theme.test.ts` 钉着这条同源，**连门槛一起比**；门那边是真发请求算对比度的权威）。
 *
 * 第三格是这一对的**门槛**，省略即 AA 正文（4.5）。三档各有出处：
 *  - `CONTRAST_AA` 4.5：正文与小字；
 *  - `GRAPHIC_FLOOR` 3.0：**非文字**（WCAG 1.4.11）—— 焦点环、只当指示点用的状态色；
 *  - 边框 1.3 / 输入框描边 2.0：**表面分离**（不靠发丝线分层时，至少得看得见那条线）。
 *
 * ⚠ `--border` / `--input` 这两族曾经**只被一次性审计脚本查过、从未进门禁**，
 * 于是 paper 的输入框描边一路低到 1.38 而没人拦（2026-10-17 记账）。
 */
export const CONTRAST_PAIRS: readonly (readonly [string, string, number?])[] = [
  ['--primary-foreground', '--primary'],
  ['--bubble-self-foreground', '--bubble-self-from'],
  ['--bubble-self-foreground', '--bubble-self-to'],
  ['--accent-foreground', '--accent'],
  ['--foreground', '--background'],
  ['--card-foreground', '--card'],
  ['--popover-foreground', '--popover'],
  ['--bubble-peer-foreground', '--bubble-peer'],
  ['--muted-foreground', '--muted'],
  // 非文字：焦点环要在它可能出现的每个面上看得见
  ['--ring', '--card', 3],
  ['--ring', '--background', 3],
  ['--ring', '--muted', 3],
  ['--ring', '--accent', 3],
  // 表面分离
  ['--border', '--card', 1.3],
  ['--border', '--background', 1.3],
  ['--input', '--card', 2],
  ['--input', '--background', 2],
]

/** presence 五态：两两可辨。 */
const PRESENCE_TOKENS = [
  '--presence-online',
  '--presence-thinking',
  '--presence-silent',
  '--presence-timeout',
  '--presence-failed',
] as const

/** Agent 身份色要与之区分的两个面。 */
const AGENT_CONTRAST_TOKENS = ['--primary', '--background'] as const

export const CONTRAST_AA = 4.5
export const CONTRAST_MIN = 3
/** 非文字那一档（WCAG 1.4.11）：焦点环、只当指示点用的状态色。 */
export const GRAPHIC_FLOOR = 3
export const DISTINCT_OK = 0.02
export const DISTINCT_MIN = 0.01

// ── 颜色解析 ────────────────────────────────────────────────────────────────

const HEX = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i

function clamp255(n: number): number {
  return Math.min(255, Math.max(0, n))
}

function parseHex(text: string): { rgb: Rgb; alpha: number } | null {
  const m = text.match(HEX)
  if (!m) return null
  const body = m[1]!
  const pick = (i: number, len: number) => parseInt(body.slice(i, i + len), 16)
  if (body.length === 3) {
    return { rgb: { r: pick(0, 1) * 17, g: pick(1, 1) * 17, b: pick(2, 1) * 17 }, alpha: 1 }
  }
  if (body.length === 4) {
    return {
      rgb: { r: pick(0, 1) * 17, g: pick(1, 1) * 17, b: pick(2, 1) * 17 },
      alpha: pick(3, 1) / 15,
    }
  }
  if (body.length === 6) {
    return { rgb: { r: pick(0, 2), g: pick(2, 2), b: pick(4, 2) }, alpha: 1 }
  }
  return { rgb: { r: pick(0, 2), g: pick(2, 2), b: pick(4, 2) }, alpha: pick(6, 2) / 255 }
}

/** 拆 `fn(a b / c)` 里的实参：逗号、空白、斜杠都算分隔。 */
function argsOf(text: string): string[] {
  return text
    .replace(/^[a-z-]+\(/i, '')
    .replace(/\)$/, '')
    .split(/[\s,/]+/)
    .filter(Boolean)
}

function numberArg(text: string): { value: number; percent: boolean } | null {
  const m = text.match(/^([+-]?(?:\d+\.?\d*|\.\d+))(%?)$/)
  if (!m) return null
  const value = Number(m[1])
  if (!Number.isFinite(value)) return null
  return { value, percent: m[2] === '%' }
}

function angleArg(text: string): number | null {
  const m = text.match(/^([+-]?(?:\d+\.?\d*|\.\d+))(deg|grad|rad|turn)?$/)
  if (!m) return null
  const value = Number(m[1])
  if (!Number.isFinite(value)) return null
  switch (m[2]) {
    case 'grad':
      return (value * 360) / 400
    case 'rad':
      return (value * 180) / Math.PI
    case 'turn':
      return value * 360
    default:
      return value
  }
}

function hslToRgb(h: number, s: number, l: number): Rgb {
  const hue = ((h % 360) + 360) % 360
  const sat = Math.min(1, Math.max(0, s))
  const light = Math.min(1, Math.max(0, l))
  const c = (1 - Math.abs(2 * light - 1)) * sat
  const x = c * (1 - Math.abs(((hue / 60) % 2) - 1))
  const m = light - c / 2
  const seg = Math.floor(hue / 60) % 6
  const table: [number, number, number][] = [
    [c, x, 0],
    [x, c, 0],
    [0, c, x],
    [0, x, c],
    [x, 0, c],
    [c, 0, x],
  ]
  const [r, g, b] = table[seg]!
  return { r: clamp255((r + m) * 255), g: clamp255((g + m) * 255), b: clamp255((b + m) * 255) }
}

/** OKLCH → sRGB（OKLab → 线性 sRGB → 伽马编码；越界分量按 CSS 的做法收敛到端点）。 */
function oklchToRgb(l: number, c: number, hDeg: number): Rgb {
  const h = (hDeg * Math.PI) / 180
  const a = c * Math.cos(h)
  const bb = c * Math.sin(h)
  const l_ = l + 0.3963377774 * a + 0.2158037573 * bb
  const m_ = l - 0.1055613458 * a - 0.0638541728 * bb
  const s_ = l - 0.0894841775 * a - 1.291485548 * bb
  const L = l_ ** 3
  const M = m_ ** 3
  const S = s_ ** 3
  const lin = [
    4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S,
    -1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S,
    -0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S,
  ]
  const encode = (v: number) => {
    const clamped = Math.min(1, Math.max(0, v))
    return Math.round(255 * (clamped <= 0.0031308 ? 12.92 * clamped : 1.055 * clamped ** (1 / 2.4) - 0.055))
  }
  return { r: encode(lin[0]!), g: encode(lin[1]!), b: encode(lin[2]!) }
}

/**
 * 解析一个 CSS 颜色值。**只认能换算的**：十六进制（不含 alpha）、`rgb()`、`hsl()`、
 * `oklch()`。别的（`var()`、`color-mix()`、渐变、`currentColor`、**任何带透明度的值**）
 * 一律 `unresolved` —— 带 alpha 的要先与底合成才算得准对比度，本模块**不做合成**，
 * 所以宁可不报，也不猜。
 */
export function parseColor(value: string): ColorParse {
  const text = value.trim()
  if (!text) return { ok: false, reason: 'syntax' }

  if (text.startsWith('#')) {
    const parsed = parseHex(text)
    if (!parsed) return { ok: false, reason: 'syntax' }
    if (parsed.alpha < 1) return { ok: false, reason: 'unresolved' }
    return { ok: true, rgb: parsed.rgb }
  }

  const fn = text.match(/^([a-z-]+)\(/i)?.[1]?.toLowerCase()
  if (!fn) return { ok: false, reason: 'unresolved' }
  const args = argsOf(text)

  if (fn === 'rgb' || fn === 'rgba') {
    if (args.length < 3) return { ok: false, reason: 'syntax' }
    if (args.length > 3) return { ok: false, reason: 'unresolved' } // 带 alpha：不做合成，如实说读不到
    const parts = args.map((a) => {
      const n = numberArg(a)
      if (!n) return null
      return n.percent ? (n.value / 100) * 255 : n.value
    })
    if (parts.some((p) => p === null)) return { ok: false, reason: 'syntax' }
    return { ok: true, rgb: { r: clamp255(parts[0]!), g: clamp255(parts[1]!), b: clamp255(parts[2]!) } }
  }

  if (fn === 'hsl' || fn === 'hsla') {
    if (args.length < 3) return { ok: false, reason: 'syntax' }
    if (args.length > 3) return { ok: false, reason: 'unresolved' }
    const h = angleArg(args[0]!)
    const s = numberArg(args[1]!)
    const l = numberArg(args[2]!)
    if (h === null || !s || !l) return { ok: false, reason: 'syntax' }
    return { ok: true, rgb: hslToRgb(h, s.value / 100, l.value / 100) }
  }

  if (fn === 'oklch') {
    if (args.length < 3) return { ok: false, reason: 'syntax' }
    if (args.length > 3) return { ok: false, reason: 'unresolved' }
    const rawL = numberArg(args[0]!)
    const rawC = numberArg(args[1]!)
    const h = angleArg(args[2]!)
    if (!rawL || !rawC || h === null) return { ok: false, reason: 'syntax' }
    const l = rawL.percent ? rawL.value / 100 : rawL.value
    // CSS 里 oklch 的彩度百分比相对 0.4。
    const c = rawC.percent ? (rawC.value / 100) * 0.4 : rawC.value
    return { ok: true, rgb: oklchToRgb(l, c, h) }
  }

  return { ok: false, reason: 'unresolved' }
}

// ── 度量 ────────────────────────────────────────────────────────────────────

function linearize(channel: number): number {
  const c = channel / 255
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
}

export function relativeLuminance(rgb: Rgb): number {
  return 0.2126 * linearize(rgb.r) + 0.7152 * linearize(rgb.g) + 0.0722 * linearize(rgb.b)
}

/** WCAG 对比度（1..21）。 */
export function contrastRatio(a: Rgb, b: Rgb): number {
  const la = relativeLuminance(a)
  const lb = relativeLuminance(b)
  const light = Math.max(la, lb)
  const dark = Math.min(la, lb)
  return (light + 0.05) / (dark + 0.05)
}

/** sRGB → OKLab（感知均匀空间，用来量"这两个颜色看着像不像"）。 */
function toOklab(rgb: Rgb): [number, number, number] {
  const r = linearize(rgb.r)
  const g = linearize(rgb.g)
  const b = linearize(rgb.b)
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ]
}

export function perceptualDistance(a: Rgb, b: Rgb): number {
  const [l1, a1, b1] = toOklab(a)
  const [l2, a2, b2] = toOklab(b)
  return Math.hypot(l1 - l2, a1 - a2, b1 - b2)
}

// ── 体检 ────────────────────────────────────────────────────────────────────

export type FindingLevel = 'ok' | 'weak' | 'fail' | 'unknown'
export type ThemeMode = 'light' | 'dark'

export interface AuditFinding {
  mode: ThemeMode
  kind: 'contrast' | 'presence' | 'agent' | 'unresolved'
  /** 涉及的令牌（用户据此去改哪一行）。 */
  tokens: string[]
  level: FindingLevel
  detail: string
}

export interface AuditBase {
  light: TokenMap
  dark: TokenMap
}

function lookup(tokens: TokenMap, prop: string): ColorParse | null {
  const value = tokens[prop]
  if (value === undefined) return null
  return parseColor(value)
}

/**
 * 一对比值的档位。`floor` 是这一对自己的门槛（见 `CONTRAST_PAIRS` 第三格）。
 * `weak` 带取门槛的 2/3 —— 正文门槛 4.5 时正好是 3.0，与老的 `CONTRAST_MIN` 逐位一致，
 * 所以原有那 9 对的判级**一字未变**；低门槛的那几对（1.3 / 2.0 / 3.0）按同一比例缩放。
 */
function ratioLevel(ratio: number, floor: number = CONTRAST_AA): FindingLevel {
  if (ratio >= floor) return 'ok'
  if (ratio >= floor * (CONTRAST_MIN / CONTRAST_AA)) return 'weak'
  return 'fail'
}

function distanceLevel(distance: number): FindingLevel {
  if (distance >= DISTINCT_OK) return 'ok'
  if (distance >= DISTINCT_MIN) return 'weak'
  return 'fail'
}

const MODE_TEXT: Record<ThemeMode, string> = { light: '日间', dark: '夜间' }

/**
 * 体检一份主题（与基底合并后逐档检查）。
 *
 * `base` 是**用户没写的令牌继承来的那一份**（官方预设的令牌表）。合并是必须的：
 * 只改一个 `--primary` 的主题，它的对比度取决于基底的 `--primary-foreground`。
 */
export function auditTheme(theme: { light: TokenMap; dark: TokenMap }, base: AuditBase): AuditFinding[] {
  const findings: AuditFinding[] = []

  for (const mode of ['light', 'dark'] as const) {
    const merged: TokenMap = { ...base[mode], ...theme[mode] }
    const label = MODE_TEXT[mode]

    for (const [foreground, background, floor] of CONTRAST_PAIRS) {
      const fg = lookup(merged, foreground)
      const bg = lookup(merged, background)
      if (!fg || !bg || !fg.ok || !bg.ok) {
        findings.push({
          mode,
          kind: 'unresolved',
          tokens: [foreground, background],
          level: 'unknown',
          detail: `${label}：${foreground} / ${background} 这一对的对比度没算出来（值不是我能换算的颜色，比如 color-mix 或带透明度）——这一项没有检查，不等于没问题。`,
        })
        continue
      }
      const ratio = contrastRatio(fg.rgb, bg.rgb)
      const need = floor ?? CONTRAST_AA
      findings.push({
        mode,
        kind: 'contrast',
        tokens: [foreground, background],
        level: ratioLevel(ratio, need),
        detail: `${label}：${foreground} 叠在 ${background} 上是 ${ratio.toFixed(2)}:1（这一对的门槛 ${need}）。`,
      })
    }

    // presence 五态两两可辨：合成一条结论（十对分开报没人看得下去）。
    const pairs: { a: string; b: string; distance: number }[] = []
    let presenceUnknown: string[] = []
    for (let i = 0; i < PRESENCE_TOKENS.length; i++) {
      for (let j = i + 1; j < PRESENCE_TOKENS.length; j++) {
        const a = PRESENCE_TOKENS[i]!
        const b = PRESENCE_TOKENS[j]!
        const ca = lookup(merged, a)
        const cb = lookup(merged, b)
        if (!ca || !cb || !ca.ok || !cb.ok) {
          presenceUnknown.push(`${a}/${b}`)
          continue
        }
        pairs.push({ a, b, distance: perceptualDistance(ca.rgb, cb.rgb) })
      }
    }
    if (pairs.length > 0) {
      const worst = pairs.reduce((acc, p) => (p.distance < acc.distance ? p : acc))
      const level = distanceLevel(worst.distance)
      const weak = pairs.filter((p) => distanceLevel(p.distance) !== 'ok')
      findings.push({
        mode,
        kind: 'presence',
        tokens: [...PRESENCE_TOKENS],
        level,
        detail:
          level === 'ok'
            ? `${label}：五个状态两两都分得清（最接近的一对是 ${worst.a} 与 ${worst.b}）。`
            : `${label}：有 ${weak.length} 对状态太像了，最难分的是 ${worst.a} 与 ${worst.b}（差距 ${worst.distance.toFixed(3)}）。`,
      })
    }
    if (presenceUnknown.length > 0) {
      findings.push({
        mode,
        kind: 'unresolved',
        tokens: [...PRESENCE_TOKENS],
        level: 'unknown',
        detail: `${label}：有 ${presenceUnknown.length} 对状态的颜色没算出来，这些对没有检查。`,
      })
    }

    for (const other of AGENT_CONTRAST_TOKENS) {
      const agent = lookup(merged, '--agent')
      const peer = lookup(merged, other)
      if (!agent || !peer || !agent.ok || !peer.ok) {
        findings.push({
          mode,
          kind: 'unresolved',
          tokens: ['--agent', other],
          level: 'unknown',
          detail: `${label}：--agent 与 ${other} 的区分度没算出来。`,
        })
        continue
      }
      const distance = perceptualDistance(agent.rgb, peer.rgb)
      const level = distanceLevel(distance)
      findings.push({
        mode,
        kind: 'agent',
        tokens: ['--agent', other],
        level,
        detail:
          level === 'ok'
            ? `${label}：Agent 色与 ${other} 分得开。`
            : `${label}：Agent 色与 ${other} 太接近了（差距 ${distance.toFixed(3)}）——Agent 与 Human 的视觉分离是硬规则。`,
      })
    }
  }

  return findings
}

/** 汇总一档结论，供界面上一句话说清（`unknown` 与 `ok` **不合并**）。 */
export function summarizeFindings(findings: AuditFinding[]): {
  ok: number
  weak: number
  fail: number
  unknown: number
} {
  const out = { ok: 0, weak: 0, fail: 0, unknown: 0 }
  for (const f of findings) out[f.level] += 1
  return out
}
