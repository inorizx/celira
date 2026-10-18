/**
 * 「它住在哪台机器上」—— Agent 主页（`/app/agents/:participantId`）第二段的**纯逻辑**。
 *
 * ## 这一段为什么存在（2026-09-29）
 *
 * 那一页原先是个**死胡同**：全部出口只有「返回我的 Agent」与「撤销 Agent」。而
 * 「我的 Agent」列表每一行上摆着「状态 / 模型 / 最近活动」三格读数，点进去**一个字都不说**
 * 它在哪台机器上 —— 用户要问"它现在在跑吗"，得退回去、自己想起来去「我的执行端」、
 * 再从机器那一头找。而机器页开口就说「上面跑的是 X」：**答案一直在，只是只从另一端说**。
 *
 * ## 主语是机器，不是 Agent
 *
 * 这一段说的是**机器那一侧的事实**（哪台机器在跑它），来路是家的读数；
 * 上半段「资料」说的是**世界侧的公共事实**（core）。两段**来源不同、权威不同**，
 * 所以本段守两条纪律：
 *
 * 1. **不复述运行状态**。它在不在跑、醒过几次、用哪条模型，**唯一权威**是机器页
 *    （`/app/runtimes/:entryId`）那三个房间。在这里再算一遍，就是**第二个答案** ——
 *    `homes.ts` 的 `mindReadoutFor` 头上记着这条教训（两个调用点各拼一遍 = 两个答案，
 *    而且"两条都在跑，门禁全绿"）。所以这一段只回答**住哪台** ＋ 给一个出口。
 * 2. **不把机器读数写成 Agent 在世界里的属性**。判据就是 `AGENTS.md`「主体进行态边界」
 *    那句话：**把 im 服务端换成微信，这一段还成立吗？** 不成立 ⇒ 它只能这么说
 *    （"哪台机器在跑它"是**机器的属性**），不能长成"这个 Agent 在线/在忙"。
 *
 * ## 归属裁决不在这里
 *
 * 「哪台机器报的就是这个 Agent」由 `homes.ts` 的 `resolveAgentHome` **一处**裁决
 * （claimed / unknown-id / elsewhere / unreachable）。本模块**只把那四态翻成人话**，
 * 不重新判一次 —— 重判就是复制执行链，两个答案迟早打架。
 *
 * 纯函数、零副作用、零运行期依赖（只 `import type`）⇒ `node --test` 直接钉住
 * （`tests/agent-machine.test.ts`）。
 *
 * ⚠ **本模块有两件事**（都不是"再判一次归属"）：① 把那四态**翻成人话**（`agentMachineReadout`）；
 * ② 说**"它用哪一条模型"这一笔写该发给哪台机器**（`modelChoiceMachine`）—— 后者只回答"发给谁"，
 * 认领裁决照旧是 `homes.ts` 那一处（`claimed` 时用它的结论）。
 */

import type { HomeStatusView } from './homeStatus.ts'
import type { AgentHomeResolution, HomeEntry } from './homes.ts'

/**
 * 这一段此刻处在哪一档。**四态不许压成两态**（读不到 ≠ 没有机器在跑它），
 * 页面按它决定**出口**（`target` 有没有）与**颜色**（语义在页面那一层才映射成令牌类）。
 *
 * - `reading`：家条目还在读（还没有裁决）；
 * - `claimed`：某台机器报的就是这个 Agent ⇒ 说得出住址；
 * - `no-id`：有读数、但那台**没报上上面跑的是谁** ⇒ **说不准**，不许说成"未接入"；
 * - `elsewhere`：读到的机器报的都是**别人**的 id ⇒ 「**能读到的执行端都没有报它在跑**」
 *   （这是事实，不是错误）；
 * - `unreachable`：一份都没读回来 ⇒ 「读不到」，把原因原样交出去。
 *
 * ⚠ **`elsewhere` 那一句必须带作用域**（2026-10-11）：它曾写作「还没有机器在跑它。」——
 * 一句**不带作用域**的断言。而这一档实际知道的事情只有"**我这边读得到的机器**都没报它"
 * （判据在 `homes.ts`：`readings.some(view !== null)` 且无一匹配 ⇒ 另有条目可能**没读到**）。
 * 边界是**两个可替换的平面**（世界侧会有别的执行端、执行端会连别的世界）⇒ 把"我没看见"
 * 说成"没有"，在别的通道在场时就是**假话**。判据一句话：**它是"我没看见"还是"没有"？**
 * 同族的正例是机器列表行那句（`agentRowStatus.ts`）：「已连上 · 上面那支 Agent
 * **不在这个世界里**」—— 带作用域。
 */
