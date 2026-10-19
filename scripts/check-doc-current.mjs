#!/usr/bin/env node
'use strict'
/**
 * 活文档守卫 —— `agent/README.md` 收的是「**当前事实**」，不是「**这一刀的经过**」。
 *
 * 填哪个洞（2026-10-18 立）
 * ---------------------------------------------------------------------------
 * `AGENTS.md` 的判据已经写死了：**这一节回答「这里现在是什么」；回答「这是哪一刀」的
 * 句子不属于活文档。** 但那条判据此前**没有任何机械判据会拦** —— 于是它长成了：
 * `本刀` 59 处、`这一刀` 45 处、`上一刀`/`下一刀`/`同刀` 17 处，散在 59 个节里。
 *
 * 这类句子的坏处不是"啰嗦"：**它把读者拉进一条他不在场的时间线**。一个陌生人读到
 * 「本刀改变了 X」时，他不知道"本刀"是哪一刀、也不知道自己该不该关心 —— 而他要的
 * 只是「X 现在是什么」。**答案本来就在同一段里，只是被包在了一层他读不懂的坐标里。**
 *
 * 判据方向（写死，不改）
 * ---------------------------------------------------------------------------
 * **拿"已提交的版本"当基线，拿"工作树"当被测**（与 `check-doc-honesty.mjs` 同款）：
 *
 *   硬判据（棘轮）：**全文刀叙述总数不许比基线多**。多一处 ⇒ 红。
 *   软读数（只报）：每个标题下的**条数**；以及 `TASK_NN` 的条数。
 *
 * ⚠ **为什么是"总数"棘轮，不是"逐节"棘轮**（这一条是设计，别改成逐节）：
 * 这份文档的整改方向是**把按落地顺序记的节归并进按主题记的节** —— 而归并**必然**
 * 让某些节的数字上升（两个节并成一个）。逐节棘轮会**挡住归并本身**，那就成了
 * "为了绿而不敢整理"。总数棘轮只禁止一件事：**净增**。
 *
 * ⚠ **`TASK_NN` 不进硬判据**：`AGENTS.md` 明写「代码注释里的施工单名是**事件标签**
 * （类似引用一个 commit），不随文件删除而改名」⇒ 它是**允许**的出处标注。这里只报数，
 * 不判红 —— 把一个"允许"的东西判红，只会训练人去删它，而删掉的是**出处**。
 *
 * 量得到 / 量不到（如实登记，不假装）
 * ---------------------------------------------------------------------------
 * 量得到：**刀叙述的净增**（硬）；**每个标题下的分布**（软）。
 * 量不到：① **换了个说法的刀叙述**（"上一次改动"、"新加的"、"从前是…现在是…"）
 *         —— 词表是封闭的，换个词它就看不见；
 *         ② **一句话是"当前事实"还是"变更叙述"**（"停止一次驱动不等于结束 Run" 两种读法
 *         都通）—— 这一格**只有人核**；
 *         ③ **`TASK_NN` 用得对不对**（是当出处，还是当主语）。
 * ⚠ 所以它**不是"文档已经干净"的证明**，只是"没有变脏"的下界。
 *
 * 退出码：0 = 通过 · 1 = 刀叙述净增了
 *
 * 用法：node scripts/check-doc-current.mjs
 */

import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(here, '..')
const DOC = 'agent/README.md'

/** 「这是哪一刀」的封闭词表。**加词要写理由** —— 每个词都是一种"把刀次当坐标"的说法。 */
const KNIFE_WORDS = [
  ['本刀', '把当前这一刀当成叙述的主语'],
  ['这一刀', '同上，另一种写法'],
  ['上一刀', '把历史刀次当成坐标'],
  ['下一刀', '把未来刀次当成坐标'],
  ['同刀', '把刀次当成分组键'],
]

/** 事件标签：**允许**（照 `AGENTS.md`），只报数不判红。 */
const EVENT_LABEL = /TASK_\d+/g

