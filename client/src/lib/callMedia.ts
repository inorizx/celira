//! **通话的媒体面**：两端浏览器之间的 WebRTC P2P（`CORE_V1_CALL.md` §1/§6）。
//!
//! 三条边界写在最前面，因为它们决定了这个文件**不该**有什么：
//! 1. **Core 永不接触媒体字节** —— 这个文件里的音频流只在本地与对端之间；发给世界的
//!    只有 SDP/ICE 这种**控制文本**（`send` 那一格），一个音频字节都不进网络的那一侧；
//! 2. **不做媒体面之外的事**：没有 TURN/SFU/混音/录制/降噪（契约 §6 明确排除），
//!    也没有"通话记录/时长"（那是持久事实，本切片不产生任何持久事实）；
//! 3. **信令是纯控制消息、不排队**：Core 只投给对端**当前在线**的连接，对端不在就**静默丢弃**
//!    ⇒ "发出去了"不等于"对方收到了"（媒体面靠 RTC 重协商/重发兜底，这是契约给的口径）。
//!
//! ## 为什么 callee 一侧要**先建对象、后采集**
//!
//! caller 发起的同一刻就发 offer，而 callee 那会儿还在振铃（用户还没点接听）——
//! 于是 callee 这一侧必须**先能收信令**（缓冲），等用户真的接听了才去采集麦克风、
//! 建 peer connection、把缓冲的 offer/ICE 应用上去。反过来（接听后才建对象）会**丢掉 offer**，
//! 表现为"接听了但一直连不通"，而两边都没有错误可看。
//!
//! ## 服务端的边界（我们按它分批，不然就是 400）
//!
//! `sdp` `1..=16384` bytes 且**禁 U+0000**；`candidates` `1..=32` 条、每条 `1..=2048` bytes
//! （`im/server/src/protocol/call.rs` 的 `SignalRequest` ＋ 契约 §2.2）。越界 ⇒ 400
//! `call_payload_invalid` —— 所以这里**先分批/先筛**，宁可少发一条也不发一个会被拒的包。

import { requestMicrophone } from './voice.ts'

/** 一条 ICE candidate（与契约 `CallIceCandidate` 同形）。 */
export interface CallIceCandidate {
  candidate: string
  sdp_mid: string | null
  sdp_mline_index: number | null
}

/** 信令的三种 `kind`（协议判别值）。 */
export type CallSignalKind = 'offer' | 'answer' | 'ice'

/** 信令的载荷（与契约 `payload` 同形：offer/answer 是 `sdp`，ice 是 `candidates`）。 */
export interface CallSignalPayload {
  sdp?: string
  candidates?: CallIceCandidate[]
}

/** 媒体面的连接态（UI 只认这几格；`idle` 是"还没开始采集"）。 */
export type CallMediaState = 'idle' | 'connecting' | 'connected' | 'failed' | 'closed'

/** 媒体面自己的失败分类（**封闭表**；人话在 `lib/callText.ts`）。 */
export type CallMediaFailure = 'unsupported' | 'sdp_out_of_bounds' | 'peer_failed'

/** 服务端的边界（唯一权威在 `im/server/src/protocol/call.rs`；这里只是镜像，改那边要同步改这里）。 */
export const MAX_SDP_BYTES = 16384
export const MAX_ICE_PER_SIGNAL = 32
export const MAX_CANDIDATE_BYTES = 2048

/**
 * 攒多久就发一批 ICE（毫秒）。
 *
 * ⚠ **必须有这个去抖，而且不能只靠"攒够 32 条"**（2026-10-09 真机探针第二次跑红了才查出来）：
 * 候选几乎总在 `setLocalDescription` **之后**才由 `onicecandidate` 攒出来，而那时 `start()` /
 * `accept()` 里那次 `flushIce()` 已经过去了 ⇒ 只按"满 32 条才发"的话，**候选一条都发不出去**
 * （一个 host 候选哪有可能攒到 32 条），对端拿不到我们的地址 ⇒ 连通性全靠"SDP 里恰好带了候选"
 * 的运气（第一次探针 9/9 就是这么过的，第二次同一格红了）。
 * 50ms 是"够把同一轮攒出来的几条并成一条信令、又不至于拖慢 trickle"的量级。
 */
