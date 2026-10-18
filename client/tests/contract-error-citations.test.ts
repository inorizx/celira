// **错误词汇表的出处引用必须是真的**（2026-09-26 立）。
//
// ## 为什么需要它
//
// `src/api/contractErrors.ts` 的每一条 `case 'xxx':` 后面都注着 im 的出处
// （`// im :N` = `im/server/src/protocol/error.rs` 第 N 行）。那是这份词表**唯一的
// 交叉索引**：读的人靠它去核"这个码在服务端到底是什么语义、什么状态码"。
//
// 2026-09-26 的前端审计发现它**漂过一次**：第 16–54 行那批是对的，而**第 64 行以上
// 全部 -1**（im 那边在这一段之前加过一行，客户端注释没跟着动）。15 处引用同时指错，
// 而且**没有任何守卫能发现**——`npm run verify` 全绿。
//
// 一条指错的出处比没有出处更糟：它让人以为核过了。
//
// ## 这条判据做什么
//
// 把两边读进来，逐条断言：**`// im :N` 说的那一行，在 `error.rs` 里真的就是那个码。**
// 不是"文件存在"，不是"行数够长"，是**逐条对得上**。
//
// ⚠ 跨仓库目录读取：本仓库是 monorepo，`im/` 就在旁边。**只读，绝不写**。
// 若 `im/` 不在本工作区（客户端被单独分发），这条判据**明确报"不适用"**并 skip ——
// 按仓库纪律：「本平台不适用时明确报"不适用"，绝不假装通过」。
// 它会在 `node --test` 的 `skipped` 计数里可见。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const CLIENT_ERRORS = fileURLToPath(new URL('../src/api/contractErrors.ts', import.meta.url))
const IM_ERROR_RS = fileURLToPath(new URL('../../im/server/src/protocol/error.rs', import.meta.url))

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})

test('★ 每条 `// im :N` 都真的指到那个错误码（出处引用漂过一次，这条是防它再漂）', (t) => {
  if (!existsSync(IM_ERROR_RS)) {
    t.skip('im 源码不在本工作区 —— 这条判据不适用（不假装通过）')
    return
  }

  const source = readFileSync(CLIENT_ERRORS, 'utf8')
  const imLines = readFileSync(IM_ERROR_RS, 'utf8').split(/\r?\n/)

  // 抓 `case 'code': // im :N`（同一行上紧挨着的那种；本文件的注释就是这么写的）。
  const cited = [...source.matchAll(/case '([a-z_]+)':\s*\/\/ im :(\d+)/g)].map((m) => ({
    code: m[1],
    line: Number(m[2]),
  }))

  // 正向锚点：一条都没抓到说明正则过期了（比"全过"更该红）。
  assert.ok(cited.length >= 20, `至少要抓到那批带出处的 case（实抓 ${cited.length} 条）—— 正则过期了？`)

  const wrong = cited.filter(({ code, line }) => {
    const actual = imLines[line - 1] ?? ''
    return !actual.includes(`"${code}"`)
  })

  // ⚠ **反例（正对照）**：一个不会红的守卫等于没有守卫。这里手工造一条**已知错误**的
  // 引用（`invalid_mention` 曾经就写在 `:63` 上——im 那边第 63 行不是它），
  // 断言上面那段判据**真的会把它挑出来**。挑不出来说明判据本身坏了。
  const knownBad = { code: 'invalid_mention', line: 63 }
  const control = [{ ...knownBad }].filter(({ code, line }) => {
    const actual = imLines[line - 1] ?? ''
    return !actual.includes(`"${code}"`)
  })
  assert.equal(
    control.length,
    1,
    '正对照失败：把 `invalid_mention` 指到 im :63（那是漂移前写错的位置）必须被挑出来 —— 挑不出来说明这条判据是空转的',
  )

  assert.deepEqual(
    wrong,
    [],
    `这些出处指错了行（改注释或改 im 都要同步这一份）：\n` +
      wrong
        .map(({ code, line }) => `  ${code} → im :${line} 实际是：${(imLines[line - 1] ?? '<越界>').trim()}`)
        .join('\n'),
  )
})
