/**
 * **壳里的出网接线**（2026-09-29 立）。
 *
 * ## 这一刀解决什么
 *
 * web 版的请求由 **webview** 发出 ⇒ 受同源策略管 ⇒ 网页只能跟同源的 im 服务端说话
 * （im 服务端不发任何跨源头）。app 里把出网换成 **Rust 侧**（`tauri-plugin-http`）⇒
 * 那条规矩不参与 ⇒ **「连任意地址的世界」不需要 im 服务端新增任何东西**。
 *
 * 换的就是一行：`setTransport(...)`（缝在 `api/transport.ts`）。
 *
 * ## 两条纪律
 *
 * 1. **web 版走这里必须什么也不做**。判据是"这个运行环境有没有 Tauri 的内部注入"
 *    （`isTauriRuntime`），**不是**平台名、不是构建变量 —— 于是同一份产物在浏览器里
 *    安静地跳过，在壳里自动接管，**不需要为两个壳各构建一份**。
 * 2. **本模块只做接线，不做策略**。它不认识"世界"、"家"、"地址从哪来"——
 *    那些归 `api/` 的其余部分。
 *
 * ## 一条今天还没解决的（如实登记，别让它变成静默的错）
 *
 * 同源形态下调用方传的是**相对路径**（`/v1/...`）：web 版里浏览器会对页面来源解析，
 * 而 **app 里没有"页面来源"**。所以壳里遇到相对路径时**明确失败**（见 `resolveShellUrl`），
 * 而不是让它打到一个莫名其妙的地址上。
 *
 * ⇒ **"app 里那条家条目 / 世界地址从哪来"是下一步（家的管理模型）要定的事**：
 * 定了之后，相对路径要么由用户填的地址解析（本地家形态），要么根本不再产生。
 */
import { setTransport, type Transport, type TransportResponse } from './transport.ts'

/** 这个运行环境是不是 Tauri 壳（判断依据是壳的内部注入，不是平台名、不是构建变量）。 */
export function isTauriRuntime(scope: Record<string, unknown> = globalThis as unknown as Record<string, unknown>): boolean {
  const internals = scope['__TAURI_INTERNALS__']
  return typeof internals === 'object' && internals !== null
}

/**
 * 把调用方给的地址解析成壳里真能发的绝对地址。
 *
 * 判据：
 * - **绝对地址**（有 scheme）⇒ 原样用。壳的世界与家都是"绝对地址 + 凭据"，
 *   这是 `PROJECT_SHAPE.md` §4 那条硬要求的直接后果（世界地址运行时填，绝不编进构建产物）；
 * - **相对路径**（同源形态）⇒ **明确失败**。壳里没有"页面来源"可解析，
 *   硬发出去只会打到一个与用户意图无关的地址上 —— 那是静默的错，比响亮地失败糟得多。
 */
export function resolveShellUrl(url: string): string {
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(url)) return url
  throw new Error(
    `壳里没有"同源"可用：网页形态的相对地址 ${url} 在 app 里没法解析。` +
      '请给这个世界/这台机器填一个完整地址（http:// 或 https:// 开头）。',
  )
}

/**
 * 用壳的 Rust 侧出网构造一份 `Transport`。
 *
 * `rustFetch` 由调用方注入（真实调用时是 `@tauri-apps/plugin-http` 的 `fetch`），
 * 所以本函数**可测**：给它一份假的 `rustFetch` 就能验形状与解析判据，不需要壳在场。
 */
export function shellTransport(
  rustFetch: (input: string, init: Record<string, unknown>) => Promise<Response>,
): Transport {
  return {
    async send(request) {
      const res = await rustFetch(resolveShellUrl(request.url), {
        method: request.method,
        headers: request.headers,
        body: request.body,
        signal: request.signal,
      })
      const out: TransportResponse = {
        status: res.status,
        ok: res.ok,
        header: (name) => res.headers.get(name),
        json: () => res.json() as Promise<unknown>,
        blob: () => res.blob(),
      }
      return out
    },
  }
}

/**
 * 装配：在壳里换上 Rust 侧出网；在浏览器里**什么也不做**。
 *
 * 在**任何网络请求之前**调用（`main.tsx` 的第一行）。失败**不抛**：装不上就退回 webview
 * 那份（web 版行为不变），但失败这件事会进控制台 —— 不静默。
 */
export async function installShellTransport(): Promise<void> {
  if (!isTauriRuntime()) return
  try {
    const { fetch: rustFetch } = await import('@tauri-apps/plugin-http')
    setTransport(
      shellTransport((input, init) => rustFetch(input, init as Parameters<typeof rustFetch>[1])),
    )
    // 留一行读数：壳里能查到它，就说明这一刀真的接上了（`tauri dev` 的窗口里
    // 右键 → 检查，或看 tauri-plugin-log 的输出）。
    console.info('Celira：出网已交给壳（Rust 侧），不再受浏览器的同源策略限制')
  } catch (error) {
    // 装不上 = 壳里仍走 webview 那份 ⇒ 只能连同源的世界。**响亮地说，不假装成功。**
    console.error('Celira：壳的出网没有接上（仍将使用 webview 的网络栈）', error)
  }
}
