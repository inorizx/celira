// 中性 UI 策略纯逻辑测试（node --test + 类型剥离）。
//
// 被测对象是 `src/api/uiPolicy.ts`：能力探测、附件入口策略、契约字段前端早
// 校验、会话行展示投影。这些纯函数的输入（`ApiError.code`、`Room` 投影、契约
// 字段）在 Host 与 core 两条线上是同一份形状 —— **与宿主无关**。
//
// TASK_64 删除刀 D 的覆盖保护：下列用例原先散在 host-*.test.ts（整份在删除
// 名单上）里，**逐字搬出**，以免中性模块的覆盖随 Host 线一起清零。用例文本、
// 断言、注释一字未改；`msg` / `room` 两个局部 fixture 随用例一并搬来。

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  agentProfileFieldError,
  attachmentInputMode,
  classifyContactRequest,
  looksLikeParticipantId,
  presentationFieldError,
  roomPeerNameOf,
  roomPreview,
  roomPreviews,
  neutralParticipant,
  supportsAgentRevoke,
  supportsAgentBindings,
  supportsAgentBindingWrite,
  supportsAgentCredentials,
  supportsAgentCredentialWrite,
  supportsAgentDetail,
  supportsAgentModels,
  supportsAgentProfile,
  supportsContactRequest,
  supportsRoomCreation,
  supportsMemberPresentations,
} from '../src/api/uiPolicy.ts'
import type { Message, Room } from '../src/api/domain'

function msg(over: Partial<Message> & { id: string }): Message {
  return {
    roomId: 'room-1',
    authorId: 'human-1',
    body: 'x',
    sentAt: '00:00',
    day: '今天',
    delivery: 'delivered',
    ...over,
  }
}

function room(over: Partial<Room> & { id: string }): Room {
  return {
    id: over.id,
    name: '会话',
    participantIds: [],
    lastMessagePreview: '',
    lastActivityAt: '',
    unreadCount: 0,
    ...over,
  }
}

test('撤销解锁判据 = 传输层真实能力（core 直连 / Host 都有，mock 没有）', () => {
  // Host 模式（TASK_52 补线后）与 core 直连都实现 → 撤销区可见。
  assert.equal(supportsAgentRevoke({ csrf: 'csrf', revokeAgent: async () => {} }), true)
  assert.equal(
    supportsAgentRevoke({ revokeAgent: async () => {} }),
    true,
    'core 直连形态（鸭子类型）同样算能力',
  )
  // mock 兜底没有实现 → 整块不渲染，不给一个点了必然报错的按钮。
  assert.equal(supportsAgentRevoke({}), false)
  assert.equal(supportsAgentRevoke(null), false)
  assert.equal(supportsAgentRevoke(undefined), false)
  assert.equal(supportsAgentRevoke({ revokeAgent: 'yes' }), false, '非函数不算能力')
})

// —— TASK_72 ⑥：三处"数据面门控"的判据（原本是 `mode === 'core'`）——
//
// 判错即红的样子有两种，这里两条都钉住：
//  ① **有这个能力** ⇒ 显形（core 直连真的实现了那些方法）；
//  ② **只有 `mode === 'core'`、没有那个方法** ⇒ 不显形 —— 这正是"拿 mode 当判据"会错的地方
//     （演示通道也活在 `mode === 'core'` 里，而它 `getApi()` 是 null）。

test('后端绑定区判据 = `listBackendBindings` 真的在（不是 mode）', () => {
  assert.equal(supportsAgentBindings({ listBackendBindings: async () => [] }), true)
  assert.equal(
    supportsAgentBindings({ mode: 'core' }),
    false,
    '只有 mode === core、没有那个方法 ⇒ 不显形（演示通道就是这一档）',
  )
  assert.equal(supportsAgentBindings({}), false)
  assert.equal(supportsAgentBindings(null), false)
  assert.equal(supportsAgentBindings(undefined), false)
  assert.equal(supportsAgentBindings({ listBackendBindings: 'yes' }), false, '非函数不算能力')
})

