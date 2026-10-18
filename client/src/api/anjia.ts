/**
 * **「把它接到这台机器上」的壳侧接线**（首刀步骤 2，2026-09-30）。
 *
 * 界面语言里这件事**不叫**「安家」（那是设计词汇，不上屏——`plain-language.ts` 的
 * 两套词典不许串门）；用户看到的说法是**「接到这台机器上」**。
 *
 * ## 两个判据，别混
 *
 * - **有没有壳**：`isTauriRuntime`（承 `shell.ts`：判**内部注入**在不在，不判平台名、
 *   不判构建变量）；
 * - **有没有这个能力**：`invoke` 这个方法**真的在**（动态 import 成功才算）——
 *   与 `uiPolicy.ts` 的能力探测同一律（按方法探，不按模式猜）。web 侧**安静缺席**，
 *   构建不受污染（动态 import）。
 *
 * ## 人话与四态在本模块；壳那边只有词表
 *
 * 壳（`app/src-tauri/src/anjia/commands.rs`）回的是**读数**：`kind` 稳定词 ＋ pid/路径。
 * 把它翻成**四态**（进行中 / 成功 / 失败 / 读不到）与**人话**是这里的事
 * （`anjiaReadout`，纯函数，`node --test` 直钉）。壳那边再写一套话就是**两个答案**。
 *
 * ## 词表（封闭；未知取值按「读不到」fail-closed）
 *
 * 壳的十态：`homed` / `homed-not-started` / `already-homed` / `in-flight` / `world-unreadable` /
 * `not-owned` / `not-active` / `refused` / `failed` / `started-no-report`；
 * 本模块另加**两个传输层自己的态**：`bridge-missing`（没有壳，或壳里没这个命令）、
 * `bridge-error`（调用本身失败）。后两者都归**「读不到」**——**读不到 ≠ 没接上**。
 *
 * ## `refused` 的子词（2026-10-02 刀：拒绝说得清）
 *
 * `refused` 一个词压住**七个来路 ＋ 空 id**（仓根找不到 / 缺可执行件 / 家没布置 /
 * id 形状 / 绑定读不懂 / 缺模型凭据 / 凭据读不懂 / 空 id），页面只能说"还缺一样东西"——
 * 用户没法据此做下一步。壳现在随 `kind=refused` 多回一个**封闭子词** `sub`
 * （`anjia::RefusalReason::sub()`），这里按子词给人话 ＋ 具体下一步；**未知子词
 * 收敛成 `null`，页面按整词 `refused` 的通用句回退**（fail-closed，不瞎猜）。
 */

import { isTauriRuntime } from './shell.ts'

/** 壳侧词表（九态）＋ 本模块自己的两态。 */
export type AnjiaKind =
  // ── 壳侧九态（`kind` 逐字对应 `anjia::commands` 的稳定词） ──
  | 'homed'
  | 'homed-not-started'
  | 'already-homed'
  | 'in-flight'
  | 'world-unreadable'
  | 'not-owned'
  | 'not-active'
  | 'refused'
  | 'failed'
  | 'started-no-report'
  // ── 本模块自己的两态（传输层） ──
  | 'bridge-missing'
  | 'bridge-error'

/** 页面自己的「还在跑」——调用已发出、壳还没回话（**不是**壳的态）。 */
export type AnjiaUiKind = AnjiaKind | 'running'

/** 壳回的那份读数（字段名照 camelCase；`lines` 是壳的原样读数，**只作日志，不上屏**）。 */
export interface AnjiaView {
  agentId: string
  kind: string
  /** `refused` 的封闭子词（别的结局没有）；壳那边是 `anjia::RefusalReason::sub()`。 */
  sub?: string | null
  pid: number | null
  envRoot: string | null
  logFile: string | null
  lines: string[]
}

/** 本模块内部的形状：词表已经收敛过（未知取值 → `bridge-error`；未知子词 → `null`）。 */
export interface AnjiaResult {
  agentId: string
  /** 壳回的态；页面自己那态 `running`（调用已发出、壳还没回话）也落在这里。 */
  kind: AnjiaUiKind
  /** `refused` 的封闭子词（未知子词收敛为 `null`——页面按整词回退，不瞎猜）。 */
  sub: RefusedSub | null
  /** 起了进程才有；**只是报告**（壳不做 PID 认领）。 */
  pid: number | null
  envRoot: string | null
  logFile: string | null
}

/**
 * **`refused` 的封闭子词表**（逐字对应壳侧 `anjia::RefusalReason::sub()`）。
 *
 * 只加不换：换掉一个词＝让老壳的老词变成"未知"（页面只好回退整句），那是**丢信息**。
 * 改壳那边的子词就要改这里 —— 用例会把两边逐字对账。
 */
export const REFUSED_SUBS = [
  'missing-repo',
  'missing-executable',
  'home-not-set-up',
  'bad-id-shape',
  'binding-unreadable',
  'missing-model-credential',
  /**
   * **这台机器上有多条模型来源，而这一支还没选用哪一条**（2026-10-09 主人裁：
   * 选哪一条 = 花谁的钱 ⇒ 不替它决定；只有"其实没有选择"时才自动绑）。
   */
  'model-not-chosen',
  'credential-unreadable',
  'empty-id',
] as const

