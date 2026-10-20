import { lazy, Suspense, useEffect, useState, type ReactNode } from 'react'
import { createBrowserRouter, Navigate, Outlet, RouterProvider, useLocation, useNavigate, useSearchParams } from 'react-router'
import { Loader2 } from 'lucide-react'
import { TooltipProvider } from '@/components/ui/tooltip'
import { Button } from '@/components/ui/button'
import { ThemeProvider } from '@/hooks/use-theme'
import { WallpaperProvider } from '@/hooks/use-wallpaper'
import { AppShell } from '@/components/celira/AppShell'
import { CallOverlay } from '@/components/celira/CallOverlay'
import { StartupLoading } from '@/components/celira/StartupLoading'
import { FixtureRouteGuard } from '@/components/celira/FixtureGuard'
import { LoginGate } from '@/components/celira/LoginGate'
import { RouteErrorBoundary } from '@/components/celira/RouteErrorBoundary'
import { SplashPage } from '@/pages/SplashPage'
import { useAppStore, useConnection } from '@/api/store'
import { syncUnreadBadge } from '@/api/unreadBadge'
import { isRealWorldSession, normalizeServerUrl } from '@/api/prefs'
import { decideFrontDoor } from '@/api/entry'
import { setLocalHomeBase } from '@/api/homes'
import { fetchHomeConfig, modelSourceAbsent } from '@/api/homeConfig'
import {
  adoptDecision,
  bootstrapLocalWorld,
  enableLocalWorld,
  localBaseOf,
  mintLocalSession,
  planLocalWorld,
  readLocalWorld,
  sessionNextStep,
  shellInvokeOrNull,
  shouldStartLocalWorld,
  disableLocalWorld,
} from '@/api/shellLocal'
import { NameYourselfCard } from '@/components/celira/NameYourselfCard'
import { TitleBar } from '@/components/celira/TitleBar'
import { isTauriRuntime } from '@/api/shell'
import { cn } from '@/lib/utils'

const ChatPage = lazy(() => import('@/pages/ChatPage').then((m) => ({ default: m.ChatPage })))
const ChatHistoryPage = lazy(() => import('@/pages/ChatHistoryPage').then((m) => ({ default: m.ChatHistoryPage })))
const GalleryPage = lazy(() => import('@/pages/GalleryPage').then((m) => ({ default: m.GalleryPage })))
const ContactsPage = lazy(() => import('@/pages/ContactsPage').then((m) => ({ default: m.ContactsPage })))
const NewContactPage = lazy(() => import('@/pages/NewContactPage').then((m) => ({ default: m.NewContactPage })))
const ProfilePage = lazy(() => import('@/pages/ProfilePage').then((m) => ({ default: m.ProfilePage })))
const MyAgentsPage = lazy(() => import('@/pages/MyAgentsPage').then((m) => ({ default: m.MyAgentsPage })))
// Agent 的主页（刀2a）：与列表**同一个模块** —— 它们共用"选中哪个 Agent"那一套状态与数据面。
const AgentHomePage = lazy(() => import('@/pages/MyAgentsPage').then((m) => ({ default: m.AgentHomePage })))
const RuntimesPage = lazy(() => import('@/pages/RuntimesPage').then((m) => ({ default: m.RuntimesPage })))
const RuntimeDetailPage = lazy(() =>
  import('@/pages/RuntimesPage').then((m) => ({ default: m.RuntimeDetailPage })),
)
const SettingsLayout = lazy(() =>
  import('@/pages/settings/SettingsLayout').then((m) => ({ default: m.SettingsLayout })),
)
const SettingsIndex = lazy(() =>
  import('@/pages/settings/SettingsLayout').then((m) => ({ default: m.SettingsIndex })),
)
const SettingsGeneralPage = lazy(() =>
  import('@/pages/settings/SettingsGeneralPage').then((m) => ({ default: m.SettingsGeneralPage })),
)
const SettingsAppearancePage = lazy(() =>
  import('@/pages/settings/SettingsAppearancePage').then((m) => ({ default: m.SettingsAppearancePage })),
)
const SettingsNotificationsPage = lazy(() =>
  import('@/pages/settings/SettingsNotificationsPage').then((m) => ({ default: m.SettingsNotificationsPage })),
)
const SettingsAboutPage = lazy(() =>
  import('@/pages/settings/SettingsAboutPage').then((m) => ({ default: m.SettingsAboutPage })),
)
const SettingsServerPage = lazy(() =>
  import('@/pages/settings/SettingsServerPage').then((m) => ({ default: m.SettingsServerPage })),
)
const ModelProvidersPage = lazy(() => import('@/pages/ModelProvidersPage').then((m) => ({ default: m.ModelProvidersPage })))
const DiscoverPage = lazy(() => import('@/pages/DiscoverPage').then((m) => ({ default: m.DiscoverPage })))
const AgentPage = lazy(() => import('@/pages/AgentPage').then((m) => ({ default: m.AgentPage })))
const NotFoundPage = lazy(() => import('@/pages/NotFoundPage').then((m) => ({ default: m.NotFoundPage })))

function RouteFallback() {
  return (
    <div className="flex h-full min-h-0 w-full items-center justify-center bg-background">
      <Loader2 className="size-5 animate-spin text-muted-foreground" aria-label="页面加载中" />
    </div>
  )
}

