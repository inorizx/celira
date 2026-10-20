// **服务端最新一页并进已有时间线**的纯逻辑测试（2026-10-16「加载更早」那一刀）。
//
// 赌注：`refreshMessages` 每次 WS 唤醒都跑一次（= 每来一条新消息），而它从前是**整条替换**：
// `messagesByRoom[roomId] = [...sys, ...最新一页]`（服务端那一页只有 50 条）⇒
// **人翻上来的历史被下一条新消息整条抹掉**，"加载更早"等于白加载，视图还会跳回去。
//
// 这三条判据就是那一次的修复口径（缺一条都会退化成另一种坏）：
// ① 已有顺序不动（Core 时间线顺序不在浏览器复制 —— 与 `mergeOlderPage` 同一条纪律）；
// ② 同一 id **以服务端这一份为准**（编辑 / 撤回要跟上；本地乐观态不许压住服务端事实）；
// ③ 最新一页里没见过的 id **追加在尾部**。
//
// 运行：cd client && node --test --experimental-strip-types tests/

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  applyRoomPage,
  mergeLatestPage,
  mergeOlderPage,
  settleOptimisticId,
} from '../src/api/asyncOwnership.ts'
import type { Message } from '../src/api/domain.ts'

const msg = (id: string, over: Partial<Message> = {}): Message =>
  ({ id, roomId: 'r1', authorId: 'a1', body: id, createdAt: '2026-10-16T00:00:00Z', ...over }) as Message

test('★ 合并最新一页：翻上来的历史**不许被抹掉**（这一条就是那个机制问题）', () => {
  const history = [msg('m1'), msg('m2'), msg('m3')]
  const latest = [msg('m3'), msg('m4'), msg('m5')]
  const merged = mergeLatestPage(history, latest)
  assert.deepEqual(
    merged.map((m) => m.id),
    ['m1', 'm2', 'm3', 'm4', 'm5'],
    '历史留在前头，新来的追加在尾部',
  )
})

test('★ 合并最新一页：同一 id 以**服务端**那一份为准（编辑/撤回要跟得上）', () => {
  const mine = [msg('m1', { body: '本地乐观态' as unknown as Message['body'] })]
  const fromServer = [msg('m1', { body: '服务端事实' as unknown as Message['body'] })]
  const merged = mergeLatestPage(mine, fromServer)
  assert.equal(merged.length, 1)
  assert.equal(merged[0]!.body, '服务端事实')
})

test('★ 合并最新一页：顺序不动、不重复（去重按 id）', () => {
  const history = [msg('m1'), msg('m2')]
  const latest = [msg('m2'), msg('m1'), msg('m3'), msg('m3')]
  const merged = mergeLatestPage(history, latest)
  assert.deepEqual(merged.map((m) => m.id), ['m1', 'm2', 'm3'])
})

test('空的那一侧照旧成立（第一次读那一间 / 服务端还没回）', () => {
  assert.deepEqual(mergeLatestPage([], [msg('m1')]).map((m) => m.id), ['m1'])
  assert.deepEqual(mergeLatestPage([msg('m1')], []).map((m) => m.id), ['m1'])
})

test('★ 两条合并函数的**方向相反**，谁都不许把对方的活干了', () => {
  const older = [msg('m0'), msg('m1')]
  const existing = [msg('m1'), msg('m2')]
  assert.deepEqual(mergeOlderPage(existing, older).map((m) => m.id), ['m0', 'm1', 'm2'])
  assert.deepEqual(mergeLatestPage(existing, older).map((m) => m.id), ['m1', 'm2', 'm0'])
})

/**
 * ★ **同一个 id 恰好一条**（2026-10-16 与"合并"同一刀）。
 *
 * 病：乐观那条在拿到服务端 id 时**改名**，而服务端回显可能**先**到了 ⇒ 改完数组里
 * 同一个 id **两条**；而消息列表是**虚拟列表**（行身份 = 消息 id）⇒ 两行画在**同一个
 * `translateY`** 上 ⇒ 真机上看到"两段字压在一行"。
 *
 * ⚠ 从前"每次唤醒整条替换"恰好盖住了这个重复；改成合并之后它会**留下来** ⇒
 * 这条去重必须同时在场（两件事是一刀）。
 */