export type RefusedSub = (typeof REFUSED_SUBS)[number]

/** 把壳回的 `sub` 收敛到封闭词表：不认识的取值 → `null`（页面按整词 `refused` 回退）。 */
export function narrowRefusedSub(raw: unknown): RefusedSub | null {
  return REFUSED_SUBS.find((candidate) => candidate === raw) ?? null
}

/** 四态（施工单 §三 前端：进行中 / 成功 / 失败 / 读不到）。 */
export type AnjiaState = 'working' | 'done' | 'failed' | 'unreadable'

export interface AnjiaReadout {
  state: AnjiaState
  /** 颜色档（语义在页面那一层才映射成令牌类，与 `agentMachine.ts` 同规矩）。 */
  tone: 'ok' | 'warn' | 'bad' | 'muted'
  /** 人话（页面直接渲染，不再拼词）。 */
  text: string
}

/** 词表里**壳侧**那十态（用例拿它对账；改壳那边的词表就要改这里）。 */
export const SHELL_KINDS: readonly AnjiaKind[] = [
  'homed',
  // ★ 2026-10-15（主人裁：接上 ≠ 选模型）：接上了、但这一支还没选模型 ⇒ **先没起它**。
  //   第三种结局：不是失败（家建好了），也不是"跑起来了"。
  'homed-not-started',
  'already-homed',
  'in-flight',
  'world-unreadable',
  'not-owned',
  'not-active',
  'refused',
  'failed',
  'started-no-report',
]

/** 把壳那串自由文本的 `kind` 收敛到封闭词表：不认识的取值**不许当成功**（返回 null）。 */
export function narrowKind(raw: unknown): AnjiaKind | null {
  const all: readonly AnjiaKind[] = [...SHELL_KINDS, 'bridge-missing', 'bridge-error']
  return all.find((candidate) => candidate === raw) ?? null
}

/**
 * **四态 ＋ 人话**（纯函数）。判据三条：
 *
 * 1. **读不到 ≠ 失败，也 ≠ 没接上**：连不上世界、壳没回话，都只说"读不到"；
 * 2. **没结论不许说成结论**：`started-no-report` 是第三态（起了、没等到报到）——
 *    它留在「进行中」并**明说没等到**，不冒充成功，也不冒充失败；
 * 3. **拒绝说清"什么都没动"**：四条拒绝之路都一个字没改（判据 1/2/3 的护栏），
 *    页面必须把这句说出来——用户才知道重试是安全的。
 *
 * `refused` 再按**封闭子词**（`sub`）说得更细：每个子词一句人话 ＋ 一个具体下一步；
 * 未知 / 缺席的子词走**通用句**（与从前一字不差），绝不瞎猜是哪一种。
 */
export function anjiaReadout(kind: AnjiaUiKind, sub?: string | null): AnjiaReadout {
  switch (kind) {
    case 'running':
      return { state: 'working', tone: 'muted', text: '正在把它接到这台机器上…' }
    case 'in-flight':
      return {
        state: 'working',
        tone: 'warn',
        text: '正在接它（上一轮还没跑完）——这次没有重开。',
      }
    case 'homed':
      return { state: 'done', tone: 'ok', text: '已经在它这台机器上接好了，进程也起来了。' }
    case 'homed-not-started':
      // ★ 2026-10-15：**接上与选模型是两件事**（主人裁）——接上了，但还没选模型 ⇒ 先没起它。
      //   这一档既不许说成失败（家真的建好了），也不许说成成功（它还没跑起来）。
      return {
        state: 'done',
        tone: 'warn',
        text: '接上了 —— 但它还没选模型，所以先没起它。去它自己的页面上选一条，选好之后下次打开这个 app 就会把它带起来。',
      }
    case 'already-homed':
      return { state: 'done', tone: 'ok', text: '这台机器上已经有它的家——没有重复动。' }
    case 'started-no-report':
      return {
        state: 'working',
        tone: 'warn',
        text: '进程起来了，但没等到它报到——它可能还在起；稍后去那台机器上看最新读数。',
      }
    case 'world-unreadable':
      return {
        state: 'unreadable',
        tone: 'bad',
        text: '读不到世界，什么都没动——先确认世界在跑、地址对不对。',
      }
    case 'not-owned':
      return {
        state: 'failed',
        tone: 'bad',
        text: '世界里没有这个 Agent，或者它不是你的——这次什么都没动。',
      }
    case 'not-active':
      return {
        state: 'failed',
        tone: 'bad',
        text: '这个 Agent 已经撤销了，不能再接到机器上——这次什么都没动。',
      }
    case 'refused': {
      const known = narrowRefusedSub(sub)
      return {
        state: 'failed',
        tone: 'warn',
        text: known
          ? REFUSED_TEXT[known]
          : /* 未知 / 缺席的子词：**通用句**（从前那一句，一字不改）——不瞎猜是哪一种。 */
            '这台机器上还缺一样东西才能接上它——这次什么都没动。',
      }
    }
    case 'failed':
      return {
        state: 'failed',
        tone: 'bad',
        text: '接它的过程中出错了，没接成——过程的原样读数留在壳那边。',
      }
    case 'bridge-missing':
      return {
        state: 'unreadable',
        tone: 'muted',
        text: '这一版页面没有壳：它接不上机器上的进程——到桌面版里做这件事。',
      }
    case 'bridge-error':
      return {
        state: 'unreadable',
        tone: 'bad',
        text: '壳没有回话（这一问失败了）——重试一次；还不行就看壳那边的读数。',
      }
  }
}