/**
 * **正门 `/` 自己不做落点决定**——落点归 `RootLayout` 的 `resolveFirstScreen`：
 * 手里有可复用的直连凭据就直连并落 `/app/chat`，没有就给登录页。
 *
 * ⚠ 这里原先挂的是 `<Navigate to="/splash" replace />`，那是一个**闭环 + 抢位**的组合，
 * 真机实测（2026-09-29）后果是**登录之后进不去应用**：
 * - 闭环：品牌页那颗「进入」正是 `navigate('/')`（`SplashPage.tsx`）⇒ 点回去又被送回 `/splash`；
 * - 抢位：`resolveFirstScreen` 里 `setStartupPending(false)`（store 同步更新，先渲染）会把 `/`
 *   渲染成这个 `<Navigate>`，它的 effect 后发先至，把紧跟着的 `navigate('/app/chat')` 覆盖掉。
 *   于是**已经连上的用户永远停在品牌页上**（网络里 `/v1/rooms`、`/v1/messages` 全 200，
 *   世界数据都在 store 里，只有界面进不去），只有手敲 `/app/chat` 才进得去。
 *
 * 所以这里只留两件事：没有凭据 ⇒ 去品牌启动页；有凭据 ⇒ 一个诚实的等待态，等定案把
 * 人落进应用。**两条路都不再由 `/` 自己写死终点。**
 */
function EntryLanding() {
  const { token } = useConnection()
  if (!token.trim()) return <Navigate to="/splash" replace />
  // 它也是**第一屏**（有凭据时的诚实等待态）⇒ 同样要带窗口栏（`FirstScreen`，2026-10-20）。
  return (
    <FirstScreen>
      <StartupLoading />
    </FirstScreen>
  )
}

/**
 * 旧地址 → **执行端**那一页（2026-09-25 组织轴刀）。
 *
 * `/app/runtime` 与 `/app/experience` 从前是两个一级页面（后来是 Agent 主页的两个房间）。
 * 组织轴换成**机器**之后，它们指向 `/app/runtimes`（机器列表）。
 *
 * ⚠ **为什么落列表而不是某台机器**：这两个地址只说明"要哪一面"，**没说明是哪台机器**。
 * 从前靠"Agent 列表里第一个"顶上去 —— 而机器列表是**本机优先**的（本机那条是预置的），
 * 直接跳本机等于替用户猜"你要看的就是这台"。落列表让他自己挑一台，是**不猜**的做法。
 *
 * ⚠ **2026-09-26 修 H12**：这里原先重定向到 **`/app/runtimes#<旧锚点>`** —— 而
 * `LEGACY_ANCHORS` 那张映射表住在 **`RuntimeDetail`**（需要 `entryId` 才渲染）。
 * 列表页没有房间、也没有 entryId ⇒ **那个 hash 永远没人认领**，锚点映射一次都没跑过；
 * `#mind` 更是两条路都到不了。而 `README.md` 与 `DESIGN_SYSTEM.md` 都承诺它成立。
 *
 * 现在：**重定向不带 hash**（列表页本来就没有房间可滚），文档同步改成"不承诺"。
 * `LEGACY_ANCHORS` 映射本身**留着且有效** —— 它服务的是**直接深链到机器页**的旧书签
 * （`/app/runtimes/<entryId>#home`），那条路是真能跑到的。
 */
function LegacyRuntimeRedirect() {
  return <Navigate to="/app/runtimes" replace />
}

/**
 * 旧地址 `/settings/providers` ⇒ **按机器寻址**的配置页（2026-10-11）。
 *
 * 判据是「**配置跟着它的对象走**」：这一页管的是**某台机器**上的钥匙清单，而旧地址里
 * **没有机器上下文**。所以两条出口，都不猜：
 *
 * - 带 `?home=<id>`（老深链接/书签；机器页的「模型」间从前就是这么跳的）⇒ 落到**那台机器**；
 * - 没有上下文 ⇒ 落**执行端列表** —— 与 `LegacyRuntimeRedirect` 同一条判据
 *   （"这两个地址只说明要哪一面，**没说明是哪台机器** ⇒ 落列表让他自己挑一台"）。
 *
 * ⚠ 地址本体不删（深链接/书签不破，与 `SettingsLayout` 那句承诺一致）。
 */
function LegacyProvidersRedirect() {
  const [params] = useSearchParams()
  const home = params.get('home')?.trim()
  return (
    <Navigate
      to={home ? `/app/runtimes/${encodeURIComponent(home)}/config` : '/app/runtimes'}
      replace
    />
  )
}

function DemoSwitcher() {
  const location = useLocation()
  const navigate = useNavigate()
  const { mode, token } = useConnection()
  const inGallery = location.pathname === '/gallery'
  // **演示通道** = 不属于真实世界（= 登录页那句「不连接，先看演示界面」，
  // 见 `LoginGate.browseDemo`）。真实世界的用户**不该**看见这个开关：
  // 陈列室是 fixtures 驱动页，已被 `FixtureRouteGuard` 拦住 —— 给入口就是给死链。
  // （这条也是「fixtures 只活在演示通道里」的机械落点：开关本身就是那个通道的门。）
  const demoChannel = !isRealWorldSession({ mode, token })
  const show =
    demoChannel &&
    (location.pathname.startsWith('/app') || inGallery || location.pathname.startsWith('/settings'))
  if (!show) return null
  return (
    <div className="fixed bottom-20 right-3 z-50 flex gap-1 rounded-full border border-border bg-popover p-1 shadow-lg md:bottom-4 md:right-4">
      {([
        { key: 'app', label: '聊天', to: '/app/chat' },
        { key: 'gallery', label: '陈列室', to: '/gallery' },
      ] as const).map((item) => (
        <button
          key={item.key}
          type="button"
          onClick={() => navigate(item.to)}
          className={cn(
            'cursor-pointer rounded-full px-3 py-1 text-caption transition-colors',
            (item.key === 'gallery') === inGallery
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:text-foreground',
          )}
        >
          {item.label}
        </button>
      ))}
    </div>
  )
}

let pendingEntry: Promise<EntryOutcome> | null = null