export const ICE_FLUSH_DELAY_MS = 50

const encoder = new TextEncoder()

/** 一段文本的 UTF-8 字节数（契约按 bytes 判，不按字符数）。 */
export function byteLength(text: string): number {
  return encoder.encode(text).length
}

/**
 * **这批 ICE 该怎么发**（纯函数，可直测）：按 ≤32 条分批，并**先筛掉**发出去必被拒的
 * （空文本、超 2048 bytes、含 U+0000）。
 *
 * ⚠ 返回 `dropped` 是**故意的**：丢了几条要说得出数（静默丢会让"连不通"没有线索）。
 */
export function planIceBatches(candidates: CallIceCandidate[]): {
  batches: CallIceCandidate[][]
  dropped: number
} {
  const usable: CallIceCandidate[] = []
  let dropped = 0
  for (const candidate of candidates) {
    const text = candidate.candidate
    if (
      typeof text !== 'string' ||
      text.length === 0 ||
      text.includes('\u0000') ||
      byteLength(text) > MAX_CANDIDATE_BYTES
    ) {
      dropped += 1
      continue
    }
    usable.push(candidate)
  }
  const batches: CallIceCandidate[][] = []
  for (let at = 0; at < usable.length; at += MAX_ICE_PER_SIGNAL) {
    batches.push(usable.slice(at, at + MAX_ICE_PER_SIGNAL))
  }
  return { batches, dropped }
}

/** 一段 SDP 能不能发（1..=16384 bytes、禁 U+0000）。 */
export function isSendableSdp(sdp: string | null | undefined): sdp is string {
  if (typeof sdp !== 'string' || sdp.length === 0) return false
  if (sdp.includes('\u0000')) return false
  return byteLength(sdp) <= MAX_SDP_BYTES
}

/** 我们真正用到的那几格（窄接口：假实现要写的就这些，不多不少）。 */
export interface PeerConnectionLike {
  addTrack(track: unknown, stream: unknown): void
  createOffer(): Promise<{ sdp?: string | null }>
  createAnswer(): Promise<{ sdp?: string | null }>
  setLocalDescription(description: { type: 'offer' | 'answer'; sdp?: string }): Promise<void>
  setRemoteDescription(description: { type: 'offer' | 'answer'; sdp?: string }): Promise<void>
  addIceCandidate(candidate: { candidate: string; sdpMid: string | null; sdpMLineIndex: number | null }): Promise<void>
  close(): void
  onicecandidate: ((event: { candidate: unknown }) => void) | null
  onconnectionstatechange: (() => void) | null
  connectionState: string
}

export interface CallMediaDeps {
  /** 采集（默认走 `voice.ts` 的**有界**那条路 —— 壳里那条永远不答的路就是它堵住的）。 */
  requestMicrophone?: () => Promise<unknown>
  createPeerConnection?: () => PeerConnectionLike
}

export interface CallMedia {
  readonly role: 'caller' | 'callee'
  readonly state: CallMediaState
  /** caller：采集 + 建连 + 发 offer。 */
  start(): Promise<void>
  /** callee：应用缓冲的 offer + 采集 + 建连 + 发 answer。 */
  accept(): Promise<void>
  /** 收到对端信令（角色不对的**一律忽略** —— 服务端也强制方向，这里是第二道）。 */
  applySignal(kind: CallSignalKind, payload: CallSignalPayload): Promise<void>
  /** 关掉：停轨道、关连接、不再发任何信令。 */
  close(): void
}

export interface CallMediaOptions {
  role: 'caller' | 'callee'
  send: (kind: CallSignalKind, payload: CallSignalPayload) => void
  onState?: (state: CallMediaState) => void
  onFailure?: (failure: CallMediaFailure, detail?: string) => void
  deps?: CallMediaDeps
}

/**
 * 造一个媒体面对象。
 *
 * ⚠ **`applySignal` 在 `start()`/`accept()` 之前会缓冲**（见文件头那段）：callee 侧
 * 收到 offer 时通常还没接听。
 */
