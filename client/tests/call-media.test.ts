// **通话媒体面（WebRTC P2P）的判据**（2026-10-09 接线时立）。
//
// 这一格最容易出的错是**"接听了但一直连不通、两边都没有错误可看"**，所以用例盯的是三件事：
// ① **方向**：caller 只认 answer/ice、callee 只认 offer/ice（角色不对的一律忽略）；
// ② **缓冲**：callee 在接听前收到的 offer/ICE 必须**留着**，接听时按序应用（丢了就是连不通）；
// ③ **服务端边界**：SDP/ICE 按 `1..=16384` / `≤32 条` / 每条 `≤2048` bytes 分批与筛除，
//    越界宁可少发也不发一个会被 400 拒掉的包。
//
// ⚠ 用假 peer connection / 假麦克风（真 RTC 在 node 里没有）；**真机的媒体连通性**
// 由真机探针那一刀负责（本文件不假装验过 P2P 打洞）。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

import {
  byteLength,
  createCallMedia,
  ICE_FLUSH_DELAY_MS,
  isSendableSdp,
  MAX_CANDIDATE_BYTES,
  MAX_ICE_PER_SIGNAL,
  MAX_SDP_BYTES,
  planIceBatches,
  type CallSignalKind,
  type CallSignalPayload,
  type PeerConnectionLike,
} from '../src/lib/callMedia.ts'

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function fakePeer(options: { sdpForOffer?: string; sdpForAnswer?: string; state?: string } = {}) {
  const log: string[] = []
  const peer: PeerConnectionLike = {
    addTrack: () => log.push('addTrack'),
    createOffer: async () => {
      log.push('createOffer')
      return { sdp: options.sdpForOffer ?? 'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n' }
    },
    createAnswer: async () => {
      log.push('createAnswer')
      return { sdp: options.sdpForAnswer ?? 'v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\n' }
    },
    setLocalDescription: async (d) => log.push(`setLocal:${d.type}`),
    setRemoteDescription: async (d) => log.push(`setRemote:${d.type}`),
    addIceCandidate: async (c) => log.push(`ice:${c.candidate}`),
    close: () => log.push('close'),
    onicecandidate: null,
    onconnectionstatechange: null,
    connectionState: options.state ?? 'new',
  }
  return { peer, log }
}

function fakeMic() {
  const stopped: string[] = []
  return {
    stream: {
      getTracks: () => [
        {
          stop: () => {
            stopped.push('audio')
          },
        },
      ],
    },
    stopped,
  }
}

/** 收集 `send` 出来的信令。 */
function collector() {
  const sent: { kind: CallSignalKind; payload: CallSignalPayload }[] = []
  return { sent, send: (kind: CallSignalKind, payload: CallSignalPayload) => sent.push({ kind, payload }) }
}

test('caller：`start()` ⇒ 采集 + 建连 + 发 offer；`close()` 停轨道、关连接', async () => {
  const { peer, log } = fakePeer()
  const mic = fakeMic()
  const { sent, send } = collector()
  const media = createCallMedia({
    role: 'caller',
    send,
    deps: { requestMicrophone: async () => mic.stream, createPeerConnection: () => peer },
  })
  await media.start()
  assert.deepEqual(
    sent.map((s) => s.kind),
    ['offer'],
    '发起时只该发一条 offer',
  )
  assert.ok(sent[0]?.payload.sdp?.startsWith('v=0'))
  assert.deepEqual(log, ['addTrack', 'createOffer', 'setLocal:offer'])
  assert.equal(media.state, 'connecting')
  media.close()
  assert.deepEqual(mic.stopped, ['audio'], '关掉通话必须停麦克风轨道')
  assert.ok(log.includes('close'))
  assert.equal(media.state, 'closed')
})

