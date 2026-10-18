import { Component, type ErrorInfo, type ReactNode } from 'react'
import { useNavigate } from 'react-router'
import { TriangleAlert } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  RELOAD_LABEL,
  classifyRenderFailure,
  renderFailureCopy,
  type RenderFailureKind,
} from '@/lib/renderFailure'

/**
 * **渲染错误边界** —— 界面自己塌了的时候，替它说话。
 *
 * ---
 * ## 为什么需要它
 *
 * 本仓已经有一条明确的纪律：**红路径不许静默**（`tests/no-silent-red-path.test.ts`）。
 * 但那条守卫钉的是"**动作**失败要有话"（撤回失败、附件下载失败）。
 * 还有另一种失败它没覆盖：**整个界面塌掉**。
 *
 * React 的行为是：渲染期抛出且**没有边界**，就卸载整棵树 ⇒ **白屏**。
 * 前端每个页面都是 `lazy()`，所以这条路真实可达 —— 宿主 `node home.js down/up`
 * 换版后，已经开着的标签页去取旧 hash 的那个 chunk 就是 404（判据与文案见
 * `lib/renderFailure.ts` 的文件头）。
 *
 * ⇒ 白屏 = **最彻底的红路径静默**。本组件把它收口成"一句真话 + 一条出路"。
 *
 * ## 两层都挂，各自管一段
 *
 * - 外层（`App.tsx` 的路由根）包住 `RootLayout` 本身 —— 首屏那几个组件（启动加载态、
 *   登录页）虽然都是静态 import、极简，但一旦崩同样是全白，这一层是它的兜底；
 * - 内层包住 `<Outlet />` —— 覆盖**全部路由**，且带 `resetKey` 可随路由切换复位。
 *
 * ## 为什么"复位"不写成 `key={pathname}`
 *
 * 那样写更短，但它会**重建整棵子树**：`/app/chat` 与 `/app/chat/:roomId` 共用同一个
 * `ChatPage`，pathname 一变就整页卸载重建 —— 页面的内部状态（滚动位置、抽屉开关、
 * 正在写的东西）会一起丢掉。本组件改用派生 state 判"路由换了"（见
 * `getDerivedStateFromProps`）：**只在出错之后**才复位，正常态子树一动不动。
 */
interface BoundaryProps {
  children: ReactNode
  /**
   * 变了就复位（调用方传路由 `pathname`）。不传 ⇒ 出错后一直停在降级页，
   * 出路是那颗「重新加载」——这就是外层那一档想要的语义。
   */
  resetKey?: string
}

interface BoundaryState {
  /** `null` = 没出错。非空 = 展示对应那一类的降级页。 */
  kind: RenderFailureKind | null
  /** 上一次见过的 `resetKey`；用来判"路由换了"。 */
  seenResetKey: string | undefined
}

export class RouteErrorBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { kind: null, seenResetKey: undefined }

  static getDerivedStateFromError(error: unknown): Partial<BoundaryState> {
    return { kind: classifyRenderFailure(error) }
  }

  /**
   * 路由换了就复位。
   *
   * 为什么用派生 state 而不是 `componentDidUpdate` + `setState`：后者是 React 明确
   * 不建议的形状（`react(no-did-update-set-state)` 会报），而这里要的恰好是
   * "props 变了 ⇒ 清掉错误"这一件事 —— 派生 state 就是它的语义。（**不为此放宽那条规则**：
   * 规则本身是对的，改的是我们的写法。）
   *
   * 它**只在出错之后才有可见效果**：`kind` 为 `null`（正常态）时重置与否都不改变渲染
   * 结果，子树一动不动 —— 这点很要紧：`/app/chat` 与 `/app/chat/:roomId` 共用同一个
   * `ChatPage`，任何"按 pathname 重建子树"的做法都会把页面的内部状态一并丢掉。
   */
  static getDerivedStateFromProps(
    props: BoundaryProps,
    state: BoundaryState,
  ): Partial<BoundaryState> | null {
    if (props.resetKey === state.seenResetKey) return null
    return { kind: null, seenResetKey: props.resetKey }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // 只落控制台。本仓没有错误上报面，**也不该悄悄上报** —— 崩溃这件事不往外送。
    console.error('[celira] 页面渲染失败：', error, info.componentStack)
  }

  render() {
    if (this.state.kind !== null) return <RouteFailureScreen kind={this.state.kind} />
    return this.props.children
  }
}

/**
 * 降级页。形状与 `FixtureGuard` 的诚实门对齐（图标 + 标题 + 说明 + 出口），
 * 但**说的是另一件事实**：那一页说"这一面还没有真实数据"，这里说"界面塌了"。
 *
 * `role="alert"`：它是一次要被立刻听见的失败（本仓红路径的既有形状）。
 * 不导出 —— 只有本文件的边界用它，导出会招来 `only-export-components` 警告。
 */
function RouteFailureScreen({ kind }: { kind: RenderFailureKind }) {
  const navigate = useNavigate()
  const copy = renderFailureCopy(kind)
  return (
    <div
      role="alert"
      className="flex h-full min-h-0 flex-col items-center justify-center gap-4 bg-background px-6"
    >
      <TriangleAlert className="size-6 text-muted-foreground" aria-hidden />
      <div className="max-w-sm space-y-2 text-center">
        <h1 className="text-title">{copy.title}</h1>
        <p className="text-body text-muted-foreground">{copy.body}</p>
      </div>
      <div className="flex gap-2">
        <Button onClick={() => window.location.reload()}>{RELOAD_LABEL}</Button>
        {copy.canReturnToMessages ? (
          <Button variant="outline" onClick={() => navigate('/app/chat', { replace: true })}>
            返回消息
          </Button>
        ) : null}
      </div>
    </div>
  )
}
