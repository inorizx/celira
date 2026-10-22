/**
 * **接口地址的那一道**：保存之前先判"这是不是一条完整线协议地址"。
 *
 * ## 为什么要有这一层（病根）
 *
 * Rust 侧要的是**连到具体接口上的完整地址**（`…/v1/chat/completions`）。而配置页的那一格
 * 允许人填 base（`https://opencode.ai/zen/go/v1`）—— 它**存得下去**，因为存的时候没人问
 * "这条地址指向哪个接口"。代价推迟到运行时：`POST <base>` 命中对方路由表之外，回 404，
 * 被归类成 `request-rejected`，用户只看到"请求被拒"，看不到"少了最后一段"。
 *
 * 判据一句话：**能在保存前判的，不要留到运行时让人猜。**
 *
 * ## 它判什么、不判什么（如实登记）
 *
 * - 判：能不能解析成网址、协议是不是 http(s)、路径末尾是不是这一种线协议那一段；
 * - **不判**：主机通不通、钥匙对不对、模型名存不存在 —— 那些是试连（`/home-config/probe`）
 *   的读数，不是这一层的判据。这一层**不发任何请求**。
 *
 * ⚠ **只看 pathname**：query / hash 不属于地址的那一段（`?api-version=…` 这类很常见），
 * 拿整串去比会把合法地址判红。
 * ⚠ **末尾多一个 `/` 不算差别**：`…/chat/completions/` 与 `…/chat/completions` 是同一格。
 * 但 `/v1/` 不会被这条放过 —— 剥掉斜杠它就露出"只到服务根"的原形（下一个判据接管）。
 *
 * 纯函数、零依赖（与 `presets.ts` 同一处境：能被 `node --test` 直接钉住）。
 */

/** 门认得的那两种线协议（`protocol` 缺省 = `chat-completions`）。 */
export type EndpointProtocol = 'chat-completions' | 'responses'

/** 校验结果：**要么过，要么带一句能直接上屏的中文**（不返回布尔 —— 布尔到了界面上只能变成"错了"）。 */
export type EndpointCheck = { ok: true } | { ok: false; message: string }

/** 两种协议各自必须以哪一段收尾。 */
const TAIL_OF: Record<EndpointProtocol, string> = {
  'chat-completions': '/chat/completions',
  responses: '/responses',
}

/**
 * 「服务根」的形状：最后一段长成 `v1` / `v2` / `v1beta` / `api` / `openai` 那样。
 *
 * 为什么单独认这一种：它是最常见的那一类填错（把基址当接口地址），而
 * **它的提示该和别的不一样** —— 说"你在服务根上，还差最后一段"比泛泛的
 * "地址不完整"更能让人当场改对。
 */
const SERVICE_ROOT_TAIL = /^(?:v\d+[a-z0-9.-]*|api|openai)$/i

/** 「这一种协议要填到哪」——给提示句用的短写法。 */
const whereTo = (kind: EndpointProtocol): string => `…${TAIL_OF[kind]}`

/**
 * 判一条接口地址是否可以存。
 *
 * @param endpoint 表单里那一格的原样值（内部自己 trim）
 * @param protocol 接口类型（`''` / `undefined` = 默认那一种，即 `chat-completions`）
 */
export function validateEndpointAddress(
  endpoint: string,
  protocol?: string | null,
): EndpointCheck {
  const raw = (endpoint ?? '').trim()
  if (!raw) {
    return {
      ok: false,
      message: `接口地址还空着 —— 要填到具体接口那一段，例如 https://api.deepseek.com${TAIL_OF['chat-completions']}。`,
    }
  }

  let url: URL
  try {
    url = new URL(raw)
  } catch {
    return {
      ok: false,
      message: `这个地址读不成网址 —— 要以 http:// 或 https:// 开头，后面跟主机名（例如 https://api.deepseek.com${TAIL_OF['chat-completions']}）。`,
    }
  }

  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    return {
      ok: false,
      message: `只认 http:// 或 https:// 开头的地址（现在是 ${url.protocol}//）。`,
    }
  }

  const kind: EndpointProtocol = protocol === 'responses' ? 'responses' : 'chat-completions'
  const want = TAIL_OF[kind]
  const other = TAIL_OF[kind === 'responses' ? 'chat-completions' : 'responses']
  // 只看路径；末尾的斜杠剥掉再比（见文件头那两条 ⚠）。
  const path = url.pathname.replace(/\/+$/, '')

  if (path === '') {
    return {
      ok: false,
      message: `这个地址只填到了主机，没到具体接口：${kind} 要填到 ${whereTo(kind)}。`,
    }
  }

  if (path.endsWith(want)) return { ok: true }

  // 两种协议**选错了那一格**：这一种最常见，且提示必须点出"是两边对不上"，不是"地址不完整"。
  if (path.endsWith(other)) {
    return {
      ok: false,
      message: `接口类型选的是 ${kind}，地址却落在 …/${other.slice(1)} —— 两处要对上：${kind} 要填到 ${whereTo(kind)}。`,
    }
  }

  const last = path.slice(path.lastIndexOf('/') + 1)
  if (SERVICE_ROOT_TAIL.test(last)) {
    return {
      ok: false,
      message: `这个地址只到服务根（…/${last}），还差最后一段：chat-completions 要填到 …/chat/completions，responses 要填到 …/responses。`,
    }
  }

  return {
    ok: false,
    message: `这个地址还差最后一段：chat-completions 要填到 …/chat/completions，responses 要填到 …/responses（现在末尾是 …/${last}）。`,
  }
}