interface EntryOutcome {
  /** 定案后是否已经有真实世界可进（core 直连成功）。 */
  enter: boolean
  /** 第一屏是「这台机器上还没有世界」那一格（§8.6-1 问一次）；其余时候为空。 */
  localAsk?: boolean
  /**
   * ★ **这个世界还没有你**（2026-10-20）：世界在跑，但没有 `human_id` ⇒ 首屏问名字。
   *
   * 与 `localAsk` **分两格**：那一格问的是"要不要在这台机器上开一个世界"（机器级表态），
   * 这一格问的是"你在这个世界里叫什么"（公共事实）。它们可能接连出现，也可能各自单独出现
   * （世界早就开着、只是还没铸人）。
   */
  needsHumanName?: boolean
  /**
   * **第一次的路**：本机世界刚被采用，而这台机器上**还没有模型配置** ⇒ 先去配一条。
   *
   * 判据是 `readModelSourceAbsent`（纯函数在 `api/homeConfig.ts`，**两个门**那两份载荷都成立）；
   * 落点是 `entryLandingTarget` —— 机器页的「模型」那一间，不是「设置 → 模型配置」（2026-10-07 裁）。
   *
   * 只在**这一次启动的这条路上**出现：配好了条件自然为假 ⇒ 不新增任何持久状态、
   * 之后每次启动照旧直落聊天首页。
   */
  firstRunModel?: boolean
  /** `local-ask` 那一格里，壳这一次没把世界起起来的原样原因。 */
  localFailure?: string | null
  /**
   * ★ **2026-10-08：这一轮是"换了个世界"进来的** —— 值是**上一个世界的地址**。
   *
   * 什么时候会有它：入口先按手里的凭据直连，**连不上** ⇒ 第二轮让本机世界接手并进去了
   * （见 `resolveFirstScreen` 那两轮）。**必须说出来**：静默换世界就是欺骗 ——
   * 用户会以为自己还在原来那个世界（那里的房间与人都还在，只是够不着）。
   */
  switchedFrom?: string | null
}

/**
 * 首屏定案：第一屏该给什么（判定见 `api/entry.ts`），以及要不要顺手直连。
 *
 * **TASK_63 收窄、TASK_64 收尾**：Host 全托管路径退役 ⇒ 不再有 Host 门，也
 * **不再有启动探测**（`detectHost` / `bootstrapHostSession` 已随 host 模块删除，
 * 裁决 3）。判据现在只剩持久偏好：
 * - 有可直接复用的直连凭据 → 自动直连（连不上就回落正门，把原因摆在表单上）；
 * - 否则 → 登录页（服务器地址 + token）——这是 client 的正门。
 *
 * **2026-10-03（完整 app 那一刀）**：桌面 app 的默认形态是**成品**（`PROJECT_SHAPE.md`
 * §9.1）——壳里**多问一次本机世界**（三个命令，见 `api/shellLocal.ts`），顺序是语义：
 *
 * 1. **壳在场就问一次读数**（浏览器里这一步整条不存在，`isTauriRuntime` 判据）；
 * 2. **装上本机家的地址**（壳解析出来的 loopback —— §8.4 推论 2"本机地址由 app 解析"；
 *    否则页面那些相对地址在壳里发不出去）；
 * 3. **页面手里没有凭据时**才采用本机条目（地址 + 壳签的一支会话）——用户上次连的是
 *    哪个世界就是哪个世界，本机那条不插队；采用之后**照旧**由 `decideFrontDoor` 定案
 *    （分派点只有一处）；
 * 4. 壳说"还没表态" ⇒ 第一屏问那一句（`local-ask`）。
 *
 * 这一步仍然是**异步**的（`connect()` 要打网络），所以 `startupPending` 那道
 * 闸门保留：定案前界面只显示诚实加载态，绝不提前渲染 Splash 或 fixtures 内容。
 * 竞态由模块级 promise 去重。
 */
/**
 * 这台机器上**还没有模型配置**吗（首次的路用它决定要不要把人带过去）。
 *
 * 判据**不在这里**：它是 `api/homeConfig.ts` 的 `modelSourceAbsent`（纯函数，有离线用例
 * 钉着两个门那两份载荷）。搬过去是因为**两条路共用同一段判定**（入口定案 ／ 「在这台机器上
 * 开一个」成功之后），而原先那个本地判据认的是 `sourcesState === 'absent'` —— 那个词只有
 * web 门会给，壳门（`app/src-tauri/src/home_doors.rs`）在文件不在时给的是 `'loaded'` 空表
 * ⇒ 这条分支在壳里**恒不触发**、在 web 形态**假绿**。
 *
 * 这里只负责**读一次**并守住那条纪律：**读不到一律当"不是"** —— 读错一份配置比读不到更坏，
 * 而且绝不能因为一次读失败就把用户拽去一个他不需要去的页面。它只读、不写、不落任何状态。
 */
async function readModelSourceAbsent(): Promise<boolean> {
  try {
    return modelSourceAbsent(await fetchHomeConfig())
  } catch {
    return false
  }
}

/**
 * **落在哪一屏** —— 唯一一处落点判定（两条路共用）。
 *
 * core 直连没有默认房间概念 ⇒ 聊天首页（沿用既有 core 行为，不替它猜房间）。
 * **例外只有一条**：第一次那条路（本机世界刚立起来、这台机器上还没有模型配置）⇒ 落
 * **机器页的「模型」那一间**（`/app/runtimes/local#model`）。
 *
 * ⚠ **2026-10-07 主人裁**：第一次该落在**机器**那一页，不是「设置 → 模型配置」——模型配置
 * 改的是**那台机器**上的私有事实（清单 `<家>/model_sources.json` ＋ 这一支的绑定），
 * 页面按机器寻址（`?home=<id>`）；从"设置"进去配"一台机器"是拐了一道。那一间里主语是
 * 执行端，并且已经把两件事分开说：**这台执行端上有哪些钥匙**（清单）／**这一支用哪一条**
 * （绑定），外加"去配置模型"的出口。供应商预设与模型目录那一套暂时仍在配置页上（把能力
 * 搬进这一间是下一刀）。
 */
function entryLandingTarget(modelAbsent: boolean): string {
  return modelAbsent ? '/app/runtimes/local#model' : '/app/chat'
}

