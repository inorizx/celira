/**
 * 主题文件：**格式的权威**（解析 / 序列化 / 令牌分类）。纯逻辑，零 DOM。
 *
 * 一份主题文件就是**一段带作用域的 CSS**——官方主题与用户主题**同构**：一个格式、
 * 一台装载器，差别只在来源（官方随产品走，用户从本机装载）。这不是新发明：
 * `tokens.css` 里那九套官方主题本来就是 `[data-theme='…']` 作用域覆盖层，
 * 本模块只是把那份既有的约定写成**可校验的形状**。
 *
 * ## 格式（三条硬要求，违反即拒收）
 *
 * 它们是**格式**要求而不是体检建议——体检（`themeAudit.ts`）照实报但**不拦人**，
 * 而这里的三条拦人，因为不满足它们的文件**装载不了**：
 *
 *  1. **每条规则都必须带作用域** `[data-theme='<id>']` 或 `.dark[data-theme='<id>']`。
 *     无作用域的全局规则会污染其他主题（`THEME_GUIDE.md` 的绝对红线）；
 *  2. **日/夜两档都要有**。少一档会让那一档的令牌在对面的明暗下被误用
 *     （注入的 `<style>` 在源序上晚于打包样式，同特异度时它赢）；
 *  3. **零外部请求**：不许 `@import`、不许 `url(http…)`、不许 `@font-face`。
 *     本地优先产品，字体必须随仓库分发（`THEME_GUIDE.md`）——所以用户主题
 *     **只能引用本机已有的字体族**，不能自带字体文件。这是本刀如实认下的限制。
 *
 * ## 可选的形态层
 *
 * 字族 / 切角 / 圆角 / 渐变 / 纹理 / 辉光写在**作用域内的普通规则**里（照旧受第 1 条管）：
 *
 * ```css
 * [data-theme='user-mine'] body { font-family: Georgia, serif; }
 * ```
 *
 * ## 存储的是**规范化后的文件**
 *
 * `parseThemeFile` → `serializeThemeFile` 幂等；装载与导出都用序列化后的那一份，
 * 于是「被校验的东西」与「被注入的东西」逐字节同一，不存在两条路各写一遍。
 */

/** 令牌表：`--name` → 值（值原样保留，不解释——解释归 `themeAudit.ts`）。 */
export type TokenMap = Record<string, string>

