// 「我」的归属纯逻辑测试（LOCAL_CLIENT_HOST.md Slice 2）。
//
// 证明：真实 Host Human id 的消息是「自己」（失败 pending 可重试、消息靠右、
// 显示回执/重试按钮）；其他参与者仍是 peer；fixtures SELF_ID 只在无连接身份
// 时兜底，绝不把 fixture 身份冒充为真实连接身份。
// 运行：cd web && node --test tests/

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  isRetriableSelfFailure,
  isSelfMessage,
  landingPlanFor,
  resolveSelfId,
  shouldFollowToBottom,
  isNearBottomOf,
  shouldMarkReadOnArrival,
  systemMessageIdentity,
} from '../src/api/selfIdentity.ts'
import { SELF_ID } from '../src/fixtures/data.ts'
import { tryGetParticipant } from '../src/api/participantDirectory.ts'
import type { Message } from '../src/api/domain'

function msg(over: Partial<Message>): Message {
  return {
    id: 'host-l-5',
    roomId: 'room-1',
    authorId: 'hm_01J8XKM3T2',
    body: 'hello',
    sentAt: '00:00',
    day: '今天',
    delivery: 'failed',
    ...over,
  }
}

test('real Host Human id is self; SELF_ID only falls back when no connection identity', () => {
  const hostHumanId = 'hm_01J8XKM3T2'
  // 带真实连接身份：优先真实 Human id
  assert.equal(resolveSelfId(hostHumanId), hostHumanId)
  // 无连接身份：回退 fixtures SELF_ID（Mock/Gallery 默认行为保持不变）
  assert.equal(resolveSelfId(''), SELF_ID)
  assert.equal(resolveSelfId(null), SELF_ID)
  assert.equal(resolveSelfId(undefined), SELF_ID)
  // 真实 Host Human 消息是自己，不是 peer
  assert.equal(isSelfMessage(hostHumanId, hostHumanId), true)
  // 其他参与者 / Agent 仍是 peer
  assert.equal(isSelfMessage('hm_other', hostHumanId), false)
  assert.equal(isSelfMessage('a-courier', hostHumanId), false)
})

test('failed own pending is retry-capable; other participants remain peer', () => {
  const hostHumanId = 'hm_01J8XKM3T2'
  // 自己失败的 pending：可重试
  assert.equal(isRetriableSelfFailure(msg({ authorId: hostHumanId, delivery: 'failed' }), hostHumanId), true)
  // 同样一条消息，无连接身份（回退 SELF_ID）时不视为自己 → 不提供重试
  assert.equal(isRetriableSelfFailure(msg({ authorId: hostHumanId, delivery: 'failed' }), undefined), false)
  // 其他参与者失败：仍是 peer，不重试
  assert.equal(isRetriableSelfFailure(msg({ authorId: 'hm_other', delivery: 'failed' }), hostHumanId), false)
  // 自己已投递：不是失败，无需重试
  assert.equal(isRetriableSelfFailure(msg({ authorId: hostHumanId, delivery: 'delivered' }), hostHumanId), false)
})

test('auto-follow: own Host human message always scrolls to bottom; peer only when near bottom', () => {
  const hostHumanId = 'hm_01J8XKM3T2'
  // 自己（Host 真实 Human id）的消息：即使用户在顶部读旧消息也跟随滚到底
  assert.equal(shouldFollowToBottom(hostHumanId, hostHumanId, false), true)
  assert.equal(shouldFollowToBottom(hostHumanId, hostHumanId, true), true)
  // 对方消息：只在已接近底部时跟随；在顶部时不打断向上翻阅
  assert.equal(shouldFollowToBottom('a-courier', hostHumanId, false), false)
  assert.equal(shouldFollowToBottom('hm_other', hostHumanId, false), false)
  assert.equal(shouldFollowToBottom('a-courier', hostHumanId, true), true)
  // 无连接身份（回退 fixtures SELF_ID）：fixture 自己的消息同样跟随
  assert.equal(shouldFollowToBottom(SELF_ID, undefined, false), true)
  // 无连接身份下，真实 Host Human 消息不视为自己 → 在顶部时按 peer 处理，不打断
  assert.equal(shouldFollowToBottom(hostHumanId, undefined, false), false)
})

