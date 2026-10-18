/**
 * **壳里的本机世界**（完整 app 自带的那一半）—— 页面这一侧的**唯一入口**。
 *
 * ## 它解决什么（第一性原理）
 *
 * 桌面 app 的默认形态是**成品**：打开就在自己的世界里（`PROJECT_SHAPE.md` §9.1）。而
 * **页面在壳里没有"页面来源"可解析** —— `api/shell.ts` 的 `resolveShellUrl` 见相对地址就
 * 响亮失败 ⇒ **本机地址与本机世界的主人必须由壳在运行时交给页面**（§8.4 推论 2：
 * *本机地址由 app 解析*）。让用户手敲 `http://127.0.0.1:8080` 是把**我们**的实现细节
 * 摊派给他 —— 那不是"让用户自己处理"。
 *
 * 壳给三样东西（三个命令，见 `app/src-tauri/src/local_world.rs`）：
 *
 * | 命令 | 给什么 | 什么时候叫 |
 * |---|---|---|
 * | `local_world_readout` | **读数**：本机世界在不在 + 地址 + 家的门两个口 ＋「这台机器要不要跑一个世界」（`null` = 还没表态） | 首屏定案时**叫一次** |
 * | `local_world_enable` / `local_world_disable` | 写下那次表态（§8.6-1 的"**问一次**"） | 用户在正门那张卡上选 |
 * | `local_world_session` | **一支本机会话**（`ok` / `world-down` / `no-core` / `failed`） | 页面手里没有凭据时叫一次 |
 *
 * ## 三条纪律
 *
 * 1. **浏览器里什么也不做**：判据是"有没有壳的内部注入"（`isTauriRuntime`），**不是平台名**、
 *    不是构建变量 —— 与 `shell.ts` 同一条纪律。于是 web 行为一字不变；
 * 2. **只在页面手里没有凭据时采用**（见 [`planLocalWorld`]）：用户已经连过某个世界（可能
 *    是他自己的远端世界）⇒ **他的选择优先**，本机那条不再插队。这条是"显式选择 > 机器默认"；
 * 3. **凭据只经这里过一次**：`local_world_session` 回的那支 token 立刻写进页面的唯一持久面
 *    （`prefs`，`store.setPrefs`），本模块**不留副本、不进日志、不进任何 URL**。
 *
 * ## 纯逻辑与 IO 分开
 *
 * 判断（[`planLocalWorld`] / [`adoptDecision`] / [`localBaseOf`]）全是纯函数 ⇒ `node --test`
 * 直接钉住；只有四个 `*From` 函数碰壳（`invoke` 可注入 ⇒ 用例里给替身，不需要壳在场）。
 */
import { isTauriRuntime } from './shell.ts'
import { decideFrontDoor } from './entry.ts'
import type { ConnectionPrefs } from './prefs.ts'
import type { LocalHomeBase } from './homes.ts'

/** 壳回的本机世界读数（字段名与 `LocalWorldReadout` 的 serde camelCase 逐字对齐）。 */
export interface LocalWorldReadout {
  /** 封闭词表：`running` / `not-running` / `no-core`。 */
  state: string
  /** 世界地址（`http://127.0.0.1:<port>`）—— 世界通道的 base。 */
  address: string
  homeDir: string
  humanId: string | null
  /** 家的门两个端面（loopback 绝对基址）。 */
  statusBase: string
  configBase: string
  /** `null` = 还没表态（§8.6-1 该问一次）；`true` / `false` = 表过态。 */
  runLocalWorld: boolean | null
  imExe: string
  detail: string | null
}

/** 壳回的一支本机会话（**唯一带 token 的形状**）。 */
export interface LocalSessionView {
  /** 封闭词表：`ok` / `world-down` / `no-core` / `failed`。 */
  state: string
  address: string
  humanId: string | null
  token: string | null
  mintedHuman: boolean
  detail: string | null
}

/** 正门该拿本机世界怎么办（纯函数的结果）。 */
export type LocalWorldPlan =
  /** 还没表态 ⇒ **问一次**（§8.6-1）。 */
  | { kind: 'ask' }
  /** 该在这台机器上跑，且它已经在跑 ⇒ 采用它的地址与主人，直进。 */
  | { kind: 'enter'; address: string; statusBase: string; configBase: string }
  /** 表过态要跑，但它没起来 / 缺件 ⇒ 如实说，两条出路（重试 / 连别的世界）。 */
  | { kind: 'blocked'; state: string; detail: string | null }
  /** 不插队：走今天那条判定（没壳 / 页面手里已有凭据 / 用户选了不跑本机）。 */
  | { kind: 'passthrough' }