/** 作用域 id 的形状：**用户文件**必须 `user-` 打头（不许与官方预设 id 撞名）。 */
const SCOPE_ID_USER = /^user-[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/

/**
 * **官方主题包**的作用域 id 形状：就是包的文件名（`glass` / `neon` / …）。
 *
 * 为什么分成两条：`user-` 前缀是**来源政策**（"这是本机装进来的"），不是**格式形状**。
 * 官方包走的是"随包目录"，不必经过导入那道门；两边共用的是形状那几条
 * （每条规则带作用域／日夜两档齐全／零外部引用）。
 */
const SCOPE_ID_PACKAGE = /^[a-z][a-z0-9-]{0,39}$/

/**
 * **预览专用**的作用域 id（刀 2）。
 *
 * 小样（"装之前先看"）靠的是"嵌套 `data-theme` 把作用域限制在卡片内"这条既有做法 ——
 * 而它的正确性依赖一件事：**`<html>` 上永远不写这个 id**。所以小样**绝不使用用户文件
 * 自己那个 id**（否则"预览正在生效中的那套主题"就会同时改到全应用）。
 *
 * ⇒ 这个 id 必须**保留**：任何主题文件都不许占用它。理由不是洁癖 ——
 * 一旦有文件叫 `user-preview`，装上之后 `<html data-theme='user-preview'>` 与
 * 小样的注入样式就会撞在同一个作用域上，那时**预览会漏到整个应用**。
 */
export const PREVIEW_SCOPE_ID = 'user-preview'

/** 保留 id（主题文件不许占用；与 `PREVIEW_SCOPE_ID` 同源，只有这一个）。 */
export const RESERVED_SCOPE_IDS: readonly string[] = [PREVIEW_SCOPE_ID]

/** 上限：主题是数据，一份几 KB 就够；超过这个数说明多半贴错了东西。 */
export const THEME_FILE_MAX_BYTES = 256 * 1024

/**
 * 令牌分类：谁只有「表面渲染权」，谁承担「语义安全」。
 *
 * 它落地的是一条既有裁决（`THEME_GUIDE.md` 硬规则 4）：**Agent 身份色与 presence
 * 五态不可省略、不可同色**。分类不是为了禁止用户改它们（主题本来就得出一套完整
 * 令牌表），而是为了让体检知道**该拿哪些令牌去算可辨性**。
 */
export const TOKEN_CLASS = {
  /** 表面与文字：改它只影响"好不好看"。 */
  surface: [
    '--radius',
    '--background',
    '--foreground',
    '--card',
    '--card-foreground',
    '--popover',
    '--popover-foreground',
    '--secondary',
    '--secondary-foreground',
    '--muted',
    '--muted-foreground',
    '--border',
    '--input',
    '--overlay',
  ],
  /** 色彩声音：主行动 / 品牌 / 焦点环 / 自己气泡 / 强调（与 `variant-lab.mjs` 的 VOICE_TOKENS 同族）。 */
  voice: [
    '--primary',
    '--primary-foreground',
    '--brand',
    '--ring',
    '--accent',
    '--accent-foreground',
    '--bubble-self-from',
    '--bubble-self-to',
    '--bubble-self-foreground',
    '--bubble-peer',
    '--bubble-peer-foreground',
  ],
  /**
   * 语义安全：这些令牌**是关于事实的**（谁是 Agent / 在不在 / 在不在写 / 这个动作有没有后果），
   * 所以它们必须两两可辨、且与主行动色可区分——体检盯的就是这一组。
   */
  semantic: [
    '--agent',
    '--agent-surface',
    '--presence-online',
    '--presence-thinking',
    '--presence-silent',
    '--presence-timeout',
    '--presence-failed',
    '--destructive',
    '--destructive-foreground',
    '--code-keyword',
    '--code-string',
    '--code-comment',
    '--code-number',
    '--code-function',
    '--code-type',
    '--code-variable',
    '--code-meta',
  ],
} as const

export type TokenClass = keyof typeof TOKEN_CLASS

/** presence 五态：体检要求它们**两两可辨**（回复 / 沉默 / 超时 / 失败必须语义分离）。 */
export const PRESENCE_TOKENS = [
  '--presence-online',
  '--presence-thinking',
  '--presence-silent',
  '--presence-timeout',
  '--presence-failed',
] as const

/** 主题文件错误：**封闭表**（新增取值必须同一刀补齐这里与 `THEME_FILE_ERROR_TEXT`）。 */
export const THEME_FILE_ERRORS = {
  empty: '文件是空的',
  too_large: '文件太大',
  external_ref: '引用了外部资源（@import / 外部 url / @font-face）',
  unsupported_at_rule: '出现了不支持的 at 规则',
  unscoped_rule: '有规则没有带主题作用域',
  bad_scope_id: '主题 id 形状不合法（必须 user- 打头，小写字母数字与连字符）',
  reserved_id: '这个主题 id 是保留的，换个名字',
  mixed_scope: '一份文件里出现了两个不同的主题 id',
  no_tokens: '作用域块里没有任何令牌',
  missing_dark: '缺夜间那一档',
} as const

export type ThemeFileErrorCode = keyof typeof THEME_FILE_ERRORS

/** 错误码 → 用户读得懂的话（与上面那张表一一对应）。 */
export const THEME_FILE_ERROR_TEXT: Record<ThemeFileErrorCode, string> = {
  empty: '这份文件是空的。',
  too_large: `文件太大了（上限 ${Math.round(THEME_FILE_MAX_BYTES / 1024)} KB）。`,
  external_ref:
    '这份主题引用了外部资源。主题只能引用本机已有的东西 —— 字体请用系统字体栈，不能自带字体文件。',
  unsupported_at_rule: '这份主题里用了不支持的 at 规则（只认 @media 与 @supports）。',
  unscoped_rule:
    '有规则没有带主题作用域。每条规则都要写成 [data-theme=\'user-xxx\'] 或 .dark[data-theme=\'user-xxx\']，否则会污染别的主题。',
  bad_scope_id: '主题 id 不合法：要用 user- 打头，后面只能是小写字母、数字和连字符。',
  reserved_id: `「${PREVIEW_SCOPE_ID}」是保留给"装之前先看"那张小样的 id，换个名字（比如 ${PREVIEW_SCOPE_ID}-2）。`,
  mixed_scope: '一份文件里出现了两个不同的主题 id，只能留一个。',
  no_tokens: '这个作用域块里一条令牌都没有，装上不会有任何变化。',
  missing_dark: '缺夜间那一档。日间与夜间都要有（.dark[data-theme=\'…\'] 那一块）。',
}

export interface ThemeRule {
  selector: string
  /** 非令牌声明（形态层），原样保留。 */
  form: string[]
}

export interface ParsedThemeFile {
  id: string
  name: string
  light: TokenMap
  dark: TokenMap
  /** 形态层规则（已确认每条都带作用域）。 */
  formRules: ThemeRule[]
}

export type ParseResult =
  | { ok: true; theme: ParsedThemeFile }
  | { ok: false; code: ThemeFileErrorCode; detail?: string }

export interface ParseOptions {
  /**
   * 作用域 id 是否必须 `user-` 打头 —— **来源政策，不是格式形状**。
   *
   * - `true`（缺省）：**导入**用户文件时用（挡住与官方预设撞名）；
   * - `false`：校验**官方主题包**时用（包的作用域名就是预设名，如 `glass`）。
   *
   * 保留 id（`user-preview`）**两边都不许占用** —— 那条不是来源政策，是安全性质。
   */
  requireUserPrefix?: boolean
}

// ── 扫描 ────────────────────────────────────────────────────────────────────

interface RawRule {
  prelude: string
  body: string
}

/** 顶层规则扫描：按花括号配对切 `前奏 { 体 }`，字符串与括号内的分号都不算分隔。 */
function scanRules(css: string): RawRule[] {
  const out: RawRule[] = []
  let i = 0
  const n = css.length
  while (i < n) {
    while (i < n && /\s/.test(css[i]!)) i++
    if (i >= n) break
    let prelude = ''
    while (i < n && css[i] !== '{' && css[i] !== ';') {
      const c = css[i]!
      if (c === '"' || c === "'") {
        const quote = c
        prelude += c
        i++
        while (i < n && css[i] !== quote) {
          if (css[i] === '\\') {
            prelude += css[i]!
            i++
          }
          prelude += css[i]!
          i++
        }
        prelude += css[i] ?? ''
        i++
        continue
      }
      prelude += c
      i++
    }
    if (i >= n) break
    if (css[i] === ';') {
      i++
      out.push({ prelude: prelude.trim(), body: '' })
      continue
    }
    i++ // 吃掉 '{'
    let depth = 1
    let body = ''
    while (i < n && depth > 0) {
      const c = css[i]!
      if (c === '"' || c === "'") {
        const quote = c
        body += c
        i++
        while (i < n && css[i] !== quote) {
          if (css[i] === '\\') {
            body += css[i]!
            i++
          }
          body += css[i]!
          i++
        }
        body += css[i] ?? ''
        i++
        continue
      }
      if (c === '{') depth++
      else if (c === '}') {
        depth--
        if (depth === 0) {
          i++
          break
        }
      }
      body += c
      i++
    }
    out.push({ prelude: prelude.trim(), body })
  }
  return out
}

/** 把声明体切成 `属性: 值`；括号内的分号不算分隔。 */
function scanDeclarations(body: string): { prop: string; value: string }[] {
  const out: { prop: string; value: string }[] = []
  let depth = 0
  let current = ''
  for (const c of body) {
    if (c === '(') depth++
    else if (c === ')') depth = Math.max(0, depth - 1)
    if (c === ';' && depth === 0) {
      out.push(...splitDeclaration(current))
      current = ''
      continue
    }
    current += c
  }
  out.push(...splitDeclaration(current))
  return out
}

function splitDeclaration(text: string): { prop: string; value: string }[] {
  const trimmed = text.trim()
  if (!trimmed) return []
  const at = trimmed.indexOf(':')
  if (at <= 0) return []
  const prop = trimmed.slice(0, at).trim()
  const value = trimmed.slice(at + 1).trim()
  if (!prop || !value) return []
  return [{ prop, value }]
}

/** 注释先摘掉再解析（但名字要**先**读，它写在注释里）。 */
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, '')
}