test('凭据区判据 = `listCredentials` 真的在', () => {
  assert.equal(supportsAgentCredentials({ listCredentials: async () => [] }), true)
  assert.equal(supportsAgentCredentials({ mode: 'core' }), false, '同上：mode 不是判据')
  assert.equal(supportsAgentCredentials(null), false)
  assert.equal(supportsAgentCredentials({ listCredentials: 1 }), false)
})

test('选中时拉详情的判据 = `getAgent` 真的在', () => {
  assert.equal(supportsAgentDetail({ getAgent: async () => ({}) as never }), true)
  assert.equal(supportsAgentDetail({ mode: 'core' }), false)
  assert.equal(supportsAgentDetail(undefined), false)
  assert.equal(supportsAgentDetail({}), false)
})

test('Agent 公共资料区判据 = `getAgentProfile` 真的在', () => {
  assert.equal(supportsAgentProfile({ getAgentProfile: async () => ({}) as never }), true)
  assert.equal(supportsAgentProfile({ mode: 'core' }), false, '演示态不再渲染空区')
  assert.equal(supportsAgentProfile(null), false)
  assert.equal(supportsAgentProfile({ getAgentProfile: {} }), false)
})

// 「模型」区是那张卡上**唯一没有门**的分区（收齐时漏掉的一个）：它读供应商目录那份事实，
// 而系统里没有它 ⇒ 真实世界照旧渲染 fixtures 的假模型名与假供应商表。
test('Agent「模型」区判据 = 供应商目录数据面 `listProviders` 真的在（今天没有任何实现有它）', () => {
  assert.equal(supportsAgentModels({ listProviders: async () => [] }), true, '接了目录之后这一区自己回来')
  assert.equal(supportsAgentModels({ mode: 'core' }), false, 'mode 不是判据（TASK_72 ⑥）')
  assert.equal(supportsAgentModels(null), false, '演示通道 getApi() 是 null')
  assert.equal(supportsAgentModels(undefined), false)
  assert.equal(supportsAgentModels({}), false)
  assert.equal(supportsAgentModels({ listProviders: 'yes' }), false, '非函数不算能力')
})

test('「贴上对方的身份 ID」那块提示的判据 = `sendContactRequest` 真的在', () => {
  assert.equal(supportsContactRequest({ sendContactRequest: async () => {} }), true)
  assert.equal(supportsContactRequest({ mode: 'core' }), false)
  assert.equal(supportsContactRequest(undefined), false)
  assert.equal(supportsContactRequest({ sendContactRequest: 'yes' }), false)
})

// —— 关系申请：输入形状 ＋ 回执的四个分支（2026-10-20）——
//
// 这一页（`/app/contacts/new`）只有一个输入框、一个按钮，所以它的两个判断都不许含糊：
// ① 贴进来的是不是一串 id（不是"这个人存不存在"——那只有服务端能答）；
// ② 回执说的是哪一件事（「我刚发出去」与「对方先发给我、该我点头」是反的两件事）。

test('身份 ID 的形状：只认规范形 UUID v4（大小写与首尾空白宽容，其余一律不算）', () => {
  const canon = '3f2504e0-4f89-41d3-9a0c-0305e82c3301'
  assert.equal(looksLikeParticipantId(canon), true)
  assert.equal(looksLikeParticipantId(`  ${canon}  `), true, '贴进来常带空白')
  assert.equal(looksLikeParticipantId(canon.toUpperCase()), true, '大写是同一个 id（发出去之前统一小写）')

  // 反例：这一格存在的理由就是这些 —— 名字会被服务端当 id 查，回一个"找不到"，
  // 而那是**服务端说没有这个人**，不是"你贴了一串名字"。
  assert.equal(looksLikeParticipantId('张三'), false)
  assert.equal(looksLikeParticipantId('hm_01J8XKM3T2'), false, '演示夹具的假 id 不是世界里的形状')
  assert.equal(looksLikeParticipantId(''), false)
  // v1 / 非 v4：服务端 `ParticipantId::parse_stable` 也只认 v4（`participant.rs:20`）。
  assert.equal(looksLikeParticipantId('3f2504e0-4f89-11d3-9a0c-0305e82c3301'), false)
  // 少一段 / 多一段。
  assert.equal(looksLikeParticipantId('3f2504e0-4f89-41d3-9a0c-0305e82c33'), false)
  assert.equal(looksLikeParticipantId('3f2504e0-4f89-41d3-9a0c-0305e82c3301x'), false)
})

