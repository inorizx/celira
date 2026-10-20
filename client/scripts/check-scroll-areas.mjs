/**
 * **滚动区守卫**：在 flex 列里当滚动区用的元素，必须带 `min-h-0`。
 *
 * ## 它守的是哪一件事
 *
 * `flex-1` 的子项默认 `min-height: auto` ⇒ 内容比容器高时它**被内容撑高**（而不是被压住）
 * ⇒ 滚动区（`ScrollArea` 的视口 / `overflow-y-auto` 那一层）**永远不溢出** ⇒ 没有滚动条、
 * 滚轮也没得滚；而它上面那几层是定高的（`AppFrame` 的 `h-screen` + 内容那格
 * `min-h-0 flex-1`，页面根 `h-full … overflow-hidden`）⇒ 多出来的部分被**直接裁掉**。
 * 用户看到的是两件事同时发生：**内容显示不全 ＋ 滚轮不动**。
 *
 * 这条坑仓库里写着（`src/pages/settings/SettingsLayout.tsx` 那句注释），但 2026-10-09 一次中了
 * **8 处**（公共滚动壳 `PageScroll` 与另外七个页面/面板）—— 在打包形态里真机上表现为
 * 「机器页显示不全、滚不动」，用户连「去配置模型」那个按钮都够不到。
 *
 * ## 判据（只认"被当作滚动区用"的那些）
 *
 * 一个元素**同时**满足这两条才算滚动区，才要求 `min-h-0`：
 *   1. 它的 className 里有 `flex-1`（即它在一个 flex 列里分空间）；
 *   2. 它是 `<ScrollArea>`，或它的 className 里有 `overflow-y-auto` / `overflow-auto`。
 *
 * 于是"固定高度的滚动区"（`h-56`、`h-full`）与"不在 flex 里滚动的元素"都不在射程内 ——
 * 不误报中立写法，也不替别的版面问题背书。
 *
 * ## 例外
 *
 * 真属于"父容器高度是 auto、本来就该被内容撑开"的写法，行尾加 `scroll-guard:allow` 并写明理由。
 * 今天**一处例外都没有**（8 处该带的都带上了）；哪天加了，理由写在那一行，不写在这里。
 *
 * ## 射程（如实登记，别把它当成万能的）
 *
 * - 只看**字面量** className：`className={cn('flex-1', x && 'min-h-0')}` 这类动态拼法读不全
 *   （保守放过，不误报）；
 * - 它量不到"这一页压根没放滚动区"那种形状（例如 `GalleryPage` 的 `min-h-full`）——
 *   那要真机上的**行为**判据（`verify-ui` 量"内容比窗口高时滚得动"），不在这条静态守卫的射程里。
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const SRC = join(ROOT, 'src')

/** 判据本体（纯函数 ⇒ 可自检）：拿一个元素的标签文本，回它缺不缺 `min-h-0`。 */
export function scrollGuardVerdict(tagText) {
  const classNames = [...tagText.matchAll(/className=(?:"([^"]*)"|\{([^}]*)\})/g)]
    .map((match) => match[1] ?? match[2] ?? '')
    .join(' ')
  const tokens = classNames.split(/\s+/).filter(Boolean)
  const isScrollArea = /^<ScrollArea\b/.test(tagText) || tokens.some((each) => /^overflow-(y-)?auto$/.test(each))
  if (!isScrollArea) return { applies: false }
  if (!tokens.includes('flex-1')) return { applies: false }
  return { applies: true, ok: tokens.includes('min-h-0') }
}

/** 扫一份源码文本，回它里面所有"被当作滚动区用"的地方。 */
export function scanSource(text) {
  const found = []
  // 标签属性扫到第一个 '>' 为止：JSX 里 `=>` 这类箭头函数会带上 '>'，所以按"行内首次出现"取，
  // 读不到 className 的（动态拼法）自然落进射程外。
  for (const match of text.matchAll(/<[A-Za-z][A-Za-z0-9.]*(?:(?!\n\s*<)[^>])*>/g)) {
    const verdict = scrollGuardVerdict(match[0])
    if (verdict.applies) found.push({ tag: match[0], ok: verdict.ok, index: match.index })
  }
  return found
}

function* walk(dir) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) yield* walk(full)
    else if (entry.endsWith('.tsx')) yield full
  }
}

/** 自检：判据必须认得出正例、反例、例外注释与"不该管"的写法（这是它唯一能被信任的理由）。 */
function selfCheck() {
  const cases = [
    { name: '滚动区带 min-h-0 ⇒ 放行', tag: '<ScrollArea className="min-h-0 flex-1">', ok: true },
    { name: '滚动区漏 min-h-0 ⇒ 违规', tag: '<ScrollArea className="flex-1">', ok: false },
    { name: '带上限写法但确实在滚 ⇒ 违规', tag: '<ScrollArea className="flex-1 [&_x]:!block">', ok: false },
    { name: '手写 overflow-y-auto 漏 min-h-0 ⇒ 违规', tag: '<div className="flex-1 overflow-y-auto">', ok: false },
    { name: '手写 overflow-y-auto 带 min-h-0 ⇒ 放行', tag: '<div className="min-h-0 flex-1 overflow-y-auto">', ok: true },
    { name: '固定高度（h-56）不判', tag: '<ScrollArea className="h-56">', ok: null },
    { name: '有确定高度（h-full）不判', tag: '<ScrollArea className="h-full">', ok: null },
    { name: '不在 flex 里分空间（没有 flex-1）不判', tag: '<div className="overflow-y-auto">', ok: null },
    { name: '压根不是滚动区 ⇒ 不判', tag: '<div className="flex-1">', ok: null },
    { name: '动态拼法读不全 ⇒ 保守放过', tag: '<ScrollArea className={cn(parts)}>', ok: null },
  ]
  let failed = 0
  for (const item of cases) {
    const verdict = scrollGuardVerdict(item.tag)
    const got = verdict.applies ? verdict.ok : null
    if (got !== item.ok) {
      console.error(`✗ 自检不过：${item.name}（期望 ${item.ok}，得到 ${got}）`)
      console.error(`  ${item.tag}`)
      failed++
    }
  }
  if (cases.length === 0) {
    console.error('✗ 自检一个用例都没有 —— 那是空跑')
    failed++
  }
  return { failed, total: cases.length }
}

const self = selfCheck()
if (self.failed > 0) process.exit(1)

let violations = 0
let checked = 0
for (const file of walk(SRC)) {
  const rel = relative(ROOT, file).replaceAll('\\', '/')
  const text = readFileSync(file, 'utf8')
  for (const hit of scanSource(text)) {
    checked++
    if (hit.ok) continue
    const line = text.slice(0, hit.index).split('\n').length
    const lineText = text.split('\n')[line - 1] ?? ''
    if (lineText.includes('scroll-guard:allow')) continue
    console.error(`✗ ${rel}:${line} 在 flex 列里当滚动区用，却漏了 min-h-0（会被内容撑高 ⇒ 滚不动、被上层裁掉）`)
    console.error(`  ${hit.tag.replace(/\s+/g, ' ').trim()}`)
    console.error('  写法：<ScrollArea className="min-h-0 flex-1">；确属例外则行尾加 scroll-guard:allow 并写明理由')
    violations++
  }
}

if (violations > 0) {
  console.error(`\n共 ${violations} 处违规（自检 ${self.total} 条已过）。`)
  process.exit(1)
}
console.log(`scroll guard passed（扫到 ${checked} 处滚动区，自检 ${self.total} 条）`)
