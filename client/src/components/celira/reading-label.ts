/**
 * 「更新时间」那一行该写什么（TASK_72 ③；人话刀 2026-09-22 改名）。
 *
 * 背景：状态口有 15 秒短 TTL 与**开机预热**（`scripts/home/status-server.js`）⇒
 * 页面可能拿到一份**不是这一轮**的读数。这一层就是那个代价的落点：
 *
 * - **时刻照旧是那次读数的时刻**（口子在缓存命中时**不改写 `readAt`**）——不许改成"当下"；
 * - 但 `stale: true` 时必须**多一句"正在刷新"** —— 陈旧被标注了，就不算冒充当下。
 *
 * ⚠ **词是给用户看的**：从前叫「读数时刻」（工程黑话），现在叫「**更新时间**」；
 * 缓存那一档也从「缓存读数」改成「正在刷新」（说它在干什么，而不是说它是什么）。
 * 判据本身一个字没动（时刻照旧、陈旧照旧要标）。
 *
 * 纯函数：只决定"加不加那句"。时刻怎么格式化归 `lib/time`，取数归 `api/homeStatus`。
 */
import type { ToneLevel } from './status-tone'

export function readingLabel(formattedAt: string, stale: boolean): string {
  return stale ? `更新时间 ${formattedAt}（正在刷新…）` : `更新时间 ${formattedAt}`
}

/**
 * **「本地服务」（世界在不在）那一行该写什么**（2026-10-11 立）。
 *
 * 输入只有**一个事实**：探活问到的那个 HTTP 状态码（`null` ＝ 它没答话）。
 * 三态，每一态的理由都不同：
 *
 * - `200` ⇒ 「在跑」——有人在那个口上答了 200；
 * - **非 200（如 500）** ⇒ 「在跑，但它自己说不健康」——**有应答本身就证明那个口上有人在**。
 *   从前页面写的是 `listening ? '在跑' : '没在跑'`，而 `listening` 是 `code === 200` 派生的
 *   ⇒ 世界活着、只是自报不健康，也会被写成「**没在跑（HTTP 500）**」：自相矛盾的字面
 *   （`scripts/home/status-server.js` 那一格的老写法，2026-10-11 删掉派生字段）；
 * - `null` ⇒ 「读不到（它没有应答）」——**不许说成"没在跑"**：探不到**证明不了**它停了
 *   （同一把尺子：`proc.portInUse` 的"证明不了它空着，不许说它空着"）。
 *   ⚠ 措辞过一遍**内行词表**（`scripts/jargon.mjs`）：'答话' 在那张表上 ⇒ 用「没有应答」。
 *
 * 色调跟着**证据强度**走：非 200 是确定的故障（`danger`）；`null` 是不确定（`muted` ——
 * 画成故障就是把"我不知道"说成了"它坏了"）。
 *
 * 纯函数，两个页面共用（机器页「运行」那一间 ＋ 设置「服务器」那一页）。别在页面里各写一份 if。
 */
export function localServiceReading(health: number | null): { text: string; tone: ToneLevel } {
  if (health === null) return { text: '读不到（它没有应答）', tone: 'muted' }
  if (health === 200) return { text: '在跑', tone: 'ok' }
  return { text: `在跑，但它自己说不健康（HTTP ${health}）`, tone: 'danger' }
}
