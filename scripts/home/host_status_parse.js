'use strict'
/**
 * `agent_host_status` 的文本输出 → 结构化读数。
 *
 * 为什么解析文本而不是让它吐 JSON：**它有真实消费者、结构多年稳定、且零 Rust 改动**。
 * 让 agent 侧新增输出格式要动那个 crate（本刀围栏禁止），而这段文本本来就是我们自己的
 * 稳定契约（`scripts/home/status.js` 一直在直接打印它）。
 *
 * 纪律：
 * - **严格**：认不出的形状一律 `{ ok: false }`，绝不"猜一个差不多"。
 *   最要紧的一条是**版本头**：认不出 `celira-agent-host-status v1` 就不解析——
 *   将来它升到 v2 时，这里会明确失败而不是把新格式读成旧格式的乱码。
 * - **`already-open` 是一等结果，不是失败**：Agent 正在跑一次活动时环境只有一个写者，
 *   读不到账本——那是"Agent 这一轮在忙"，必须与"读不到（真故障）"分开。
 *   ⚠ **2026-10-06 起本命令不再产生它**：`agent_host_status` 改成**读者打开**（只读句柄，
 *   不取写资格），她正在跑活动时照样读得到账本。这一支保留只为**旧运行时的输出**——
 *   家脚本与运行时在发布形态里可能不是同一版，把旧输出读成"真故障"比多留一支更糟。
 *   （写者那一侧一字未改：环境仍然只有一个写者，`already-open` 仍是写命令的一等结果。）
 * - 不产出任何凭据：这个输出里本来就没有，解析器也不新增字段。
 */

const HEADER = 'celira-agent-host-status v1'

/**
 * 活动行：`run=<id> state=<state> [reason=<reason>] [outcome=<outcome>] [failure=<class>] [unresolved=<n>]`。
 *
 * ⚠ **四个可选段的名字与顺序就是 CLI 的写出顺序**（`agent/src/bin/agent_host_status.rs`
 * 那一串 `write!`），不是我们排的。
 *
 * ⚠ **`outcome=` 是可选的**（2026-10-03 修，真机撞出来的）。**未收尾的活动没有 outcome**：
 * 它停在半路，运行时就还没收场 —— 真输出长这样（同一个家 38 条活动里 12 条是这样）：
 *
 * ```text
 * run=host-13556-1791090187913-2 state=unfinished
 * ```
 *
 * 而这一版之前要求 `outcome=` ⇒ **账本里只要有一条未收尾活动，整份读数**被判
 * `malformed-run-line` ⇒ 运行状态页说"读不到账本"，而 Agent 其实好好的（真机读数：
 * 「我的 Agent」那三格与机器页三间全部读不到）。这与下面 `failure=` 那一条是**同一个坑**：
 * 真输出会缺段/带后缀，别要求整行精确匹配。
 *
 * ⚠ **`failure=` 也是可选的尾段**（TASK_59 修）。收场是失败时真输出长这样：
 *
 * ```text
 * run=host-27972-1789808195975-3 state=ended outcome=system:model-failed failure=transport-failure
 * ```
 *
 * 而 TASK_55 那版要求**整行三段** ⇒ 只要账本里有**一条**失败活动，**整份读数**被判
 * `malformed-run-line` ⇒ 运行状态页在**Agent 真出问题的时候**显示"读不到"——最坏的时机瞎。
 * 与 `activities-truncated=true next-after=…` 是**同一个坑的兄弟**：真输出会带后缀，
 * 别要求整行精确匹配。
 *
 * ⚠ **`reason=` 与 `unresolved=` 也是可选的**（2026-10-05 修，**同一个坑第三次**，真机撞到）。
 * CLI 的第三类判定长这样（那一行逐字抄自真输出）：
 *
 * ```text
 * run=host-17680-1791175794423-1 state=unknown reason=dispatched-without-result unresolved=1
 * ```
 *
 * 老正则不认 `reason=` / `unresolved=` ⇒ 账本里只要有一条"已派发却没有结果"的活动，
 * **整份读数**又被判 `malformed-run-line` ⇒ app 与网页两条路一起说"读不到"（真机读数：
 * 机器页三间全瞎，而 Agent 好好的、正在正常跑活动）。
 * ⚠ 这两段**认得出、但不往载荷里带**（今天没有消费者：`reason` 是判定类目自己的读数，
 * 页面要用它得先在客户端那边开一格）—— 与"认不出就整行丢开"是两件事。
 *
 * ⚠ **`stopped=` 也是可选的尾段**（2026-10-18 修，**同一个坑第四次**，`verify:ui:app` 真机撞到）。
 * CLI 2026-10-13 起在行尾多写这一段（为什么停在这里：`held` / `yielded` / `no-response` /
 * `unreadable` —— 操作者由此分得出「它自己停下的」与「被杀在半路」），真输出长这样：
 *
 * ```text
 * run=host-33736-1792318397097-1 state=unfinished stopped=no-response
 * ```
 *
 * 两个解析器（本文件与壳的 `parse_run_status` 镜像）都还停在六段上 ⇒ 账本里只要有一条
 * **说得出为什么停下**的未收尾活动，**整份读数**又被判 `malformed-run-line` ⇒
 * 机器页「运行」那一间说"读不到"（真机读数：打包形态门禁 13/14，红的那一条正是活动读数，
 * 而 Agent 好好的）。⚠ 这一段同样**认得出、不带出来**（今天没有消费者：操作者那一行是
 * 原始输出，`node home.js status` 直接打印它 —— 页面要用它得先开一格）。
 *
 * **取值用 `\S+` 整吃、不写死清单**：分类词表还会继续细化（拟如 `transport-failure:dns`），
 * 解析层不该跟着改。（`unresolved` 是计数 ⇒ `\d+`。）
 */