/**
 * **一轮入口定案**（②–④）。
 *
 * ★ **2026-10-08：两轮只差一条判据**（`credentialsFailed`，见 `planLocalWorld`）。
 * 第一轮按今天那条走：**手里有凭据 ⇒ 本机那条不插队**（用户上次连的是哪个世界就是哪个）。
 * 那一轮以"连不上"收场时，**那份凭据已经被证明没用** ⇒ 第二轮让本机世界接手 ——
 * 而不是把用户丢进一张要手填地址与 token 的表单（`app/README.md` §5 第 15 行的另一半）。
 * 分派点仍然只有这一处：两轮走的是**同一段代码**。
 */
async function attemptEntry(credentialsFailed: boolean): Promise<EntryOutcome> {
  const store = useAppStore.getState()
  // ① 壳在场吗：不在 ⇒ 下面整段是空操作（web 行为一字不变）。
  const invoke = await shellInvokeOrNull()
  const readout = await readLocalWorld(invoke)
  // ② 壳给了本机地址 ⇒ 装上去（家那一侧从此走绝对地址；缺一个口就不装，见 `localBaseOf`）。
  if (readout) setLocalHomeBase(localBaseOf(readout))

  const plan = planLocalWorld(readout, storePrefsOf(), { credentialsFailed })

  if (plan.kind === 'ask') {
    // 还没表态：第一屏问一句（**不替用户起世界** —— 它会写磁盘、占端口、常驻后台）。
    return { enter: false, localAsk: true }
  }
  // **进本机世界之前先确保这个家跑起来**（世界 ＋ 住户；`enable` 幂等）。
  // 两种情形都走这里：世界没在跑（起它）与世界已经在跑（**确认住户也在跑** —— 托盘语义下
  // 那才是常态：关窗口/退出 app 时本地服务照旧跑，下次打开时世界是活的、本机住户可能已经不在）。
  // 起不来就如实摆出来（缺可执行件 / 起失败是人要看的，不偷着重试）。
  let effective = plan
  if (shouldStartLocalWorld(plan)) {
    const fallback = plan.kind === 'blocked' ? plan : null
    const started = await enableLocalWorld(invoke)
    if (started) setLocalHomeBase(localBaseOf(started))
    if (started && started.state === 'running') {
      effective = {
        kind: 'enter',
        address: started.address,
        statusBase: started.statusBase,
        configBase: started.configBase,
      }
    } else if (fallback) {
      effective = {
        kind: 'blocked',
        state: started?.state ?? fallback.state,
        detail: started?.detail ?? fallback.detail,
      }
    } else {
      // 本来在跑、而 enable 回来说不在跑（起失败了）：如实摆出来，别假装进了。
      effective = {
        kind: 'blocked',
        state: started?.state ?? 'not-running',
        detail: started?.detail ?? null,
      }
    }
  }
  if (effective.kind === 'blocked') {
    // 表过态要跑，但它没起来：如实说，两个出口（重试 / 连别的世界）。
    return { enter: false, localAsk: true, localFailure: effective.detail ?? null }
  }
  // 这一次启动是不是**刚在这台机器上立起世界并采用了它**（第一次那条路）——只用来决定
  // "要不要先带用户去配模型"，不落任何状态、不影响别的分支。
  let adoptedLocal = false
  if (effective.kind === 'enter') {
    // ③ 采用本机条目：先取一支本机会话（壳签、只在内存里过一次），再写进页面的唯一持久面。
    const session = await mintLocalSession(invoke)
    // ★ **这个世界还没有你**（2026-10-20）⇒ 首屏问名字，拿到名字再叫壳铸人。
    //   绝不在这里替谁起一个（从前壳会拿机器用户名铸一个 —— 那正是这一刀修掉的东西）。
    if (sessionNextStep(session) === 'name') return { enter: false, needsHumanName: true }
    const verdict = adoptDecision(session)
    if (!verdict.adopt) {
      // **绝不假装成功**：拿不到会话就把原因摆在那一格上，用户还能选"连别的世界"。
      return { enter: false, localAsk: true, localFailure: session?.detail ?? null }
    }
    store.setPrefs(
      'core',
      session!.token!,
      normalizeServerUrl(session!.address || effective.address),
    )
    adoptedLocal = true
  }

  // ⚠ 采用之后**照旧**由正门那一处定案（分派点只有一处）：手里有凭据 ⇒ `core-auto`。
  const decision = decideFrontDoor(storePrefsOf())
  if (decision.screen === 'core-auto') {
    store.setNeedsLogin(false)
    await useAppStore.getState().connect()
    // 直连失败（token 失效 / 服务器不可达）：**先别急着回正门** —— 还有一轮
    // （第二轮会让本机世界接手，见 `resolveFirstScreen`）。两轮都不行才回。
    if (useAppStore.getState().connection !== 'connected') {
      if (!credentialsFailed) {
        const failedAddress = storePrefsOf().serverUrl.trim()
        const second = await attemptEntry(true)
        if (second.enter && !second.localAsk && failedAddress) {
          // ★ **换了世界必须说出来**（静默换世界 = 欺骗）：把上一个世界的地址带出去。
          return { ...second, switchedFrom: failedAddress }
        }
        // ⚠ 2026-10-20：`needsHumanName` 也要往上传 —— 它既不是 enter 也不是 localAsk，
        //   漏了它就会掉进下面那句"回登录页"，而用户其实该看到命名卡（真机撞到过）。
        if (second.enter || second.localAsk || second.needsHumanName) return second
      }
      useAppStore.getState().setNeedsLogin(true)
      return { enter: false }
    }
    // **第一次的路**：世界刚在这台机器上立起来，而它还没有任何模型来源 ⇒ 先去配一条
    // （模型来源是机器级事实：不配就等于"借"机器环境里那一把，用户看不见）。
    if (adoptedLocal && (await readModelSourceAbsent())) {
      return { enter: true, firstRunModel: true }
    }
    return { enter: true }
  }
  // 唯一剩下的：login —— 没有可用凭据。
  // 顺带把模式归一到 core：世界上只剩 core 一条路（Host 已退役），归一同时
  // 让非入口路径的深链接（/gallery、/app/…）落回 core 语义（`setPrefs` 保留
  // 用户已填的 token 与地址，不吞任何字段）。
  //
  // ⚠ TASK_56 起这里的结果与更早的注释**相反**，别照旧读：core 语义下
  // fixture 驱动页由 `FixtureRouteGuard` 按「是不是真实世界」判——
  // **带凭据**（真实直连）⇒ 深链接 /gallery、/app/discover 等显示诚实缺省；
  // **没凭据**（演示通道，`browseDemo` 写的正是 `core` + 空 token）⇒ 才看得到它们。
  // ⚠ 这里原先还有一行 `if (store.mode !== 'core') store.setPrefs('core', store.token)`
  // —— 恒假（`ConnectionMode` 只有一个取值），是 Host 退役后留下的死代码，已删
  // （2026-09-26，审计 D18）。旧持久值里的 `'host'` 由 `parsePrefs` 归一。
  useAppStore.getState().setNeedsLogin(true)
  return { enter: false }
}

