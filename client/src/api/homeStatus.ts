/**
 * 本机状态口的读取层（TASK_55；TASK_61 起走同源代转；**家口远程访问刀起按家条目分流**）。
 *
 * 状态口是**家的门**（`home.js doors`）的**读端面** —— 只读小服务（家在跑时才在）；
 * 这里只负责"取回来 + 形状守卫"，不负责呈现。
 *
 * 四条纪律：
 * - **形状守卫 fail-closed**：认不出的响应一律当"读不到"，绝不半读半猜
 *   （读错一份状态比读不到更坏：页面会拿它当事实画给你看）。
 * - **地址按条目算，页面不知道端口**：本机条目问**同源**的 `/home-status/*`
 *   （**两种形态都是** `scripts/serve_client.mjs` 代转；
 *   改 `status_port`，页面自己跟着走）；远程条目问**绝对地址 + Bearer 家钥匙**
 *   （`homes.ts` 的 `statusRequestUrl` / `homeRequestHeaders` —— 分流只在那里发生）。
 * - **同源之后不需要 CORS**（本机那条路）：口子为此专门长过一层响应头白名单，那一层已随
 *   TASK_61 删掉；远程那条路反过来**要靠 CORS**（模式二开放它，见 `scripts/home/home-key.js`）。
 * - **失败是结构化分类**（家口远程访问刀）：不抛一句 `new Error(...)` 就完事 ——
 *   `HomeRequestError` 把"钥匙不对 / 连不上 / 答的话读不懂"分开，固定人话在 `homes.ts`。
 */

import {
  HomeRequestError,
  homeRequestHeaders,
  localHomeEntry,
  statusRequestUrl,
  type HomeEntry,
} from './homes.ts'
import { sendRequest } from './transport.ts'

/**
 * 状态口的**同源相对基址**。
 *
 * 为什么要一个可读的路径段而不是直接 `/status`：口令式的短路径会与客户端路由抢位
 * （`/status` 这种名字迟早有人想用），而 `/home-status` 一眼能看出它属于"家"这一层，
 * 与 `/v1`（im 服务端）并列，两条互不覆盖。
 */
export const HOME_STATUS_URL = '/home-status'

/**
 * 一次状态读数的超时。
 *
 * **不是"本机就应该很快"**：状态口每次都要 spawn `agent_host_status`（读整个环境账本）
 * 再探一次 im 健康，实测约 **3.7 秒**。超时给 3 秒会把**正常**读数掐掉，页面于是显示
 * "口子未开"——一个由超时造成的假故障（本刀实测踩过）。
 */
const STATUS_TIMEOUT_MS = 12000

/**
 * Agent 在**这台机器上**的处境（封闭词表）。
 *
 * ⚠ **`no-resident` 与 `missing-binary` 是两件事**（2026-10-05）：前者是"这个家里还没有
 * Agent 住进来"（**缺省**，机器好好的），后者是"程序缺件"（**故障**）。从前两者共用
 * `missing-binary` ⇒ 刚建好世界的新用户在「运行」那一格读到"本机缺少必要的程序"，
 * 那是一句**假警报**（真机撞到）。判据：**缺省不等于故障**。
 */
export type AgentState = 'loaded' | 'busy' | 'no-resident' | 'missing-binary' | 'unreadable'

/**
 * **宿主进程那一格**（2026-10-06 补进契约）。
 *
 * ⚠️ 它**一直在口子的响应里**（`scripts/home/status-server.js` 的 `host`），而客户端契约
 * 从来没声明过它 ⇒ 这一格此前被**整格丢掉**（`isHomeStatusView` 是"多一个字段不管"，
 * 于是它连"读到了但没人看"都算不上：它根本没进过页面）。
 *
 * **它是机器侧的事实**：那支 Agent 的宿主进程此刻在不在跑。它与世界侧的「在线」
 * （`GET /v1/rooms/{id}/live` 的连接派生投影）**不是同一件事**，也不许互相冒充：
 * 那一个说"世界这条线还连着"，这一个说"这台机器上那个进程还活着"。
 *
 * 取值是口子给的**稳定词表**：`running` 在跑 ／ `running-orphan` 在跑但没登记在册
 * ／ `stopped` 没在跑 ／ `unknown` 这一格没读数。
 */
export type HostState = 'running' | 'running-orphan' | 'stopped' | 'unknown'