test('★ 乐观改名：服务端那一份已经到手时，改名之后仍然只有一条（不许同 id 两条）', () => {
  const optimistic = msg('local-1', { delivery: 'sending' as Message['delivery'] })
  const fromServer = msg('srv-1')
  const settled = settleOptimisticId([optimistic, fromServer], 'local-1', 'srv-1')
  assert.equal(settled.length, 1, '同 id 两条就是那个"两行压一行"')
  assert.equal(settled[0]!.id, 'srv-1')
  assert.equal(settled[0]!.delivery, 'delivered')
  // 正常那一趟（回显还没到）照旧：改名、标已投递。
  const plain = settleOptimisticId([optimistic, msg('srv-2')], 'local-1', 'srv-1')
  assert.deepEqual(plain.map((m) => m.id), ['srv-1', 'srv-2'])
  assert.equal(plain[0]!.delivery, 'delivered')
  // 不相干的一条都不许动。
  const others = settleOptimisticId([msg('m1'), optimistic], 'local-1', 'srv-1')
  assert.deepEqual(others.map((m) => m.id), ['m1', 'srv-1'])
})

// ---------------------------------------------------------------------------
// ★ **一页时间线落进某一间房：先问"这间房的窗口读过没有"**（2026-10-20 真机事故）
//
// 病：页面启动不预加载任何房间；门铃先送来一条新消息，直播间那条路把它写成了
// `messagesByRoom[room] = [那一条]`（**片段**，不是时间线）；用户开房时服务端整页
// 才到，而「合并最新一页」的口径是"已有顺序不动、没见过的追尾" ⇒ 那**一条**
// 被当成时间线的开头，最新那条被顶到**最上面**。
//
// 判据：**片段不是窗口**。窗口读没读过决定这一页该怎么落 ——
//   · 读过（翻过更早）：沿用 `mergeLatestPage`，历史留在前头；
//   · 没读过：这一页就是全部，**照它的顺序替换**（顺序仍归 Core，浏览器不排）；
//   · 片段里比这一页还新、页里还没有的那些，留住并排在页尾（否则用户的乐观那条会闪没）。
// ---------------------------------------------------------------------------

test('★ 直播先到的那一条：开房落页时不许被当成时间线的开头（2026-10-20 事故）', () => {
  // 事故现场原样：门铃送来第 6 条 → 片段 = [第6条]；开房时服务端整页 1..6 才到。
  const fragment = [msg('m6')]
  const page = [msg('m1'), msg('m2'), msg('m3'), msg('m4'), msg('m5'), msg('m6')]
  const merged = applyRoomPage({ existing: fragment, page, loaded: false })
  assert.deepEqual(
    merged.map((m) => m.id),
    ['m1', 'm2', 'm3', 'm4', 'm5', 'm6'],
    '最新那条排到最上面 —— 用户看到的就是这个',
  )
})

test('窗口读过了：照旧合并，翻上来的历史留在前头（2026-10-16 那条判据不许退回去）', () => {
  const history = [msg('m0')] // `loadOlder` 翻出来的更早那一截
  const page = [msg('m1'), msg('m2')]
  assert.deepEqual(
    applyRoomPage({ existing: history, page, loaded: true }).map((m) => m.id),
    ['m0', 'm1', 'm2'],
  )
})

test('窗口没读过：位置服从这一页 —— 但**一个字都不排序**', () => {
  // 页里的顺序故意不是字典序：谁敢在浏览器里排一遍，这条就红。
  const page = [msg('z'), msg('a'), msg('m')]
  // 片段里那条页里也有（第 6 条那个形状）：它的位置必须服从这一页，
  // **不是**它原来在片段里的第 0 位。
  assert.deepEqual(
    applyRoomPage({ existing: [msg('m')], page, loaded: false }).map((m) => m.id),
    ['z', 'a', 'm'],
  )
})

test('片段里服务端这一页还没有的那条要留住，排在页尾（否则乐观那条会闪没）', () => {
  const fragment = [msg('m6'), msg('local-7')] // m6 页里有；local-7 是刚发出去的乐观那条
  const page = [msg('m1'), msg('m2'), msg('m3'), msg('m4'), msg('m5'), msg('m6')]
  assert.deepEqual(
    applyRoomPage({ existing: fragment, page, loaded: false }).map((m) => m.id),
    ['m1', 'm2', 'm3', 'm4', 'm5', 'm6', 'local-7'],
  )
})

test('未读过的空房：那一页原样落下来（第一次读那一间）', () => {
  assert.deepEqual(applyRoomPage({ existing: [], page: [], loaded: false }), [])
  assert.deepEqual(
    applyRoomPage({ existing: [], page: [msg('m1')], loaded: false }).map((m) => m.id),
    ['m1'],
  )
})
