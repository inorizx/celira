/**
 * **人话词表**（2026-09-22 主人裁决：设计词汇给设计者，界面语言给用户——两套词典不许串门）。
 *
 * 这一层只做一件事：把**后端给的机械取值**翻译成用户读得懂的话。
 * 三条纪律：
 *
 * - **只翻译，不发明**：没有对应词就**原样交出来**（绝不编一个"看起来对"的）。
 *   露出一个生词本身就是一条诚实读数：它说"这一格我们还没给它起人话名"，
 *   而编一个词会让下一刀以为这一格已经翻译过了；
 * - **判据不在这里**：本层不决定"该不该显示"、不判断成功失败 —— 那是调用方的事，
 *   这一层只负责换词；
 * - **纯函数**：所以能离线钉住（`tests/plain-language.test.ts`）。
 */

/**
 * 一次活动的**结果**：`completed` 完成 ／ `silent` 未回复 ／ `declined` 拒绝
 * ／ `waiting` 等外部结果 ／ 别的原样。
 *
 * ⚠ 后端给的是 `subject:completed` 这种带前缀的机械值（`agent.runs[].outcome` 与
 * trace 里的 `ending` 行同源），前缀在这里剥掉、词在这里换掉 —— **一处权威**。
 *
 * ⚠ **`subject:` 那一族是四个**（`agent/src/foundation.rs` 的 `SubjectEnding`：
 * `Completed` / `Silent` / `Declined` / `Waiting`）。2026-10-06 补上后两个：
 * 这一格从前只译了三个，于是「主体选择拒绝」在页面上原样露出 `declined` 这个英文词。
 * **系统那一族（`system:*`）不在这张表里**：那是"没正常收场"，分类归机器页的「活动」房间，
 * 不在这一层换词（同一份事实不许有两个说法）。
 */
export function resultLabel(ending: string | null): string | null {
  if (ending === null) return null
  const raw = ending.startsWith('subject:') ? ending.slice('subject:'.length) : ending
  if (raw === 'completed') return '完成'
  if (raw === 'silent') return '未回复'
  if (raw === 'declined') return '拒绝'
  if (raw === 'waiting') return '等外部结果'
  if (raw === 'failed') return '失败'
  return raw
}

/**
 * 一次活动的**判定**（五类互斥；词汇表在 `agent/README.md` §3.10）。
 *
 * ⚠ **为什么要这一层**（2026-10-05）：`outcome`（结果）**只有收场了的活动才有** ——
 * 另外四类判定从前在页面上被**压成同一句**「这次没有读到结果」，而它们的真义差得很远：
 * - `unknown` = 「**已派发却没有结果**（`dispatched-without-result`）：外部效果**可能已经发生**」
 *   —— 对 Agent 做过的事，这句是"**可能已经做了**"，不是"我们没读到"；
 * - `blocked` = 被挡住了（环境暂停 / 两存储矛盾 / 已准备未派发 / 输入未确认 / 轨迹配对缺陷）；
 * - `refused` = 这一场判定不出来（活动不存在、事实读不出来、序列自相矛盾…）；
 * - `unfinished` = 停在半路。
 *
 * 判据与「缺省不等于故障」「没给不等于没有」同一条：**该说的话说准**。
 *
 * ⚠ **判定的原因（`reason=`）不上这一层**：那是操作者的机械细节（`node home.js status` 给），
 * 这一页只说到"哪一类"。取值不认识就**原样带出去**（不编一个人话）。
 */
export function runStateLabel(state: string | null | undefined): string | null {
  if (!state) return null
  if (state === 'ended') return '已收场'
  if (state === 'unfinished') return '还没结束'
  if (state === 'blocked') return '被挡住了'
  if (state === 'unknown') return '不明 —— 它可能已经做了'
  if (state === 'refused') return '这一场的记录读不出来'
  return state
}

/**
 * 「这个家还是老形态」那一句**该怎么说** —— 按壳给的**稳定原因码**选，**不替它猜**（2026-10-05）。
 *
 * 为什么要有这一层：壳从 2026-10-05 起在开机时**自己搬**（`home_shape.rs` 镜像 `residents.js`
 * 的 `migrateHomeShape`）⇒ 这一格出现时意思是"**这一轮没搬成**"，而三种原因的**下一步完全不同**：
 * 停掉它下次自己搬 ／ 只有 CLI 能做 ／ 重跑一次。并列摆出来让人猜，就是"该说的话没说准"。
 *
 * ⚠ 取值不认识（壳升级了、页面还没跟上）⇒ **走兜底那句**：说清"这一轮没搬成"＋两种常见原因，
 * 并指向 app 的日志 —— **不编一个具体原因**（那是拿缺省当事实）。
 */
