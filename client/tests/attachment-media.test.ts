// 附件**显示归类**（`src/lib/attachmentMedia.ts`）的行为测试。
//
// 运行：cd client && npm test
//
// 这一刀要钉住的判据一条：**视频不再落进「文件」**（它有自己的内联播放件），
// 而**「不知道是什么」仍然老实落文件**——不许猜。
// ★ 最要紧的是那几条**负向**：认不出时猜成图片/视频，等于让显示层替契约说话。

import assert from 'node:assert/strict'
import test from 'node:test'

import { attachmentMedia } from '../src/lib/attachmentMedia.ts'
import type { Attachment } from '../src/api/domain.ts'

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 11

function mediaCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

function att(fields: Partial<Attachment> & Pick<Attachment, 'kind'>): Attachment {
  return { id: 'a-1', name: 'file.bin', ...fields }
}

/** 这一刀之前的判据（`MessageBubble` / `ChatHistoryPage` 各写了一遍的那份）。 */
function legacyKind(a: Attachment): 'image' | 'audio' | 'file' {
  if (a.kind === 'image') return 'image'
  if (a.mime?.startsWith('audio/') || a.durationSeconds != null) return 'audio'
  return 'file'
}

// ---------------------------------------------------------------------------
// 认得出的四类
// ---------------------------------------------------------------------------

mediaCase('图片：契约投影说它是图片，就按图片算（哪怕 mime 认不出）', () => {
  // `kind` 来自服务端的 `content_type`，是**契约投影**；投影比前缀可信。
  assert.equal(attachmentMedia(att({ kind: 'image', mime: 'application/octet-stream' })), 'image')
  assert.equal(attachmentMedia(att({ kind: 'image', mime: 'image/svg+xml' })), 'image')
  assert.equal(attachmentMedia(att({ kind: 'image', mime: undefined })), 'image')
})

mediaCase('语音：audio/* 三种常见编码都算', () => {
  for (const mime of ['audio/webm', 'audio/mp4', 'audio/ogg']) {
    assert.equal(attachmentMedia(att({ kind: 'file', mime })), 'audio', mime)
  }
})

mediaCase('语音：只有时长、没有 mime 的录音仍算语音（老判据保留）', () => {
  assert.equal(attachmentMedia(att({ kind: 'file', durationSeconds: 7 })), 'audio')
})

mediaCase('★ 视频：video/* 三种常见编码都算（这一刀新增的那一类）', () => {
  for (const mime of ['video/mp4', 'video/webm', 'video/quicktime']) {
    assert.equal(attachmentMedia(att({ kind: 'file', mime })), 'video', mime)
  }
})

// ---------------------------------------------------------------------------
// ★ 负向：不知道就是不知道，且顺序是判据的一部分
// ---------------------------------------------------------------------------

mediaCase('★ 负对照：旧判据在同一输入下把视频说成了「文件」', () => {
  const video = att({ kind: 'file', mime: 'video/mp4' })
  assert.equal(legacyKind(video), 'file') // 旧行为：视频只能下载
  assert.equal(attachmentMedia(video), 'video') // 新判据：内联播放
})

mediaCase('★ 认不出 ⇒ file，绝不猜成图片/语音/视频', () => {
  assert.equal(attachmentMedia(att({ kind: 'file', mime: undefined })), 'file')
  assert.equal(attachmentMedia(att({ kind: 'file', mime: 'application/octet-stream' })), 'file')
  assert.equal(attachmentMedia(att({ kind: 'file', mime: 'application/pdf' })), 'file')
})

mediaCase('★ 判据只认 content_type，不认文件名', () => {
  // 文件名是用户给的自由文本：拿它猜类型就是把显示层建立在猜测上。
  assert.equal(attachmentMedia(att({ kind: 'file', name: 'video.mp4', mime: 'text/plain' })), 'file')
  assert.equal(
    attachmentMedia(att({ kind: 'file', name: 'notes.txt', mime: 'video/mp4' })),
    'video',
  )
})

mediaCase('★ 大小写**不**归一（与既有 audio 判据同口径；要改得单独裁）', () => {
  // 这一条不是在说"这样更好"，是在**钉住现状**：归一会同时改掉语音那条老行为。
  assert.equal(attachmentMedia(att({ kind: 'file', mime: 'Video/MP4' })), 'file')
  assert.equal(attachmentMedia(att({ kind: 'file', mime: 'Audio/WEBM' })), 'file')
})

mediaCase('顺序是判据的一部分：时长那一格先于视频前缀', () => {
  // 现实里出不来这个组合（时长只由本机录音写），钉住是为了让顺序改动必须是有意的。
  assert.equal(
    attachmentMedia(att({ kind: 'file', mime: 'video/mp4', durationSeconds: 3 })),
    'audio',
  )
})

mediaCase('一条消息里四种附件各自归类，互不吞并', () => {
  const list = [
    att({ id: '1', kind: 'image', mime: 'image/png' }),
    att({ id: '2', kind: 'file', mime: 'video/mp4' }),
    att({ id: '3', kind: 'file', mime: 'audio/webm' }),
    att({ id: '4', kind: 'file', mime: 'application/zip' }),
  ]
  assert.deepEqual(list.map(attachmentMedia), ['image', 'video', 'audio', 'file'])
})

// ---------------------------------------------------------------------------
// 防空跑
// ---------------------------------------------------------------------------

mediaCase('门禁不许空跑：本文件真的执行了用例', () => {
  // 减一：本条自己也是用例。
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
