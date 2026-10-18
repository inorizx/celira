/**
 * **出网的窄口** —— 状态层与"谁去发这个请求"之间唯一的一道缝。
 *
 * ## 为什么要有这个模块
 *
 * 前端原先**直接调 `fetch`**。于是"请求由谁发出"这件事跟**webview**绑死了 ——
 * 而这件事在 Celira 里是**有语义的**：
 *
 * > 浏览器的同源策略是加在**网页**这个身份上的。网页拿着身份去跟"别的源"说话，
 * > 服务端必须明确允许（CORS）。而 im 服务端**不发任何跨源头**。
 * > 请求改由**壳的 Rust 侧**发出时，那条规矩不参与 ⇒ 「连任意地址的世界」不需要
 * > im 服务端新增任何东西。
 *
 * 所以这不是"为了跨平台而抽象"——**它是一条安全与连通性的边界**，必须在**一个**地方。
 *
 * ## 三条纪律
 *
 * 1. **零平台分支**：本模块里**不许**出现 `if (isTauri)` / 能力探测 / 插件注册。
 *    换平台 = **换一份实现**（`setTransport`），调用方一行不改。
 * 2. **形状贴着真实实现**：响应只暴露 `status` / `headers` / `json()` / `blob()` ——
 *    浏览器 `Response` 与 Tauri 的 `fetch` **都有这四个**。所以"换实现"是改名的量级，
 *    不是重写；反过来，把响应解成字符串或字节再让调用方自己解析，会逼每个实现各写一遍解码。
 * 3. **WebSocket 不走这里**：WS 不受同源策略管（浏览器只对 HTTP 做那套检查），
 *    所以它在 webview 里照旧能用，**不要**把它也抽象进来（没有被隔离的变化）。
 *
 * ## 今天的实现
 *
 * 只有一种：webview 的 `window.fetch`（浏览器 / web 版 / 源码形态）。
 * **Tauri 壳那一步换的就是 `setTransport` 那一行** —— 换的时候本模块以外的文件一行不用动。
 *
 * ## 一条行为细节（浏览器实现里）
 *
 * 同源形态下调用方传的是**相对路径**（`/v1/...`）。浏览器会自动对页面来源解析；
 * 而 Rust 侧实现没有"页面来源"这个概念，所以 `setTransport` 那一侧**必须**把相对路径
 * 解析成绝对地址再发。这条要求写在接口上（见 `TransportRequest.url` 的注释），
 * 免得将来换实现时才发现"壳里所有相对路径都打到了 app 自己身上"。
 */

/** 出网请求。字段是**中立**名字（不是 `RequestInit`），免得实现被浏览器的类型绑住。 */
export interface TransportRequest {
  /**
   * 目标地址。**可能是一条相对路径**（同源形态：`/v1/...`）。
   *
   * 实现的责任：相对路径要按"调用方所在的那个来源"解析成绝对地址再发。
   * 浏览器实现天然会做；Rust 侧实现必须自己带一个 base 来解析（见 `setTransport`）。
   */
  url: string
  method: string
  headers: Record<string, string>
  /** 已序列化好的请求体（字符串或 multipart）。GET 一律 `undefined`。 */
  body?: string | FormData
  /** 身份凭据随请求携带的方式。浏览器实现下 `same-origin` 是既有行为。 */
  credentials?: 'same-origin' | 'omit' | 'include'
  /**
   * 取消 / 超时信号（web 标准 `AbortSignal`，两种实现都支持）。
   *
   * **它不是可选项**：家通道（`homeStatus` / `homeConfig`）要连远端机器，可能永远不回 ——
   * 没有超时，一次"那台机器关机了"会让界面挂在转圈上。取消时实现按 `AbortError` 拒绝，
   * 调用方据此报"连不上"。
   */
  signal?: AbortSignal
}

/**
 * 响应。**故意只暴露真实实现都有的那四个**（浏览器 `Response` / Tauri `fetch`）。
 * 多一个方法就多一份"实现补不齐"的风险，而调用方一个都不需要。
 */
export interface TransportResponse {
  readonly status: number
  readonly ok: boolean
  /** 头部读取（大小写不敏感由实现保证）。未命中返回 `null`。 */
  header(name: string): string | null
  json(): Promise<unknown>
  blob(): Promise<Blob>
}

export interface Transport {
  send(request: TransportRequest): Promise<TransportResponse>
}

/**
 * **出网那一层发不出去**（连接被拒 / DNS 不通 / 证书不认 / 别的传输故障）。
 *
 * ## 为什么要有这个类型（2026-10-08）
 *
 * 在这之前，实现抛出来的东西**原样**穿到界面：壳的 Rust 侧抛的是
 * `error sending request for url (http://127.0.0.1:8180/v1/me)`（reqwest 的原文），
 * 浏览器抛的是 `TypeError: Failed to fetch`。**两者都是库的话，不是用户的话** ——
 * 而它出现在登录页上（`app/README.md` §5 第 15 行那条缺陷的现场）。
 *
 * ## 形状与它为什么长这样
 *
 * 它带一个**稳定分类**（`code`）⇒ 与 `ApiError` **同一种形状** ⇒ 走 `contractErrors`
 * 那张表拿固定人话，`errorText(error)` 一个函数就够，**不需要第二个入口**。
 * 原文进 `reason`：诊断留得住（控制台），界面不露。
 *
 * ⚠ **它不是 `ApiError` 的近亲**：`ApiError` 是"服务端答了话、答的是一个错误"，
 * 这个是"**根本没答话**"。两者必须分得开（一个是"它说不行"，一个是"够不着它"）。
 */
