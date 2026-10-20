// **"一改全局就改"的三条硬约束**（2026-10-20 主人："最好就是一改 全局就改了那种"）。
//
// 这一条为什么按源码判：这三样量的都是"重复了几遍"——`tsc` 与 lint 都不会因为它们重复而报错，
// 而重复恰恰是"想改的时候发现要改五个文件"的根因。判据很窄、很机械：
//
//   1. 气泡的**阅读宽度**只在 `tokens.css` 里定（`--measure-bubble*` 三档），组件只挂 `.measure-bubble`；
//   2. **行内代码/代码块字号**只走 `text-code` 一档，不许再有 `text-[0.9em]` 这种一次性值；
//   3. **未读角标**只由 `UnreadBadge` 画，别处不许再出现 `h-[18px] min-w-[18px]` 那串。
//
// 要改这三样东西时，改令牌/组件即可 —— 这几条会挡住"顺手又在某个页面里写一遍"的第二次。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const CLIENT = fileURLToPath(new URL('..', import.meta.url))
const SRC = join(CLIENT, 'src')

/** 所有 .ts/.tsx（`src/styles` 除外：令牌本来就住在那儿）。 */
function sources(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) {
      if (entry === 'styles') continue
      out.push(...sources(full))
    } else if (/\.tsx?$/.test(entry)) {
      out.push(full)
    }
  }
  return out
}

const files = sources(SRC).map((path) => ({
  rel: relative(CLIENT, path).replace(/\\/g, '/'),
  text: readFileSync(path, 'utf8'),
}))

const hits = (pattern: RegExp, except?: string) =>
  files
    .filter((file) => (except ? !file.rel.endsWith(except) : true))
    .filter((file) => pattern.test(file.text))
    .map((file) => file.rel)

test('★ 行内代码字号只有一档：`text-code`（不许再写 `text-[0.9em]`）', () => {
  assert.deepEqual(hits(/text-\[0?\.9em\]/), [], '这串一次性字号又在组件里出现了')
  const tokens = readFileSync(join(SRC, 'styles', 'tokens.css'), 'utf8')
  assert.match(tokens, /--text-code:\s*[\d.]+em/, '`--text-code` 要用 em（跟着周围正文缩放）')
  assert.ok(
    files.some((f) => /text-code/.test(f.text)),
    '至少有一处真的用上了 `text-code`（否则这档令牌是摆设）',
  )
})

test('★ 气泡行宽只有一处：`.measure-bubble` ＋ `--measure-bubble*` 三档', () => {
  // 组件里不许再写死那三个百分比（它们是**阅读尺度**，改一处就该全站跟着变）。
  for (const pattern of [/max-w-\[85%\]/, /md:max-w-\[70%\]/, /xl:max-w-\[76%\]/]) {
    assert.deepEqual(hits(pattern), [], `${pattern} 又回到组件里了 —— 应该改 tokens.css 的 --measure-bubble*`)
  }
  const bubble = files.find((f) => f.rel.endsWith('MessageBubble.tsx'))
  assert.ok(bubble, '找不到 MessageBubble.tsx')
  assert.match(bubble!.text, /measure-bubble/, '气泡要挂 `.measure-bubble`')
  const tokens = readFileSync(join(SRC, 'styles', 'tokens.css'), 'utf8')
  for (const name of ['--measure-bubble:', '--measure-bubble-md:', '--measure-bubble-wide:']) {
    assert.ok(tokens.includes(name), `tokens.css 里少了 ${name}`)
  }
})

test('★ 未读角标只有一处：`UnreadBadge`（不许再写 `h-[18px] min-w-[18px]`）', () => {
  // ⚠ 扫的时候**排除 `UnreadBadge.tsx` 自己**：那串字就写在它的注释里（它就是"那一处"）。
  //   排除的是**那个文件**，不是那个尺寸 —— 别处再写一遍照样红。
  assert.deepEqual(
    hits(/h-\[18px\]/, 'UnreadBadge.tsx'),
    [],
    '角标尺寸又写进组件了 —— 应该改 UnreadBadge',
  )
  const badge = files.find((f) => f.rel.endsWith('UnreadBadge.tsx'))
  assert.ok(badge, '找不到 UnreadBadge.tsx')
  // 尺寸只在这个文件里（两档：18px / 16px），且用标尺而不是任意值。
  assert.match(badge!.text, /h-4\.5/, 'md 档应为 h-4.5（18px，走 Tailwind 标尺）')
  assert.match(badge!.text, /h-4\b/, 'sm 档应为 h-4（16px）')
})
