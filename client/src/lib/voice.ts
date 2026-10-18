//! 语音消息录制（MediaRecorder 封装）：纯本地采集，不依赖任何识别服务或
//! 外网。产出音频 Blob，作为 kind:'file' 附件走既有附件管道（≤16MiB）。
//! 已知取舍：Chrome/Firefox 录为 webm/opus，Safari 录为 mp4/aac，跨浏览器
//! 播放存在格式死角（同端录放无问题）。
//!
//! ## 起录音**不许无限等**（2026-10-09 真机实测）
//!
//! 壳里那份 WebView2 的权限处理器原先**只放行剪贴板读**（`wry-0.57.0/src/webview2/mod.rs`）
//! ⇒ 麦克风请求在壳里**永远不答**：实测 `getUserMedia({audio:true})` 10 秒无应答，
//! 而**只改浏览器参数**（自动放行 ＋ 假设备）立刻 `granted` ⇒ 管道没问题，堵点在权限那一格。
//! 没有界的时候，用户按下麦克风之后**界面什么都不发生**（静默失败，本仓最忌讳的形状）。
//! ⇒ 这里给 `getUserMedia` 一条**有界等待**，超时折成 `no_answer`（人话见下），
//! 并且**迟到的流必须放掉**（用户在我们不等了之后才点"允许"时，麦克风不许没人录还开着）。

export interface VoiceRecording {
  blob: Blob
  mime: string
  durationSeconds: number
}

export interface VoiceRecorder {
  readonly mime: string
  start(): Promise<void>
  /** 结束并交出录音；时长过短（<1s）返回 null。 */
  stop(): Promise<VoiceRecording | null>
  /** 丢弃录音并释放麦克风。 */
  cancel(): void
}

/** 起录音时**分得开的**几种处境（封闭表；每格一句人话在 `VOICE_START_FAILURE_TEXT`）。 */
export type VoiceStartFailure = 'unsupported' | 'no_answer' | 'denied' | 'no_device' | 'failed'

/**
 * 等麦克风的**界**（毫秒）。等不到就按 `no_answer` 收，绝不无限等。
 *
 * 8 秒是"人愿意等"的上限量级：真实授权框一般在 1–2 秒内就有结果，而壳里那条**不答**的路
 * （见文件头）必须有个头。它比"永远转圈"好，也比"立刻说不行"诚实（后者会把慢当成坏）。
 */
export const VOICE_PERMISSION_TIMEOUT_MS = 8000

/** 超时的**稳定标记**（不靠 message 文本判——那会随实现漂）。 */
const TIMEOUT_NAME = 'VoicePermissionTimeout'

/**
 * 把一次起录音的异常折成**稳定分类**（纯函数，可直测）。
 *
 * 判据是 `error.name`（DOM 规范里的稳定取值），不是 message 文本；认不出的一律 `failed`
 * —— **不许把"不知道"说成某一种具体原因**。
 */
export function classifyVoiceStartFailure(error: unknown): VoiceStartFailure {
  const name = typeof (error as { name?: unknown })?.name === 'string' ? (error as { name: string }).name : ''
  if (name === TIMEOUT_NAME) return 'no_answer'
  if (name === 'NotAllowedError' || name === 'SecurityError') return 'denied'
  if (name === 'NotFoundError' || name === 'DevicesNotFoundError' || name === 'OverconstrainedError') {
    return 'no_device'
  }
  return 'failed'
}

/**
 * 分类 → 一句人话（**封闭表**，逐格都有出口）。
 *
 * ⚠ `no_answer` 那一句说的是"这台机器上的应用还没放行麦克风"——那是**壳里那条路**的实情
 * （见文件头），它给用户的出口是"更新一次应用"，同时明说"现在可以先打字"。
 * ⚠ 认不出的那一格**不编原因**（"这一格没记下原因"），与仓库那条"原因没有被记录不许渲染成空括号"同规。
 */
export const VOICE_START_FAILURE_TEXT: Record<VoiceStartFailure, string> = {
  unsupported: '这里不支持录音 —— 可以先打字，或用「文件」发一段录音',
  no_answer: '麦克风没有应答 —— 这台机器上的应用还没放行麦克风（更新一次应用就好）；现在可以先打字',
  denied: '麦克风没有被允许 —— 在浏览器或系统的权限里放行之后，再点一次',
  no_device: '这台机器上没有找到麦克风',
  failed: '录音没能开始（这一格没记下原因）',
}

