import type { ApiErrorEnvelope } from './contracts'
import { sendRequest } from './transport.ts'

export class ApiError extends Error {
  readonly code: string
  readonly requestId?: string
  readonly status: number

  constructor(status: number, code: string, requestId?: string) {
    super(`${code} (HTTP ${status})`)
    this.name = 'ApiError'
    this.status = status
    this.code = code
    this.requestId = requestId
  }
}

export interface RequestOptions {
  method?: string
  token?: string | null
  idempotencyKey?: string
  body?: unknown
  headers?: Record<string, string>
  /** 凭据模式（缺省 `same-origin`）。 */
  credentials?: RequestCredentials
  /** 服务器地址（空/未给 = 同源：走页面自身来源的 `/v1`）。 */
  baseUrl?: string
}

/**
 * 把 Core 路径拼到服务器地址上。**空地址 = 同源相对路径**（家里由
 * home_up / serve_client 托管的页面走 `/v1` 转发，一个字符都不变）。
 */
export function joinBaseUrl(baseUrl: string | null | undefined, path: string): string {
  if (!baseUrl) return path
  return `${baseUrl.replace(/\/+$/, '')}${path}`
}

/**
 * 同一服务器的 WebSocket 前缀；同源（空地址）返回 null，由调用方按页面来源
 * 推导 `ws(s)://<location.host>`。
 *
 * ⚠ **WebSocket 不走 `./transport`**：同源策略只管 HTTP，不管 WS。所以壳里
 * 这一条照旧（webview 的 WS 能用），不需要第二个实现。理由见 `transport.ts` 纪律 3。
 */
export function webSocketBase(baseUrl: string | null | undefined): string | null {
  if (!baseUrl) return null
  const trimmed = baseUrl.replace(/\/+$/, '')
  if (trimmed.startsWith('https://')) return `wss://${trimmed.slice('https://'.length)}`
  if (trimmed.startsWith('http://')) return `ws://${trimmed.slice('http://'.length)}`
  return null
}

export async function fetchJson<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { ...opts.headers }
  if (opts.token) headers['Authorization'] = `Bearer ${opts.token}`
  if (opts.idempotencyKey) headers['Idempotency-Key'] = opts.idempotencyKey
  if (opts.body !== undefined) headers['Content-Type'] = 'application/json'

  // 出网实现由 `./transport` 决定（webview 或壳的 Rust 侧）。**这里只构造请求、解错误信封。**
  const res = await sendRequest({
    url: joinBaseUrl(opts.baseUrl, path),
    method: opts.method ?? (opts.body !== undefined ? 'POST' : 'GET'),
    headers,
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
    credentials: opts.credentials ?? 'same-origin',
  })
  const requestId = res.header('x-request-id') ?? undefined
  if (!res.ok) {
    let code = 'unknown_error'
    try {
      const envelope = (await res.json()) as ApiErrorEnvelope
      if (envelope?.error?.code) code = envelope.error.code
    } catch {
      // non-JSON error body
    }
    throw new ApiError(res.status, code, requestId)
  }
  if (res.status === 204) return undefined as T
  return (await res.json()) as T
}

export function newIdempotencyKey(): string {
  return crypto.randomUUID()
}

/**
 * 取字节（附件下载、头像读取共用）。
 *
 * `codePrefix` 只影响**失败时的内部分类名**（`ApiError.code`）。默认值保持附件的
 * 旧口径（行为零变化）；头像传 `avatar_download`，免得调试时看到一个 404 报成
 * "attachment_download_404" 而去找附件的毛病。
 */
export async function fetchBlob(
  path: string,
  token: string | null,
  baseUrl = '',
  codePrefix = 'attachment_download',
): Promise<Blob> {
  const res = await sendRequest({
    url: joinBaseUrl(baseUrl, path),
    method: 'GET',
    headers: token ? { Authorization: `Bearer ${token}` } : {},
    credentials: 'same-origin',
  })
  if (!res.ok) {
    throw new ApiError(res.status, `${codePrefix}_${res.status}`)
  }
  return res.blob()
}

/**
 * multipart 上传（附件、头像共用）。
 *
 * `options` 是给头像加的（附件走默认值，行为零变化）：
 * - `method`：头像写入契约是 **PUT**（附件是 POST）；
 * - `fields`：头像要**额外带一个 `base_version` 文本 part**（附件只发文件本身）；
 * - `idempotencyKey`：头像写入**契约要求** `Idempotency-Key`（缺了 im 直接回
 *   `idempotency_key_required`；实测踩过一次）。
 *
 * 为什么放在这里而不是各写一份：构造 FormData、错误信封解码、`x-request-id`
 * 这三件事两条线一模一样；差别只有"怎么发"，那才该参数化。
 */
export async function uploadMultipart(
  path: string,
  token: string | null,
  file: File,
  fieldName = 'file',
  baseUrl = '',
  options: {
    method?: 'POST' | 'PUT'
    fields?: Record<string, string>
    idempotencyKey?: string
  } = {},
): Promise<unknown> {
  const form = new FormData()
  for (const [name, value] of Object.entries(options.fields ?? {})) {
    form.append(name, value)
  }
  form.append(fieldName, file, file.name)
  const headers: Record<string, string> = {}
  if (token) headers['Authorization'] = `Bearer ${token}`
  if (options.idempotencyKey) headers['Idempotency-Key'] = options.idempotencyKey
  const res = await sendRequest({
    url: joinBaseUrl(baseUrl, path),
    method: options.method ?? 'POST',
    headers,
    body: form,
    credentials: 'same-origin',
  })
  if (!res.ok) {
    let code = 'unknown_error'
    try {
      const envelope = (await res.json()) as ApiErrorEnvelope
      if (envelope?.error?.code) code = envelope.error.code
    } catch {
      // non-JSON error body
    }
    throw new ApiError(res.status, code, res.header('x-request-id') ?? undefined)
  }
  return (await res.json()) as unknown
}