export interface HomeStatusHost {
  /** 口子给的稳定取值；**认不出的取值由 [`hostStateOf`] 读作 `'unknown'`**（不编、也不整份拒绝）。 */
  state: string
  /** 进程号（没在跑时为 `null`）。界面不显示它，留着给"此刻是谁在里面"那条路。 */
  pid?: number | null
  /** 这一支的宿主日志（相对家根）。界面不显示它。 */
  log?: string
}

/**
 * **一支住户**在状态口载荷里的那一份（`residents[]` 的一项）—— 2026-10-08 补进契约。
 *
 * ⚠️ 这一格**一直在口子的响应里**（`status-server.js` 的 `buildResidentView` 逐支建），
 * 而客户端契约从来没声明过它 ⇒ 与 `host` 当初一样**整格丢掉**。丢掉之后页面只看得到
 * **第一支**（`agent` / `lastWake` 是抬到顶层的那一支）⇒ **第 2、3 支的页面被写成
 * 「还没有机器在跑它」——那是假话**（它们就在这台机器上跑着，只是各住一个环境根）。
 *
 * ⚠️ **只声明"认领 + 它在不在这台机器上"要用的那三格**（`agentId` / `slot` / `host`）：
 * 逐支的**账本与经历**（`agent.runs` / `experience`）**壳那个门今天不给**（权威在 JS 门，
 * 壳里抄第二份就是造第二个权威）⇒ 声明了它就会有人读，读了拿到的是**第一支的**。
 * **不许声明**——那正是本文件开头那条纪律要防的事（"缺省不等于零"，反过来"别人的读数"
 * 也不等于这一支的）。
 */
export interface HomeStatusResident {
  /** 这一支的 Agent id（环境根 `identity.json`；读不到 ⇒ `null`）。**认领的判据**。 */
  agentId?: string | null
  /** 住户序号（正整数；老家那一支可能是 `null`）。界面只说"第几支"，不拿它当身份。 */
  slot?: number | null
  /** 这一支**自己的**宿主那一格（逐支判：一支一个进程）。 */
  host?: HomeStatusHost
}

/**
 * 宿主那一格 → 四态（**认不出的取值读作 `'unknown'`**，即"这一格没读数"）。
 *
 * ⚠️ 为什么不照 `agent.state` 那张封闭表**整份拒绝**：那条路已经被真机咬过三次
 * （`host_status_parse.js` 记着"真输出会缺段/带后缀 ⇒ **整份读数**被判读不到，而 Agent
 * 其实好好的"，最坏的时机瞎）。这一格是**新加的可选格**，它读不出来不该把整页拖塌——
 * 「把缺省说成事实」与「整页瞎掉」是两种不同的错，这一格只犯得起前一种的一半。
 */
export function hostStateOf(host: HomeStatusHost | undefined | null): HostState {
  if (!host) return 'unknown'
  if (host.state === 'running' || host.state === 'running-orphan' || host.state === 'stopped') {
    return host.state
  }
  return 'unknown'
}

/**
 * 宿主那一格的形状守卫：**只要求 `state` 是字符串**（取值由 `hostStateOf` 归一）。
 *
 * 它**不是**封闭表校验，理由同上；`pid` / `log` 是可选的（旧版口子可能只给 `state`，
 * 而"少给一格"不该让整份读数读不出来）。
 */
function isHostView(value: unknown): value is HomeStatusHost {
  if (typeof value !== 'object' || value === null) return false
  return typeof (value as Record<string, unknown>)['state'] === 'string'
}

/**
 * 住户清单那一项的形状守卫（**与 `isHostView` 同一条口径**：只要求认得出那几格，
 * 不搞封闭表 —— 多给一格不算坏，"少给一格"也不该让整份读数读不出来）。
 *
 * ⚠️ `agentId` / `slot` 允许 `null`（**"这一支没报 id"是一个读数**，不是形状坏）——
 * 认领那一侧按"精确相等"判，`null` 永远不等于任何 id ✓（不匹配不许说成匹配）。
 */
function isResidentView(value: unknown): value is HomeStatusResident {
  if (typeof value !== 'object' || value === null) return false
  const resident = value as Record<string, unknown>
  const agentId = resident['agentId']
  if (agentId !== undefined && agentId !== null && typeof agentId !== 'string') return false
  const slot = resident['slot']
  if (slot !== undefined && slot !== null && typeof slot !== 'number') return false
  if (resident['host'] !== undefined && !isHostView(resident['host'])) return false
  return true
}