export function isVoiceRecordingSupported(): boolean {
  return (
    typeof navigator !== 'undefined' &&
    typeof navigator.mediaDevices?.getUserMedia === 'function' &&
    typeof MediaRecorder !== 'undefined'
  )
}

/**
 * **有界地**要一次麦克风。
 *
 * ⚠ 超时之后那条流不能不管：用户可能在我们不等了之后才点"允许" ⇒ 拿到就**立刻放掉**
 *   （否则麦克风灯亮着、页面上却什么都没在录 —— 比挂起更糟）。
 * `timeoutMs` 可注入：用例拿它把"不答"这一格压到毫秒级。
 */
export function requestMicrophone(timeoutMs: number = VOICE_PERMISSION_TIMEOUT_MS): Promise<MediaStream> {
  const request = navigator.mediaDevices.getUserMedia({ audio: true })
  let settled = false
  const release = (stream: MediaStream) => {
    for (const track of stream.getTracks()) track.stop()
  }
  return new Promise<MediaStream>((resolve, reject) => {
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      const timeout = new Error('麦克风请求没有应答')
      timeout.name = TIMEOUT_NAME
      reject(timeout)
    }, timeoutMs)
    request.then(
      (stream) => {
        // ⚠ **迟到的流**（用户在我们不等了之后才点"允许"）：拿到就放掉，且**只放这一处**
        //   —— 两条路径各放一次的话，同一批音轨会被 `stop()` 两遍（看着无害，其实是"两处判据"）。
        if (settled) {
          release(stream)
          return
        }
        settled = true
        clearTimeout(timer)
        resolve(stream)
      },
      (error) => {
        // 已经超时 ⇒ 什么都不用做（那时麦克风根本没开）。
        if (settled) return
        settled = true
        clearTimeout(timer)
        reject(error)
      },
    )
  })
}

function pickMime(): string {
  for (const mime of ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4', 'audio/ogg;codecs=opus']) {
    if (MediaRecorder.isTypeSupported(mime)) return mime
  }
  return ''
}

export function audioExtension(mime: string): string {
  if (mime.includes('mp4') || mime.includes('aac')) return 'm4a'
  if (mime.includes('ogg')) return 'ogg'
  return 'webm'
}

export function createVoiceRecorder(): VoiceRecorder | null {
  if (!isVoiceRecordingSupported()) return null
  const mime = pickMime()
  let stream: MediaStream | null = null
  let recorder: MediaRecorder | null = null
  let chunks: Blob[] = []
  let startedAt = 0
  let cancelled = false

  const release = () => {
    if (stream) for (const track of stream.getTracks()) track.stop()
    stream = null
    recorder = null
  }

  return {
    get mime() {
      return mime
    },
    async start() {
      // ⚠ **有界**（见文件头）：壳里那条路原先永远不答 —— 超时按 `no_answer` 收，
      //   由调用方翻成人话（`VOICE_START_FAILURE_TEXT`）。
      stream = await requestMicrophone()
      cancelled = false
      chunks = []
      recorder = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined)
      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) chunks.push(e.data)
      }
      startedAt = Date.now()
      recorder.start()
    },
    stop() {
      const active = recorder
      const elapsed = (Date.now() - startedAt) / 1000
      if (!active || active.state === 'inactive') {
        release()
        return Promise.resolve(null)
      }
      return new Promise((resolve) => {
        active.onstop = () => {
          const type = active.mimeType || mime || 'audio/webm'
          const blob = new Blob(chunks, { type })
          release()
          if (cancelled || elapsed < 1 || blob.size === 0) {
            resolve(null)
            return
          }
          resolve({ blob, mime: type, durationSeconds: Math.max(1, Math.round(elapsed)) })
        }
        active.stop()
      })
    },
    cancel() {
      cancelled = true
      if (recorder && recorder.state !== 'inactive') {
        recorder.onstop = null
        recorder.stop()
      }
      release()
    },
  }
}
