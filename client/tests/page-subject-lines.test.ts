// **"同一屏那句"**：一个词（或一份混了两个平面的清单）出现在用户面前时，
// 同一屏里必须有一句话能让人跨过它。
//
// ## 判据（2026-10-03 主人裁决）
//
// *"要符合用户直觉的那种……语义化，也不是小白化，因为假设用户懂点 AI"* ⇒ 词的取舍**不看难易**，
// 看它是不是"用户要操作的那件事"的名字：
//
// - **概念词**（世界 / 执行端 / Agent / 模型）＝ 用户要操作的对象 ⇒ **保留产品词**，不换大白话；
// - **实现词**（清醒 / 准入 / 读数 / 门铃…）＝ 我们内部怎么做的 ⇒ 不上屏（那是人话刀管的）；
// - **门槛必须能被跨过**：一个懂点 AI 的人第一次看到那个词，**同一屏**里能不能拿到"它是什么"？
//
// 这条测试钉的就是最后一格 —— 它是可验收的（"我觉得挺语义化"不是验收）。
//
// ⚠ 钉的是**事实**不是整句（与 `scripts/home/tests/docs.test.js` 同一风格）：改错字可以，
// 事实不许消失。谁要重写这些句子，先回来读这条判据。

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

test('「我的 Agent」那份清单说清它拼了两个平面（每行的数字来自机器，不是服务器）', () => {
  // 为什么必须有：每一行摆着「状态 / 模型 / 最近活动」，而用户看不出那三格是**某台机器**上的
  // 读数（甚至可能是"没有机器在跑它"）。清单页不点破，用户就会把机器事实当世界事实读。
  const panel = read('../src/components/celira/AgentsListPanel.tsx')
  assert.ok(panel.includes('拼两件事'), '清单页没有那句"拼两件事"')
  assert.ok(panel.includes('机器上的数据'), '清单页没有说清那一半来自机器')
  assert.ok(panel.includes('执行端'), '清单页该用产品词「执行端」（不是大白话）')
})

test('「我的执行端」那份清单说清它列的是什么（跑 Agent 的机器）', () => {
  const page = read('../src/pages/RuntimesPage.tsx')
  assert.ok(page.includes('跑 Agent 的机器'), '机器清单页没有"它列的是什么"那句')
  assert.ok(page.includes('执行端'), '该用产品词「执行端」')
})

test('「活动」这个词出现在哪儿，定义就跟到哪儿（同一屏那句要自洽）', () => {
  // 原句是"这台执行端上做过的每一次活动" —— 用「活动」定义「活动」，等于没说。
  // 现在这句是自洽的：**它每做一件事就算一次活动**。
  //
  // ⚠ **2026-10-12**：「活动」不再是一个房间（明细降为「运行」那一间里的**下钻**，
  // 主人裁：不问也该知道的留在页面上、问了才知道的交给问询）——**判据一字不变**：
  // 这个词出现在用户面前（「运行」那一间里就有「活动次数」那一格），同一屏就得有一句话能跨过它
  // ⇒ 定义句**跟着词走，不跟着房间走**。
  const rooms = read('../src/components/celira/Runtimes.tsx')
  assert.ok(rooms.includes('就算一次活动'), '「活动」的定义句不见了（或又变回循环定义）')
  assert.ok(
    !rooms.includes('做过的每一次活动'),
    '「活动」那一间又拿自己定义自己了（用户跨不过这个词）',
  )
  // 两间（运行 / 模型）都要有 `subject`（同一屏那句是这一页的形状，不是某一间的特权）。
  const subjects = rooms.match(/subject=/g) ?? []
  assert.ok(subjects.length >= 2, `两间（运行 / 模型）都该有那句：实际 ${subjects.length} 处`)
})