export class TransportFailure extends Error {
  readonly code = 'transport_unreachable'
  /** 实现抛出来的原文。**只给诊断看**（控制台），任何界面都不许显示它。 */
  readonly reason: unknown

  constructor(reason: unknown) {
    super('transport_unreachable')
    this.name = 'TransportFailure'
    this.reason = reason
  }
}

/**
 * 取消 / 超时：**原样放行**，不折成 [`TransportFailure`]。
 *
 * 为什么这条区分必须在窄口上做：调用方靠它分"**等太久**"与"**够不着**"
 * （`TransportRequest.signal` 的注释就是这么写的：实现按 `AbortError` 拒绝，
 * 调用方据此报"连不上"）。折了它，那一格就没了。
 *
 * 两个名字都收：浏览器 `AbortSignal.timeout()` 拒绝的是 `TimeoutError`，
 * 而 `AbortSignal.any([...])` 会把它原样透出来。
 */
function isCancellation(error: unknown): boolean {
  const name =
    error && typeof error === 'object' && 'name' in error
      ? String((error as { name?: unknown }).name ?? '')
      : ''
  return name === 'AbortError' || name === 'TimeoutError'
}

/**
 * **当前出网实现**。应用装配处只认这一个名字 —— 壳那一步换的就是它。
 *
 * 默认是 webview 那份（浏览器 / web 版）。Tauri 壳在启动时用 `setTransport` 覆盖。
 */
let current: Transport = browserTransport()

/** 换一份出网实现。**只在应用装配期调用一次**（壳的入口），不要在页面里调。 */
export function setTransport(next: Transport): void {
  current = next
}

/** 取当前实现。业务代码**不要**用它 —— 用 `sendRequest`（那是唯一的调用面）。 */
export function activeTransport(): Transport {
  return current
}

/**
 * **唯一的出网调用面**。业务代码只该走这里。
 *
 * 为什么不直接把 `current` 暴露出去：那会让"换实现"变成一个可以在任何地方发生的动作，
 * 而这条例外只该发生在装配期。
 *
 * ## 它还是"发不出去"这件事的**唯一分类点**（2026-10-08）
 *
 * 包在这里而不是各实现里：两种实现（webview / 壳的 Rust 侧）的失败形状**不一样**
 * （`TypeError: Failed to fetch` / reqwest 的原文），而"发不出去"是**同一件事** ——
 * 让每个实现各折一次，就是同一件事的两处权威。取消照旧原样放行（见 `isCancellation`）。
 */
export async function sendRequest(request: TransportRequest): Promise<TransportResponse> {
  try {
    return await current.send(request)
  } catch (error) {
    if (isCancellation(error)) throw error
    throw new TransportFailure(error)
  }
}

/**
 * webview 实现：`window.fetch`。
 *
 * 这份实现在效果上等于"直接用浏览器那份 fetch"，所以它**不是**为了可替换而存在的
 * 中间层 —— 它存在的理由是**把"请求由谁发出"收成一个可替换的东西**（见文件头）。
 *
 * ## 一条实现纪律：**不缓存 `fetch`**
 *
 * `typeof fetch === 'function'` 与取用都要发生在**每次请求的时候**，绝不在模块加载期
 * 绑定一份引用。理由有两条，都不是洁癖：
 *  1. **测试**会替换 `globalThis.fetch` 来打桩；加载期绑定会让那些桩静默失效
 *     —— 用例照样"通过"，但验的根本不是它们以为的那条路（本仓实测踩过：17 条用例）；
 *  2. **环境**可能在运行期换掉全局（打包器 polyfill、扩展、webview 注入），
 *     缓存的那份会变成陈旧引用。
 *
 * `fetchImpl` 参数保留给**显式注入**（写新用例时可以用它，不必碰全局）。
 */
export function browserTransport(fetchImpl?: typeof fetch): Transport {
  return {
    async send(request) {
      const doFetch =
        fetchImpl ?? (typeof globalThis.fetch === 'function' ? globalThis.fetch.bind(globalThis) : null)
      if (!doFetch) {
        throw new Error('这个平台没有内置 fetch：请用 setTransport 提供一份出网实现')
      }
      const res = await doFetch(request.url, {
        method: request.method,
        headers: request.headers,
        body: request.body as BodyInit | undefined,
        credentials: request.credentials ?? 'same-origin',
        signal: request.signal,
      })
      return {
        status: res.status,
        ok: res.ok,
        header: (name) => res.headers.get(name),
        json: () => res.json() as Promise<unknown>,
        blob: () => res.blob(),
      }
    },
  }
}
