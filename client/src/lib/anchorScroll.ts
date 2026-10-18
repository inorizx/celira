/**
 * **深链接带锚点 ⇒ 把那一节送进视野**（2026-10-11 立；两处页面共用一份实现）。
 *
 * ## 它治的是什么（**只有这两件，都是确定的**）
 *
 * 1. **一份实现**：机器页三间、Agent 主页两段原先各写了一遍同一段
 *    `document.getElementById(id)?.scrollIntoView({ block: 'start' })`。
 *    同一条不变量只该有一个权威实现 —— 判据与旧名映射（`{home→run, days→activity, mind→model}`）
 *    现在能**离线直测**（见 `resolveAnchorId`），不必靠真机去猜。
 * 2. **不再"晚一拍就静默什么都不做"**：那一节是随数据后渲染的，元素还没进 DOM 时
 *    `?.` 会**不报错、也不滚**。这里在**一秒**内有界地等它出现，出现即滚、随即收工。
 *
 * ## ⚠ 它**没有**修的东西（如实登记，别照着自己的想象读它）
 *
 * 2026-10-11 量过：真机上容器**每次都滚到了上限**（`scrollTop` = `max`：171/171、348/348、768/768），
 * 「模型」都在视野里。**注入"滚一次就收工"的旧实现再量，读数一模一样** ⇒
 * **"滚一次不够"这个缺陷没有被量出来**。所以这一层**不做**"盯住它、内容长高就再滚一次"
 * 那类事（那是为想象中的未来建设，仓库明文不许），也**不接管用户的滚动**。
 *
 * ⚠ 推论（这一刀真正的教训）：**「模型」是最后一间，它顶不到最上面是物理事实**
 * —— 下面没有内容可滚了。第一次那条路的正解**不是**把滚动做花，
 * 而是**让动作排在读数之前**（机器页那张「第一次」卡，见 `pages/RuntimesPage.tsx`）。
 */

/**
 * 等那一节出现多久（毫秒）。到点还没出现就**什么都不做**（不报错、不跳走 —— 与原来那条纪律一致）。
 *
 * 一秒的依据：它只用来盖"数据比页面晚一拍"这一档（真机上元素通常**当帧就在**，所以正常路径上
 * 第一帧就滚完收工）。⚠ 这**不是**"内容长高就再滚一次"的窗口 —— 那件事没被量出来，不做。
 */
export const ANCHOR_SCROLL_WAIT_MS = 1000

/**
 * 锚点 → 元素 id（**纯函数**，可 `node --test` 直测）。
 *
 * - 空 / 只有 `#` / 全是空白 ⇒ `null`（**不滚**，也不报错）；
 * - 旧名经 `legacy` 映射（机器页那张 `{home→run, days→activity, mind→model}`）；
 * - 认不出的名字**原样交出去**（元素不在就滚不动 —— 与"不报错、不跳走"同一条纪律）。
 */
export function resolveAnchorId(
  hash: string,
  legacy: Readonly<Record<string, string>> = {},
): string | null {
  const raw = String(hash ?? '').replace(/^#/, '').trim()
  if (!raw) return null
  return legacy[raw] ?? raw
}

/**
 * 把那一节送进视野：**有界地等它出现**（元素随数据后渲染），出现即滚、随即收工。
 *
 * 返回**取消函数**（React 的 `useEffect` 直接返回它：卸载或换锚点时收工）。
 * 在 `document` 不在场的环境里（`node --test`）调用是**安全的空操作** —— 纯判据部分可直测，
 * 这一层只被页面调用。
 */
export function scrollToAnchor(
  hash: string,
  options: { legacy?: Readonly<Record<string, string>>; waitMs?: number } = {},
): () => void {
  const id = resolveAnchorId(hash, options.legacy)
  if (id === null) return () => {}
  if (typeof document === 'undefined' || typeof requestAnimationFrame !== 'function') return () => {}

  const waitMs = options.waitMs ?? ANCHOR_SCROLL_WAIT_MS
  const deadline = Date.now() + waitMs
  let stopped = false

  const tick = (): void => {
    if (stopped) return
    const element = document.getElementById(id)
    if (element) {
      element.scrollIntoView({ block: 'start' })
      // **出现即收工**：不盯、不抢用户的滚动（"内容长高再滚一次"那件事没被量出来，不做）。
      stopped = true
      return
    }
    if (Date.now() >= deadline) return
    requestAnimationFrame(tick)
  }
  tick()

  return () => {
    stopped = true
  }
}