/** 名字来自可选的头注释：`/* celira-theme: 我的主题 *\/`。读不到就用 id。 */
function readDeclaredName(raw: string): string | null {
  const m = raw.match(/\/\*\s*celira-theme\s*:\s*([^*]+?)\s*\*\//)
  const name = m?.[1]?.trim()
  return name ? name : null
}

/** 外部引用与危险构造：本地优先产品的红线，一律拒收。 */
function hasExternalRef(css: string): boolean {
  if (/@import/i.test(css)) return true
  if (/@font-face/i.test(css)) return true
  if (/url\(\s*['"]?\s*(?:https?:)?\/\//i.test(css)) return true
  if (/javascript\s*:/i.test(css)) return true
  if (/expression\s*\(/i.test(css)) return true
  return false
}

/**
 * 从选择器里取出作用域 id、它是不是夜间档、以及**作用域后面还剩什么**。
 *
 * `rest` 是保真的关键：只有 `rest` 为空（选择器**恰好**就是这个作用域）的规则才算
 * **令牌块**；带后代选择器的（`[data-theme='x'] body`）一律原样保留为形态层规则。
 * 否则 `[data-theme='x'] .dark { --foo: … }` 会被错当成日间令牌，序列化时丢掉
 * `.dark` 那一节 —— 那等于**替用户改了他写下的意思**。
 */
function readScope(selector: string): { id: string; dark: boolean; rest: string } | null {
  const darkMatch = selector.match(/^\.dark\s*\[data-theme\s*=\s*['"]([^'"]+)['"]\](.*)$/)
  if (darkMatch) return { id: darkMatch[1]!, dark: true, rest: darkMatch[2] ?? '' }
  const lightMatch = selector.match(/\[data-theme\s*=\s*['"]([^'"]+)['"]\](.*)$/)
  if (lightMatch) return { id: lightMatch[1]!, dark: false, rest: lightMatch[2] ?? '' }
  return null
}

// ── 解析 ────────────────────────────────────────────────────────────────────

/**
 * 解析一份主题文件。**拦人的只有格式**（见文件头三条）；体检不在这里，
 * 它归 `themeAudit.ts`——那里照实报但从不阻止装载。
 */
export function parseThemeFile(raw: string, options: ParseOptions = {}): ParseResult {
  const requireUserPrefix = options.requireUserPrefix ?? true
  const idShape = requireUserPrefix ? SCOPE_ID_USER : SCOPE_ID_PACKAGE
  if (!raw.trim()) return { ok: false, code: 'empty' }
  if (raw.length > THEME_FILE_MAX_BYTES) return { ok: false, code: 'too_large' }
  if (hasExternalRef(raw)) return { ok: false, code: 'external_ref' }

  const declaredName = readDeclaredName(raw)
  const css = stripComments(raw)

  const light: TokenMap = {}
  const dark: TokenMap = {}
  const formRules: ThemeRule[] = []
  let id: string | null = null
  let sawLight = false
  let sawDark = false

  const visit = (rules: RawRule[]): ThemeFileErrorCode | null => {
    for (const rule of rules) {
      const prelude = rule.prelude
      if (!prelude) continue
      if (prelude.startsWith('@')) {
        const at = prelude.slice(1).split(/[\s(]/)[0]?.toLowerCase()
        if (at === 'media' || at === 'supports') {
          const nested = visit(scanRules(rule.body))
          if (nested) return nested
          continue
        }
        return 'unsupported_at_rule'
      }
      const scope = readScope(prelude)
      if (!scope) return 'unscoped_rule'
      if (!idShape.test(scope.id)) return 'bad_scope_id'
      if (RESERVED_SCOPE_IDS.includes(scope.id)) return 'reserved_id'
      if (id === null) id = scope.id
      else if (id !== scope.id) return 'mixed_scope'

      // 只有「选择器恰好就是作用域」的规则才提供令牌；带后代的一律走形态层原样保留。
      const tokenBlock = scope.rest.trim() === ''
      const declarations = scanDeclarations(rule.body)
      const form: string[] = []
      for (const { prop, value } of declarations) {
        if (tokenBlock && prop.startsWith('--')) {
          if (scope.dark) dark[prop] = value
          else light[prop] = value
        } else {
          form.push(`${prop}: ${value}`)
        }
      }
      if (tokenBlock) {
        if (scope.dark) sawDark = true
        else sawLight = true
      }
      if (form.length > 0) formRules.push({ selector: prelude, form })
    }
    return null
  }

  const failure = visit(scanRules(css))
  if (failure) return { ok: false, code: failure }
  if (!id) return { ok: false, code: 'unscoped_rule' }
  if (!sawLight || !sawDark) return { ok: false, code: 'missing_dark' }
  if (Object.keys(light).length === 0) return { ok: false, code: 'no_tokens' }

  return {
    ok: true,
    theme: { id, name: declaredName ?? id, light, dark, formRules },
  }
}

// ── 序列化 ──────────────────────────────────────────────────────────────────

function declarationLines(tokens: TokenMap): string[] {
  return Object.keys(tokens)
    .sort()
    .map((prop) => `  ${prop}: ${tokens[prop]};`)
}

/**
 * 规范化输出。**幂等**（`parse → serialize → parse` 得到同一份）——装载与导出
 * 都用它，于是「被校验的东西」与「被注入的东西」是同一份文本。
 */
export function serializeThemeFile(theme: ParsedThemeFile): string {
  const lines = [`/* celira-theme: ${theme.name} */`, '']
  lines.push(`[data-theme='${theme.id}'] {`, ...declarationLines(theme.light), '}', '')
  lines.push(`.dark[data-theme='${theme.id}'] {`, ...declarationLines(theme.dark), '}')
  const formBySelector = new Map<string, string[]>()
  for (const rule of theme.formRules) {
    const existing = formBySelector.get(rule.selector) ?? []
    existing.push(...rule.form)
    formBySelector.set(rule.selector, existing)
  }
  for (const [selector, form] of formBySelector) {
    lines.push('', `${selector} {`, ...form.map((d) => `  ${d};`), '}')
  }
  return lines.join('\n') + '\n'
}

/**
 * 用一份令牌表造一个主题文件对象 —— **纯构造函数，不读任何东西**。
 *
 * 导出的唯一来路：调用方（界面层）把官方预设的令牌表读出来交给它，它只管组装。
 * 这样"读哪儿"与"是什么形状"分开：本模块不依赖打包器特性，所以能被 `node --test`
 * 直接跑（`themeCatalog.ts` 才是那个碰 `?raw` 的模块）。
 */
export function buildThemeFile(id: string, name: string, tokens: ScopedTokens): ParsedThemeFile {
  return { id, name, light: { ...tokens.light }, dark: { ...tokens.dark }, formRules: [] }
}

/**
 * 把选择器里的作用域换成另一个 id。
 *
 * 只在**已验证过**的选择器上用（`parseThemeFile` 已经确认每条规则都带
 * `[data-theme='<原 id>']`），所以 id 里不会有正则元字符，这里不做转义。
 * 写成 `[data-theme = 'x']` 那种带空格的形式也认（与 `readScope` 同一口径）。
 */
function rescopeSelector(selector: string, from: string, to: string): string {
  const pattern = new RegExp(`\\[\\s*data-theme\\s*=\\s*['"]${from}['"]\\s*\\]`, 'g')
  return selector.replace(pattern, `[data-theme='${to}']`)
}

/**
 * 造一份**小样**：同一套令牌与形态层，作用域换成 [`PREVIEW_SCOPE_ID`]。
 *
 * 这是"装之前先看"的全部机制 —— **不新增任何东西**，只是把既有那份主题改个作用域。
 * 两条安全性质都由它保证：
 *
 * 1. **小样绝不使用文件自己那个 id** ⇒ "预览正在生效中的那套主题"也不会把改动漏到全应用；
 * 2. 注入的小样样式只有 `<html data-theme='…'>` 才可能全应用生效，而那个属性**只有生效的
 *    用户主题会写**（`use-theme.tsx`）—— 保留 id 挡住了最后一条撞车路。
 */
export function asPreviewTheme(theme: ParsedThemeFile): ParsedThemeFile {
  return {
    id: PREVIEW_SCOPE_ID,
    name: theme.name,
    light: { ...theme.light },
    dark: { ...theme.dark },
    formRules: theme.formRules.map((rule) => ({
      selector: rescopeSelector(rule.selector, theme.id, PREVIEW_SCOPE_ID),
      form: [...rule.form],
    })),
  }
}

/** 分类一个令牌（认不出归类为 `null`——体检不猜）。 */export function classifyToken(token: string): TokenClass | null {
  for (const [cls, tokens] of Object.entries(TOKEN_CLASS)) {
    if ((tokens as readonly string[]).includes(token)) return cls as TokenClass
  }
  return null
}

// ── 通用抽取（`tokens.css` 也吃这一套）─────────────────────────────────────

export interface ScopedTokens {
  light: TokenMap
  dark: TokenMap
}

/**
 * 从任意带作用域的样式表里抽出令牌表，按作用域 id 归类（`''` = 无 `data-theme` 的基底，
 * 即 `:root` / `.dark`）。三个消费者共用它：
 *
 *  - **测试**：拿官方目录逐套体检（格式能不能表达官方主题，是它量的）；
 *  - **导出**：把当前生效的官方主题抽出来，写成一份用户主题文件；
 *  - **基底**：用户没写的令牌继承基底，体检要拿它补齐。
 *
 * 与 `parseThemeFile` 的差别只有政策：这里**不要求** `user-` 前缀、也不要求两档齐全
 * ——`tokens.css` 的 `:root` 就是只有日间档，而那是合法的基底。
 */
export function parseThemeCss(css: string): Record<string, ScopedTokens> {
  const out: Record<string, ScopedTokens> = {}
  const ensure = (id: string): ScopedTokens => {
    out[id] ??= { light: {}, dark: {} }
    return out[id]!
  }
  for (const rule of scanRules(stripComments(css))) {
    const prelude = rule.prelude
    if (!prelude || prelude.startsWith('@')) continue
    let id: string | null = null
    let dark = false
    if (/^:root$/.test(prelude)) {
      id = ''
      dark = false
    } else if (/^\.dark$/.test(prelude)) {
      id = ''
      dark = true
    } else {
      const scope = readScope(prelude)
      if (!scope || scope.rest.trim() !== '') continue
      id = scope.id
      dark = scope.dark
    }
    const bucket = ensure(id)
    for (const { prop, value } of scanDeclarations(rule.body)) {
      if (!prop.startsWith('--')) continue
      if (dark) bucket.dark[prop] = value
      else bucket.light[prop] = value
    }
  }
  return out
}
