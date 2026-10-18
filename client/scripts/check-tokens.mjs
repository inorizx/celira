import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const SRC = join(ROOT, 'src')

/**
 * 色值只许出现在**主题文件**里（2026-10-08 重述 —— 刀 3 把官方主题搬成了包）：
 *
 *   - `src/styles/tokens.css` —— **基底**（`:root` / `.dark`，即默认主题本体）与字号档；
 *   - `src/styles/themes/*.css` —— **官方主题包**：一个包 = 一个文件 = 一个作用域。
 *
 * 其余一切（组件、页面、api 层）照旧不得出现色值。用户主题是**运行时文本**，
 * 天然在源码守卫射程外 —— 那道校验由装载路径自己扛（`src/lib/themeAudit.ts` 的体检）。
 */
const ALLOWED = new Set(['src/styles/tokens.css'])
/** 官方主题包：路径形状即判据（`src/styles/themes/<作用域名>.css`）。 */
const isThemePackage = (rel) => /^src\/styles\/themes\/[a-z][a-z0-9-]*\.css$/.test(rel)

const PALETTE =
  '(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)'
const COLOR_UTILITY =
  '(?:bg|text|border|from|via|to|ring|fill|stroke|divide|outline|decoration|accent|caret|placeholder)'

const patterns = [
  { re: /#[0-9a-fA-F]{3,8}\b/, desc: 'hex 颜色' },
  { re: /-\[(?:#[0-9a-fA-F]{3,8}|(?:oklch|hsl|rgb)a?\()/, desc: 'Tailwind arbitrary 颜色值' },
  {
    re: new RegExp(`\\b${COLOR_UTILITY}-(?:white|black)(?:/\\d+)?\\b`),
    desc: '裸色类（white/black）',
  },
  {
    re: new RegExp(`\\b${COLOR_UTILITY}-${PALETTE}-\\d{2,3}(?:/\\d+)?\\b`),
    desc: '裸色阶类（如 gray-400、purple-500）',
  },
  {
    re: /style=\{\{(?:(?!\}\})[\s\S])*(?:color|background)(?:(?!\}\})[\s\S])*\}\}/i,
    desc: '内联 style 颜色（仅允许 var() 形式，见 Gallery Swatch 例外写法）',
  },
]

function* walk(dir) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) yield* walk(full)
    else if (/\.(tsx?|css)$/.test(entry)) yield full
  }
}

const targets = [...walk(SRC), join(ROOT, 'index.html')]

let violations = 0
for (const file of targets) {
  const rel = relative(ROOT, file).replaceAll('\\', '/')
  if (ALLOWED.has(rel) || isThemePackage(rel)) continue
  const content = readFileSync(file, 'utf8')
  const lines = content.split('\n')
  lines.forEach((line, i) => {
    if (line.includes('token-check:allow')) return
    for (const { re, desc } of patterns) {
      if (re.test(line)) {
        console.error(`✗ ${rel}:${i + 1} 含${desc}，颜色必须来自主题文件（src/styles/tokens.css 的基底或 src/styles/themes/ 里的包）`)
        console.error(`  ${line.trim()}`)
        violations++
      }
    }
  })
}

if (violations > 0) {
  console.error(`\n共 ${violations} 处违规。确属例外的行尾加 token-check:allow 注释并在 DESIGN_SYSTEM.md 登记。`)
  process.exit(1)
}
console.log('token check passed')