test('★ 回执的四个分支不许合并成「请求已发送」', () => {
  const peer = { id: 'p', kind: 'agent' as const, displayName: '助手', initials: '助' }
  const base = { peer, status: 'pending' as const, direction: 'outgoing' as const }

  // 201：刚建的段。
  assert.equal(classifyContactRequest({ ...base, replayed: false }), 'sent')
  // 200 + outgoing：我之前发过，还在等。
  assert.equal(classifyContactRequest({ ...base, replayed: true }), 'awaiting-them')
  // 200 + incoming：**对方先发过给我** —— 这一支从前被说成「已发送，等对方」，
  // 而真实处境是"轮到我点头"（反了）。
  assert.equal(
    classifyContactRequest({ ...base, direction: 'incoming', replayed: true }),
    'needs-my-answer',
  )
  // 200 + accepted：早就是联系人（不是"刚发出去"）。
  assert.equal(
    classifyContactRequest({ ...base, status: 'accepted', direction: 'incoming', replayed: true }),
    'now-contact',
  )
  // accepted 优先于方向：accepted 段的方向对"发生了什么"没有意义。
  assert.equal(classifyContactRequest({ ...base, status: 'accepted', replayed: true }), 'now-contact')
})

// —— 写面判据（2026-09-26 立）：**读得到 ≠ 改得动** ——
//
// 审计发现：绑定区 / 凭据区只探了**读面**（`listBackendBindings` / `listCredentials`），
// 而按钮走的是**写面** —— 演示通道（MockApi 有读无写）里四个按钮全都渲染出来、
// 点下去什么也不发生、也不报错。写能力因此**单独探**。

test('★ 接入的写面判据：创建与撤销**都在**才算能改（只探读面会让按钮点了没反应）', () => {
  assert.equal(
    supportsAgentBindingWrite({
      createBackendBinding: async () => {},
      revokeBackendBinding: async () => {},
    }),
    true,
  )
  // 只有读面 ⇒ **不能改**（演示通道就是这一档）
  assert.equal(supportsAgentBindingWrite({ listBackendBindings: async () => ({}) }), false)
  // 缺一条也不算（两条路都得在）
  assert.equal(supportsAgentBindingWrite({ createBackendBinding: async () => {} }), false)
  assert.equal(supportsAgentBindingWrite(null), false)
  assert.equal(supportsAgentBindingWrite({ createBackendBinding: 'yes' }), false, '非函数不算能力')
})

test('★ 凭据的写面判据：签发与撤销**都在**才算能改', () => {
  assert.equal(
    supportsAgentCredentialWrite({ issueCredential: async () => ({}), revokeCredential: async () => {} }),
    true,
  )
  assert.equal(supportsAgentCredentialWrite({ listCredentials: async () => [] }), false)
  assert.equal(supportsAgentCredentialWrite({ issueCredential: async () => ({}) }), false)
  assert.equal(supportsAgentCredentialWrite(undefined), false)
})

// ⚠ **2026-09-28**：这里原有一条「对外服务的写面判据」用例
// （`supportsAgentServiceWrite`：发布 / 授予 / 撤销**都在**才算能改）。
// 判据、它守的 `AgentServicePanel` 与整条服务面（Publication / Service Access /
// 服务房）一起随 **Agent 服务层退役**删除 ⇒ 断言对象不存在了，用例随之删除
// （不是覆盖缺口：同形状的判据在上面两条与下面几条里各有用例）。
// 页面级"不许回来"由 `route-admission.test.ts`（已删的 /app/services）钉住。