/**
 * **Agent 的经历**这一面的行级形状（TASK_72 ④）。
 *
 * ⚠️ 这是**机械读数**：动作 / 轮次 / 状态 / 顺序号。⚠️ **排除清单与口子那份逐字对齐**
 * （`scripts/home/status-server.js` 的同一段注释）—— 判据是**一个字段进来之前先问
 * "它会不会带正文"，会就不进**：
 *
 * - **会带正文的**：`params`（参数值里就是 Agent 说的话、写的标题）、`blocks`（结果正文）、
 *   `text`（宿主输入正文）、`content` / `payload`（模型原文与推理）、`extra-actions`
 *   （动作清单，可能带参数）；
 * - **不带正文、但也不在白名单里**：`protocol` / `protocol-version`（实测取值
 *   `chat-completions` / `1`）。它们被丢掉是**白名单制的必然结果**（不在名单就不过桥），
 *   不是漏掉 —— 点这两条名，是为了让这份清单**不再看起来像穷举**：凡是白名单外的键
 *   都会丢，这里只点名其中"曾被人问起过"的那些。
 *
 * `EXPERIENCE_ROW_FIELDS` 与口子那份白名单（`scripts/home/status-server.js` 的
 * `EXPERIENCE_FIELDS`）**必须逐字相同** —— 有一条测试专门对表（防两侧漂移）。
 */
export const EXPERIENCE_ROW_FIELDS: readonly string[] = [
  'seq',
  'run',
  'turn',
  'kind',
  'stage',
  'action',
  'index',
  'state',
  'decision',
  'subject',
  'system',
  'ending',
  'failure',
  // 刀2b C②：**派生**字段（trace 里没有这一格）——那一场的起源，由口子从 `run-start` 注入行的
  // `activity-origin` 块机械映射出来（`bell` / `timer` / `manual` / `unknown`）。
  // 它照样过"会不会带正文"那一问：不会（是两三个稳定取值之一）。
  'origin',
]

export interface ExperienceRow {
  kind: string
  [field: string]: string | number | undefined
}

export interface ExperienceView {
  /** `missing` = 这一面这次读不到（**不是**"Agent 没有经历"）。 */
  state: 'loaded' | 'missing' | 'unreadable'
  /** 只有 `loaded` 时有：最近若干条（机械字段）。 */
  rows?: ExperienceRow[]
  /** 窗口是从文件尾取的 ⇒ `windowed` 表示"前面的没读"。 */
  windowed?: boolean
  /** 窗口内有更多条没给。 */
  truncated?: boolean
}


export interface HomeStatusRun {
  id: string
  state: string
  /**
   * 收场结果。⚠ **可以是 `null`**（2026-10-03）：**未收尾的活动没有结果** ——
   * 它停在半路，运行时就还没收场（真读数里 38 条活动有 12 条是这样）。
   * 从前这里写死 `string`（守卫也要求字符串）⇒ 一条未收尾活动会让**整份读数**被判"形状不认识"，
   * 页面于是说"读不到账本"，而 Agent 其实好好的。
   */
  outcome: string | null
  /**
   * 收场带出来的**失败分类**（口子一直在给这一格，界面从来没用过）。
   *
   * ⚠ 这是**客户端类型对齐现实**，不是改形状（`isRun` 本来就是"多一个字段不管"）。
   * 「活动」房间用它显示"哪一场没走完、为什么"——**只看分类，不看正文**。
   */
  failure?: string | null
}