export function legacyHomeNote(blocker: string | null | undefined): string {
  const head = '这台机器的布局还是住户化之前的样子（一支一个环境根）—— 这台 app 会在开机时自己把它搬成新形态'
  if (blocker === 'host-running') {
    return `${head}；**这一轮没搬成**：那支 Agent 这会儿正跑着。先把它停下来，下次开机 app 自己就搬。`
  }
  if (blocker === 'world-key') {
    return `${head}；**这一轮没搬成**：它是最老的那一档（home.conf 里没有家 id）—— 换形状会在世界里另建一支 Agent，那一步要动世界，**只有 CLI 能做**：node home.js up。`
  }
  if (blocker === 'move-failed') {
    return `${head}；**这一轮没搬成**：搬的时候没成功（**什么都没改**）。再开一次 app 就会重试。`
  }
  return `${head}；**这一轮没搬成**（常见两种原因：它正跑着 ／ 最老的那一档要走 CLI）—— 详情在 app 的日志里。`
}

/**
 * Agent **现在**的状态（后端 `admission` 那一格）。
 *
 * 「待命」= 准入放行（它随时可以开始一次活动）；暂停 / 没做完都照实说，
 * 而**不再把后端那句 `paused(reason=…)` 原样摆出来**（那是工程话）。
 */
export function stateLabel(admission: string | null | undefined): string {
  if (!admission) return '状态未知'
  if (admission === 'ready') return '待命'
  if (admission === 'paused' || admission.startsWith('paused(')) return '已暂停'
  if (admission === 'unfinished' || admission.startsWith('unfinished(')) return '上一次没做完'
  return admission
}

/**
 * 「接下来」那一行：待命 ⇒ **当前空闲**；别的状态如实描述。
 *
 * 它替换掉从前那一行**后端原话**（`[next]` 那段，例如"活动已收场——无需动作，或开始新活动"）——
 * 那句话是给操作员看的，不是给用户看的。
 */
export function nextLabel(admission: string | null | undefined): string {
  if (admission === 'ready') return '当前空闲，没有待处理的事'
  if (admission === 'unfinished' || admission?.startsWith('unfinished(')) {
    return '有一次活动没做完，需要收尾'
  }
  if (admission === 'paused' || admission?.startsWith('paused(')) return '已暂停，需要你确认'
  return '——'
}

/**
 * 一次活动**为什么开始**（刀2b C②：口子从起源行映射出的稳定取值）。
 *
 * ⚠ 后端的取值是给机器的（`bell` / `timer` / `manual`）；**认不出的原样交出来**（同本模块开头
 * 那条纪律）。`null` = 这一格没有（那一场没有起源行）。
 *
 * 字面映射（与主人 2026-09-22 的裁决一致）：门铃 = **有新消息**（门铃就是 im 那边有新动静）；
 * 定时器 = **它自己约的时间到了**；手动 = **手动开始**。
 */
export function originLabel(origin: string | null | undefined): string | null {
  if (origin === null || origin === undefined || origin === '') return null
  if (origin === 'bell') return '有新消息'
  if (origin === 'timer') return '它自己约的时间到了'
  if (origin === 'manual') return '手动开始'
  if (origin === 'unknown') return '不知道'
  return origin
}

/**
 * 「它在哪台机器上」那一格（`agentAgentMatch` 的三态；前端填肉刀 2026-09-23）。
 *
 * ⚠ **这一格只答住址**（2026-10-20 修）：从前它写「在本机运行 / 未接入 / 状态未知」——
 * 前两句把**住址**与**在不在跑**说成一件事，而"在不在跑"另有其格（`api/agentHealth.ts`
 * 那一句，唯一权威）。真机上撞到过一行：`机器没在跑 · 上次：完成` 紧挨着绿的
 * 「在本机运行」—— 两个相反的答案并排，谁也不知道该信哪一句。
 * ⇒ 现在它只说"在哪台机器上"：**一个词只答一件事**（远程那台的写法是
 * 「在<机器名>上」，在 `api/agentRowStatus.ts` 里拼 —— 它得点名）。
 *
 * ⚠ **三态不许压成两态**（判据的来历见 `api/homeStatus.ts` 的 `agentAgentMatch`）：
 * `different` 是"读数里那个 id 是**别人**" ⇒ 「不在这台机器上」；而 `unknown` 是
 * "这一格**读不到**"（环境根记录读不出来、或口子版本旧）⇒ 「不知道在哪台机器上」。
 * **把 `unknown` 说成「不在这台」，就是把读不到说成一个事实** —— 这一格的全部意义
 * 就在于这两件事在页面上长得不一样。
 */
export function machineMatchLabel(match: 'same' | 'different' | 'unknown'): string {
  if (match === 'same') return '在这台机器上'
  if (match === 'different') return '不在这台机器上'
  return '不知道在哪台机器上'
}