// —— 同一缺陷收齐（主人 2026-09-21 批）：名单外那三处 ——
//
// 这三处的旧判据同样是 `mode === 'core'`（**恒真**）。判据形状照旧：有能力 ⇒ 真；
// **只有 `mode === 'core'` 没有方法 ⇒ 假** —— 演示通道就是这一档（`getApi()` 是 null）。

test('「新的会话」建房判据 = `createHumanDirectRoom` 与 `createGroup` **都在**', () => {
  assert.equal(
    supportsRoomCreation({ createHumanDirectRoom: async () => ({ roomId: 'r' }), createGroup: async () => ({ roomId: 'r' }) }),
    true,
  )
  assert.equal(
    supportsRoomCreation({ createHumanDirectRoom: async () => ({ roomId: 'r' }) }),
    false,
    '缺一条（群聊）⇒ 不走真实分支：那两条路都要在',
  )
  assert.equal(supportsRoomCreation({ mode: 'core' }), false)
  assert.equal(supportsRoomCreation(undefined), false)
  assert.equal(supportsRoomCreation({ createHumanDirectRoom: 'x', createGroup: 'y' }), false)
})

test('房间详情抽屉的成员呈报判据 = `getMemberPresentations` 真的在', () => {
  assert.equal(supportsMemberPresentations({ getMemberPresentations: async () => ({}) as never }), true)
  assert.equal(supportsMemberPresentations({ mode: 'core' }), false)
  assert.equal(supportsMemberPresentations(null), false)
  assert.equal(supportsMemberPresentations({ getMemberPresentations: 0 }), false)
})

test('hostUi byte-length validation matches the contract limits (UTF-8 bytes)', () => {
  // room_display_name ≤ 80 字节（ASCII 每字 1 字节，中文每字 3 字节）。
  assert.equal(presentationFieldError({ roomDisplayName: 'a'.repeat(80), roomRoleName: '', roomRoleDescription: '' }), null)
  assert.equal(
    typeof presentationFieldError({ roomDisplayName: 'a'.repeat(81), roomRoleName: '', roomRoleDescription: '' }),
    'string',
  )
  // 超限中文返回稳定文案。
  const tooLong = presentationFieldError({
    roomDisplayName: '中'.repeat(27), // 81 字节
    roomRoleName: '',
    roomRoleDescription: '',
  })
  assert.equal(typeof tooLong, 'string')
  assert.match(tooLong ?? '', /80 字节/)
  assert.equal(
    presentationFieldError({ roomDisplayName: '', roomRoleName: '', roomRoleDescription: 'x'.repeat(4096) }),
    null,
  )
  assert.equal(
    typeof presentationFieldError({ roomDisplayName: '', roomRoleName: '', roomRoleDescription: 'x'.repeat(4097) }),
    'string',
  )
  // 禁 U+0000。
  assert.equal(
    typeof presentationFieldError({ roomDisplayName: 'a\u0000b', roomRoleName: '', roomRoleDescription: '' }),
    'string',
  )
})

test('hostUi profile byte-validation matches 512/16384 limits and NUL rejection', () => {
  assert.equal(agentProfileFieldError({ shortDescription: 'x'.repeat(512), publicDescription: '' }), null)
  assert.equal(typeof agentProfileFieldError({ shortDescription: 'x'.repeat(513), publicDescription: '' }), 'string')
  assert.equal(agentProfileFieldError({ shortDescription: '', publicDescription: 'y'.repeat(16384) }), null)
  assert.equal(typeof agentProfileFieldError({ shortDescription: '', publicDescription: 'y'.repeat(16385) }), 'string')
  assert.equal(
    typeof agentProfileFieldError({ shortDescription: 'ok', publicDescription: 'bad\u0000byte' }),
    'string',
  )
})