export interface HomeStatusView {
  /** **这次读数的时刻**（口子探测那一刻）。**TTL 缓存命中时不改写它** —— 它就是"何时读的"。 */
  readAt: string
  /**
   * **这一份是不是缓存读数**（TASK_72 ③）：口子有 15 秒短 TTL 与开机预热，
   * `stale: true` 表示"给你的是上一次那一份，刷新正在后台跑"。
   *
   * ⚠️ 它**不**表示"读不到"：读数内容照旧有效，只是**不是这一轮的**。
   * 页面必须把这一点说出来（见 [`readingLabel`]），不许把它画成当下。
   */
  stale: boolean
  /**
   * **本机世界在不在** —— 一个事实：探活问到的那个 HTTP 状态码（`null` ＝ 它没答话）。
   *
   * ⚠ 2026-10-11：这里从前还有一格派生的 `listening`（`health === 200`），页面拿它当主词，
   * 于是「世界活着、只是自报 500」被写成**「没在跑（HTTP 500）」**——自相矛盾的字面。
   * 判据：**有应答（任何码）⇒ 那个口上有人在 ⇒ 它在跑**；`null` ⇒ **它没答话**
   * （读不到，**不等于它停了**）。要说的话只有一处：`reading-label.ts` 的
   * [`localServiceReading`]（两个页面共用），别在页面里各写一遍 if。
   */
  im: { health: number | null }
  agent: {
    state: AgentState
    /**
     * **这是哪个 Agent 的账本**（刀2b C①；可缺省）。
     *
     * 它来自**环境根自己的记录**（`<环境根>/identity.json`），与 Agent 的状态无关 ⇒ Agent 忙、二进制缺、
     * 账本读不出来时**也照给**。有了它，页面才能判「这个 Agent 是否在本机运行」。
     * ⚠ **只有 id，没有名字**：名字在 im 的公共世界，而状态口的进程环境不含凭据（拿不到）。
     */
    agentId?: string
    /** 只有 `loaded` 时有。 */
    admission?: string
    activities?: number | null
    truncated?: boolean
    /** 最近几条（口子只给最近 N 条）；总数单独给。 */
    runs?: HomeStatusRun[]
    runsTotal?: number
    next?: string
    /** `unreadable` 的原因（解析器给的稳定分类）。 */
    reason?: string
    /** `busy` 的说明。 */
    note?: string
  }
  lastWake: string | null
  /**
   * **宿主进程那一格**（可选）：那支 Agent 的宿主进程此刻在不在跑（见 [`HomeStatusHost`]）。
   *
   * ⚠️ **缺这一格 = 这一面没读数**（旧版状态口 / 壳那条路），**不许**读成"没在跑"
   * ——两者在页面上必须长得不一样（判据同 `experience` / `legacyHome`）。
   */
  host?: HomeStatusHost
  /**
   * ★ **这一台机器上住着哪几支**（2026-10-08 补进契约；见 [`HomeStatusResident`]）。
   *
   * ⚠️ **可选**：老门不给这一格 ⇒ 行为与从前一字不变（认领只按顶层那一格）。
   * ⚠️ 它与 `agent` / `lastWake` **不是一回事**：那两格是**第一支**的（抬到顶层给冻结前端读），
   * 而这一格是**全部**住户的名单 —— 页面靠它才认得出第 2、3 支（否则它们被写成"没有机器在跑"）。
   */
  residents?: HomeStatusResident[]
  /**
   * Agent 的经历（机械投影；TASK_72 ④）。**可选**：口子这一面读不到时它整个不在 ——
   * 页面据此说「这一面读不到」，而**不**画成一列空表（缺省不等于零）。
   */
  experience?: ExperienceView
  /**
   * **这个家还是住户化之前的形状**（环境根 `<家>/env`）——**只在是的时候出现**（2026-10-05）。
   *
   * 为什么要有它：**壳只认新布局** `<家>/agents/<支>/`，而把老家搬成新形态是 `up.js` 的
   * `migrateHomeShape`（**app 不做迁移**）⇒ 老家在壳里会落到"零住户"，页面于是说
   * 「这台机器上还没有 Agent」——**而那一支明明在**，只是住在老形态里。
   * 判据一句话：**认不出的形状不许说成不存在**。
   *
   * ⚠ 它**不是** Agent 的状态（所以不进 `AgentState` 那张封闭表）：它是**家的形状**这一格事实，
   * 与 `agent.state === 'no-resident'`（"这个门没有住户"）**两件事分开说**。
   * JS 那条路（`node home.js doors`）不送这一格 —— 它本来就认老家（`runtimeResidents` 含老家那一支）。
   */
  legacyHome?: boolean
  /**
   * **这一轮为什么没搬成**（稳定原因码；`legacyHome === true` 时才有）——2026-10-05 加。
   *
   * 壳在开机时会自己搬（`home_shape.rs` 镜像 `residents.js` 的 `migrateHomeShape`）；
   * 这一格出现 ⇒ 那次没搬成。两种原因的**下一步完全不同**，所以必须分开说：
   * - `'host-running'`：那支 Agent 这会儿正跑着 ⇒ 停掉它，下次开机 app 自己搬；
   * - `'world-key'`：最老的那一档（`home.conf` 没有 `home_id`）⇒ 换形状会在世界里另建一支，
   *   要动世界，**只有 CLI 能做**；
   * - `'move-failed'`：改名/建目录失败 ⇒ 什么都没改，重跑一次即可。
   *
   * ⚠ 它是**稳定字符串（一份词汇表）**，不是给人看的句子 —— 人话归页面（`legacyHomeNote`）；
   * 取值不认识就**原样带出去**（页面走那句诚实的兜底，不替它猜）。
   */
  legacyBlocker?: string
}

