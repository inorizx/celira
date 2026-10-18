/**
 * 渲染失败的**分类与人话**（纯逻辑；可在 `node --test` 直测，见 `tests/render-failure.test.ts`）。
 *
 * ---
 * ## 它治的是什么
 *
 * 前端每个页面都是 `lazy()`（`App.tsx`），而宿主的部署动作是 `node home.js down/up` ——
 * 每换一次版就换一批构建 hash。于是有一条**由本机自己的用法产出**的路：
 *
 * > 标签页还开着 → 宿主换版 → 这一页去取**旧 hash 的那个文件** → 404
 * > ⇒ 动态 import 失败 ⇒ React 在渲染期抛 ⇒ **没有边界就卸载整棵树 = 白屏**。
 *
 * 白屏是最彻底的一种"红路径静默"：用户不知道出了什么事，也不知道能做什么。
 * 本模块与 `RouteErrorBoundary` 一起把它变成**一句真话 + 一条出路**。
 *
 * ## 为什么必须分两类，不能合成一句
 *
 * 「文件拿不到」与「页面自己抛错」是**两件不同的事实**，出路也不同：
 *
 * - 前者是**环境变了**（这台服务重新部署过），重新加载必然有效；
 * - 后者是**代码有问题**，重新加载未必有效。
 *
 * 把它们说成同一句，就是把其中一种**说反了** —— 本仓对"说反了"有明确的既有判据
 * （见 `contractErrors.ts` 里为"下载失败被说成发送失败"打的那块补丁）。
 *
 * ## 分类方向是 fail-closed
 *
 * `classifyRenderFailure` **只在认得出**各家浏览器/打包器那句原话时才判 `chunk-load`，
 * 其余一律 `render`。宁可把"文件拿不到"保守地说成"页面出错了"，也绝不反过来——
 * 反过来会让真正的代码 bug 显示成"页面已更新，重新加载一下就好"，那是**教用户白费力气**。
 */

export type RenderFailureKind = 'chunk-load' | 'render'

/**
 * 认得出"动态 import 的模块没拿到"的原话。
 *
 * 各家措辞不同（形状与出处见 `tests/render-failure.test.ts` 的用例名）：
 * Chromium / Firefox / WebKit 各一句，打包器另有一句通用措辞。
 * **只收这些确定的形状** —— 收宽了就会把普通网络错误也算进来（那是另一件事实）。
 */
const CHUNK_LOAD_PATTERNS: readonly RegExp[] = [
  /failed to fetch dynamically imported module/i, // Chromium：取动态模块时请求失败
  /error loading dynamically imported module/i, // Firefox
  /importing a module script failed/i, // WebKit
  /loading chunk [^ ]+ failed/i, // 打包器通用措辞（含 ChunkLoadError 的 message）
  /chunkloaderror/i, // 只报了类名的那些
]

/**
 * 从任意抛出的东西里拼出一段可搜的文本（**鸭子类型**，不 import 任何东西，
 * 也不假设它是 `Error` —— 打包器偶尔抛普通对象）。
 */
function searchableText(error: unknown): string {
  if (typeof error === 'string') return error
  if (error === null || error === undefined) return ''
  if (typeof error !== 'object') return String(error)
  const bag = error as { name?: unknown; message?: unknown }
  const parts: string[] = []
  if (typeof bag.name === 'string') parts.push(bag.name)
  if (typeof bag.message === 'string') parts.push(bag.message)
  return parts.join(' ')
}

/**
 * 这次渲染失败属于哪一类。**认不出的一律 `render`**（见文件头「fail-closed」）。
 */
export function classifyRenderFailure(error: unknown): RenderFailureKind {
  const text = searchableText(error)
  return CHUNK_LOAD_PATTERNS.some((pattern) => pattern.test(text))
    ? 'chunk-load'
    : 'render'
}

/**
 * 两类降级页共用的那颗按钮的文案。
 *
 * ⚠ **它有第二个读者**：仓外的真机门 `scripts/verify-ui.mjs` 按它判"降级页在场"
 * （正向断言，TASK_58 起本仓不许断"某串不在场"）。**改这句要同步改那边。**
 */
export const RELOAD_LABEL = '重新加载'

/**
 * ⚠ 下面两句标题同样有第二个读者（`scripts/verify-ui.mjs` 的 chunk 那一类断言）。
 * 改这里就要同步改那儿，否则那条断言会**静默失效**。
 */
export const CHUNK_FAILURE_TITLE = '页面已更新，需要重新加载'
export const RENDER_FAILURE_TITLE = '这个页面出错了'

export interface RenderFailureCopy {
  /** 降级页标题。 */
  title: string
  /** 降级页正文（**说清是"哪一类"事实，不许混说**）。 */
  body: string
  /**
   * 要不要给「返回消息」这条出路。
   *
   * `chunk-load` 给 `false`：那种情形下**别的页面的 chunk 同样可能没拿到**，
   * 给一个点下去还会崩的按钮，等于本仓明确禁止的"给了必然会报错的按钮"。
   * 只留「重新加载」——那是这一类唯一真正解决它的动作。
   */
  canReturnToMessages: boolean
}

/**
 * 分类 ⇒ 降级页文案。**纯函数**，页面只消费它的输出。
 */
export function renderFailureCopy(kind: RenderFailureKind): RenderFailureCopy {
  if (kind === 'chunk-load') {
    return {
      title: CHUNK_FAILURE_TITLE,
      body: '这台服务重新部署过，当前这一页要的文件已经换了一批新的。重新加载一下就能跟上。',
      canReturnToMessages: false,
    }
  }
  return {
    title: RENDER_FAILURE_TITLE,
    body: '界面在渲染这一步出了问题，不是你的操作有误。重新加载多半能恢复；也可以先回消息页。',
    canReturnToMessages: true,
  }
}