/**
 * **入口定案**（一次启动只跑一次；**第二轮不走记忆**）。
 *
 * 记忆（`pendingEntry`）是给**并发调用者**去重的（`RootLayout` 的 effect 与别处可能同时问），
 * 不是给第二轮用的 —— 第二轮必须真的再跑一遍，否则它会 await 到自己那份还在飞的 promise 上。
 *
 * ★ **两轮**（2026-10-08）：第一轮按今天的判据（手里有凭据 ⇒ 本机那条不插队）；它以
 * "连不上"收场时，**凭据已被证明没用** ⇒ 第二轮让本机世界接手（`attemptEntry(true)`）。
 * 两轮都没成 ⇒ 回正门（登录页），那里有地址与 token 两格，是"换一个世界"的出口。
 */
async function resolveFirstScreen(credentialsFailed = false): Promise<EntryOutcome> {
  if (credentialsFailed) return attemptEntry(true)
  if (pendingEntry) return pendingEntry
  pendingEntry = attemptEntry(false).finally(() => {
    pendingEntry = null
  })
  return pendingEntry
}

/** 当前持久偏好的**一份快照**（形状与 `ConnectionPrefs` 一致；正门只读它）。 */
function storePrefsOf() {
  const store = useAppStore.getState()
  return {
    mode: store.mode,
    token: store.token,
    serverUrl: store.serverUrl,
    worldId: store.worldId,
  }
}

/**
 * 「在这台机器上开一个世界」——正门那一格按下去之后走的路（§8.6-1 的表态 + 起世界）。
 *
 * 三步：叫壳写下表态并确保世界在跑 ⇒ 取一支本机会话 ⇒ 写进 `prefs` 再直连。
 * **任何一步没成都不往下走**：原因原样交回去，由那张卡显示（绝不假装成功）。
 *
 * ★ 2026-10-20：世界起来了、但**这个世界还没有你**时，它不编一个名字 —— 交回
 * `needsHumanName`，由外层切到命名卡（`NameYourselfCard`）。
 */
export async function startLocalWorld(): Promise<{
  ok: boolean
  detail: string | null
  needsHumanName?: boolean
}> {
  const invoke = await shellInvokeOrNull()
  const readout = await enableLocalWorld(invoke)
  if (!readout) return { ok: false, detail: null }
  setLocalHomeBase(localBaseOf(readout))
  if (readout.state !== 'running') return { ok: false, detail: readout.detail }
  const session = await mintLocalSession(invoke)
  if (sessionNextStep(session) === 'name') return { ok: false, detail: null, needsHumanName: true }
  const verdict = adoptDecision(session)
  if (!verdict.adopt) return { ok: false, detail: session?.detail ?? null }
  const store = useAppStore.getState()
  store.setPrefs('core', session!.token!, normalizeServerUrl(session!.address || readout.address))
  await store.connect()
  const next = useAppStore.getState()
  if (next.connection !== 'connected') {
    return { ok: false, detail: next.connectionError ?? null }
  }
  next.setNeedsLogin(false)
  return { ok: true, detail: null }
}

/**
 * 「你在这个世界里叫什么」——命名卡按下去之后走的路（2026-10-20）。
 *
 * 三步与 `startLocalWorld` 的后半截是同一条：叫壳**铸人**（`local_world_bootstrap`，
 * 名字由人给）⇒ 拿回会话 ⇒ 写进 `prefs` 再直连。名字不合法时壳回一句人话（`detail`），
 * 卡上原样显示；**不**偷偷换一个名字、**不**把失败说成成功。
 */
export async function nameLocalHuman(
  displayName: string,
): Promise<{ ok: boolean; detail: string | null }> {
  const invoke = await shellInvokeOrNull()
  const readout = await readLocalWorld(invoke)
  if (readout) setLocalHomeBase(localBaseOf(readout))
  const session = await bootstrapLocalWorld(invoke, displayName)
  const verdict = adoptDecision(session)
  if (!verdict.adopt) return { ok: false, detail: session?.detail ?? null }
  const store = useAppStore.getState()
  store.setPrefs(
    'core',
    session!.token!,
    normalizeServerUrl(session!.address || readout?.address || ''),
  )
  await store.connect()
  const next = useAppStore.getState()
  if (next.connection !== 'connected') return { ok: false, detail: next.connectionError ?? null }
  next.setNeedsLogin(false)
  return { ok: true, detail: null }
}

/** 「连别的世界」——正门那一格按下去之后走的路：写下"这台机器上不跑世界"，回登录页。 */
export async function declineLocalWorld(): Promise<void> {
  const invoke = await shellInvokeOrNull()
  await disableLocalWorld(invoke)
  useAppStore.getState().setNeedsLogin(true)
}