test('★ attachmentInputMode 按**能力**判，不按 `mode`（2026-09-26 修）', () => {
  // 原判据是 `apiMode === 'core' ? 'real' : …` —— 而**演示通道的 MockApi 也声明
  // `mode = 'core'`**（`ConnectionMode` 如今只剩这一个取值，那一格不携带任何信息），
  // 于是演示通道被判成 'real'：输入区只渲染真实上传项，而 `MockApi.uploadAttachment`
  // 是抛错的 ⇒ 选文件必报错，演示附件那条路整条死掉。
  //
  // 现在问的是**这一层有没有字节通道**（`fetchAttachmentBlob`）：
  // 真实 CoreApi 有、MockApi 不实现（演示附件是本地静态资源，由 attachmentSource 直出）。
  const realTransport = { fetchAttachmentBlob: async () => new Blob() }
  const demoTransport = { mode: 'core', uploadAttachment: async () => ({ attachmentId: 'x' }) }

  assert.equal(attachmentInputMode(realTransport), 'real', '有字节通道 ⇒ 真实上传')
  assert.equal(
    attachmentInputMode(demoTransport),
    'demo',
    '★ 声明 mode="core" 但**没有字节通道** ⇒ 演示通道，绝不许判成 real',
  )
  assert.equal(attachmentInputMode(null), 'unavailable', '连传输层都没有 ⇒ fail closed')
  assert.equal(attachmentInputMode(undefined), 'unavailable', '缺省同样 fail closed，不回退 Demo')
})

test('roomPreview: latest loaded Host message overrides empty room preview and activity time', () => {
  const r = room({
    id: 'room-1',
    participantIds: ['human-1', 'agent-1'],
    peer: { id: 'agent-1', kind: 'agent', displayName: 'Agent', initials: 'A' },
    lastMessagePreview: '',
    lastActivityAt: '',
  })
  const messages = [
    msg({ id: 'm1', body: '旧消息', day: '昨天', sentAt: '10:00' }),
    msg({ id: 'm2', authorId: 'agent-1', body: '这是最新一条宿主消息内容', day: '今天', sentAt: '14:30' }),
  ]
  const derived = roomPreview(r, messages, (id) => (id === 'agent-1' ? 'Agent' : undefined))
  assert.equal(derived.lastMessagePreview, 'Agent：这是最新一条宿主消息内容')
  assert.equal(derived.lastActivityAt, '14:30')
  assert.notEqual(derived, r, 'derivation returns a new display room, not the same reference')
})

test('roomPreview: attachment-only message becomes [图片]/[文件] preview; non-today uses day label', () => {
  const r = room({ id: 'room-1' })
  const imageDerived = roomPreview(
    r,
    [msg({ id: 'm3', body: '', attachments: [{ id: 'a1', kind: 'image', name: 'p.png' }] })],
  )
  assert.equal(imageDerived.lastMessagePreview, '[图片] p.png')
  const fileDerived = roomPreview(
    r,
    [msg({ id: 'm4', body: '', attachments: [{ id: 'a2', kind: 'file', name: 'd.pdf' }] })],
  )
  assert.equal(fileDerived.lastMessagePreview, '[文件] d.pdf')
  const yesterday = roomPreview(r, [msg({ id: 'm5', body: '旧内容', day: '昨天', sentAt: '23:00' })])
  assert.equal(yesterday.lastActivityAt, '昨天')
})

test('roomPreview: no loaded message leaves room unchanged (same reference, same fields)', () => {
  const r = room({ id: 'room-1', lastMessagePreview: '已有预览', lastActivityAt: '昨天' })
  assert.equal(roomPreview(r, []), r, 'empty messages returns the input room unchanged')
  const onlySystem = roomPreview(r, [msg({ id: 'sys-1', body: '', system: true })])
  assert.equal(onlySystem, r, 'all-system messages leave the room unchanged')
  assert.equal(onlySystem.lastMessagePreview, '已有预览')
  assert.equal(onlySystem.lastActivityAt, '昨天')
})