/**
 * 这份本机读数**是不是这个 Agent 的**（刀2b C①：闭合刀2a 登记的判据缺口）。
 *
 * 三态，**不许压成两态**：
 * - `same`：读数里的 id 与我正在看的这个 Agent**同一个人**；
 * - `different`：两个人 ⇒ 页面该说「**此 Agent 未在本机运行**」，不是错误、不是隐藏；
 * - `unknown`：读数里**没有** id（环境根记录读不出来，或口子版本旧）⇒ **不知道**，
 *   这时候**不许**说"Agent 不在这台机器上"（那是把读不到说成一个事实），照常显示并保留主语句。
 */
export function agentAgentMatch(
  view: HomeStatusView,
  agentId: string | null | undefined,
): 'same' | 'different' | 'unknown' {
  const onMachine = view.agent.agentId
  if (typeof onMachine !== 'string' || onMachine === '') return 'unknown'
  if (typeof agentId !== 'string' || agentId === '') return 'unknown'
  return onMachine === agentId ? 'same' : 'different'
}

const AGENT_STATES: readonly string[] = [
  'loaded',
  'busy',
  'no-resident',
  'missing-binary',
  'unreadable',
]

function isRun(value: unknown): value is HomeStatusRun {
  if (typeof value !== 'object' || value === null) return false
  const run = value as Record<string, unknown>
  return (
    typeof run['id'] === 'string' &&
    typeof run['state'] === 'string' &&
    // `outcome` 缺席（未收尾）与 `null` 都是**合法**的：收场结果本来就可能还没有。
    (typeof run['outcome'] === 'string' || run['outcome'] === null)
  )
}

/**
 * 经历一行的形状守卫：**键必须落在白名单里**。
 *
 * 这是**第二道**（口子那边已经按白名单投影过）：万一口子漏了正文（`params` / `blocks` /
 * `text` / `content` / `payload`），客户端**也不许把它画出来** ⇒ 整个读数判成"形状不认识"
 * ⇒ 页面说读不到。判据落在这里，是因为"Agent 的经历是私有世界"这条不能只靠一层。
 */
function isExperienceRow(value: unknown): value is ExperienceRow {
  if (typeof value !== 'object' || value === null) return false
  const row = value as Record<string, unknown>
  if (typeof row['kind'] !== 'string') return false
  for (const key of Object.keys(row)) {
    if (!EXPERIENCE_ROW_FIELDS.includes(key)) return false
  }
  return true
}

function isExperienceView(value: unknown): value is ExperienceView {
  if (typeof value !== 'object' || value === null) return false
  const view = value as Record<string, unknown>
  const state = view['state']
  if (state !== 'loaded' && state !== 'missing' && state !== 'unreadable') return false
  if (state !== 'loaded') return true
  const rows = view['rows']
  if (!Array.isArray(rows) || !rows.every(isExperienceRow)) return false
  if (view['windowed'] !== undefined && typeof view['windowed'] !== 'boolean') return false
  if (view['truncated'] !== undefined && typeof view['truncated'] !== 'boolean') return false
  return true
}