/**
 * **第一屏也要有窗口按钮**（2026-10-20 主人："为啥这个界面没有那个比如关闭啊什么按钮"）。
 *
 * 病：标题栏只挂在 `AppShell`（**进世界之后**那一层）⇒ 启动加载 / 命名卡 / 正门这几屏
 * 一个窗口按钮都没有；而窗口是 `decorations: false`（自绘），于是人**只能** Alt+F4 或
 * 任务栏右键 —— 这在一个"我还没进来"的屏幕上尤其糟。
 *
 * 判据两条：
 * 1. **凡是不在 `AppShell` 里的一屏，都经过这一层**（只有一个写者，不会出现两条栏）；
 * 2. 它是**浮层**（`fixed`），不改任何一屏的布局：那几屏自己都是 `min-h-screen` 的居中卡，
 *    把栏塞进 flex 流里就会多出 36px 的溢出。
 *
 * 浏览器里 `TitleBar` 自己返回 null（`isTauriRuntime`）⇒ web 版布局一行都不变。
 */
function FirstScreen({ children }: { children: ReactNode }) {
  return (
    <>
      {isTauriRuntime() && (
        <div className="fixed inset-x-0 top-0 z-50">
          <TitleBar />
        </div>
      )}
      {children}
    </>
  )
}