/**
 * **正门该拿本机世界怎么办**（纯函数：壳的读数 + 页面有没有凭据 ⇒ 该做什么）。
 *
 * 判据的顺序是语义，不是随手排的：
 *
 * 1. **没壳 / 壳没答** ⇒ `passthrough`（web 与"壳里问不到"都走今天那条路，一字不变）；
 * 2. **页面手里有凭据** ⇒ `passthrough`：`decideFrontDoor` 会判 `core-auto` —— 用户上次
 *    连的是哪个世界就是哪个世界，**本机那条不插队**；
 *    ⚠ **例外只有一条**（`options.credentialsFailed`，2026-10-08）：那份凭据**已经被证明
 *    连不上**（入口先按它直连过一次、失败了）⇒ 这条理由不成立了 —— 它护的是"用户上次的
 *    选择优先"，而那份选择刚刚被证明用不了。**只**放宽这一条：下面三态一字未变；
 * 3. **还没表态** ⇒ `ask`（§8.6-1：它会写磁盘、占端口、常驻后台，所以问一次）；
 * 4. **表过态说不跑** ⇒ `passthrough`（第一屏回登录页，去连别的世界）；
 * 5. **说要跑** ⇒ 在跑就 `enter`，没跑就 `blocked`（**绝不假装成功**）。
 */
export function planLocalWorld(
  readout: LocalWorldReadout | null,
  prefs: ConnectionPrefs,
  options: { credentialsFailed?: boolean } = {},
): LocalWorldPlan {
  if (!readout) return { kind: 'passthrough' }
  // ⚠ 判据复用正门那一份（"手里有没有可直接复用的直连凭据"）——两处各写一套就是两个答案。
  if (!options.credentialsFailed && decideFrontDoor(prefs).screen !== 'login') {
    return { kind: 'passthrough' }
  }
  if (readout.runLocalWorld === null) return { kind: 'ask' }
  if (readout.runLocalWorld === false) return { kind: 'passthrough' }
  if (readout.state === 'running') {
    return {
      kind: 'enter',
      address: readout.address,
      statusBase: readout.statusBase,
      configBase: readout.configBase,
    }
  }
  return { kind: 'blocked', state: readout.state, detail: readout.detail }
}

/**
 * 进入本机世界之前，**先确保这个家跑起来**（世界 ＋ 住户；`enable` 是幂等的）。
 *
 * 两态都要：`enter`（世界已经在跑 ⇒ **也要确认住户在跑**）与 `not-running`（世界没在跑 ⇒ 起它）。
 *
 * ⚠ 为什么 `enter` 也要（2026-10-03 真机发现的缺口）：从前只在"世界没在跑"时才调 `enable`，
 * 于是**世界在跑、而住户不在跑**这一态没人管 —— 而那正是托盘语义下的常态（关窗口 / 退出 app
 * 时本地服务照旧跑，下次打开 app 时世界是活的、本机住户可能已经不在）。用户看到的是
 * "Agent 不在线、也不回话"。判据一句话：**"打开 app ⇒ 本机住户就在"必须由正门保证**，
 * 不能只在"顺便起世界"时保证。
 *
 * 判据只有这一条，别扩：`no-core` / `failed` 是**另一种事实**（这台机器上没有 world 二进制 /
 * 起失败了），那两态该如实摆出来给人看，不该由页面偷偷重试（重试是用户按一下的事）。
 */
export function shouldStartLocalWorld(plan: LocalWorldPlan): boolean {
  if (plan.kind === 'enter') return true
  return plan.kind === 'blocked' && plan.state === 'not-running'
}

/**
 * 一次会话回报该不该采用（纯函数）：**只有 `ok` 且真的有原文**才算数。
 *
 * ⚠ 这不是"防御式编程"：`ok` 而没有原文是一个**读不懂的回执**——静默当成成功，
 * 用户会停在一个连不上的世界里，而且没有任何人告诉他为什么。
 */
export function adoptDecision(session: LocalSessionView | null): {
  adopt: boolean
  reason: string | null
} {
  if (!session) return { adopt: false, reason: 'unreachable' }
  if (session.state !== 'ok') return { adopt: false, reason: session.state }
  if (!session.token) return { adopt: false, reason: 'no-token' }
  return { adopt: true, reason: null }
}