export type AgentMachineKind = 'reading' | 'claimed' | 'no-id' | 'elsewhere' | 'unreachable'

export interface AgentMachineReadout {
  kind: AgentMachineKind
  /** 这一段的正文（**主语是机器**；页面直接渲染，不再拼词）。 */
  text: string
  /** 能去的那台机器；`null` = 说不准是哪台，页面只给"去看执行端"的出口。 */
  target: { entryId: string; label: string } | null
}

/**
 * 条目在句子里的写法：**本机那条不写它的名字**。
 * 它的 `name` 字面量就是「本机」（`homes.ts` 的 `localHomeEntry`），
 * 拼进句子会变成「它住在本机本机。」这类重复；这里只认 `local` 那一格。
 */
function entryLabel(entry: { local: boolean; name: string }): string {
  return entry.local ? '本机' : entry.name
}

/**
 * 四态 → 一段人话 ＋ 一个出口。
 *
 * ⚠ `elsewhere` 那一档**刻意不给安慰句**（如"去执行端把 Agent 接上"）：v1 **没有**
 * 给一个已存在的 Agent 安家的路径（`client/README.md`「已知缺口」），写了就是**假路径**
 * —— `scripts/verify-ui.mjs` 的 `BANNED_FALSE_PATH` 正是治这个的。
 */
export function agentMachineReadout(input: {
  /** 归属裁决；`null` = 家条目还没就绪（还在读）。 */
  resolution: AgentHomeResolution<HomeStatusView> | null
}): AgentMachineReadout {
  const { resolution } = input
  if (resolution === null) {
    return { kind: 'reading', text: '正在读…', target: null }
  }
  if (resolution.kind === 'claimed') {
    const label = entryLabel(resolution.entry)
    return {
      kind: 'claimed',
      text: `它住在${label}。`,
      target: { entryId: resolution.entry.id, label },
    }
  }
  if (resolution.kind === 'unknown-id') {
    const label = entryLabel(resolution.entry)
    return {
      kind: 'no-id',
      text: `${label}没报上上面跑的是谁，说不准它住不住在那台。`,
      target: { entryId: resolution.entry.id, label },
    }
  }
  if (resolution.kind === 'elsewhere') {
    // **作用域写进句子里**（见文件头那条）：这一档知道的是"读得到的机器都没报它"，
    // 不是"没有机器在跑它" —— 少一个字就从"我没看见"变成"没有"。
    return { kind: 'elsewhere', text: '能读到的执行端都没有报它在跑。', target: null }
  }
  // unreachable：原因（`homeFailureText` 给的固定人话）原样带上，**不吞** ——
  // 吞掉它就等于把"读不到"说成一个事实。
  return {
    kind: 'unreachable',
    text: resolution.failure
      ? `读不到它住在哪台机器上 —— ${resolution.failure}。`
      : '读不到它住在哪台机器上。',
    target: null,
  }
}

