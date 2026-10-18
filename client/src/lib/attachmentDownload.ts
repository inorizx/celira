import type { Attachment } from '@/api/domain'
import { getApi } from '@/api/store'
import { apiErrorText, errorText } from '@/api/contractErrors'
import { isTauriRuntime } from '@/api/shell'
import { type DownloadOutcome, directoryNameOf } from '@/lib/downloadOutcome'
import { nextFileName } from '@/lib/downloadName'

export type { DownloadOutcome } from '@/lib/downloadOutcome'
export { directoryNameOf, downloadOutcomeText } from '@/lib/downloadOutcome'

export function canDownloadAttachment(attachment: Attachment): boolean {
  if (attachment.localUrl || attachment.url?.startsWith('/mock/')) return true
  const api = getApi()
  // ⚠ 原判据 `api?.mode === 'core' && …` —— `ConnectionMode` 只剩一个取值 ⇒ 前半恒真，
  // 它实际起的作用是**空值守卫**。改写成它真正在说的那句话（2026-09-26，审计 D18）。
  return Boolean(api && attachment.url && api.fetchAttachmentBlob)
}

/**
 * 下载一个附件（落盘）。
 *
 * **返回值是这一刀的收场**（TASK_72 立的形状，2026-10-08 结构化）：
 *
 * - 从前返回 `string | null`（`null` = 成功）—— 那在**浏览器里**是对的（浏览器有自己的下载
 *   UI），在**壳里**是一句假话：壳里原先没有任何人接下载，点了**一点不落盘**，而这个函数
 *   照样回 `null`（"成功"）。今天壳里由页面把字节交给 fs 插件写盘（见 `saveInShell`），
 *   于是这里多出一件事：**把"到底存没存下来、存哪了"如实交给界面**。
 * - 判别式（`{ok:true,readout}` / `{ok:false,text}`）的判据与两档的差别写在
 *   `lib/downloadOutcome.ts` 的文件头。
 */
export async function downloadAttachment(
  roomId: string,
  attachment: Attachment,
): Promise<DownloadOutcome> {
  const direct =
    attachment.localUrl ?? (attachment.url?.startsWith('/mock/') ? attachment.url : null)
  if (direct) {
    triggerDownload(direct, attachment.name)
    // 演示/本机源：字节不经过我们，落盘由浏览器或壳自己管 ⇒ 页面不宣布结果。
    return { ok: true, readout: null }
  }
  const api = getApi()
  if (!(api?.mode === 'core' && attachment.url && api.fetchAttachmentBlob)) {
    // 没有任何可用的下载路径（例：这条连接不提供附件字节）—— 也说一句，不假装成功。
    return { ok: false, text: apiErrorText('attachment_download_failed') }
  }
  try {
    const blob = await api.fetchAttachmentBlob(roomId, attachment.id)
    // 壳里：**字节交给壳写盘**（落点由 capability 只放开下载目录）。
    if (isTauriRuntime()) {
      return await saveInShell(attachment.name, new Uint8Array(await blob.arrayBuffer()))
    }
    // 浏览器：老路（浏览器自己给下载条 ⇒ 页面不宣布结果，用户可能当场取消了）。
    const url = URL.createObjectURL(blob)
    triggerDownload(url, attachment.name)
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
    return { ok: true, readout: null }
  } catch (error) {
    // 稳定分类 → 人话（`fetchBlob` 抛的是 `attachment_download_<status>` 那一族，
    // 由 `apiErrorText` 折到"附件下载失败"；认不出的分类落到通用文案）。
    return { ok: false, text: errorText(error) }
  }
}

/**
 * **壳里落盘**（2026-10-08 建）。
 *
 * 为什么是"页面写盘"而不是"壳接下载"：Tauri 的下载钩子（`on_download`）在**本壳里一次都
 * 不触发** —— 真机插桩证据（钩子装上了、点击到达了锚点、带用户激活也不触发）记在
 * `app/README.md` §3.1 ⇒ 改走 **fs 插件**。这条路的两条好处：
 *
 * 1. **落点由 capability 的作用域强制**（`$DOWNLOAD/**`，见 `capabilities/default.json`）：
 *    页面算出来的名字再离谱也出不了那个圈 —— 比"页面自觉"硬；
 * 2. **`writeFile` 的 promise 就是回执** ⇒ 不需要事件、监听与等待者，成功就说得出落在哪。
 */
async function saveInShell(name: string, bytes: Uint8Array): Promise<DownloadOutcome> {
  try {
    const [fs, path] = await Promise.all([
      import('@tauri-apps/plugin-fs'),
      import('@tauri-apps/api/path'),
    ])
    const dir = await path.downloadDir()
    const fileName = await nextFileName(name, async (candidate) =>
      fs.exists(await path.join(dir, candidate)),
    )
    const target = await path.join(dir, fileName)
    await fs.writeFile(target, bytes)
    const where = directoryNameOf(target)
    return { ok: true, readout: where ? `已保存到 ${where}` : '已保存' }
  } catch (error) {
    // ⚠ 失败**不吞**：原文进日志（诊断用），界面说那句既有的人话。
    console.error('Celira：往下载目录写文件失败', error)
    return { ok: false, text: apiErrorText('attachment_download_failed') }
  }
}

function triggerDownload(url: string, name: string) {
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.rel = 'noopener'
  document.body.appendChild(a)
  a.click()
  a.remove()
}