function RootLayout() {
  const location = useLocation()
  const navigate = useNavigate()
  const { startupPending, needsLogin } = useConnection()
  // 「这台机器上还没有世界」那一格（§8.6-1 问一次）：**壳在场且还没表态**时才可能出现。
  // 它是首屏定案的产物、只活在这一次启动里（答复本身落在壳那边 —— 机器级事实，不在浏览器里）。
  const [localAsk, setLocalAsk] = useState(false)
  /**
   * 「你在这个世界里叫什么」（2026-10-20）：世界在跑、但没有主人时出现。
   * 与 `localAsk` 一样是**首屏定案的产物、只活这一次启动** —— 名字本身落在世界那一侧
   * （`participants.display_name`），答复不在浏览器里。
   */
  const [needsName, setNeedsName] = useState(false)
  const [localFailure, setLocalFailure] = useState<string | null>(null)
  /**
   * ★ **换了世界就说出来**（2026-10-08）：值是上一个世界的地址。
   * 只活这一次启动 —— 用户按「知道了」或换路由就不再出现（不落任何持久状态）。
   */
  const [switchedFrom, setSwitchedFrom] = useState<string | null>(null)
  // 入口路径（/ 或 /splash 重定向目标）才做默认房间导航；显式深链接
  // （/app/chat/:id、/settings 等）保持用户自己的路径。
  const isEntryPath = location.pathname === '/' || location.pathname === '/splash'

  useEffect(() => {
    void resolveFirstScreen().then((outcome) => {
      const s = useAppStore.getState()
      const { enter } = outcome
      // 首屏定案完成：直连成功 / 登录页 / 那一问都已定案，收起加载态。
      s.setStartupPending(false)
      // ★ 换了世界就说出来（一次性的；`null` = 没换）。
      if (outcome.switchedFrom) setSwitchedFrom(outcome.switchedFrom)
      if (outcome.localAsk) {
        setLocalAsk(true)
        setLocalFailure(outcome.localFailure ?? null)
        return
      }
      if (outcome.needsHumanName) {
        setNeedsName(true)
        return
      }
      if (!enter || !isEntryPath) return
      // 落点判定只有 `entryLandingTarget` **一处**（它写着两条出口与 2026-10-07 那条裁决的理由）
      // —— 下面 `local-ask` 那一格（按「在这台机器上开一个」成功之后）走的是**同一个函数**。
      // 这里只把"是不是第一次那条路"交给它（那个布尔是上面 `attemptEntry` 判出来的）。
      navigate(entryLandingTarget(outcome.firstRunModel === true), { replace: true })
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navigate, isEntryPath])

  /**
   * **未读角标**（2026-10-08）：把未读总数报给壳（托盘悬停 ＋ 任务栏小红点）。
   *
   * ⚠ 选择器里**直接求和**，不订阅整个 `rooms`：订阅数组的话，任何一次 presence 或房间元数据
   * 变化都会让 App 重渲染；求和成一个数之后，这个数不变就**不重渲染、也不过桥** ——
   * 而"这个数变了"正是唯一该动角标的时候。
   */
  const unreadTotal = useAppStore((s) =>
    s.rooms.reduce((sum, room) => sum + (room.unreadCount ?? 0), 0),
  )
  useEffect(() => {
    void syncUnreadBadge(unreadTotal)
  }, [unreadTotal])

  // 首屏定案之前：无论当前路径是什么，都不渲染 Splash / fixtures 内容，
  // 只显示诚实的加载态。
  if (startupPending) {
    return (
      <FirstScreen>
        <StartupLoading />
        <DemoSwitcher />
      </FirstScreen>
    )
  }
  // 正门优先于一切：**入口路径**（`/`、`/splash`）且没有可复用的直连凭据时，
  // 第一屏就是这张卡。
  // 深链接（/app/…、/gallery 等）保持各自既有语义，不做拦截——首访者总是从
  // 入口路径进来，而深链接的语义（见得 `FixtureRouteGuard` 与各页面自身）
  // 本来就是「已经决定过连接之后」的路径。
  if (needsName && isEntryPath) {
    return (
      <FirstScreen>
        <NameYourselfCard
        onSubmit={async (displayName) => {
          const done = await nameLocalHuman(displayName)
          if (done.ok) {
            setNeedsName(false)
            // 落点与入口定案**同一段判定**（`entryLandingTarget`）：刚进自己的世界，
            // 而它还没有任何模型来源 ⇒ 先去机器页的「模型」那一间。
            navigate(entryLandingTarget(await readModelSourceAbsent()), { replace: true })
          }
          return done
        }}
        onConnectElsewhere={async () => {
          await declineLocalWorld()
          setNeedsName(false)
        }}
        />
      </FirstScreen>
    )
  }
  if (localAsk && isEntryPath) {
    return (
      <FirstScreen>
        <LoginGate
        mode="run-local"
        localFailure={localFailure}
        onRunLocal={async () => {
          setLocalFailure(null)
          const done = await startLocalWorld()
          // ★ 世界起来了，但**这个世界还没有你** ⇒ 切到命名卡（不编名字、不当失败）。
          if (done.needsHumanName) {
            setLocalAsk(false)
            setNeedsName(true)
            return
          }
          if (!done.ok) {
            // 起不来就说清（**不假装成功**）：卡留在原地，两个出口都还在。
            setLocalFailure(done.detail ?? '世界没能起来——可以再试一次，或去连别的世界。')
            return
          }
          setLocalAsk(false)
          // ★ 落点与入口定案**同一段判定**（`entryLandingTarget`）：世界刚在这台机器上立起来，
          // 所以照旧先问一次"这台机器上有没有模型配置" —— 没有就先去机器页的「模型」那一间
          // （与"第一次那条路"落的是同一处，不是各写一个目标）。问不到就当"有"（读不到一律
          // 不当"没配"：绝不因为一次读失败把人拽去一个他不需要去的页面）。
          navigate(entryLandingTarget(await readModelSourceAbsent()), { replace: true })
        }}
        onConnectElsewhere={async () => {
          await declineLocalWorld()
          setLocalAsk(false)
        }}
        />
      </FirstScreen>
    )
  }
  if (needsLogin && isEntryPath) {
    return (
      <FirstScreen>
        <LoginGate />
      </FirstScreen>
    )
  }
  return (
    <>
      {/*
        ★ **换了世界就说出来**（2026-10-08）。为什么必须有这一条：静默换世界就是**欺骗** ——
        用户会以为自己还在原来那个世界（那里的房间与人都在，只是此刻够不着）。
        一次性的（只活这一次启动），并且**说清怎么回去**（连接设置在 `/settings/general`，
        那里是地址与 token 两格 —— 那是"换一个世界"的出口，不是假路径）。
      */}
      {switchedFrom && (
        <div
          role="status"
          className="flex items-center gap-3 border-b border-border bg-card px-4 py-2 text-caption text-muted-foreground"
        >
          <span className="min-w-0 flex-1">
            上次连的世界（{switchedFrom}）连不上，已经带你来这台机器上的世界。要回到那个世界：设置 →
            连接设置。
          </span>
          <Button variant="ghost" size="sm" onClick={() => setSwitchedFrom(null)}>
            知道了
          </Button>
        </div>
      )}
      {/*
        **渲染错误边界（内层）** —— 覆盖全部路由。
        本仓的红路径纪律（`tests/no-silent-red-path.test.ts`）原先只管"动作失败要有话"，
        没管"整个界面塌掉"：React 在没有边界时会把整棵树卸载 = 白屏，而那是最彻底的静默。
        `resetKey` 传 pathname ⇒ 出错后一旦换路由就复位（用户点「返回消息」走的正是这条路）。
        判据与两类文案见 `lib/renderFailure.ts`。
      */}
      <RouteErrorBoundary resetKey={location.pathname}>
        <Suspense fallback={<RouteFallback />}>
          <Outlet />
        </Suspense>
      </RouteErrorBoundary>
      <DemoSwitcher />
    </>
  )
}

/**
 * fixture 驱动路由的守卫搬去了 `components/celira/FixtureGuard.tsx`（TASK_56）。
 *
 * 这里原先是个内联的 `HostRouteGuard`，判据写 `mode === 'host'` —— 把
 * 「这是不是真实世界」错写成了「这是不是 Host 模式」，于是 core 直连的真实用户
 * 一路放行，「发现」页给他看 fixtures 假居民、点进不存在的房间。
 * 判据本体现在只有一份（`api/prefs.ts` 的 `isRealWorldSession`）。
 */

const router = createBrowserRouter([
  {
    // **渲染错误边界（外层）** —— 兜住 `RootLayout` 自身：内层边界在它的返回里，
    // 够不到它自己。首屏那几个组件（启动加载态、登录页）都是静态 import、极简，
    // 但一旦崩同样是全白。这一层**不传 `resetKey`** —— 首屏塌了不该靠换路由蒙混过去，
    // 出路就是那颗「重新加载」。
    element: (
      <RouteErrorBoundary>
        <RootLayout />
      </RouteErrorBoundary>
    ),
    children: [
      { path: '/', element: <EntryLanding /> },
      { path: '/splash', element: <SplashPage /> },
      {
        element: <FixtureRouteGuard />,
        children: [
          { path: '/gallery', element: <GalleryPage /> },
        ],
      },
      // 世界侧参与者页（`/agent/:participantId`）：2026-09-26 **从诚实门里放出来**。
      //
      // 它原先在 `FixtureRouteGuard` 之下（`side: 'demo'`），而**真实世界有三处入口链到它**
      // ——消息头像 / 右键菜单「查看主页」/ 联系人资料卡 ⇒ 真实用户点得到、点进去撞门。
      // 这是审计里最重的一条硬伤。放出来的前提是那一刀已经把它的假东西拆干净：
      // 写死的「在线」徽标、fixtures 的模型/绑定分区、「发消息」跳演示房号、漏网的内行词。
      // 现在它**只读 core**（目录 + `getAgentProfile` 能力探测），演示通道由目录回落自己处理。
      { path: '/agent/:participantId', element: <AgentPage /> },
      // 模型配置页（TASK_77 重写）：它从"静态供应商目录（fixtures 驱动）"改成了**配置面**——
      // 读本机模型门、写本机那格私有配置，因此**拆掉 `FixtureRouteGuard`**（与 `/app/runtime`
      // 同款：它不是 fixtures 页，非真实世界由页面自身按 `isRealWorldSession` 处理——
      // 读显示照实、写动作关着）。
      //
      // 历史（TASK_69 收尾）：此前它在守卫之外、真实世界会露出 fixtures 的假供应商目录，
      // 于是被挂进守卫；现在**那一份 fixtures 已经不再是这一页的数据源**，守卫的理由随之消失。
      //
      // ★ **2026-10-11 按机器寻址**：它管的是**某一台机器**上的钥匙清单（清单落
      //   `<家>/model_sources.json`，"用哪一条"落那个环境根）⇒ 地址里点名那台机器，
      //   搬进壳里 `/app/runtimes/:entryId/config`（**配置跟着对象走**）。
      //   旧地址 `/settings/providers` 保留成**别名**（深链接/书签不破）——见上面那个组件。
      { path: '/settings/providers', element: <LegacyProvidersRedirect /> },
      {
        path: '/settings',
        element: <SettingsLayout />,
        children: [
          { index: true, element: <SettingsIndex /> },
          { path: 'general', element: <SettingsGeneralPage /> },
          { path: 'appearance', element: <SettingsAppearancePage /> },
          { path: 'notifications', element: <SettingsNotificationsPage /> },
          { path: 'server', element: <SettingsServerPage /> },
          // ★ **`/settings/homes` 已退役**（2026-10-12：机器清单**只留一个门**）——
          //   清单、加一台、管一台现在都在一级入口 `/app/runtimes` 那一侧
          //   （加：清单页那颗「添加」就地开；管：机器那一页的「这台机器的设置」）。
          //   这一格留成**别名**：旧书签与旧链接照旧到得了，只是不再有第二个清单。
          { path: 'homes', element: <Navigate to="/app/runtimes" replace /> },
          { path: 'about', element: <SettingsAboutPage /> },
        ],
      },
      { path: '*', element: <NotFoundPage /> },
      {
        path: '/app',
        element: <AppShell />,
        children: [
          { index: true, element: <Navigate to="/app/chat" replace /> },
          { path: 'chat', element: <ChatPage /> },
          { path: 'chat/:roomId', element: <ChatPage /> },
          { path: 'chat/:roomId/history', element: <ChatHistoryPage /> },
          // 联系人读写闭环：联系人列表/详情（读）+「新的朋友」发起请求/
          // 接受 pending（写，经 Core 端点，见 CLIENT_CONTACT_REQUEST P1）。
          // discover 保持诚实拦截——Core v1 无 Directory/发现契约，不是「后续切片」。
          // （Agent 服务面 `/app/services` 已随服务层退役删除，2026-09-28。）
          { path: 'contacts', element: <ContactsPage /> },
          { path: 'contacts/:participantId', element: <ContactsPage /> },
          { path: 'contacts/new', element: <NewContactPage /> },
          {
            element: <FixtureRouteGuard />,
            children: [
              { path: 'discover', element: <DiscoverPage /> },
            ],
          },
          // R7a 只读执行状态页与「经历」页 ⇒ **刀2a 起不再是两个一级页面**，
          // 2026-09-25 组织轴刀之后归**执行端**：两者都重定向到 `/app/runtimes`（机器列表）。
          // 路由本体保留成**重定向**（深链接/书签不破）；旧锚点（#home / #days / #mind）
          // 由 `RuntimeDetail` 认旧名映射到新房间 —— 两条路都不 404。
          { path: 'runtime', element: <LegacyRuntimeRedirect /> },
          { path: 'experience', element: <LegacyRuntimeRedirect /> },
          // 我的 Agent：**列表**（真实世界分支：所有权 + 公共资料查看/编辑 +
          // **撤销 Agent**，见 TASK_52 壳层分区）；后端绑定/凭证/
          // 模型三块是 core-only 数据面，**由页面自身做能力探测**（`supportsAgentBindings` /
          // `supportsAgentCredentials` / `supportsAgentProfile`，TASK_72 ⑥ —— 原先写的是
          // `mode === 'core'`，而那个判据恒真、在演示通道会渲染出空区）。
          //
          // 刀2a：`/app/agents` 是列表，`/app/agents/:participantId` 是 **Agent 主页**。
          // 2026-09-25 组织轴刀之后它只剩「资料 + 服务」两间（世界侧）；
          // 运行 / 活动 / 模型搬去了 `/app/runtimes/:entryId`（那一台机器那一页）。
          // 2026-09-28 服务层退役之后它只剩「资料」一间。
          // 两个 agents 路由同一个模块：它们共用"选中哪个 Agent"那一套状态与数据面。
          { path: 'agents', element: <MyAgentsPage /> },
          { path: 'agents/:participantId', element: <AgentHomePage /> },
          // 执行端（2026-09-25 组织轴刀）：Runtime 区的主语是**机器**。
          // 列表一台一行、详情一台一页；「运行 / 活动 / 模型」三个房间在详情里。
          { path: 'runtimes', element: <RuntimesPage /> },
          { path: 'runtimes/:entryId', element: <RuntimeDetailPage /> },
          // ★ **某一台机器的模型配置**（2026-10-11 按机器寻址）：清单（这台机器上有哪些钥匙）
          //   归机器、绑定（这一支用哪条）归住户 —— 所以配置页挂在**机器**这条路径下，
          //   与「模型」那一间是同一台机器的两个深度（那一间给读数与换绑，这一页给增删改）。
          { path: 'runtimes/:entryId/config', element: <ModelProvidersPage /> },
          { path: 'profile', element: <ProfilePage /> },
        ],
      },
    ],
  },
])

export default function App() {
  return (
    <ThemeProvider>
      <WallpaperProvider>
        <TooltipProvider>
          <RouterProvider router={router} />
          {/* 通话那一层挂在外壳上（**不挂在会话页里**）：来电不看你正停在哪一页，
              挂进 ChatPage 就等于"只有打开那间房才收得到电话"。 */}
          <CallOverlay />
        </TooltipProvider>
      </WallpaperProvider>
    </ThemeProvider>
  )
}