/** 形状守卫：**只认已知形状**，多一个字段不管、少一个必需字段就拒。 */
export function isHomeStatusView(value: unknown): value is HomeStatusView {
  if (typeof value !== 'object' || value === null) return false
  const view = value as Record<string, unknown>
  if (typeof view['readAt'] !== 'string') return false
  // `stale` **必需**：缺了它就是"不知道这份新不新"——那必须 fail-closed 成"读不到"，
  // 而不是替它猜一个"新"（把缓存说成当下正是这一件要防的事）。
  if (typeof view['stale'] !== 'boolean') return false
  // 经历可选；给了就必须合形状（含**行键白名单**，见 `isExperienceRow`）。
  if (view['experience'] !== undefined && !isExperienceView(view['experience'])) return false
  // 老家形态那一格可选；给了就必须是布尔（"读不懂"不许当"不是" —— 那是缺省说成事实）。
  if (view['legacyHome'] !== undefined && typeof view['legacyHome'] !== 'boolean') return false
  // 原因码同理：给了就必须是字符串（词表本身不在这里校验 —— 页面按稳定字符串选人话，
  // 认不出就**原样带出去**走兜底那句，不编）。
  if (view['legacyBlocker'] !== undefined && typeof view['legacyBlocker'] !== 'string') return false
  const im = view['im']
  if (typeof im !== 'object' || im === null) return false
  // **一个事实**：探活问到的码（`null` ＝ 没答话）。键必须在场（缺一格 = 形状不认识），
  // 值要么是 `null`、要么是数字 —— 字符串（如 `"200"`）**不接受**：那是没转换的读数。
  const imRecord = im as Record<string, unknown>
  if (!('health' in imRecord)) return false
  if (imRecord['health'] !== null && typeof imRecord['health'] !== 'number') return false
  const agent = view['agent']
  if (typeof agent !== 'object' || agent === null) return false
  const agentRecord = agent as Record<string, unknown>
  if (typeof agentRecord['state'] !== 'string' || !AGENT_STATES.includes(agentRecord['state'])) return false
  const runs = agentRecord['runs']
  if (runs !== undefined && (!Array.isArray(runs) || !runs.every(isRun))) return false
  const lastWake = view['lastWake']
  if (lastWake !== null && typeof lastWake !== 'string') return false
  // 宿主那一格可选；给了就必须认得出形状（**取值不在封闭表里不算形状坏** —— 见 `hostStateOf`）。
  if (view['host'] !== undefined && !isHostView(view['host'])) return false
  // ★ 住户清单可选（老门不给）；给了就**逐项**合形状 —— 一项坏不算整份坏是**不行**的：
  // 认领靠它，坏掉一项就可能把"别人"认成"这一支"（那正是这一格要防的事）。
  const residents = view['residents']
  if (residents !== undefined) {
    if (!Array.isArray(residents) || !residents.every(isResidentView)) return false
  }
  return true
}

/**
 * 取一次状态读数（**按条目**）。
 *
 * 地址由 `homes.ts` 的 `statusRequestUrl` 算：本机条目 ⇒ `/home-status/status`
 * （后一段是**口子自己的路径**：代转只做搬运，不给端面改名）；远程条目 ⇒ 绝对地址 + `/status`
 * （直连口子，用它自己的路由名）+ Bearer 家钥匙。
 *
 * 失败一律抛**带分类的** `HomeRequestError`（没读到 / 钥匙不对 / 答的话读不懂互不混淆）——
 * 由调用方决定"怎么诚实地说没读到"。**不在这里兜默认值**：给一份空读数总会有人把它当成
 * "Agent 没在跑"。
 */
export async function fetchHomeStatus(
  entry: HomeEntry = localHomeEntry(),
  init?: { signal?: AbortSignal },
): Promise<HomeStatusView> {
  const timeout = AbortSignal.timeout(STATUS_TIMEOUT_MS)
  const signal = init?.signal ? AbortSignal.any([init.signal, timeout]) : timeout
  let payload: unknown = null
  try {
    // 出网走 `./transport`：**这一条正是"app 读远端家"要走的路** —— 壳里由 Rust 侧发出，
    // 因此不需要那台机器的状态口开任何跨源面（它确实开着 CORS，但 web 版才需要）。
    const res = await sendRequest({
      url: statusRequestUrl(entry, '/status'),
      method: 'GET',
      headers: homeRequestHeaders(entry),
      signal,
    })
    if (!res.ok) {
      throw new HomeRequestError(res.status === 401 ? 'unauthorized' : 'http', res.status)
    }
    try {
      payload = await res.json()
    } catch {
      payload = null
    }
  } catch (error) {
    if (error instanceof HomeRequestError) throw error
    // 断网 / 证书不认 / 拒绝连接 / 超时取消 —— 一律只给这一档（没有更细的因）。
    throw new HomeRequestError('network')
  }
  // 形状守卫：认不出就是"读不到"（**不半读半猜**）。判据与先前同一条、报错换成结构化分类。
  if (!isHomeStatusView(payload)) {
    throw new HomeRequestError('shape', null, 'status endpoint returned an unknown shape')
  }
  return payload
}
