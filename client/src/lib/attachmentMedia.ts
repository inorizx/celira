import type { Attachment } from '@/api/domain'

/**
 * 附件在**显示层**的归类（唯一判据）。
 *
 * 为什么单独一个模块：`mime?.startsWith('audio/')` 这条判据原先在
 * `MessageBubble`（挑内联播放器）与 `ChatHistoryPage`（挑筛选页签）**各写了一遍**，
 * 再给视频加一遍就是三遍 —— 三处各判一次，迟早分叉（一处认了、一处不认）。
 * 判据一处、消费者两处。
 *
 * ⚠ 它与 Core 投影里的 `kind` **不是一回事**，别合并：
 *   - `kind`（`image` / `file`）来自 `content_type`，是**契约投影**的两分法；
 *   - `audio` / `video` 是显示层按 `content_type` 再分出来的**内联播放**类型 ——
 *     契约里没有它们，也不该有（「用哪个组件渲染」是客户端的事）。
 *
 * ⚠ 判据只认 `mime`（= 服务端的 `content_type`），**不认文件名**：文件名是用户给的
 * 自由文本，拿它猜类型就是把显示层建立在猜测上。判据按契约给的原样值比较（与既有
 * audio 判据同口径），**不做大小写归一** —— 归一是一条行为变更，要单独裁。
 */
export type AttachmentMedia = 'image' | 'audio' | 'video' | 'file'

export function attachmentMedia(attachment: Attachment): AttachmentMedia {
  // 契约投影说它是图片 ⇒ 就是图片（哪怕 mime 认不出：投影比前缀可信）。
  if (attachment.kind === 'image') return 'image'
  if (attachment.mime?.startsWith('audio/') || attachment.durationSeconds != null) return 'audio'
  if (attachment.mime?.startsWith('video/')) return 'video'
  return 'file'
}
