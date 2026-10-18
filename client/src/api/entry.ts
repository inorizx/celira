/**
 * 正门判定（纯函数）—— 持久偏好 ⇒ 第一屏该是什么。
 *
 * 产品定位（2026-09-18 主人批准）：client 是连任意 core 的通用 IM 客户端，
 * 「登录 = 连哪个服务器 + 你是谁」才是正门。
 *
 * **TASK_63 收窄 ＋ TASK_64 收尾**：Host 全托管路径退役 ⇒ 不再有 Host 门，也
 * **不再有启动探测**（`detectHost` 随 host 模块删除，裁决 3）。判据只剩持久偏好：
 * - 手里有可直接复用的直连凭据 ⇒ 自动直连，不经过登录页；
 * - 否则 ⇒ 第一屏就是登录页（服务器地址 + token）。
 *
 * **2026-10-03 补第三格（`local-ask`）**：桌面 app 的默认形态是**成品**（`PROJECT_SHAPE.md`
 * §9.1：Windows ＝"完整 app：壳 + 本机世界 + 本机执行端"），而 §8.6-1 裁的是
 * 「**第一次启动问一次**"要不要在这台机器上跑一个世界"（它会写磁盘、占端口、常驻后台）」。
 * 于是这一格是**机器级前置**：还没表态时，第一屏问那一句 —— **不是第二条连接路径**
 * （连哪个世界的路子一条都没多），所以分派点仍然是**这里这一处**。
 * 表态与否、以及本机世界在不在，都由壳的读数回答（`api/shellLocal.ts`），
 * 浏览器里那一格恒不存在 ⇒ web 行为一字不变。
 *
 * ⚠ 这里有一条**被有意放弃的区分**，别当成疏忽：收窄前，「探不到」（网络失败 /
 * 畸形响应）与「可证明不是 Host」导向不同结果（前者 fail-closed 留在 Host 语境）。
 * Host 语境已经不存在，那个区分也随之消失——登录页本身就是"由用户自己决定连哪台"
 * 的诚实结果，所以这个降级不丢事实。
 *
 * 本模块零运行期依赖（`import type` 在类型剥离时被抹掉），可在 `node --test`
 * 的类型剥离模式下直接验。
 */

import type { ConnectionPrefs } from './prefs'

export type FrontDoorScreen =
  /** 已有可直接复用的直连凭据：自动直连，不经过登录页。 */
  | 'core-auto'
  /** 没有可用凭据：第一屏是登录页（服务器地址 + token）。 */
  | 'login'
  /** 这台机器上还没有世界（壳说"还没表态"）：第一屏问那一句（§8.6-1）。 */
  | 'local-ask'

export interface FrontDoorDecision {
  screen: FrontDoorScreen
}

/**
 * 首屏定案。`localAsk` 由壳的读数给（`null` = 没有这一格：浏览器里、或壳说不用问）。
 *
 * 顺序是语义：**手里有凭据 ⇒ 用户的选择优先**（连哪个世界由他上次决定的那次说了算，
 * 本机那条不插队）；没有才轮到"这台机器上要不要开一个世界"。
 */
export function decideFrontDoor(
  prefs: ConnectionPrefs,
  localAsk: boolean | null = null,
): FrontDoorDecision {
  const hasCredential = prefs.token.trim().length > 0
  if (hasCredential) return { screen: 'core-auto' }
  if (localAsk === true) return { screen: 'local-ask' }
  return { screen: 'login' }
}
