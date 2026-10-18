// **语音消息起录音那一格**的判据用例（2026-10-09 立）。
//
// 为什么值得钉：这一格出过一次**静默失败**——壳里那份 WebView2 的权限处理器原先只放行剪贴板读
// ⇒ `getUserMedia` **永远不答**（真机实测 10 秒无应答；只改浏览器参数就 granted ⇒ 堵点在权限）。
// 于是用户按下麦克风之后**界面什么都不发生**：没有录音态、没有错误、没有出口。
//
// 三条判据：
//   ① **有界**：等不到就按 `no_answer` 收，绝不无限等；
//   ② **分得开**：被拒 / 没设备 / 不答 / 认不出，各自一个稳定分类（不压成一句"不可用"）；
//   ③ **迟到的流必须放掉**：用户在我们不等了之后才点"允许"时，麦克风不许没人录还开着。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

import {
  classifyVoiceStartFailure,
  requestMicrophone,
  VOICE_START_FAILURE_TEXT,
  VOICE_PERMISSION_TIMEOUT_MS,
  type VoiceStartFailure,
} from '../src/lib/voice.ts'

/** 一个假 `navigator.mediaDevices.getUserMedia`；返回一个"被放掉的音轨"计数器。 */
function stubMicrophone(behaviour: 'grant' | 'never' | 'late' | 'reject', rejectName = 'NotAllowedError') {
  const stopped: string[] = []
  const stream = {
    getTracks: () => [
      {
        stop: () => {
          stopped.push('audio')
        },
      },
    ],
  }
  const getUserMedia = () => {
    if (behaviour === 'grant') return Promise.resolve(stream as unknown as MediaStream)
    if (behaviour === 'never') return new Promise<MediaStream>(() => {})
    if (behaviour === 'late') {
      return new Promise<MediaStream>((resolve) => setTimeout(() => resolve(stream as unknown as MediaStream), 60))
    }
    const error = new Error('不给')
    error.name = rejectName
    return Promise.reject(error)
  }
  const had = 'navigator' in globalThis
  const before = (globalThis as Record<string, unknown>).navigator
  Object.defineProperty(globalThis, 'navigator', {
    value: { mediaDevices: { getUserMedia } },
    configurable: true,
    writable: true,
  })
  return {
    stopped,
    restore() {
      if (had) {
        Object.defineProperty(globalThis, 'navigator', { value: before, configurable: true, writable: true })
      } else {
        delete (globalThis as Record<string, unknown>).navigator
      }
    },
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

test('★ 分类：被拒 / 没设备 / 认不出 —— 各自一格，不压成同一句', () => {
  const named = (name: string) => {
    const error = new Error('x')
    error.name = name
    return error
  }
  assert.equal(classifyVoiceStartFailure(named('NotAllowedError')), 'denied')
  assert.equal(classifyVoiceStartFailure(named('SecurityError')), 'denied')
  assert.equal(classifyVoiceStartFailure(named('NotFoundError')), 'no_device')
  assert.equal(classifyVoiceStartFailure(named('OverconstrainedError')), 'no_device')
  assert.equal(classifyVoiceStartFailure(named('WeirdError')), 'failed')
  assert.equal(classifyVoiceStartFailure(undefined), 'failed', '认不出的一律 failed（不编原因）')
  assert.equal(classifyVoiceStartFailure({}), 'failed')
})

test('★ 人话表：封闭表的**每一格**都有一句（逐格都有出口，不是空括号）', () => {
  const keys: VoiceStartFailure[] = ['unsupported', 'no_answer', 'denied', 'no_device', 'failed']
  assert.deepEqual(Object.keys(VOICE_START_FAILURE_TEXT).sort(), [...keys].sort())
  for (const key of keys) {
    const text = VOICE_START_FAILURE_TEXT[key]
    assert.ok(text.length > 0, `${key} 没有人话`)
    assert.ok(!text.includes('（）'), `${key} 那句是空括号`)
  }
  // `no_answer` 那一句必须说得出**出口**（壳里那条路：更新一次应用；当下可以先打字）。
  assert.match(VOICE_START_FAILURE_TEXT.no_answer, /更新|先打字/)
  assert.ok(VOICE_PERMISSION_TIMEOUT_MS >= 3000 && VOICE_PERMISSION_TIMEOUT_MS <= 15000, '界要在人能等的量级上')
})

test('要得到麦克风 ⇒ 原样交回那条流（正常路径不是空壳）', async () => {
  const mic = stubMicrophone('grant')
  try {
    const stream = await requestMicrophone(50)
    assert.equal(stream.getTracks().length, 1)
    assert.deepEqual(mic.stopped, [], '拿到的那条流不该被放掉')
  } finally {
    mic.restore()
  }
})

test('★ 永远不答 ⇒ 按 `no_answer` 收（**有界**，不是无限等）', async () => {
  const mic = stubMicrophone('never')
  try {
    const started = Date.now()
    await assert.rejects(
      () => requestMicrophone(30),
      (error: unknown) => classifyVoiceStartFailure(error) === 'no_answer',
    )
    assert.ok(Date.now() - started < 2000, '该在界内就收，而不是等下去')
  } finally {
    mic.restore()
  }
})

test('★ 迟到的流必须放掉（用户在我们不等了之后才点"允许"）', async () => {
  const mic = stubMicrophone('late')
  try {
    await assert.rejects(() => requestMicrophone(20))
    await sleep(120) // 等那条"迟到"的流回来
    assert.deepEqual(mic.stopped, ['audio'], '迟到的流必须立刻放掉 —— 麦克风不许没人录还开着')
  } finally {
    mic.restore()
  }
})

test('被拒 ⇒ `denied`（浏览器/系统权限那条路）', async () => {
  const mic = stubMicrophone('reject', 'NotAllowedError')
  try {
    await assert.rejects(
      () => requestMicrophone(50),
      (error: unknown) => classifyVoiceStartFailure(error) === 'denied',
    )
  } finally {
    mic.restore()
  }
})

test('★ 接线：起录音**只走有界那条路**（`getUserMedia` 全文件只有一处调用点）', () => {
  // 为什么要有这一条（2026-10-09 证伪时发现）：上面那些用例测的是 `requestMicrophone` 本身，
  // 而**把它摘掉、退回无界 `await`** 时它们照样全绿 ⇒ 判据没钉住接线。这条钉住它。
  const raw = readFileSync(new URL('../src/lib/voice.ts', import.meta.url), 'utf8')
  const source = raw.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
  const calls = [...source.matchAll(/navigator\.mediaDevices\.getUserMedia\(/g)]
  assert.equal(
    calls.length,
    1,
    `getUserMedia 有 ${calls.length} 处调用点 —— 只该在 requestMicrophone 里那一处（有界那条路）`,
  )
  assert.match(source, /stream = await requestMicrophone\(\)/, 'createVoiceRecorder 的 start() 该走有界那条路')
})

test('★ 接线（呈现面）：起录音失败必须**说话**，不许空 catch、也不许退回那一句通用话', () => {
  // 这一格的病灶是"按下去什么都不发生"——所以判据不只是"有界"，还有"**失败要说出来**"。
  // ⚠ 从前那两句通用话（`麦克风不可用或未授权` / `当前浏览器不支持录音`）把三种处境压成一句，
  //   而壳里那条路连它们都到不了（`getUserMedia` 永远不答）⇒ 这里把"退回旧形状"一并堵上。
  const raw = readFileSync(new URL('../src/components/celira/MessageInput.tsx', import.meta.url), 'utf8')
  const source = raw.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
  assert.match(
    source,
    /setInputError\(VOICE_START_FAILURE_TEXT\[classifyVoiceStartFailure\(error\)\]\)/,
    '起录音的 catch 分支该把**分类翻成人话**说出来（不许空 catch）',
  )
  for (const old of ['麦克风不可用或未授权', '当前浏览器不支持录音']) {
    assert.ok(!source.includes(old), `旧那一句通用话又回来了：${old}（它把几种处境压成一句）`)
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})
