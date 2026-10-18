/**
 * 下载的**收场** —— 结构化，不是一句话（纯逻辑，`node --test` 直测）。
 *
 * 为什么不再只返回 `string | null`（TASK_72 那一版的形状）：壳里现在拿得到**真实结果**
 * （落盘了没有、落在哪），而"成功"在两种形态里说的**不是同一句话**：
 *
 * - **浏览器形态**：浏览器自己有下载 UI（那个小条/气泡就是它的回执），页面**不该**替它
 *   宣布"已保存" —— 用户可能当场取消了；
 * - **壳形态**：那边**没有**那套 UI，不说一句用户就什么都不知道（点了没反应？存哪了？）
 *   ⇒ 壳里**必须**说，而且要说得出**落在哪个目录**。
 *
 * 挤在一个 `null` 里说不清这两档，所以分开：`ok:true` 带一句可选的读数，`ok:false` 带一句
 * 必须说的话。
 */
export type DownloadOutcome = { ok: true; readout: string | null } | { ok: false; text: string }

/**
 * 路径的目录名（`C:\Users\x\Downloads\a.txt` ⇒ `Downloads`）。
 *
 * 只说**目录名**、不甩一整条路径：那句话是给气泡里看的。认不出（没有分隔符）⇒ `null`，
 * 调用方**不许**编一个位置出来。
 */
export function directoryNameOf(path: string): string | null {
  const parts = path.split(/[\\/]+/).filter((each) => each !== '')
  // 最后一段是文件名 ⇒ 目录名是倒数第二段。
  return parts.length >= 2 ? parts[parts.length - 2] : null
}

/** 组件那一侧唯一要说的话（`null` = 什么都不说 —— 浏览器形态的成功就是这一档）。 */
export function downloadOutcomeText(outcome: DownloadOutcome): string | null {
  return outcome.ok ? outcome.readout : outcome.text
}
