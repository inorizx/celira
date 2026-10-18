// ⚠ 本模块**要被 `node --test` 直接测**（`tests/forward-message.test.ts`）⇒ 相对路径 ＋
//   带 `.ts` 后缀（Node 的 ESM 解析器不认 `@/` 别名、也不认省略后缀的相对路径）。
import type { Attachment } from '../api/domain.ts'
import { errorText } from '../api/contractErrors.ts'

/**
 * **转发一条消息**（2026-10-08 刀：附件也一起转）。
 *
 * ## 判据
 *
 * 转发 = 在目标会话里发一条**新消息**（不是引用、不是"同一份事实换个地方看"）。
 * 附件怎么办？契约里附件绑定**绑死在"上传者本人 ＋ 同 Room ＋ 24h 窗口内"**
 * （`CORE_V1_IM_CONTRACT.md` §8 的绑定节）⇒ **跨房引用做不到**，所以这条路是
 * **把原字节取下来、在目标房重新上传一份**，再作为新消息的附件绑定。
 *
 * 这是 §6 说的那条路（"新产品能力归上层"，不动冻结契约）：逐条对下来全在现有契约内
 * —— 上传要目标房 active 成员、≤16 MiB、新 pending 24h 内绑定；下载原附件要同房 active 成员。
 *
 * ## 三条代价（如实登记，不藏）
 *
 * 1. **字节在家里存两份**（家是单机磁盘）；
 * 2. **"转发自谁"没有事实可依**：契约没有转发语义，正文是作者写的 ⇒ **客户端不伪造**；
 * 3. **撤回联动断开**：源消息撤回后，转发出去的那份仍在（它是一条新消息的附件）——
 *    这一点与主流一致（转发是副本）。
 *
 * ## 全有或全无
 *
 * 任一附件下载/上传失败 ⇒ **不发那条消息**，并说清是哪个文件。半转发比不转发更糟：
 * 用户以为转过去了，对面只收到一半。已经传上去的那几个是**到期回收的私有暂存租约**
 * （契约原文），24h 后自己回收，不泄漏、不需要页面去删。
 *
 * 尺寸与条数**不在这里再判一遍**：那两条判据归服务端（超限回稳定分类
 * `payload_too_large`），人话由 [`errorText`] 折 —— 在这里抄一遍就是第二处判据。
 */
export interface ForwardDeps {
  /** 取原附件的字节（源房）。 */
  fetchBlob: (attachmentId: string) => Promise<Blob>
  /** 在目标房上传一份（返回新的附件 id）。 */
  upload: (file: File) => Promise<string>
  /** 把新消息发到目标房（附件是刚上传的那些 id）。 */
  send: (input: { body: string; attachments: Attachment[] }) => Promise<void>
}

export type ForwardOutcome = { ok: true; forwarded: number } | { ok: false; text: string }

export async function forwardMessage(
  source: { body: string; attachments: Attachment[] },
  deps: ForwardDeps,
): Promise<ForwardOutcome> {
  const carried: Attachment[] = []
  for (const each of source.attachments) {
    try {
      const blob = await deps.fetchBlob(each.id)
      const file = new File([blob], each.name, {
        type: each.mime || blob.type || 'application/octet-stream',
      })
      const id = await deps.upload(file)
      // 显示归类跟着原件走（`kind` 是契约投影；`mime` / `durationSeconds` 是展示提示）。
      carried.push({ ...each, id, state: 'pending', localUrl: undefined })
    } catch (error) {
      return { ok: false, text: `「${each.name}」没转过去：${errorText(error)}` }
    }
  }
  try {
    await deps.send({ body: source.body, attachments: carried })
    return { ok: true, forwarded: carried.length }
  } catch (error) {
    return { ok: false, text: errorText(error) }
  }
}