const RUN_LINE =
  /^run=(\S+) state=(\S+)(?:\s+reason=(\S+))?(?:\s+outcome=(\S+))?(?:\s+failure=(\S+))?(?:\s+unresolved=(\d+))?(?:\s+stopped=(\S+))?$/

function field(lines, name) {
  const prefix = `${name}=`
  const line = lines.find((candidate) => candidate.startsWith(prefix))
  return line === undefined ? null : line.slice(prefix.length)
}

/**
 * 取一个键的**第一段**（按空白切开），不做整串比较。
 *
 * 为什么（实测踩过，2026-09-29 随 `activities-truncated` 退场时收进这里）：真输出里同一个键
 * 后面**还会挂着别的字段** —— 从前那行长这样：
 *
 * ```text
 * activities-truncated=true next-after=host-25800-1789724413812-1
 * ```
 *
 * 用 `field(…) === 'true'` 会让这一行读成 `false`（整串是 `true next-after=…`），页面于是
 * **不说列表被截断**，而它其实被截断了。现在这行长成 `activities-page=32 of=36 scope=latest`，
 * **三个字段一行** ⇒ 取 `of=` 与 `scope=` 都必须按空白切开取段，不能拿整串去 `parseInt`
 * （`parseInt('32 of=36', 10)` 是 32，看着对，其实是把本页条数当成了别的意思）。
 *
 * 这个坑的入口一直是**测试夹具**：夹具写简化行、真输出带后缀 ⇒ 单测全绿而真读数错。
 * 因此夹具一律按真输出原样抄。
 */
function segment(lines, name, index) {
  const raw = field(lines, name)
  if (raw === null) return null
  const parts = raw.split(/\s+/).filter((part) => part !== '')
  return parts[index] ?? null
}

/**
 * @param {string} text `agent_host_status` 的 stdout+stderr 原文
 * @returns {{ok:true, value:object} | {ok:false, reason:string} | {ok:'busy'}}
 */
