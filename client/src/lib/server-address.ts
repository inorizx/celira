/**
 * 「服务器」页那几格读数的**纯逻辑**（前端填肉刀，2026-09-23）。
 *
 * 这里只回答两个问题，都不碰 DOM、不碰 store：
 * ① 地址那一格写什么；② 要不要挂「本机」标注。
 *
 * 纪律（与页面那一条同源）：**不知道就不标**。「本机」是一个**事实**（这个地址落在
 * loopback 上）——地址读不懂、或同源时页面自己那个主机名解析不出来，都只能说"不知道"，
 * 绝不许把"不知道"画成"远程"或者"本机"。
 */

/** 只认字面的 loopback 主机名（`URL.hostname` 对 IPv6 会给 `[::1]` 这种带方括号的形状）。 */
const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '::1', '[::1]'])

/** 这个主机名是不是本机（loopback）。 */
export function isLoopbackHost(host: string): boolean {
  return LOOPBACK_HOSTS.has(host.trim().toLowerCase())
}

export interface ServerAddressReading {
  /** 地址那一格右边写什么。 */
  address: string
  /** 要不要挂「本机」标注 —— 只有**确实**落在 loopback 上才是 `true`。 */
  local: boolean
}

/**
 * 地址那一格。
 *
 * `serverUrl` 是**连接偏好里用户填的那个**（`api/prefs.ts` 的唯一持久面）；空串 = 同源
 * （页面自身来源的 `/v1`）——那时"本机不本机"只能看**页面自己**的主机名，而不是猜。
 *
 * `pageOrigin` 由调用方从 `window.location.origin` 取（本层不碰 `window`，所以可离线钉住）。
 */
export function serverAddressReading(serverUrl: string, pageOrigin: string): ServerAddressReading {
  const trimmed = serverUrl.trim()
  if (!trimmed) {
    const host = hostOf(pageOrigin)
    return {
      address: pageOrigin ? `本页面所在的地址（${pageOrigin}）` : '本页面所在的地址',
      local: host !== null && isLoopbackHost(host),
    }
  }
  const host = hostOf(trimmed)
  return { address: trimmed, local: host !== null && isLoopbackHost(host) }
}

/** 取主机名；地址解析不出来 ⇒ `null`（不猜）。 */
function hostOf(url: string): string | null {
  try {
    return new URL(url).hostname
  } catch {
    return null
  }
}