// ★ 2026-10-05 主人真机反馈：「离底部近吗」原先写死 64px —— 她回一段长话就把人推出去了，
// 于是新消息来了**既不跟随滚动、也不推进读游标**（页面把两件事写在同一个 if 里）⇒
// 人明明在房里看着，未读却一直挂着、提示不消失，非得用鼠标去会话列表点一下才清。
// 判据改成**一屏之内都算在底部**（主流做法），这里把那条判据钉住。
test('near-bottom: 一屏之内算在底部（原先 64px 太紧）；翻到很上面才不算', () => {
  const viewport = { scrollHeight: 2000, clientHeight: 800 }

  // 贴底：在底部。
  assert.equal(isNearBottomOf({ ...viewport, scrollTop: 1200 }), true, '正好贴底')
  // 差 200px（她那段长话的长度）：**仍在底部** —— 这正是主人撞到的那一格（修之前是 false）。
  assert.equal(isNearBottomOf({ ...viewport, scrollTop: 1000 }), true, '差 200px 仍在底部（修的就是这一格）')
  // 差一屏（800px）：**边界是严格的** —— 正好一屏算"远了"（`< slack`，不是 `<=`）。
  // 把边界钉在这里：改判据的人会看到这一条，而不是让它悄悄漂。
  assert.equal(isNearBottomOf({ ...viewport, scrollTop: 401 }), true, '差 799px：在底部')
  assert.equal(isNearBottomOf({ ...viewport, scrollTop: 400 }), false, '正好差一屏：算远了（严格小于）')
  // 差得比一屏还多（翻到上面看历史）：不算 —— 不打断他。
  assert.equal(isNearBottomOf({ ...viewport, scrollTop: 300 }), false, '翻了 900px：不打断')
  assert.equal(isNearBottomOf({ ...viewport, scrollTop: 0 }), false, '在顶部读旧消息：不打断')

  // 视口很小 / 动态行高还没测出来（clientHeight 为 0）⇒ 至少留 64px 宽容，不许变成"必须精确贴底"。
  assert.equal(isNearBottomOf({ scrollHeight: 500, scrollTop: 440, clientHeight: 0 }), true, '小视口留 64px')
  assert.equal(isNearBottomOf({ scrollHeight: 500, scrollTop: 400, clientHeight: 0 }), false, '超出 64px 才算远')

  // 内容比视口还短（没有滚动条）：永远算在底部。
  assert.equal(isNearBottomOf({ scrollHeight: 300, scrollTop: 0, clientHeight: 800 }), true, '内容不满一屏')
})

// ── 新内容落位之后"要不要上报已读"（2026-10-14 主人真机反馈：读完了角标还挂着）
//
// 这一格与上面那条是**同一族病、不同的病灶**：上面修的是"判据太紧"（64px），
// 这条修的是"**什么时候问**"——滚动监听只在**过渡**里上报，而长消息把底部推走是
// **布局**变化、不发滚动事件 ⇒ 快照过期，人滚回底部时被当成"没变化"提前返回。
// 判据：**现在到底了**（落位之后重算）**且真有未读** ⇒ 上报；两者缺一都不报。
test('读到就报：到底 ＋ 真有未读才上报（缺一都不报，没有未读时一个请求都不发）', () => {
  assert.equal(
    shouldMarkReadOnArrival({ nearBottom: true, unreadCount: 1 }),
    true,
    '这就是主人那一场：人在底部把消息读完了 ⇒ 该上报（修之前永远不报）',
  )
  assert.equal(
    shouldMarkReadOnArrival({ nearBottom: false, unreadCount: 3 }),
    false,
    '翻上去看历史 ⇒ 不打断、也不上报',
  )
  assert.equal(
    shouldMarkReadOnArrival({ nearBottom: true, unreadCount: 0 }),
    false,
    '没有未读 ⇒ 什么都不做（这条挡的是"每来一条都发一次请求"）',
  )
  assert.equal(shouldMarkReadOnArrival({ nearBottom: false, unreadCount: 0 }), false)
})

// ── Slice 4 SystemMessage fail-closed（LOCAL_CLIENT_HOST.md）：SystemMessage 的
// SystemMessage.tsx 撤回文案只经此纯函数解析。证明：真实 resolver 上下文中，
// resolver 返回 undefined 且 authorId 与 fixtures 碰撞时，仍是中性「参与者」、
// 不是「你」、不是 Agent、不显示 fixture 身份名；selfId 缺失绝不回退 SELF_ID；
// 只有 resolver 缺省（Mock/Gallery）才允许 fixtures 回退与 SELF_ID 兜底。