test('roomPreviews: 作者解析不出时**不阻断整批投影**（store 内联版曾在这一步抛、被 catch 吞掉整个列表刷新）', () => {
  // 这是「同一规则两份、且已行为分叉」那处的判据。`store.refreshRooms` 原先**内联重写**
  // 了一遍本规则，且那版用 `getParticipant` 解析作者名 —— 它遇到解析不出的作者会**抛异常**，
  // 而异常又被 `refreshRooms` 的 `catch` 吞掉 ⇒ 一个解析不出的作者能让**整个房间列表
  // 刷新静默停掉**（本仓"红路径不许静默"要治的形状）。用回本函数（有测试的这一侧）后：
  // 作者未知 ⇒ 省略前缀、预览照给、整批照投。
  const rooms = [
    room({ id: 'room-1', lastMessagePreview: '', lastActivityAt: '' }),
    room({ id: 'room-2', lastMessagePreview: '原有的', lastActivityAt: '昨天' }),
  ]
  const out = roomPreviews(
    rooms,
    (roomId) =>
      roomId === 'room-1' ? [msg({ id: 'm1', authorId: '解析不出的作者', body: '你好世界' })] : [],
    null,
  )
  assert.equal(out.length, 2, '一个解析不出的作者不许让整批投影消失')
  assert.equal(out[0].lastMessagePreview, '你好世界', '作者未知 ⇒ 省略前缀，正文照给')
  assert.equal(out[1], rooms[1], '没有已加载消息的房间原样返回（同一引用）')
})

test('neutralParticipant：中性参与者叫「参与者」，且**不猜 kind**', () => {
  const p = neutralParticipant('x')
  assert.equal(p.displayName, '参与者')
  assert.equal(p.kind, 'human', '绝不猜 agent（与 AuthorResolver 的约定一致）')
})

test('roomPreview: input room object is never mutated', () => {  const r = room({ id: 'room-1', participantIds: ['human-1', 'agent-1'], lastMessagePreview: '', lastActivityAt: '' })
  const before = JSON.stringify(r)
  const derived = roomPreview(
    r,
    [msg({ id: 'm9', authorId: 'agent-1', body: 'hello host', day: '今天', sentAt: '09:30' })],
    () => 'Agent',
  )
  assert.equal(JSON.stringify(r), before, 'input room unchanged after derivation')
  assert.notEqual(derived, r)
  assert.equal(derived.id, r.id)
  assert.equal(r.lastMessagePreview, '', 'static room preview still empty after derivation')
})

// ── roomPreviews（store.refreshRooms 生产调用的纯投影）──
// 根因回归：真实 Host Agent 的 UUID 不在 fixtures 时，旧实现 getParticipant
// 抛 unknown participant，整个 refresh 被 catch 丢弃，Core 权威 unreadCount
// 无法落进 store，read-cursor 因 unreadCount>0 永不成立而 0 次 PUT。

test('roomPreviews: real unknown author UUID never blocks authoritative unreadCount projection (P1 root cause)', () => {
  const me = { participant: { id: 'human-host', kind: 'human', display_name: 'Host User', status: 'active' } }
  const realAgentId = '9f8d2c1e-aaaa-bbbb-cccc-000000000001'
  const r = room({
    id: 'r-agent-1',
    name: '真实 Agent 会话',
    participantIds: ['human-host', realAgentId],
    participants: [
      { id: 'human-host', kind: 'human', displayName: 'Host User', initials: 'H' },
      { id: realAgentId, kind: 'agent', displayName: 'Real Agent', initials: 'R' },
    ],
    peer: { id: realAgentId, kind: 'agent', displayName: 'Real Agent', initials: 'R' },
    unreadCount: 3,
  })
  // 模拟另一真实 Agent 的回复：UUID 不在 fixtures、也不在当前房间身份里。
  const unknownUuid = '6e4c7f9b-1111-2222-3333-444455556666'
  const projected = roomPreviews(
    [r, room({ id: 'r-empty' })],
    (id) =>
      id === 'r-agent-1'
        ? [
            msg({ id: 'm-before', authorId: 'human-host', body: '旧消息', day: '昨天', sentAt: '10:00' }),
            msg({ id: 'm-agent', authorId: unknownUuid, body: '任务已完成，已更新契约', day: '今天', sentAt: '14:05' }),
          ]
        : [],
    me,
  )
  assert.equal(projected.length, 2, '未知作者绝不使整个 rooms 投影失败')
  const agentRoom = projected.find((x) => x.id === 'r-agent-1')!
  assert.equal(agentRoom.unreadCount, 3, '权威 unreadCount 必须随投影保留并被应用')
  assert.equal(agentRoom.lastMessagePreview, '任务已完成，已更新契约', '未知作者省略前缀（fail-soft，不 throw）')
  assert.equal(agentRoom.lastActivityAt, '14:05')
  assert.deepEqual(agentRoom.participants, r.participants, '预览不得污染身份/参与者')
  assert.equal(agentRoom.peer?.id, realAgentId)
  assert.ok(!agentRoom.lastMessagePreview.includes('Real Agent'))
  assert.ok(!agentRoom.lastMessagePreview.includes('undefined'))
})