test('★ 方向：caller 认 answer/ice，**忽略** offer', async () => {
  const { peer, log } = fakePeer()
  const media = createCallMedia({
    role: 'caller',
    send: () => {},
    deps: { requestMicrophone: async () => fakeMic().stream, createPeerConnection: () => peer },
  })
  await media.start()
  await media.applySignal('offer', { sdp: 'v=0\r\no=- 9 9 IN IP4 10.0.0.1\r\n' })
  assert.ok(!log.includes('setRemote:offer'), 'caller 收到 offer 必须忽略（方向不对）')
  await media.applySignal('answer', { sdp: 'v=0\r\no=- 2 2 IN IP4 10.0.0.2\r\n' })
  assert.ok(log.includes('setRemote:answer'))
  await media.applySignal('ice', { candidates: [{ candidate: 'candidate:1 1 udp 1 10.0.0.2 5 typ host', sdp_mid: 'audio', sdp_mline_index: 0 }] })
  assert.ok(log.some((line) => line.startsWith('ice:')))
})

test('★ 缓冲：callee 接听前收到的 offer 留着，`accept()` 时按序应用并回 answer', async () => {
  const { peer, log } = fakePeer()
  const mic = fakeMic()
  const { sent, send } = collector()
  let created = 0
  const media = createCallMedia({
    role: 'callee',
    send,
    deps: {
      requestMicrophone: async () => mic.stream,
      createPeerConnection: () => {
        created += 1
        return peer
      },
    },
  })
  // 振铃期间：offer 与一条 ICE 先到（用户还没接）。
  await media.applySignal('offer', { sdp: 'v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\n' })
  await media.applySignal('ice', {
    candidates: [{ candidate: 'candidate:1 1 udp 1 10.0.0.1 9 typ host', sdp_mid: 'audio', sdp_mline_index: 0 }],
  })
  assert.equal(created, 0, '没接听之前**不许**建连（更不许开麦克风）')
  assert.deepEqual(sent, [], '没接听之前不该发任何信令')
  await media.accept()
  assert.equal(created, 1)
  assert.deepEqual(log, ['addTrack', 'setRemote:offer', 'createAnswer', 'setLocal:answer', 'ice:candidate:1 1 udp 1 10.0.0.1 9 typ host'])
  assert.deepEqual(
    sent.map((s) => s.kind),
    ['answer'],
    '接听这一下回一条 answer',
  )
})

test('★ 方向：callee **忽略** answer（那是 caller 才收的）', async () => {
  const { peer, log } = fakePeer()
  const media = createCallMedia({
    role: 'callee',
    send: () => {},
    deps: { requestMicrophone: async () => fakeMic().stream, createPeerConnection: () => peer },
  })
  await media.accept()
  await media.applySignal('answer', { sdp: 'v=0\r\no=- 2 2 IN IP4 10.0.0.2\r\n' })
  assert.ok(!log.includes('setRemote:answer'))
})

test('★ 服务端边界：ICE 按 ≤32 条分批，越界/含 NUL/空的**筛掉并计数**', () => {
  const many = Array.from({ length: 70 }, (_, index) => ({
    candidate: `candidate:${index} 1 udp 1 10.0.0.1 ${index} typ host`,
    sdp_mid: 'audio',
    sdp_mline_index: 0,
  }))
  const { batches, dropped } = planIceBatches(many)
  assert.deepEqual(
    batches.map((b) => b.length),
    [32, 32, 6],
  )
  assert.equal(dropped, 0)

  const bad = [
    { candidate: '', sdp_mid: null, sdp_mline_index: null },
    { candidate: 'a\u0000b', sdp_mid: null, sdp_mline_index: null },
    { candidate: 'x'.repeat(MAX_CANDIDATE_BYTES + 1), sdp_mid: null, sdp_mline_index: null },
    { candidate: 'x'.repeat(MAX_CANDIDATE_BYTES), sdp_mid: null, sdp_mline_index: null },
  ]
  const filtered = planIceBatches(bad)
  assert.equal(filtered.dropped, 3, '空 / 含 NUL / 超 2048 bytes 各算一条被丢')
  assert.equal(filtered.batches.flat().length, 1, '刚好 2048 bytes 的那条要留下')
})