function parseHostStatus(text) {
  if (typeof text !== 'string' || text.length === 0) {
    return { ok: false, reason: 'empty' }
  }
  // Agent 这一轮在跑活动：环境同时只有一个写者，读不到账本。这是一等结果。
  if (text.includes('already-open')) {
    return { ok: 'busy' }
  }

  const lines = text.split(/\r?\n/).map((line) => line.trimEnd())
  if (lines[0] !== HEADER) {
    return { ok: false, reason: 'unknown-header' }
  }

  const stateAt = lines.indexOf('[state]')
  const nextAt = lines.indexOf('[next]')
  if (stateAt < 0) return { ok: false, reason: 'missing-state-section' }
  if (nextAt < 0 || nextAt <= stateAt) return { ok: false, reason: 'missing-next-section' }

  const head = lines.slice(1, stateAt)
  const admission = field(lines.slice(stateAt + 1, nextAt), 'admission')
  if (admission === null || admission === '') {
    return { ok: false, reason: 'missing-admission' }
  }

  const runs = []
  for (const line of lines.slice(stateAt + 1, nextAt)) {
    if (line === '' || line.startsWith('admission=')) continue
    const match = RUN_LINE.exec(line)
    if (!match) {
      // 认不出的行就是认不出：宁可整体失败，也不安静跳过（跳过会让"少了一条活动"
      // 看起来像"Agent 本来就没跑过那一轮"）。
      return { ok: false, reason: 'malformed-run-line' }
    }
    runs.push({
      id: match[1],
      state: match[2],
      // 收场结果（未收尾的活动**没有**这一格 ⇒ `null`，不编一个 "ended" 出来）。
      // ⚠ 组号跟着正则走：3 = `reason`（认得出、不带出来），4 = `outcome`。
      outcome: match[4] ?? null,
      // 失败分类（有就带上，没有是 null）。**这是加法字段**：老记录没有它、
      // 页面今天也不渲染它 —— 但它是"Agent 为什么失败"的唯一线索（TASK_60 会细化取值）。
      failure: match[5] ?? null,
    })
  }

  const nextText = lines
    .slice(nextAt + 1)
    .map((line) => line.trim())
    .filter((line) => line !== '')
    .join(' ')

  // `activities-page=<本页条数> of=<一共几条> scope=latest`（2026-09-29）。
  //
  // ⚠ 这一行**替代**了从前的 `activities=<本页条数>`：那个字段名读起来像"一共这么多"，
  //   而它其实是本页条数 —— 活动数超过一页之后，界面上的「活动次数」就冻在 32 上不动
  //   （真机实测：账本 36 条，界面说 32），而那一页还是**最早**的一页 ⇒ 最近的活动
  //   整条链都看不见。两个事实（本页 / 总数）合成一个字段是那条缺陷的根。
  //
  // **取不到 `of=` 就是 `null`，绝不拿本页条数冒充总数** —— 那正是要修的那件事。
  // 一行三段：`activities-page=32 of=36 scope=latest`。
  const page = segment(head, 'activities-page', 0)
  const of = segment(head, 'activities-page', 1)

  return {
    ok: true,
    value: {
      environmentRoot: field(head, 'environment-root'),
      agent: field(head, 'agent'),
      // **活动总数** —— 界面上的「活动次数 / 活动总数」要的就是它。
      activities:
        of === null || !of.startsWith('of=') ? null : Number.parseInt(of.slice('of='.length), 10),
      // 本页列了几条（判"有没有更早的没列出来"用它跟总数比）。
      activitiesPage: page === null ? null : Number.parseInt(page, 10),
      // 本页取自账本的哪一头（`latest` = 最近那一头）。读不到就不给，不猜。
      activitiesScope: segment(head, 'activities-page', 2)?.replace(/^scope=/, '') ?? null,
      admission,
      runs,
      next: nextText,
    },
  }
}

module.exports = { parseHostStatus, HEADER }