test('real resolver + unknown author + fixture id collision stays neutral (not you / not Agent)', () => {
  // Host 上下文：真实 resolver 存在，但对真实 Core id 返回 undefined（作者不在
  // me + room.participants 投影）；该 id 恰好与 fixtures 的 Agent「Courier」碰撞。
  const resolver = () => undefined
  const authorId = 'a-courier'
  const fixture = tryGetParticipant(authorId)
  assert.equal(fixture?.kind, 'agent', 'precondition: authorId collides with a fixture Agent')
  // selfId 缺失（无真实连接身份）：仍是中性作者、不是自己。
  const missing = systemMessageIdentity(authorId, undefined, resolver, tryGetParticipant)
  assert.equal(missing.authorName, '参与者', 'unresolved author must show neutral 参与者')
  assert.notEqual(missing.authorName, fixture?.displayName, 'must never fall back to fixture name')
  assert.equal(missing.isSelfAuthor, false, 'missing real selfId must not be treated as self')

  // selfId 是真实 Human id（非 authId）→ 同样不是自己。
  const withSelf = systemMessageIdentity(authorId, 'hm_real_human_1', resolver, tryGetParticipant)
  assert.equal(withSelf.authorName, '参与者')
  assert.equal(withSelf.isSelfAuthor, false)
})

test('real resolver never falls back to fixtures even when selfId collides with SELF_ID', () => {
  // Host 上下文 + selfId 缺失：即使 authorId 恰好等于 fixtures SELF_ID，也绝不回退
  // SELF_ID 把它当成「我」——fixtures 身份绝不是真实连接身份。
  const resolver = () => undefined
  const done = systemMessageIdentity(SELF_ID, undefined, resolver, tryGetParticipant)
  assert.equal(done.isSelfAuthor, false, 'real resolver context must not fall back to SELF_ID')
  assert.equal(done.authorName, '参与者')
})

test('real resolver resolves author name and self authorship from the connected identity only', () => {
  const authorId = 'hm_real_human_1'
  const resolver = (id: string) => (id === authorId ? '真实我' : undefined)
  const self = systemMessageIdentity(authorId, authorId, resolver, tryGetParticipant)
  assert.equal(self.authorName, '真实我')
  assert.equal(self.isSelfAuthor, true, 'own message via real identity')
  const peer = systemMessageIdentity('hm_peer_1', authorId, resolver, tryGetParticipant)
  assert.equal(peer.authorName, '参与者')
  assert.equal(peer.isSelfAuthor, false)
})

test('resolver absent (Mock/Gallery) keeps fixtures fallback and SELF_ID self-detection', () => {
  // 无 resolver：认识的名字回退 fixtures；自己按 fixtures SELF_ID（或无则默认）。
  const fixture = systemMessageIdentity('a-courier', undefined, undefined, tryGetParticipant)
  assert.equal(fixture.authorName, 'Courier')
  const selfByDefault = systemMessageIdentity(SELF_ID, undefined, undefined, tryGetParticipant)
  assert.equal(selfByDefault.isSelfAuthor, true, 'Mock/Gallery SELF_ID self-detection preserved')
  const selfByExplicit = systemMessageIdentity(SELF_ID, 'hm_other', undefined, tryGetParticipant)
  assert.equal(selfByExplicit.isSelfAuthor, false, 'explicit selfId wins over SELF_ID in Mock/Gallery')
  const unknown = systemMessageIdentity('unknown-id', undefined, undefined, tryGetParticipant)
  assert.equal(unknown.authorName, '参与者')
  assert.equal(unknown.isSelfAuthor, false)
})

/**
 * ★ **进会话落在哪一行**（2026-10-16 主人真机反馈："每次我进入会话的时候，默认不是聚焦在最新的那种"）。
 *
 * 判据两条：**有未读分隔线 ⇒ 落分隔线**（微信语义，2026-10-05 定过）；**没有 ⇒ 落最后一条**。
 * 再加一条边界：**这一间还没消息 ⇒ `null`**（什么都没得落 —— 不许当成"落到第 0 行"，
 * 那会把人送到历史最前头，正是这一次症状的另一半）。
 */
test('★ 进会话的落点：有未读落分隔线、没有落最后一条、没消息就不落', () => {
  assert.deepEqual(landingPlanFor({ unreadIndex: -1, rowCount: 12 }), {
    kind: 'latest',
    index: 11,
  })
  assert.deepEqual(landingPlanFor({ unreadIndex: 7, rowCount: 12 }), {
    kind: 'divider',
    index: 7,
  })
  assert.equal(landingPlanFor({ unreadIndex: -1, rowCount: 0 }), null, '没消息 ⇒ 不落（不是落到 0）')
  // 分隔线越界（读数陈旧）⇒ 退回"最后一条"，不许 `scrollToIndex` 越界。
  assert.deepEqual(landingPlanFor({ unreadIndex: 99, rowCount: 3 }), { kind: 'latest', index: 2 })
  assert.deepEqual(landingPlanFor({ unreadIndex: 0, rowCount: 1 }), { kind: 'divider', index: 0 })
})