/**
 * `refused` 每个子词的人话 ＋ **具体下一步**（人话唯一权威位置——壳那边不写第二套话）。
 *
 * 措辞纪律（与 `verify-ui.mjs` / `jargon.mjs` 同一批判据）：
 * - 不许出现设计词汇「安家」「入籍」，不许出现假路径「设置 → 模型配置」
 *   （本行渲染在「我的 Agent」列表页——那一页上它是被点名的假路径）；
 * - 不许出现内行词（`读数` / `Runtime` / `Core` 等，`scripts/jargon.mjs` 是唯一词表）。
 *
 * ⚠ 子词 → 下一步只写**真能走的路**：今天「模型」房间（家面）在壳里**已经通了**
 * （2026-10-05 收口；`verify-ui:app` 的「机器页三间」那一条验的就是它），而且 2026-10-08 起
 * 「我的 Agent」列表里 `missing-model-credential` 那一句**自带出口**、直接指过去
 * （`AgentsListPanel` 的 `ModelSourceExit`）——所以这里说"配好模型源"这个**事实**，
 * 由调用方给它一个真出口；**不指**「设置 → 模型配置」那种假路径（指错门＝与假路径同族的病）。
 */
const REFUSED_TEXT: Record<RefusedSub, string> = {
  'missing-repo':
    '这台机器上找不到装好的那套程序（它的位置没认出来）——先把它装好或指一下位置，再回来接它。这次什么都没动。',
  'missing-executable':
    '这台机器上还缺跑它要用的程序文件——先把这套程序装齐，再回来接它。这次什么都没动。',
  'home-not-set-up':
    '这台机器上的家还没布置过——先在这台机器上布置一次，再回来接它。这次什么都没动。',
  'bad-id-shape':
    '这个 Agent 的编号形状不对（空、带空格或太长）——回「我的 Agent」列表里重新打开它，再试一次。这次什么都没动。',
  'binding-unreadable':
    '这台机器上给这一支记的模型指定读不懂——先在这台机器上重新指定它用哪条模型源，再回来接它。这次什么都没动。',
  'missing-model-credential':
    '这台机器上还缺模型凭据（模型那一半还没配）——先在这台机器上配好模型源，再回来接它。这次什么都没动。',
  'model-not-chosen':
    '这台机器上有好几条模型源，而这一支还没选用哪一条——选哪一条就是花谁的钱，所以不替它定。选好之后再回来接它。这次什么都没动。',
  'credential-unreadable':
    '这台机器上它的凭据文件读不懂（可能坏了，或者是别人的）——先看清那份文件，再重试。这次什么都没动。',
  'empty-id':
    '这一问没带上 Agent 的编号——回「我的 Agent」列表里重新打开它，再试一次。这次什么都没动。',
}

/** 壳在不在、这个命令在不在（**两个判据合起来**才算有桥）。 */
export function hasAnjiaBridge(
  scope: Record<string, unknown> = globalThis as unknown as Record<string, unknown>,
): boolean {
  return isTauriRuntime(scope)
}

/**
 * 叫壳接一次。
 *
 * 三种回法，**一种都不许假装成功**：
 * - 有桥：调 `anjia_start`，壳回的读数按封闭词表收敛；
 * - **没有桥**（浏览器）：回 `bridge-missing`（页面据此说"到桌面版里做"）；
 * - 调用失败（命令不在 / 壳没回话）：回 `bridge-error`。
 */
export async function startAnjia(agentId: string): Promise<AnjiaResult> {
  if (!hasAnjiaBridge()) {
    return { agentId, kind: 'bridge-missing', sub: null, pid: null, envRoot: null, logFile: null }
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const view = await invoke<AnjiaView>('anjia_start', { agentId })
    return {
      agentId,
      kind: narrowKind(view?.kind) ?? 'bridge-error',
      // 子词只在 `refused` 上有；未知取值收敛成 null（页面按整词回退）。
      sub: narrowRefusedSub(view?.sub),
      pid: typeof view?.pid === 'number' ? view.pid : null,
      envRoot: typeof view?.envRoot === 'string' ? view.envRoot : null,
      logFile: typeof view?.logFile === 'string' ? view.logFile : null,
    }
  } catch (error) {
    // 响亮地说：桥在、但这问没成（命令没注册 / 壳内部出错）。绝不吞。
    console.error('Celira：接它的那一问没能完成', error)
    return { agentId, kind: 'bridge-error', sub: null, pid: null, envRoot: null, logFile: null }
  }
}
