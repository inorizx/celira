#!/usr/bin/env node
// 源码守卫：**Host 全托管路径不许再生**（TASK_64 裁决 6 立的）。
//
// 为什么需要一条守卫，而不是"删干净就行"：
// TASK_63 用 `mode === 'host'` 的正则清了共享层的判定点，实测那个正则对
// **模块本体**完全隐形（`hostCall.ts` 里连一个字面量都没有；`ProviderConfigPanel`
// 的整个 Host 模式是靠一个 `isHost` 布尔开关切的）⇒ "再长一个 host 模块"是最容易
// 的复生路径，而且复生之后没有任何判据会响。这条守卫把两个机械判据固化下来。
//
// 判据（两条，都只扫 `src/`）：
//   1. **文件名**里不许出现 `host`（不分大小写）；
//   2. **import 说明符**里不许出现 `host` —— 静态 `import … from '…'`（**可跨行**）、
//      `export … from '…'`、动态 `import('…')`、`require('…')` 都算。
//
// ⚠ 它**不**管注释里提到 Host（历史叙述要留），也**不**管中文文案里的「本机执行端」
//   （那是「Agent」的执行端，仍然活着，跟退役的 Local Client Host 不是一回事）。
//
// 退出码：0 = 干净；1 = 命中（按纪律 fail loudly，不静默放行）。
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const SRC = join(ROOT, 'src')
const HOSTY = /host/i
/** 只看这些扩展名——它们是会被打包进产物的源码面。 */
const EXTS = ['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs']

function walk(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...walk(full))
    else if (entry.isFile() && EXTS.some((e) => entry.name.endsWith(e))) out.push(full)
  }
  return out
}

/**
 * 单遍扫描：剔除注释（换空格，保持下标不变），并记录每个字符串字面量的区间。
 *
 * 为什么不能只跑正则、也不能整文件子串匹配：注释与普通字符串里出现的
 * `import … from '…'` 不是真实的 import 语法，不能算（本仓就有先例——测试的**注释**里
 * 写过 `@/fixtures/data` 字样）。剔除注释后，再用"匹配起点是否落在字符串区间内"
 * 把这层也堵上。
 */
function stripComments(code) {
  const chars = code.split('')
  const stringRanges = []
  const n = code.length
  let i = 0
  while (i < n) {
    const c = code[i]
    if (c === '/' && code[i + 1] === '/') {
      while (i < n && code[i] !== '\n') {
        chars[i] = ' '
        i++
      }
      continue
    }
    if (c === '/' && code[i + 1] === '*') {
      chars[i] = ' '
      chars[i + 1] = ' '
      i += 2
      while (i < n && !(code[i] === '*' && code[i + 1] === '/')) {
        if (code[i] !== '\n') chars[i] = ' '
        i++
      }
      if (i < n) {
        chars[i] = ' '
        chars[i + 1] = ' '
        i += 2
      }
      continue
    }
    if (c === "'" || c === '"' || c === '`') {
      const start = i
      i++
      while (i < n) {
        if (code[i] === '\\') {
          i += 2
          continue
        }
        if (code[i] === c) {
          i++
          break
        }
        i++
      }
      stringRanges.push([start, i])
      continue
    }
    i++
  }
  return { code: chars.join(''), stringRanges }
}

// ⚠ 静态/`export…from` 的"子句"用 `[^'"]` 而不是 `[^'"\n]`：它必须能跨行
//   （本仓多行 import 遍地都是），但**不能跨过引号**——否则会把下一条语句的字符串
//   误当成它的说明符。原来这里写的是 `[^'"\n]*?`（显式排除换行），对多行写法
//   一条都匹配不到 ⇒ 判据二名存实亡。形状与 `check-fixtures-imports.mjs` 同源。
const SPEC_PATTERNS = [
  /(?:^|\n)[ \t]*import\b\s*(?:type\s+)?[^'"]{0,2000}?\bfrom\s*['"]([^'"]+)['"]/g,
  /(?:^|\n)[ \t]*import\s*['"]([^'"]+)['"]/g,
  /(?:^|\n)[ \t]*export\b\s*(?:type\s+)?[^'"]{0,2000}?\bfrom\s*['"]([^'"]+)['"]/g,
  /\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)/g,
  /\brequire\s*\(\s*['"]([^'"]+)['"]\s*\)/g,
]

/** 取 import 的说明符（静态 / re-export / 动态 / require 四种写法，静态可跨行）。 */
function importSpecifiers(raw) {
  const { code, stringRanges } = stripComments(raw)
  const insideString = (idx) => stringRanges.some(([s, e]) => idx >= s && idx < e)
  const specs = []
  for (const re of SPEC_PATTERNS) {
    re.lastIndex = 0
    for (const m of code.matchAll(re)) {
      if (insideString(m.index)) continue
      specs.push(m[1])
    }
  }
  return specs
}

const problems = []
for (const file of walk(SRC)) {
  const rel = file.slice(ROOT.length).replace(/\\/g, '/')
  const base = rel.slice(rel.lastIndexOf('/') + 1)
  if (HOSTY.test(base)) {
    problems.push(`${rel} —— 文件名带 host`)
  }
  const text = readFileSync(file, 'utf8')
  for (const spec of importSpecifiers(text)) {
    if (HOSTY.test(spec)) problems.push(`${rel} —— import '${spec}'`)
  }
}

if (problems.length > 0) {
  console.error('源码守卫：Host 全托管路径已退役（TASK_64），但检测到它又在长出来：\n')
  for (const p of problems) console.error(`  ✗ ${p}`)
  console.error(
    '\n（判据二看的是 import 说明符本身，**多行写法也算**——`import { … } from …`' +
      ' 折成几行照样会被指出来；注释与字符串里提到 host 不算。）',
  )
  console.error(
    '如果这是有意的（例如新特性恰好叫 host），先改名字 —— 这个词在本仓库里' +
      '已经是退役那条线的专名，换一个不会撒谎的词。',
  )
  process.exit(1)
}
console.log('no-host check passed')