/**
 * 从读数里取出"本机家的 loopback 基址"（纯函数）。
 *
 * 两个口都读得懂才算数：家那一侧要走**绝对地址**（壳里没有代转层），缺一个就宁可
 * 不装 —— 装了半个只会让一半请求打到一个编出来的地址上。
 */
export function localBaseOf(readout: LocalWorldReadout | null): LocalHomeBase | null {
  if (!readout) return null
  const statusPort = portOf(readout.statusBase)
  const configPort = portOf(readout.configBase)
  const host = hostOf(readout.statusBase)
  if (!host || statusPort === null || configPort === null) return null
  if (hostOf(readout.configBase) !== host) return null
  return { host, statusPort, configPort }
}

function hostOf(url: string): string | null {
  const match = /^(https?:\/\/[^/:]+)(?::\d+)?\/?$/.exec(String(url ?? '').trim())
  return match ? match[1] : null
}

function portOf(url: string): number | null {
  const match = /:(\d+)\/?$/.exec(String(url ?? '').trim())
  if (!match) return null
  const port = Number.parseInt(match[1], 10)
  return Number.isInteger(port) && port > 0 && port <= 65535 ? port : null
}

// ---------------------------------------------------------------------------
// 与壳说话（四个窄口；`invoke` 注入 ⇒ 用例不需要壳在场）
// ---------------------------------------------------------------------------

/** 壳的 `invoke`（只有名字与参数会进去；参数里**没有任何凭据**）。 */
export type ShellInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>

/**
 * 取壳的 `invoke`：**不在壳里 ⇒ `null`**（浏览器里这几条路整条不存在）。
 *
 * 与 `shell.ts` 的 `installShellTransport` 同一条判据（`isTauriRuntime`）—— 两处都只认
 * "有没有壳的内部注入"这件事，谁也不去猜平台名。
 */
export async function shellInvokeOrNull(): Promise<ShellInvoke | null> {
  if (!isTauriRuntime()) return null
  try {
    const api = await import('@tauri-apps/api/core')
    return (command, args) => api.invoke(command, args) as Promise<unknown>
  } catch {
    // 拿不到桥 = 这个壳里没有这条路 ⇒ 当"没壳"处置（**不抛**：首屏不该因为一个可选能力塌掉）。
    return null
  }
}

function asReadout(value: unknown): LocalWorldReadout | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  if (typeof raw.state !== 'string' || typeof raw.address !== 'string') return null
  return {
    state: raw.state,
    address: raw.address,
    homeDir: typeof raw.homeDir === 'string' ? raw.homeDir : '',
    humanId: typeof raw.humanId === 'string' ? raw.humanId : null,
    statusBase: typeof raw.statusBase === 'string' ? raw.statusBase : '',
    configBase: typeof raw.configBase === 'string' ? raw.configBase : '',
    runLocalWorld:
      raw.runLocalWorld === true ? true : raw.runLocalWorld === false ? false : null,
    imExe: typeof raw.imExe === 'string' ? raw.imExe : '',
    detail: typeof raw.detail === 'string' ? raw.detail : null,
  }
}

function asSession(value: unknown): LocalSessionView | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  if (typeof raw.state !== 'string') return null
  return {
    state: raw.state,
    address: typeof raw.address === 'string' ? raw.address : '',
    humanId: typeof raw.humanId === 'string' ? raw.humanId : null,
    token: typeof raw.token === 'string' && raw.token !== '' ? raw.token : null,
    mintedHuman: raw.mintedHuman === true,
    detail: typeof raw.detail === 'string' ? raw.detail : null,
  }
}

/** 读数（只读，不动任何东西）。 */
export async function readLocalWorld(invoke: ShellInvoke | null): Promise<LocalWorldReadout | null> {
  if (!invoke) return null
  try {
    return asReadout(await invoke('local_world_readout'))
  } catch {
    return null
  }
}

/** 表态：在这台机器上跑一个世界（壳会顺手确保它起来）。 */
export async function enableLocalWorld(invoke: ShellInvoke | null): Promise<LocalWorldReadout | null> {
  if (!invoke) return null
  try {
    return asReadout(await invoke('local_world_enable'))
  } catch {
    return null
  }
}

/** 表态：这台机器上不跑世界（第一屏回登录页，去连别的世界）。 */
export async function disableLocalWorld(invoke: ShellInvoke | null): Promise<LocalWorldReadout | null> {
  if (!invoke) return null
  try {
    return asReadout(await invoke('local_world_disable'))
  } catch {
    return null
  }
}