test('★ 服务端边界：SDP 越界 ⇒ **不发**，报 `sdp_out_of_bounds`（fail closed）', async () => {
  assert.equal(isSendableSdp('v=0'), true)
  assert.equal(isSendableSdp('x'.repeat(MAX_SDP_BYTES)), true, '刚好 16384 bytes 合法')
  assert.equal(isSendableSdp('x'.repeat(MAX_SDP_BYTES + 1)), false)
  assert.equal(isSendableSdp('a\u0000b'), false, '禁 U+0000')
  assert.equal(isSendableSdp(''), false)
  assert.equal(isSendableSdp(null), false)
  assert.equal(byteLength('小满'), 6, '按 UTF-8 字节数判，不按字符数')

  const { peer } = fakePeer({ sdpForOffer: 'x'.repeat(MAX_SDP_BYTES + 1) })
  const { sent, send } = collector()
  const failures: string[] = []
  const media = createCallMedia({
    role: 'caller',
    send,
    onFailure: (failure) => failures.push(failure),
    deps: { requestMicrophone: async () => fakeMic().stream, createPeerConnection: () => peer },
  })
  await media.start()
  assert.deepEqual(sent, [], '越界的 SDP 一条都不许发出去（发了就是 400）')
  assert.deepEqual(failures, ['sdp_out_of_bounds'])
  assert.equal(media.state, 'failed')
})

test('★ 采集失败：如实转交，**不自己解释**原因（分类归 `voice.ts`）', async () => {
  const failures: string[] = []
  const media = createCallMedia({
    role: 'caller',
    send: () => {},
    onFailure: (failure, detail) => failures.push(`${failure}:${detail ?? ''}`),
    deps: {
      requestMicrophone: async () => {
        const error = new Error('不给')
        error.name = 'NotAllowedError'
        throw error
      },
      createPeerConnection: () => fakePeer().peer,
    },
  })
  await assert.rejects(() => media.start())
  assert.deepEqual(failures, ['unsupported:NotAllowedError'])
  assert.equal(media.state, 'failed')
})

test('★ 顺序：对端的 ICE **早于** offer 到达 ⇒ 先排队，描述落地后一条不少地补上', async () => {
  // 2026-10-09 复查时改的：`addIceCandidate` 在 `setRemoteDescription` 之前会抛，而
  // "试一下、失败就算了"会**丢掉对方唯一的可达地址** ⇒ 这一通永远连不上、两端都没错误可看
  // （`connectionState` 只会停在 `connecting`）。ICE 与描述本来就没有先后保证。
  const { peer, log } = fakePeer()
  const media = createCallMedia({
    role: 'callee',
    send: () => {},
    deps: { requestMicrophone: async () => fakeMic().stream, createPeerConnection: () => peer },
  })
  // 先到两条 ICE（**没有** offer），再到 offer。
  await media.applySignal('ice', {
    candidates: [
      { candidate: 'candidate:1 1 udp 1 10.0.0.1 1 typ host', sdp_mid: 'audio', sdp_mline_index: 0 },
      { candidate: 'candidate:2 1 udp 1 10.0.0.1 2 typ host', sdp_mid: 'audio', sdp_mline_index: 0 },
    ],
  })
  await media.applySignal('offer', { sdp: 'v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\n' })
  await media.accept()
  assert.deepEqual(
    log.filter((line) => line.startsWith('ice:')),
    ['ice:candidate:1 1 udp 1 10.0.0.1 1 typ host', 'ice:candidate:2 1 udp 1 10.0.0.1 2 typ host'],
    '两条候选都要补上，且顺序照到达顺序',
  )
  assert.ok(
    log.indexOf('setRemote:offer') < log.findIndex((line) => line.startsWith('ice:')),
    '补候选必须发生在远端描述落地**之后**',
  )
})