test('roomPreviews: known real participant/me still get correct author prefix (no core/mock preview regression)', () => {
  const me = { participant: { id: 'human-host', kind: 'human', display_name: 'Host User', status: 'active' } }
  const agentId = 'a-courier'
  const rGroup = room({
    id: 'r-group',
    participantIds: ['human-host', 'p-aria', agentId],
    participants: [
      { id: 'human-host', kind: 'human', displayName: 'Host User', initials: 'H' },
      { id: 'p-aria', kind: 'human', displayName: 'Aria', initials: 'A' },
      { id: agentId, kind: 'agent', displayName: 'Courier', initials: 'C' },
    ],
    unreadCount: 0,
  })
  const projected = roomPreviews(
    [rGroup],
    (id) =>
      id === 'r-group'
        ? [
            msg({ id: 'm-1', authorId: 'p-aria', body: '首页视觉定稿了吗？' }),
            msg({ id: 'm-2', authorId: agentId, body: '定稿了，我整理了备注。', day: '今天', sentAt: '11:20' }),
          ]
        : [],
    me,
  )
  assert.equal(projected[0].lastMessagePreview, 'Courier：定稿了，我整理了备注。', '可见的真实 participant 仍正确加前缀')
  assert.equal(projected[0].lastActivityAt, '11:20')
  const projectedMe = roomPreviews(
    [rGroup],
    (id) => (id === 'r-group' ? [msg({ id: 'm-3', authorId: 'human-host', body: '收到，谢谢', day: '今天', sentAt: '11:25' })] : []),
    me,
  )
  assert.equal(projectedMe[0].lastMessagePreview, 'Host User：收到，谢谢')
})

test('roomPeerNameOf: only real room projection identities, never fixtures', () => {
  const direct = room({
    id: 'r-1',
    peer: { id: 'agent-uuid-1', kind: 'agent', displayName: '真实助手', initials: '真' },
  })
  assert.equal(roomPeerNameOf(direct), '真实助手', 'Direct 房间优先 peer 实名')
  const group = room({
    id: 'r-g',
    peer: undefined,
    participants: [
      { id: 'human-1', kind: 'human', displayName: 'Aria', initials: 'A' },
      { id: 'agent-uuid-2', kind: 'agent', displayName: 'Courier', initials: 'C' },
    ],
  })
  assert.equal(roomPeerNameOf(group), 'Courier', '群房从 participants 的真实 agent 取实名')
  assert.equal(roomPeerNameOf(room({ id: 'r-empty' })), undefined, '无投影身份 → undefined（泛化文案）')
  const groupNoAgent = room({
    id: 'r-g2',
    peer: undefined,
    participants: [{ id: 'human-1', kind: 'human', displayName: 'Aria', initials: 'A' }],
  })
  assert.equal(roomPeerNameOf(groupNoAgent), undefined, '无 agent/peer → undefined')
})