/**
 * 按标题切块：任何带号的标题（`## 4.` / `### 3.37`）开一块；标题之前的内容归 `(前言)`。
 *
 * ⚠ **键是标题原文**，不是号 —— 这份文档有过重复节号（`3.93` 出现两轮），按号做键会
 * 让后一节静默覆盖前一节。这里只做"分布读数"，不需要跨版本对齐，所以用原文更诚实。
 */
function blocks(text) {
  const lines = text.split('\n')
  const out = new Map()
  let key = '(前言)'
  out.set(key, [])
  for (const line of lines) {
    const m = /^#{2,3}\s+(\S.*)$/.exec(line)
    if (m) {
      key = m[1].trim()
      if (!out.has(key)) out.set(key, [])
      continue
    }
    out.get(key).push(line)
  }
  return out
}

/** 全文的刀叙述条数（硬判据用）。 */
function knifeTotal(text) {
  let n = 0
  for (const [word] of KNIFE_WORDS) n += text.split(word).length - 1
  return n
}

/** 每个标题下的刀叙述条数（软读数用）。 */
function knifeByBlock(text) {
  const rows = []
  for (const [title, body] of blocks(text)) {
    const joined = body.join('\n')
    let n = 0
    for (const [word] of KNIFE_WORDS) n += joined.split(word).length - 1
    if (n > 0) rows.push({ title, n })
  }
  return rows.sort((a, b) => b.n - a.n)
}

function eventLabels(text) {
  return (text.match(EVENT_LABEL) ?? []).length
}

function readBaseline() {
  return execFileSync('git', ['show', `HEAD:${DOC}`], {
    cwd: repoRoot,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  })
}

/** 植入违规：往一节里加一处刀叙述 ⇒ 必须被抓住。 */
function selfCheck() {
  const sample = '### 3.1 甲\n\n这一格是当前事实。\n'
  const broken = '### 3.1 甲\n\n本刀改了这一点。\n'
  const same = knifeTotal(sample)
  const grew = knifeTotal(broken)
  const ok = same === 0 && grew === 1
  if (!ok) {
    console.error('[check-doc-current] selfCheck 失败：植入的违规没被抓到 —— 否则这个守卫就是装饰')
    console.error(JSON.stringify({ same, grew }))
    process.exit(1)
  }
  return { planted: 1 }
}

const baselineText = readBaseline()
const currentText = readFileSync(join(repoRoot, DOC), 'utf8')
const self = selfCheck()

const was = knifeTotal(baselineText)
const is = knifeTotal(currentText)
const byBlock = knifeByBlock(currentText)
const labels = eventLabels(currentText)

if (is < was) {
  console.log(
    `[check-doc-current] 棘轮下降：刀叙述 ${was} → ${is}（少了 ${was - is} 处）—— 继续。`,
  )
}

console.log(
  `[check-doc-current] 软读数：刀叙述 ${is} 处、散在 ${byBlock.length} 个标题下；` +
    `事件标签 \`TASK_NN\` ${labels} 处（**允许**，只报不判）。`,
)
for (const row of byBlock.slice(0, 15)) {
  console.log(`    ${String(row.n).padStart(3)} 处  ${row.title.slice(0, 64)}`)
}
if (byBlock.length > 15) console.log(`    …另有 ${byBlock.length - 15} 个标题`)

if (is > was) {
  console.error(
    `[check-doc-current] 失败：刀叙述净增 ${was} → ${is}（多了 ${is - was} 处）。`,
  )
  console.error(
    '  ⇒ 判据：这一节要回答「**这里现在是什么**」；回答「**这是哪一刀**」的句子不属于活文档。',
  )
  console.error(
    '     改法：把「本刀改了 X」写成「X 是……」—— 事实本来就在同一段里，只是被包在了一层读者读不懂的坐标里。',
  )
  console.error(
    '     若确实要留一处刀次标注（出处），把它写成 `TASK_NN` 那种**事件标签**：那个是允许的。',
  )
  process.exit(1)
}

console.log(
  `[check-doc-current] 通过：刀叙述 ${is} 处（基线 ${was} 处，未净增）· ` +
    `自检植入 ${self.planted} 条违规已抓住。`,
)