/** 交一支本机会话（页面手里没有凭据时叫它；拿到就立刻写进 `prefs`，本模块不留副本）。 */
export async function mintLocalSession(invoke: ShellInvoke | null): Promise<LocalSessionView | null> {
  if (!invoke) return null
  try {
    return asSession(await invoke('local_world_session'))
  } catch {
    return null
  }
}

/**
 * 壳回的**本机服务读数**（字段名与 `ServicesReadout` 的 serde camelCase 逐字对齐）。
 *
 * 「本机服务」= 这颗 app 自己起的东西：世界 ＋ 住户（§8.1：本地进程是本地 app 的孩子）。
 * 它**只在壳里成立**（浏览器形态没有本机服务这一回事）。
 */
export interface LocalServicesReadout {
  worldRunning: boolean
  /** 这个家住着几支 / 其中几支在跑。 */
  residents: number
  residentsRunning: number
  /**
   * **这个家还是住户化之前的形状**（环境根 `<家>/env`）——2026-10-05 加。
   *
   * ⚠ 它与「运行」那一页的 `HomeStatusView.legacyHome` 是**同一格事实**（壳同一个判据），
   * 只是两条读数各带一份：那一页说"这个家还是老形态"，这一格就不能说"这台机器上还没有 Agent"
   * ——同一屏两句话不能互相打脸（判据：**认不出的形状不许说成不存在**）。
   */
  legacyHome: boolean
  /** 一句话读数（壳给的，页面可以直接摆）。 */
  summary: string
  /** 「退出时停不停本机服务」的生效值（没表态 ⇒ `true`，§8.3 的默认）。 */
  quitStopsServices: boolean
  /** 这一格**表过态没有**（`false` ⇒ 生效值是默认值，页面该说清"默认"）。 */
  quitStopsServicesStated: boolean
}

function asServices(value: unknown): LocalServicesReadout | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  if (typeof raw['worldRunning'] !== 'boolean') return null
  if (typeof raw['residents'] !== 'number' || typeof raw['residentsRunning'] !== 'number') return null
  if (typeof raw['quitStopsServices'] !== 'boolean') return null
  return {
    worldRunning: raw['worldRunning'],
    residents: raw['residents'],
    residentsRunning: raw['residentsRunning'],
    // 老家那一格：读不懂**不许**当"不是"（那是把缺省说成事实）——与 `stale` 同一条纪律。
    legacyHome: raw['legacyHome'] === true,
    summary: typeof raw['summary'] === 'string' ? raw['summary'] : '',
    quitStopsServices: raw['quitStopsServices'],
    // 读不懂 ⇒ **当成没表态**（页面于是说"默认"，而不是替它宣称用户选过）。
    quitStopsServicesStated: raw['quitStopsServicesStated'] === true,
  }
}

/** 读本机服务读数（没壳 / 壳没答 ⇒ `null`）。 */
export async function readLocalServices(
  invoke: ShellInvoke | null,
): Promise<LocalServicesReadout | null> {
  if (!invoke) return null
  try {
    return asServices(await invoke('local_services_readout'))
  } catch {
    return null
  }
}

/** **全部停止**（§8.3 的总开关）：世界与住户一起停，读数原样回来（谁停了、谁没动）。 */
export async function stopLocalServices(
  invoke: ShellInvoke | null,
): Promise<LocalServicesReadout | null> {
  if (!invoke) return null
  try {
    return asServices(await invoke('local_services_stop'))
  } catch {
    return null
  }
}

/** 换「退出时」怎么办（§8.3：`[保持运行 ▾]` 那一个开关）。 */
export async function setQuitStopsServices(
  invoke: ShellInvoke | null,
  stopsServices: boolean,
): Promise<LocalServicesReadout | null> {
  if (!invoke) return null
  try {
    return asServices(await invoke('local_quit_behaviour_set', { stopsServices }))
  } catch {
    return null
  }
}

// ---------------------------------------------------------------------------
// 那一格该怎么说话（纯函数：判据在这里，组件只渲染 —— 与 `agentRowStatus` 同一条先例）
// ---------------------------------------------------------------------------

/** 本机服务面板上的一行。 */
export interface LocalServiceRow {
  label: string
  value: string
  /** `on` = 在跑；`off` = 没在跑；`muted` = 与"跑没跑"无关的读数。 */
  tone: 'on' | 'off' | 'muted'
}