/**
 * **这一支该不该给"接到这台机器上"那个出口**（2026-10-09 主人裁 B）。
 *
 * ## 判据一句话
 *
 * **本机没认领它 ⇒ 给出口。** 四态里唯一"已经有家"的是 `claimed` ⇒ 只有它不给；
 * `unreachable`（一份都没读回来）与"还没读回来"（`null`）也**不给** —— 读不到的时候给一个
 * 会动手的按钮，等于让用户替我们赌一把。
 *
 * ⚠ **`unknown-id` 也给**（2026-10-09 实测改正：这一格我第一版读错了 ✗）：它的意思是
 * **有读数、但都没有 id 那一格**（`resolveAgentHome` 原文：「照旧显示，**不许说成"未接入"**」）
 * —— 也就是"这台机器**没报**上面跑的是谁"，最常见的样子就是**一台还没有住户的机器**。
 * 第一版只认 `elsewhere`（"报的都是别人的 id"）⇒ 于是"机器上没人"这一格反而**没有出口** ✗，
 * 而它恰恰是主人今天撞上的那一格（`agent.state = no-resident`）。**用例当时还把这个错信念
 * 钉住了**（它跟着我错 ⇒ 绿的）—— 这就是"先读原文，再写判据"的现场。
 *
 * ⚠ 给错的**代价是有界的**：若这台机器其实已经跑着它（老门不报 id 那种），壳那六步会说
 * `already-homed`（「这台机器上已经有它的家——没有重复动」）⇒ 只是白点一下 ＋ 一句如实的话，
 * 不会重复动任何东西。⇒ 判据取"**认领的否定**"，不取"猜机器里有没有人"。
 *
 * ## 判据翻转的记账（原约束 → 新边界 → 真实消费者）
 *
 * - **原约束**：v1 **没有**给一个已存在的 Agent 安家的路径 ⇒ 本文件上一版写着"`elsewhere`
 *   那一档**刻意不给安慰句**（如'去执行端把 Agent 接上'）：写了就是**假路径**"
 *   （`client/README.md`「已知缺口」＋ `scripts/verify-ui.mjs` 的 `BANNED_FALSE_PATH`）。
 * - **新边界**（2026-10-09 主人裁 **B**）：**身份 ↔ 执行端的关系是一等事实**（协议里就是
 *   Backend Binding ＋ Credential），"什么时候建立它"不该写死在"创建身份"那个流程里 ⇒
 *   **执行端可以随时认领一个已经存在的身份**。壳里那六步（`anjia_start`）与命令行
 *   （`home.js up --agent`）**本来就有这条路**，这一刀只是把**出口**给到页面上。
 * - **真实消费者**：`AgentsListPanel` 那一行 —— 创建时缺模型源而被拒的那一支，从此有救。
 *
 * ⚠ 屏上措辞用**现成的界面语言**「接到这台机器上」（`anjia.ts` 文件头那条）：不许出现设计词
 * 「安家」—— `BANNED_FALSE_PATH` 里那条**照旧**拦它（它拦的是那个词，不是这件事）。
 */
export function shouldOfferAttach(
  resolution: AgentHomeResolution<HomeStatusView> | null,
): boolean {
  if (resolution === null) return false
  return resolution.kind === 'unknown-id' || resolution.kind === 'elsewhere'
}

/**
 * ★ **"它用哪一条模型"这件事该落在哪台机器上**（2026-10-14「选择先于接上」那一刀）。
 *
 * ## 判据（三档，一个都不压）
 *
 * | 它此刻的处境 | 落点 | 为什么 |
 * |---|---|---|
 * | 某台机器报的就是它（`claimed`） | **那一台** | 它确实住在那儿（旧口径，一字未改） |
 * | 还没读回来（`resolution === null`） | `null` | 还没裁决，摆一个选择器就是让人对着空气点 |
 * | 别的（`elsewhere` / `unknown-id` / `unreachable`） | **本机那一条**（＋ 它的配置门读得到时） | 它还没被接到任何机器上，而"接上"这件事在 app 里只指**本机**（壳那六步）—— 所以"先给它选好"就是在这台机器上选 |
 *
 * ⚠ **为什么要求"本机那条的配置门读得到"**：选择器要列的是**那台机器上配好的钥匙**
 * （`entries`）。读不到清单还摆一个空壳选择器，等于让人对着一件读不出来的东西做决定
 * （页面宁可如实说"这一格读不到"）。
 *
 * ⚠ **这不是"张冠李戴"**：`claimed` 之外，本函数**不读任何机器的读数**（不拿别人的状态或
 * 账本顶给它）—— 它只回答"这一笔写该发给谁"，而那一笔写的主语是**用户自己点的那一下**。
 * 机器侧那句"能读到的执行端都没有报它在跑"照旧由 `agentMachineReadout` 说（那一格没变）。
 */
export function modelChoiceMachine(input: {
  /** 归属裁决；`null` = 家条目还没就绪（还在读）。 */
  resolution: AgentHomeResolution<HomeStatusView> | null
  /** 本机那一条 ＋ 它的配置门这一轮读到了没有（`null` = 没有这一条 / 这一轮没读到）。 */
  local: { entry: HomeEntry; configReadable: boolean } | null
}): { entry: HomeEntry; homed: boolean } | null {
  const { resolution, local } = input
  if (resolution === null) return null
  if (resolution.kind === 'claimed') return { entry: resolution.entry, homed: true }
  if (local === null || !local.configReadable) return null
  return { entry: local.entry, homed: false }
}