test('★ 候选攒着不达 32 条也要发出去（去抖）—— 只按"满一批才发"的话一条都发不出去', async () => {
  // 2026-10-09 真机探针第二次跑红了同一格才查出来的：候选几乎总在 `setLocalDescription`
  // **之后**才攒出来，而 `start()` / `accept()` 里那次 flush 已经过去了 ⇒ 只等"满 32 条"
  // 等于**候选永远不发**（对端拿不到我们的地址，连通性全靠 SDP 里恰好带了候选的运气）。
  const { peer } = fakePeer()
  const { sent, send } = collector()
  const media = createCallMedia({
    role: 'caller',
    send,
    deps: { requestMicrophone: async () => fakeMic().stream, createPeerConnection: () => peer },
  })
  await media.start()
  assert.deepEqual(
    sent.map((each) => each.kind),
    ['offer'],
    '刚 start 时还没有候选',
  )
  // 服务端在这一刻"攒出"两条候选（真实浏览器就是这样：异步、且不达一批）。
  peer.onicecandidate?.({
    candidate: { candidate: 'candidate:1 1 udp 1 10.0.0.1 1 typ host', sdpMid: 'audio', sdpMLineIndex: 0 },
  })
  peer.onicecandidate?.({
    candidate: { candidate: 'candidate:2 1 udp 1 10.0.0.1 2 typ host', sdpMid: 'audio', sdpMLineIndex: 0 },
  })
  await sleep(ICE_FLUSH_DELAY_MS + 80)
  const iceSignals = sent.filter((each) => each.kind === 'ice')
  assert.equal(iceSignals.length, 1, '同一轮的两条该并成**一条**信令')
  assert.equal(iceSignals[0]?.payload.candidates?.length, 2, '两条都要在里面（一条都不能丢）')
  media.close()
})

test('★ 依赖方向：媒体面**不依赖**传输层（它只拿一个 `send` 回调）', () => {
  // 判据：`lib/callMedia.ts` 不许 import `api/` 里的任何东西（除类型）——
  // 媒体面不知道"世界"是什么，它只知道"有一条信令要发出去"。反过来 `api/core.ts`
  // 也不许 import 媒体面（传输层不该知道 WebRTC 存在）。
  const media = readFileSync(new URL('../src/lib/callMedia.ts', import.meta.url), 'utf8')
  const imports = [...media.matchAll(/^import[^\n]*from '([^']+)'/gm)].map((m) => m[1])
  assert.deepEqual(imports, ['./voice.ts'], `媒体面只该 import voice.ts（有界麦克风），实际：${imports.join(', ')}`)
  const core = readFileSync(new URL('../src/api/core.ts', import.meta.url), 'utf8')
  assert.ok(!core.includes('callMedia'), '传输层不许知道媒体面')
})

test('★ 边界镜像：客户端那三个常数与**已实现的服务端**对得上', () => {
  // 这一条钉的是"镜像没漂"（不是证明服务端行为）：`im/server/src/protocol/call.rs`
  // 的 `SignalRequest` 文档里写着 `1..=16384` / `1..=32` / `1..=2048`。改了那边没改这边
  // ⇒ 客户端会发出被 400 拒掉的包（`call_payload_invalid`），这条当场红。
  const rust = readFileSync(new URL('../../im/server/src/protocol/call.rs', import.meta.url), 'utf8')
  const doc = rust.slice(rust.indexOf('SignalRequest') - 400, rust.indexOf('pub struct SignalRequest'))
  for (const [label, value] of [
    ['SDP 上限', MAX_SDP_BYTES],
    ['每批 ICE 条数上限', MAX_ICE_PER_SIGNAL],
    ['单条 candidate 上限', MAX_CANDIDATE_BYTES],
  ] as const) {
    assert.ok(doc.includes(String(value)), `服务端那一侧没写着 ${label} = ${value} —— 镜像漂了`)
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})
