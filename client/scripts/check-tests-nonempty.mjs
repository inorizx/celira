#!/usr/bin/env node
// 源码守卫：**测试套件不许空跑**（2026-09-26 立）。
//
// ## 为什么需要它
//
// `AGENTS.md` 记着一笔账：「**门禁脚本必须断言语用例数 > 0**……`PASSED` 但
// `running 0 tests` 是**空跑**——看着绿，其实什么都没验。」
//
// 而本仓的真实处境是：`node --test` 在**收集到 0 条**用例时**照样退出 0**，
// 31 个测试文件里只有 6 个自带那条断言，且此前 `npm run verify` **根本不跑测试**
// （2026-09-26 审计发现）。三个条件叠起来 ⇒ 测试被误删、被 `.only` 挤掉、
// 或整批变成 `skip`，门禁都是绿的。
//
// ## 判据（只扫 `tests/`，三条）
//
// 1. **每个 `*.test.ts` 至少有一条用例声明**（`test(` / `it(`）—— 空文件是空跑；
// 2. **全仓用例声明总数 > 0**；
// 3. **不许出现 `.only`** —— 它会让**同文件其余用例静默不跑**，是本仓最隐蔽的一种空跑
//    （`node --test` 不会报错，只是少跑了）。
//
// `.skip(` / `test.todo(` **不拦**，但会**列出来**：它们本来就该可见
// （`node --test` 的 summary 会报 `skipped` / `todo` 计数），与 `agent/` 侧
// 「`#[ignore]` 必须带理由、让它在每次运行的 ignored 计数里可见」同一条纪律。
//
// 退出码：0 = 干净；1 = 命中；2 = 用法/环境错误（扫不到 tests/ 或 0 个文件）。
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const TESTS = join(ROOT, 'tests')

/** 用例声明：`test(` / `it(`（含 `test.only(` 这类带修饰的写法，交给第 3 条单独判）。 */
const CASE_DECL = /(^|[^.\w])(test|it)\s*\(/g
/** `.only` 一律拦：它静默挤掉同文件其余用例。 */
const ONLY = /(test|it|describe)\.only\s*\(/
/** 可见但不拦：跳过与待办。 */
const SKIP = /(test|it|describe)\.skip\s*\(|\btest\.todo\s*\(/

let files
try {
  files = readdirSync(TESTS).filter((f) => f.endsWith('.test.ts'))
} catch {
  console.error('扫不到 tests/ 目录 —— 守卫无法工作（不是"没问题"）')
  process.exit(2)
}
if (files.length === 0) {
  console.error('tests/ 里一个 *.test.ts 都没有 —— 整套测试是空的')
  process.exit(2)
}

const problems = []
const skips = []
let totalCases = 0

for (const name of files) {
  const rel = `tests/${name}`
  const source = readFileSync(join(TESTS, name), 'utf8')
  const lines = source.split('\n')

  const cases = source.match(CASE_DECL)?.length ?? 0
  totalCases += cases
  if (cases === 0) {
    problems.push(`${rel} —— 一条用例声明都没有（空文件就是空跑）`)
  }

  lines.forEach((line, index) => {
    if (ONLY.test(line)) {
      problems.push(`${rel}:${index + 1} —— 出现 .only（同文件其余用例会被静默挤掉）`)
    }
    if (SKIP.test(line)) {
      skips.push(`${rel}:${index + 1} —— ${line.trim().slice(0, 90)}`)
    }
  })
}

if (totalCases === 0) {
  problems.push('全仓用例声明总数为 0 —— 门禁会全绿，但什么都没验')
}

if (problems.length > 0) {
  console.error('源码守卫：测试套件不许空跑（2026-09-26 立），但检测到：\n')
  for (const p of problems) console.error(`  ✗ ${p}`)
  console.error('\n（怎么改：补回被删的用例；`.only` 一律拿掉——调试完就删，不许进提交。）')
  process.exit(1)
}

if (skips.length > 0) {
  console.log(`注意：有 ${skips.length} 处跳过/待办（不拦，但要看得见）：`)
  for (const s of skips) console.log(`  · ${s}`)
}

console.log(`no-empty-tests check passed（${files.length} 个文件 / ${totalCases} 条用例声明）`)