/**
 * 「退出时」那一格该写什么（**三态**：停止 / 保持运行 / 还没表态 ⇒ 停止（默认））。
 *
 * §8.3 那句「**默认值：不替用户做隐蔽的事**」落在这一格上：没表过态时，
 * 页面必须写出**这是默认**（`quitStopsServicesStated === false`），
 * 不许把它说成"你选过"。
 */
export function quitBehaviourLabel(readout: LocalServicesReadout | null): string {
  if (readout === null) return '退出时：读不到'
  const behaviour = readout.quitStopsServices ? '停止本机服务' : '保持运行'
  return readout.quitStopsServicesStated ? `退出时：${behaviour}` : `退出时：${behaviour}（默认）`
}

/**
 * ★ **「本机世界」那一格该怎么写**（2026-10-08：把"问一次"的答案做成**可以再改**的开关）。
 *
 * 三态，与 [`quitBehaviourLabel`] 同一条纪律：**还没表态不许说成"你选过"**。
 *
 * ⚠ 它说的是**表态**（`runLocalWorld`），不是**状态**（世界此刻在不在跑）—— 两者必须分开：
 * 一个人可以"开着本机世界"而此刻它没在跑（刚被「全部停止」停过、或起失败了）。上面那两行
 * 读数说的是状态，这一行说的是表态。
 *
 * ⚠ 读不到（`null`）**不许**落进"还没表态"那一档：缺省不等于零，也不等于没答过。
 */
export function localWorldChoiceLabel(readout: LocalWorldReadout | null): string {
  if (readout === null) return '本机世界：读不到'
  if (readout.runLocalWorld === true) return '本机世界：开着'
  if (readout.runLocalWorld === false) return '本机世界：不跑（打开时直接去连别的世界）'
  return '本机世界：还没表态（下次打开时问你一次）'
}

/** 面板上的几行读数（世界 / 住户）＋ 那一格该说什么。 */
export function localServiceRows(readout: LocalServicesReadout | null): LocalServiceRow[] {
  // ⚠ **这一格只可能在 app 里渲染**（面板挂在壳上，浏览器形态整格不渲染 —— 见
  // `components/celira/LocalServices.tsx`）⇒ 走到这里 `readout === null` 的意思是
  // **这一次没从壳里读到**（命令抛了，或形状不认识），**不是"这一格只有 app 里才有"**：
  // 那句话只会在 app 里出现，等于把"读失败"说成了"形态不给"
  // （"把某个失败说成另一个失败"是明文不许的；2026-10-11 改）。
  if (readout === null) {
    return [{ label: '本机服务', value: '读不到（这次没从壳里读到）', tone: 'muted' }]
  }
  return [
    {
      label: '世界',
      value: readout.worldRunning ? '在跑' : '没在跑',
      tone: readout.worldRunning ? 'on' : 'off',
    },
    {
      label: 'Agent',
      // 住户数是"住着几支"；跑着几支单独说 —— 两者不同时那句话才有信息量。
      // ⚠ **老形态 ≠ 没有 Agent**（2026-10-05）：壳只认新布局，而老家是 `up.js` 搬的
      // （app 不做迁移）⇒ 认不出就说认不出 —— 同一屏上面那张卡已经这么说了。
      // 判据的**次序**：真有住户就先说住户（那是主事实）；一支都没有、而家是老形态，
      // 才说"老形态"；两样都不是，才是"还没有 Agent"。
      value:
        readout.residents > 0
          ? `${readout.residentsRunning} / ${readout.residents} 支在跑`
          : readout.legacyHome
            ? '这个家还是老形态（住户化之前）'
            : '这台机器上还没有 Agent',
      // 老形态那一格是"认不出"，不是"没在跑" ⇒ 灰（`muted`），不给 `off` 那个"确实没有"的灯。
      tone:
        readout.residentsRunning > 0
          ? 'on'
          : readout.residents === 0 && readout.legacyHome
            ? 'muted'
            : 'off',
    },
  ]
}

/**
 * 「全部停止」那颗按钮现在该不该可按。
 *
 * 判据：**有东西在跑才可按** —— 什么都没跑时给一颗可按的"全部停止"，用户按下去只会
 * 得到"本来就没跑"，那是拿一个空动作骗一次点击。
 */
export function canStopLocalServices(readout: LocalServicesReadout | null): boolean {
  if (readout === null) return false
  return readout.worldRunning || readout.residentsRunning > 0
}