export function createCallMedia(options: CallMediaOptions): CallMedia {
  const { role, send, onState, onFailure } = options
  const deps = options.deps ?? {}
  const collect = deps.requestMicrophone ?? (() => requestMicrophone())
  const makePeer =
    deps.createPeerConnection ??
    (() => new RTCPeerConnection({ iceServers: [] }) as unknown as PeerConnectionLike)

  let state: CallMediaState = 'idle'
  let peer: PeerConnectionLike | null = null
  let stream: { getTracks(): { stop(): void }[] } | null = null
  let closed = false
  /** 还没建连时收到的信令（callee 侧正常会走到这里）。 */
  const pending: { kind: CallSignalKind; payload: CallSignalPayload }[] = []
  /** 攒着的 ICE（攒够一批或建连后 flush）。 */
  let iceBuffer: CallIceCandidate[] = []
  /**
   * **对端的 ICE 早于 offer/answer 到达时先排在这里**（trickle 的正常情形之一）。
   *
   * ⚠ 为什么不"试一下、失败就算了"（2026-10-09 写完刀①复查时改的）：`addIceCandidate` 在
   * `setRemoteDescription` 之前会抛 —— 而那条 candidate 是**对方唯一的可达地址**时，
   * 丢掉它 = 这一通永远连不上，且两端都没有错误可看（`onconnectionstatechange` 只会停在
   * `connecting`）。ICE 的候选与描述本来就没有先后保证 ⇒ 必须**排队、等描述落地再应用**。
   */
  let pendingRemoteIce: CallIceCandidate[] = []
  /** 远端描述落地了吗（`addIceCandidate` 的前置条件）。 */
  let remoteReady = false

  const setState = (next: CallMediaState) => {
    if (state === next) return
    state = next
    onState?.(next)
  }
  const fail = (failure: CallMediaFailure, detail?: string) => {
    setState('failed')
    onFailure?.(failure, detail)
  }

  const flushIce = () => {
    if (closed || iceBuffer.length === 0) return
    const { batches, dropped } = planIceBatches(iceBuffer)
    iceBuffer = []
    if (dropped > 0) onFailure?.('peer_failed', `丢掉了 ${dropped} 条越界的 ICE candidate`)
    for (const batch of batches) send('ice', { candidates: batch })
  }

  /** 去抖：同一轮攒出来的几条并成一条信令发出去（见 `ICE_FLUSH_DELAY_MS` 那段）。 */
  let flushTimer: ReturnType<typeof setTimeout> | null = null
  const scheduleFlush = () => {
    if (flushTimer !== null) return
    flushTimer = setTimeout(() => {
      flushTimer = null
      flushIce()
    }, ICE_FLUSH_DELAY_MS)
  }

  const attach = (pc: PeerConnectionLike) => {
    pc.onicecandidate = (event) => {
      const raw = event?.candidate as
        | { candidate?: string; sdpMid?: string | null; sdpMLineIndex?: number | null }
        | null
      if (!raw || typeof raw.candidate !== 'string') return
      iceBuffer.push({
        candidate: raw.candidate,
        sdp_mid: raw.sdpMid ?? null,
        sdp_mline_index: raw.sdpMLineIndex ?? null,
      })
      // 满一批就立刻发；否则**去抖发**（⚠ 不能只等"满 32 条" —— 见 `ICE_FLUSH_DELAY_MS` 那段）。
      if (iceBuffer.length >= MAX_ICE_PER_SIGNAL) flushIce()
      else scheduleFlush()
    }
    pc.onconnectionstatechange = () => {
      const next = pc.connectionState
      if (next === 'connected') setState('connected')
      else if (next === 'failed') fail('peer_failed', 'RTC 连接失败')
      else if (next === 'connecting' || next === 'new') setState('connecting')
    }
  }

  /** 建连 + 采集（caller 与 callee 共用；差别只在谁发 offer）。 */
  const connect = async () => {
    if (peer) return
    let media: unknown
    try {
      media = await collect()
    } catch (error) {
      // ⚠ 采集失败的**分类**由调用方翻（`voice.ts` 的 `classifyVoiceStartFailure`）——
      //   这里只如实转交：媒体面不重新解释"为什么拿不到麦克风"。
      fail('unsupported', error instanceof Error ? error.name : String(error))
      throw error
    }
    stream = media as { getTracks(): { stop(): void }[] }
    const pc = makePeer()
    peer = pc
    attach(pc)
    setState('connecting')
    for (const track of stream.getTracks()) pc.addTrack(track, stream)
  }

  const applyToPeer = async (kind: CallSignalKind, payload: CallSignalPayload) => {
    const pc = peer
    if (!pc) return
    if (kind === 'ice') {
      const candidates = payload.candidates ?? []
      // 远端描述还没落地 ⇒ 先排队（见 `pendingRemoteIce` 那段：丢了就是"永远连不上"）。
      if (!remoteReady) {
        pendingRemoteIce.push(...candidates)
        return
      }
      for (const candidate of candidates) {
        try {
          await pc.addIceCandidate({
            candidate: candidate.candidate,
            sdpMid: candidate.sdp_mid,
            sdpMLineIndex: candidate.sdp_mline_index,
          })
        } catch {
          // 描述已落地还失败的（对端换了代 / 这条候选不合法）才真的算了 —— 不因此判整通失败。
        }
      }
      return
    }
    const sdp = payload.sdp
    if (!isSendableSdp(sdp)) {
      fail('sdp_out_of_bounds', `收到 ${byteLength(String(sdp ?? ''))} bytes 的 ${kind}`)
      return
    }
    await pc.setRemoteDescription({ type: kind, sdp })
    remoteReady = true
    // 描述落地了 ⇒ 把先到的 ICE 补上（**顺序不影响结果**，但一条都不能少）。
    const queued = pendingRemoteIce.splice(0, pendingRemoteIce.length)
    for (const candidate of queued) {
      try {
        await pc.addIceCandidate({
          candidate: candidate.candidate,
          sdpMid: candidate.sdp_mid,
          sdpMLineIndex: candidate.sdp_mline_index,
        })
      } catch {
        /* 同上：单条不被接受不判整通失败 */
      }
    }
    if (kind === 'offer') {
      // 只有 callee 会走到这里（方向由下面 `applySignal` 先筛过）。
      const answer = await pc.createAnswer()
      if (!isSendableSdp(answer.sdp ?? undefined)) {
        fail('sdp_out_of_bounds', 'answer 越界')
        return
      }
      await pc.setLocalDescription({ type: 'answer', sdp: answer.sdp ?? undefined })
      send('answer', { sdp: answer.sdp ?? undefined })
    }
  }

  return {
    role,
    get state() {
      return state
    },
    async start() {
      if (closed) return
      if (role !== 'caller') throw new Error('只有 caller 能 start()；callee 走 accept()')
      await connect()
      const pc = peer as PeerConnectionLike
      const offer = await pc.createOffer()
      if (!isSendableSdp(offer.sdp ?? undefined)) {
        fail('sdp_out_of_bounds', `offer ${byteLength(String(offer.sdp ?? ''))} bytes`)
        return
      }
      await pc.setLocalDescription({ type: 'offer', sdp: offer.sdp ?? undefined })
      send('offer', { sdp: offer.sdp ?? undefined })
      flushIce()
    },
    async accept() {
      if (closed) return
      if (role !== 'callee') throw new Error('只有 callee 能 accept()；caller 走 start()')
      await connect()
      const buffered = pending.splice(0, pending.length)
      for (const each of buffered) await applyToPeer(each.kind, each.payload)
      flushIce()
    },
    async applySignal(kind, payload) {
      if (closed) return
      // **方向判据**（契约 §4.2）：caller 只收 answer/ice，callee 只收 offer/ice。
      // 角色不对的信令**忽略**（服务端已经强制过一次；这里是第二道，绝不"照单全收"）。
      const expected = role === 'caller' ? kind === 'answer' || kind === 'ice' : kind === 'offer' || kind === 'ice'
      if (!expected) return
      if (!peer) {
        // 还没建连（callee 振铃期间）⇒ 缓冲，接听时按顺序应用。
        pending.push({ kind, payload })
        return
      }
      await applyToPeer(kind, payload)
    },
    close() {
      closed = true
      pending.length = 0
      iceBuffer = []
      pendingRemoteIce = []
      remoteReady = false
      if (flushTimer !== null) {
        clearTimeout(flushTimer)
        flushTimer = null
      }
      if (stream) for (const track of stream.getTracks()) track.stop()
      stream = null
      try {
        peer?.close()
      } catch {
        // 关一个已经关掉的连接不是错误。
      }
      peer = null
      setState('closed')
    },
  }
}
