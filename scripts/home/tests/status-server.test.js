'use strict'
// 状态口与解析器的机械判据（TASK_55）。
//
// 这一刀最要紧的两件事都在这里钉住：
//  1. **解析器认不出的形状必须明确失败**——它是"Agent 那一轮到底怎么样了"的唯一来源，
//     猜一个差不多的比读不到更坏；
//  2. **口上无秘密是机械可判的**——spawn 白名单 env 里不许出现家的凭据键。
//
// TASK_72 ③ 又添一件：**短 TTL 与开机预热**引入的"这份读数可能不是这一轮的"必须
// **可判错**——判据落在那三个读数上：spawn 了几次 / `readAt` 变没变 / `stale` 是哪一档。
//
// 2026-09-24 收口刀再添一件：探测链从 `spawnSync` 换成 `spawn` ⇒ **注入点的形状跟着实现走**
// （替身必须交回一个带流的异步孩子）；并且新增一条判据本体：**慢探测不许按住事件循环**
// （TTL 过期后那次"后台刷新"进行中，第二个请求也必须立刻拿到响应）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const http = require('node:http')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { EventEmitter } = require('node:events')
const { PassThrough } = require('node:stream')

const P = require('../paths')
const { parseHostStatus } = require('../host_status_parse')
const {
  minimalEnv,
  serveEnv,
  createStatusServer,
  buildStatusPayload,
  readAgentStatus,
  readAgentExperience,
  projectExperience,
  projectOrigin,
  readEnvIdentity,
  ORIGIN_SENTENCES,
  STATUS_TTL_MS,
  EXPERIENCE_FIELDS,
  ALLOWED_ENV_KEYS,
  ORIGIN_NOT_ALLOWED,
} = require('../status-server')
const homeKey = require('../home-key')

let cases = 0
/**
 * `await fn()` 而不是 `fn()`：本文件里有 async 用例（要真起一个 server）。
 * 漏掉这个 await，那些用例的断言会被**静默跳过**、还假装通过 —— 那正是"测试空跑"。
 */
function statusCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 一份**真实形状**的读数（照 `agent_host_status` 的实际输出抄的）。 */
const GOOD = [
  'celira-agent-host-status v1',
  'environment-root=D:/celira-home/env',
  'agent=agent-1',
  // 与下面两行 `run=` 自洽：本页 2 条、一共 2 条（不截断）。夹具自己先不许撒谎。
  'activities-page=2 of=2 scope=latest',
  '',
  '[state]',
  'admission=ready',
  'run=host-1-1-1 state=ended outcome=subject:silent',
  'run=host-1-2-2 state=ended outcome=subject:completed',
  '',
  '[next]',
  '活动已收场——无需动作，或开始新活动',
  '',
].join('\n')

/**
 * **真实截断读数**：照 `agent_host_status` 的输出**逐字抄的**（2026-09-29 实测抓取）。
 *
 * 为什么多这一份：这一行是**三个字段挤在一行**——
 * `activities-page=32 of=36 scope=latest`。夹具如果只写一个光秃秃的数（`activities-page=32`），
 * "拿整串去 parseInt"这种 bug 就会悄悄溜过去：`parseInt('32 of=36', 10)` 是 32，看着对，
 * 却把**本页条数当成了别的意思**。**夹具与真读数不同形 = 判据是假的**（TASK_55 踩过同款）。
 *
 * ⚠ 2026-09-29 这条夹具换过一次形状：从前那行是 `activities-truncated=true next-after=…`，
 * 而 `next-after` 那个游标**这个命令根本消费不了**（它没有 `--after` 参数），于是收掉。
 * 现在这行照样是"一键多字段"，守卫的意图一字未变。
 */
const GOOD_TRUNCATED = [
  'celira-agent-host-status v1',
  'environment-root=D:\\\\celira-home\\\\env',
  'agent=2cf4e998-01fa-4053-83f8-2ed82c1735e6',
  'activities-page=32 of=36 scope=latest',
  '',
  '[state]',
  'admission=ready',
  'run=host-14352-1789722592389-3 state=ended outcome=subject:completed',
  '',
  '[next]',
  '活动已收场——无需动作，或开始新活动',
  '',
].join('\n')

statusCase('解析正常读数：admission / 活动 / [next] 三样都读出来', () => {
  const parsed = parseHostStatus(GOOD)
  assert.equal(parsed.ok, true)
  assert.equal(parsed.value.admission, 'ready')
  // 「活动次数」要的是**总数**（`of=` 那一段）。
  assert.equal(parsed.value.activities, 2)
  assert.equal(parsed.value.activitiesPage, 2)
  assert.equal(parsed.value.activitiesScope, 'latest')
  assert.equal(parsed.value.runs.length, 2)
  assert.deepEqual(parsed.value.runs[1], {
    id: 'host-1-2-2',
    state: 'ended',
    outcome: 'subject:completed',
    // TASK_59 起的**加法字段**：失败分类（没有就是 null）。页面今天不渲染它，
    // 但它是"Agent 为什么失败"的唯一线索。
    failure: null,
  })
  assert.match(parsed.value.next, /活动已收场/)
})

/**
 * **★ 回归：本页条数不许冒充总数**（2026-09-29 真机实测那条）。
 *
 * 症状：账本 36 条活动，界面上「活动次数」写 **32** 并且从此不再增长 —— 因为读数里那个
 * `activities` 放的是**一页**的条数，而读者（与界面）都当成总数。同一页还是**最早**的一页，
 * 于是"最近的活动"一条都看不见。
 *
 * 判据：`of=` 说多少就是多少，本页那 32 只管"这一页列了几条"。
 */
statusCase('★ 截断时 activities 必须是总数（of=36），不是本页条数（32）', () => {
  const parsed = parseHostStatus(GOOD_TRUNCATED)
  assert.equal(parsed.ok, true)
  assert.equal(parsed.value.activities, 36, '「活动次数」是总数')
  assert.equal(parsed.value.activitiesPage, 32, '本页条数另有一个字段')
  assert.notEqual(parsed.value.activities, parsed.value.activitiesPage, '两个数在截断时必须不同')
})

// ── 真输出的失败行（TASK_59 修的坑；夹具**逐字**抄自真实 `agent_host_status`）──
//
// 那行的形态是：`outcome=` 后面**还挂着一段** `failure=<class>`。TASK_55 那版要求
// 整行三段 ⇒ 只要账本里有**一条**失败活动，整份读数被判 `malformed-run-line`
// ⇒ 运行状态页在 Agent 真出问题时显示"读不到"。**夹具必须与真读数同形**（TASK_55 的教训）。
const GOOD_WITH_FAILURE = [
  'celira-agent-host-status v1',
  'environment-root=D:\\\\celira-home\\\\env',
  'agent=2cf4e998-01fa-4053-83f8-2ed82c1735e6',
  'activities-page=3 of=3 scope=latest',
  '',
  '[state]',
  'admission=ready',
  'run=host-27972-1789807731996-1 state=ended outcome=subject:silent',
  'run=host-27972-1789808195975-3 state=ended outcome=system:model-failed failure=transport-failure',
  '',
  '[next]',
  '活动已收场——无需动作，或开始新活动',
  '',
].join('\n')

statusCase('回归：收场带 failure= 后缀的行不再把整份读数判成"读不到"', () => {
  const parsed = parseHostStatus(GOOD_WITH_FAILURE)
  assert.equal(parsed.ok, true, '有一条失败活动 ≠ 读不到（这正是 Agent 出问题时最需要的读数）')
  assert.equal(parsed.value.runs.length, 2, '两条活动都要在，不许因为一条带后缀就整份丢')
  assert.equal(parsed.value.runs[1].outcome, 'system:model-failed')
  assert.equal(parsed.value.runs[1].failure, 'transport-failure', 'failure 要整吃下来并留在结构里')
  assert.equal(parsed.value.runs[0].failure, null, '没有后缀的那条是 null，不是空串')
})

statusCase('失败分类不写死清单：将来细化成 `transport-failure:dns` 也照吃', () => {
  const parsed = parseHostStatus(
    GOOD_WITH_FAILURE.replace('failure=transport-failure', 'failure=transport-failure:dns'),
  )
  assert.equal(parsed.ok, true)
  assert.equal(parsed.value.runs[1].failure, 'transport-failure:dns')
})

// ── 未收尾的活动行（2026-10-03 修，真机撞出来的）──
//
// 真读数里，**停在半路的活动没有 `outcome=`**（它还没收场）。这一版之前要求 `outcome=`
// ⇒ **账本里只要有一条未收尾活动，整份读数**被判 `malformed-run-line` ⇒ 页面说"读不到账本"，
// 而 Agent 其实好好的（真机读数：同一个家 38 条活动里 12 条未收尾，「我的 Agent」三格与
// 机器页三间全部读不到）。夹具**逐字**抄自那一次真实输出。
const GOOD_WITH_UNFINISHED = [
  'celira-agent-host-status v1',
  'environment-root=D:\\\\celira-home\\\\agents\\\\e4e4410d',
  'agent=e4e4410d-75e0-436c-a35b-273eb4dfe3f8',
  'activities-page=3 of=3 scope=latest',
  '',
  '[state]',
  'admission=unfinished(run=host-13556-1791090187913-2)',
  'run=host-13556-1791090175692-1 state=ended outcome=subject:silent',
  'run=host-13556-1791090187913-2 state=unfinished',
  '',
  '[next]',
  '有未收尾活动（host-13556-1791090187913-2）：活动停在半路——**不**接着跑它：重启宿主即可',
  '',
].join('\n')

statusCase('回归：未收尾的活动（没有 outcome=）不再把整份读数判成"读不到"', () => {
  const parsed = parseHostStatus(GOOD_WITH_UNFINISHED)
  assert.equal(parsed.ok, true, '有一条未收尾活动 ≠ 读不到（那正是"她在不在跑"最需要的读数）')
  assert.equal(parsed.value.runs.length, 2, '两条活动都要在，不许因为一条没收尾就整份丢')
  assert.equal(parsed.value.runs[1].state, 'unfinished')
  assert.equal(parsed.value.runs[1].outcome, null, '没收尾就是没有结果 ⇒ null，不许编一个出来')
  assert.equal(parsed.value.runs[0].outcome, 'subject:silent', '收尾那条照旧带着结果')
})

statusCase('未收尾与失败分类可以同时在：`state=unfinished failure=…`（真要出事也是这个形状）', () => {
  const parsed = parseHostStatus(
    GOOD_WITH_UNFINISHED.replace(
      'run=host-13556-1791090187913-2 state=unfinished',
      'run=host-13556-1791090187913-2 state=unfinished failure=transport-failure',
    ),
  )
  assert.equal(parsed.ok, true)
  assert.equal(parsed.value.runs[1].outcome, null)
  assert.equal(parsed.value.runs[1].failure, 'transport-failure')
})

statusCase('负对照：只有 failure= 而没值（畸形）⇒ 仍然明确失败，不安静跳过', () => {
  const parsed = parseHostStatus(GOOD_WITH_FAILURE.replace('failure=transport-failure', 'failure='))
  assert.equal(parsed.ok, false, '认不出的行就该整份失败（不许安静跳过一条活动）')
  assert.equal(parsed.reason, 'malformed-run-line')
})

// ── 第三类判定：`state=unknown reason=dispatched-without-result unresolved=1`（2026-10-05 修）──
//
// **同一个坑的第三次**（前两次：未收尾没有 `outcome=`、失败收场多一段 `failure=`）：
// CLI 的第三类判定**两段都在**，老正则认不出 ⇒ 账本里只要有一条"已派发却没有结果"的活动，
// **整份读数**又被判 `malformed-run-line` ⇒ app 与网页两条路一起说"读不到"，
// 而 Agent 好好的、正在正常跑活动（2026-10-05 真机读数：机器页三间全瞎）。
// 夹具**逐字**抄自那一次真输出（含它前后两条正常的活动行）。
const GOOD_WITH_UNKNOWN = [
  'celira-agent-host-status v1',
  'environment-root=D:\\\\celira-home\\\\agents\\\\e4e4410d',
  'agent=e4e4410d-75e0-436c-a35b-273eb4dfe3f8',
  'activities-page=32 of=59 scope=latest',
  '',
  '[state]',
  'admission=ready',
  'run=host-1-1000-1 state=ended outcome=subject:completed',
  'run=host-17680-1791175794423-1 state=unknown reason=dispatched-without-result unresolved=1',
  'run=host-1-2000-2 state=unfinished',
  '',
  '[next]',
  '活动已收场——无需动作，或开始新活动',
  '',
].join('\n')

statusCase('回归：`state=unknown reason=… unresolved=…` 不再把整份读数判成"读不到"', () => {
  const parsed = parseHostStatus(GOOD_WITH_UNKNOWN)
  assert.equal(parsed.ok, true, '有一条"已派发却没有结果" ≠ 读不到')
  assert.equal(parsed.value.runs.length, 3, '三条活动都要在，不许因为一条带两段后缀就整份丢')
  assert.equal(parsed.value.runs[1].state, 'unknown')
  assert.equal(parsed.value.runs[1].outcome, null, '这一类没有 outcome ⇒ null，不编')
  assert.equal(parsed.value.runs[1].failure, null, '也没有 failure ⇒ null')
  assert.equal(parsed.value.runs[0].outcome, 'subject:completed', '前后两条正常的照旧')
  assert.equal(parsed.value.runs[2].state, 'unfinished')
})

statusCase('段序是判据的一部分：`unresolved=` 跑到 `outcome=` 前面 ⇒ 整行认不出（不安静吃）', () => {
  const swapped = GOOD_WITH_UNKNOWN.replace(
    'run=host-17680-1791175794423-1 state=unknown reason=dispatched-without-result unresolved=1',
    'run=host-17680-1791175794423-1 state=unknown unresolved=1 reason=dispatched-without-result',
  )
  const parsed = parseHostStatus(swapped)
  assert.equal(parsed.ok, false, '段的顺序是 CLI 定的：换了顺序就是另一种形状')
  assert.equal(parsed.reason, 'malformed-run-line')
})

// ── 第四类判定：行尾多一段 `stopped=`（2026-10-18 修，**同一个坑第四次**）─────────────
//
// CLI 2026-10-13 起在行尾多写"为什么停在这里"（`held` / `yielded` / `no-response` / `unreadable`
// —— `agent/README.md` §3.82 那一刀），而**两份解析器都还停在六段上** ⇒ 账本里只要有一条
// 说得出为什么停下的未收尾活动，**整份读数**又被判 `malformed-run-line` ⇒ 机器页「运行」
// 那一间整间说"读不到"（真机读数：打包形态门禁 `verify:ui:app` 13/14，红的那一条正是
// 活动读数 —— 而 Agent 好好的）。夹具**逐字**抄自那一次真输出。
const GOOD_WITH_STOPPED = [
  'celira-agent-host-status v1',
  'environment-root=D:\\\\celira-home\\\\agents\\\\7de85b6f',
  'agent=7de85b6f-215b-45be-a81c-747ec9350ae6',
  'activities-page=32 of=79 scope=latest',
  '',
  '[state]',
  'admission=unfinished(run=host-31240-1791817079006-2)',
  'run=host-33736-1792318397097-1 state=unfinished stopped=no-response',
  'run=host-2188-1792318525403-1 state=unfinished stopped=no-response',
  'run=host-1-1000-1 state=ended outcome=subject:completed',
  '',
  '[next]',
  '上一段没有收尾 —— 看它停在哪',
  '',
].join('\n')

statusCase('回归：行尾多一段 `stopped=` 不再把整份读数判成"读不到"', () => {
  const parsed = parseHostStatus(GOOD_WITH_STOPPED)
  assert.equal(parsed.ok, true, '有一条"说得出为什么停下"的未收尾活动 ≠ 读不到')
  assert.equal(parsed.value.runs.length, 3, '三条活动都要在，不许因为一条带尾段就整份丢')
  assert.equal(parsed.value.runs[0].state, 'unfinished')
  assert.equal(parsed.value.runs[0].outcome, null, '未收尾 ⇒ null，不编一个 outcome')
  assert.equal(parsed.value.runs[2].outcome, 'subject:completed', '前后两条正常的照旧')
  // 认得出、**不带出来**：`stopped` 今天没有消费者（操作者那一行是原始输出）。
  assert.equal('stopped' in parsed.value.runs[0], false, '这一段今天不上载荷')
})

statusCase('段序仍是判据的一部分：`stopped=` 跑到 `unresolved=` 前面 ⇒ 整行认不出', () => {
  const swapped = GOOD_WITH_STOPPED.replace(
    'run=host-33736-1792318397097-1 state=unfinished stopped=no-response',
    'run=host-33736-1792318397097-1 stopped=no-response state=unfinished',
  )
  const parsed = parseHostStatus(swapped)
  assert.equal(parsed.ok, false, '段的顺序是 CLI 定的：换了顺序就是另一种形状')
  assert.equal(parsed.reason, 'malformed-run-line')
})

statusCase('Agent 正忙（already-open）是一等结果，不是失败', () => {
  const parsed = parseHostStatus('something already-open something')
  assert.equal(parsed.ok, 'busy', 'Agent 是"这一拍在忙"，不是"读不出来"')
})

statusCase('正对照：真读数里三个字段挤在一行（`activities-page=32 of=36 scope=latest`）要按段取', () => {
  // 这一条是**回归判据**：本族字段一直是"一键多字段"，整串比较或整串 parseInt 都会读错 ——
  // `parseInt('32 of=36', 10)` 是 32，看着对，其实是把本页条数当成了别的意思。
  // 夹具与真读数同形才挡得住这一类（TASK_55 与本刀各踩过一次）。
  const parsed = parseHostStatus(GOOD_TRUNCATED)
  assert.equal(parsed.ok, true)
  assert.equal(parsed.value.activities, 36, '`of=` 才是总数')
  assert.equal(parsed.value.activitiesPage, 32, '第一段是本页条数')
  assert.equal(parsed.value.activitiesScope, 'latest', '第三段说这一页取自账本哪一头')
  assert.equal(parsed.value.admission, 'ready')
})

// —— 负对照三态：认不出就必须失败，绝不猜 ——

statusCase('负对照：缺 [next] 段 ⇒ 明确失败', () => {
  const truncated = GOOD.replace('[next]\n活动已收场——无需动作，或开始新活动\n', '')
  const parsed = parseHostStatus(truncated)
  assert.equal(parsed.ok, false)
  assert.equal(parsed.reason, 'missing-next-section')
})

statusCase('负对照：活动行畸形 ⇒ 明确失败（不安静跳过）', () => {
  const malformed = GOOD.replace(
    'run=host-1-1-1 state=ended outcome=subject:silent',
    'run=host-1-1-1 ended silently',
  )
  const parsed = parseHostStatus(malformed)
  assert.equal(parsed.ok, false)
  assert.equal(parsed.reason, 'malformed-run-line')
})

statusCase('负对照：版本头不认识 ⇒ 明确失败（将来升 v2 时不会把新格式读成乱码）', () => {
  const parsed = parseHostStatus(GOOD.replace('celira-agent-host-status v1', 'celira-agent-host-status v2'))
  assert.equal(parsed.ok, false)
  assert.equal(parsed.reason, 'unknown-header')
})

statusCase('负对照：空文本 ⇒ 明确失败', () => {
  assert.equal(parseHostStatus('').ok, false)
  assert.equal(parseHostStatus('').reason, 'empty')
})

// —— 口上无秘密：机械可判 ——

statusCase('spawn 白名单 env：家的凭据一个都不在里面', () => {
  const env = minimalEnv({
    PATH: '/usr/bin',
    SystemRoot: 'C:/Windows',
    DEEPSEEK_API_KEY: 'sk-should-never-pass',
    CELIRA_IM_BACKEND_AUTH: 'backend-should-never-pass',
    CELIRA_LOCAL_CLIENT_HOST_TOKEN: 'token-should-never-pass',
    HOME: '/home/me',
  })
  assert.equal(env.PATH, '/usr/bin', '必要项要留下（否则子进程起不来）')
  assert.equal(env.SystemRoot, 'C:/Windows')
  for (const key of ['DEEPSEEK_API_KEY', 'CELIRA_IM_BACKEND_AUTH', 'CELIRA_LOCAL_CLIENT_HOST_TOKEN', 'HOME']) {
    assert.ok(!(key in env), `${key} 不许进子进程环境`)
  }
  assert.deepEqual(Object.keys(env).filter((k) => !ALLOWED_ENV_KEYS.includes(k)), [])
})

statusCase('readAgentStatus 把解析结果原样抬上来（用假 spawn，不真跑二进制）', async () => {
  const agent = await readAgentStatus({
    statusExe: 'ignored.exe',
    envDir: 'env',
    agentId: 'agent-1',
    spawn: () => fakeSpawn(GOOD),
  })
  assert.equal(agent.state, 'loaded')
  assert.equal(agent.admission, 'ready')
  assert.equal(agent.runsTotal, 2) // runs 只给最近几条，总数单独给
  assert.ok(agent.next.includes('活动已收场'))
})

statusCase('readAgentStatus：二进制缺 / 输出畸形 各自成态（不混成一个"读不到"）', async () => {
  const busy = await readAgentStatus({
    statusExe: 'x',
    envDir: 'e',
    agentId: 'a',
    spawn: () => fakeSpawn('already-open', { code: 1 }),
  })
  assert.equal(busy.state, 'busy')

  const broken = await readAgentStatus({
    statusExe: 'x',
    envDir: 'e',
    agentId: 'a',
    spawn: () => fakeSpawn('something else entirely', { code: 1 }),
  })
  assert.equal(broken.state, 'unreadable')
  assert.equal(broken.reason, 'unknown-header')
})

statusCase('readAgentStatus：孩子起不来（spawn 的 `error` 事件，不是抛）⇒ 读不出来，不抛', async () => {
  // 异步化之后这条缝多了一个入口：起不来是 `error` **事件**（同步版是返回 `{error}`）。
  // 结论必须与同步版一致：没有输出 ⇒ `unreadable` / `empty`（不编一个别的原因）。
  const agent = await readAgentStatus({
    statusExe: 'nope.exe',
    envDir: 'e',
    agentId: 'a',
    spawn: failingSpawn,
  })
  assert.equal(agent.state, 'unreadable')
  assert.equal(agent.reason, 'empty')
})

// —— 端面穷举：只有两个 GET，其余明确拒绝 ——

async function withServer(run, overrides = {}) {
  const ctx = {
    homeDir: 'D:/nowhere',
    // 默认那个路径**不存在**（⇒ `missing-binary` 那一态）。要判"真的探了一次"的用例
    // 传一个**确实存在**的文件（用本文件自己），再配一个计数的 `spawn` 替身。
    layout: { statusExe: overrides.statusExe ?? 'D:/nowhere/agent_host_status.exe' },
    logDir: 'D:/nowhere/logs',
    imListen: '127.0.0.1:1',
    agentId: 'agent-1',
    // `residents` 给了就照给（**给了空数组**＝这个家零住户 —— 与"没给"是两件事：
    // 没给会走旧形状的合成住户，见 `buildStatusPayload`）。
    // ⚠ 它是**函数**（2026-10-12）：门每次请求现问 —— 用例给的是 `() => [一份固定表]`，
    // 生产给的是 `residents.runtimeResidents` 那次现读。
    ...(overrides.residents ? { residents: overrides.residents } : {}),
    ...(overrides.spawn ? { spawn: overrides.spawn } : {}),
    proc: {
      httpCode: overrides.httpCode ?? (async () => 0),
      isFile: overrides.isFile ?? (() => false),
    },
  }
  const server = createStatusServer(ctx, overrides.serverOptions ?? {})
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address()
  const call = (method, pathname, headers = {}) =>
    new Promise((resolve, reject) => {
      const req = http.request({ host: '127.0.0.1', port, method, path: pathname, headers }, (res) => {
        let body = ''
        const responseHeaders = res.headers
        res.on('data', (chunk) => {
          body += chunk
        })
        res.on('end', () =>
          resolve({ status: res.statusCode, body, headers: responseHeaders }),
        )
      })
      req.on('error', reject)
      req.end()
    })
  try {
    // 第二个参数给 server：需要"开机预热"那种用例（先 warmStatus、再问）。
    await run(call, server)
  } finally {
    await new Promise((resolve) => server.close(resolve))
  }
}

statusCase('GET /healthz 通，GET /status 通（家没开机时也如实给读数）', async () => {
  await withServer(async (call) => {
    const health = await call('GET', '/healthz')
    assert.equal(health.status, 200)
    assert.deepEqual(JSON.parse(health.body), { ok: true })

    const status = await call('GET', '/status')
    assert.equal(status.status, 200)
    const payload = JSON.parse(status.body)
    // 探活那一格**只有一个事实**（`health`）：家没开 ⇒ 探不到 ⇒ `null`。
    // ⚠ 页面**不许**把 `null` 说成"没在跑"（探不到 ≠ 证明它停了）—— 措辞判据在
    // `client/tests/home-status.test.ts` 的 `localServiceReading` 那三条里。
    assert.equal(payload.im.health, null, '家没开 ⇒ 探不到 ⇒ null（不是 0，也不伪造一个码）')
    assert.equal('listening' in payload.im, false, '派生字段 `listening` 已删：一个事实一个权威')
    assert.equal(payload.agent.state, 'missing-binary', '二进制不在也如实说')
    assert.ok(payload.readAt)
  })
})

// ★ 2026-10-05：**零住户与"二进制不在"是两件事**（真机撞出来的）
//
// 从前两者共用 `missing-binary` ⇒ 刚建好世界的新用户在「运行」那一格读到
// 「本机缺少必要的程序」——一句**假警报**（机器好好的，只是还没有 Agent 住进来）。
// 判据：**缺省不等于故障**。两条各自成态，谁也不冒充谁。
statusCase('零住户（`residents: []`）⇒ no-resident；而"有住户、二进制不在"仍是 missing-binary', async () => {
  // ① 零住户：这个家一支都没住
  await withServer(
    async (call) => {
      const payload = JSON.parse((await call('GET', '/status')).body)
      assert.equal(payload.agent.state, 'no-resident', '还没有 Agent 不是缺件')
      assert.deepEqual(payload.residents, [], '住户表如实是空的')
      assert.equal(payload.lastWake, null)
    },
    { residents: () => [] },
  )
  // ② 有住户、二进制不在：这一态**一个字都不许变**（那才是真的缺件）
  await withServer(
    async (call) => {
      const payload = JSON.parse((await call('GET', '/status')).body)
      assert.equal(payload.agent.state, 'missing-binary', '缺件照旧是缺件')
    },
    { residents: () => [{ agentId: 'agent-1', envDir: 'D:/nowhere/env', slot: 1 }] },
  )
})

// ── 住户表**每次现问**（2026-10-12）────────────────────────────────────────
//
// 真机撞到的症状：门起来之后才安的那一支，在门的逐支读数里**永远不出现**（门把自己开口那一刻
// 的世界当成了现在）；反过来，搬走的那一支会**一直占着一行**。权威是磁盘上那份住户表，
// 门只是通道（与 `config-server.js` 那一处、与 `agent/src/model_source.rs` 的"不缓存"同一条纪律）。
statusCase('★ 住户表**每次现问**：两次构建之间多一支 ⇒ 第二次读数里有它', async () => {
  const resident = (n) => ({
    agentId: `agent-${n}`,
    envDir: `D:/nowhere/agents/a-${n}`,
    slot: n,
    roomId: '',
    legacy: false,
    mismatch: null,
  })
  let live = [resident(1)]
  const ctx = () => ({
    homeDir: 'D:/nowhere',
    layout: { statusExe: 'D:/nowhere/agent_host_status.exe' },
    logDir: 'D:/nowhere/logs',
    imListen: '127.0.0.1:1',
    residents: () => live,
    spawn: () => fakeSpawn('celira-agent-host-status v1\n\n[state]\nadmission=ready\n'),
    proc: { httpCode: async () => 0, isFile: () => false },
  })
  const first = await buildStatusPayload(ctx())
  assert.equal(first.residents.length, 1, '前提：此刻只有一支')
  // **门跑着的时候**安第二支（`live` 就是"磁盘上此刻住着谁"的替身）。
  live = [...live, resident(2)]
  const after = await buildStatusPayload(ctx())
  assert.deepEqual(
    after.residents.map((each) => each.agentId),
    ['agent-1', 'agent-2'],
    '第二支必须在读数里（从前它到门退出都不出现）',
  )
})

statusCase('端面穷举：别的路径 404、写方法 405（没有第三种入口）', async () => {
  await withServer(async (call) => {
    for (const pathname of ['/', '/status/extra', '/serve', '/api/v1/local/status']) {
      const res = await call('GET', pathname)
      assert.equal(res.status, 404, `${pathname} 不该存在`)
      assert.equal(JSON.parse(res.body).error, 'not_found')
    }
    for (const method of ['POST', 'PUT', 'DELETE']) {
      const res = await call(method, '/status')
      assert.equal(res.status, 405, `${method} /status 必须被拒`)
      assert.equal(JSON.parse(res.body).error, 'method_not_allowed')
    }
  })
})

statusCase('im 读数：httpCode 给字符串也要判对（"200" 必须转成数字 200）', async () => {
  // 实测踩过：`proc.httpCode` 给的是字符串 "200"，而代码写 `code === 200` 永远 false
  // ⇒ 响应里出现 `listening:false` 配 `health:"200"` 这种自相矛盾的读数。
  await withServer(
    async (call) => {
      const res = await call('GET', '/status')
      assert.equal(res.status, 200)
      const payload = JSON.parse(res.body)
      assert.equal(payload.im.health, 200, '"200"（字符串）也必须判成 200，别把字符串原样透出去')
    },
    { httpCode: async () => '200' },
  )
})

statusCase('★ im 读数：非 200 也是**有应答** ⇒ health 就是那个码（不许压成"没在跑"）', async () => {
  // ★ 2026-10-11：从前这一格只有一个派生布尔 `listening = (code === 200)` ⇒
  // **世界活着、只是自报 500**，也会被写成「没在跑（HTTP 500）」——同一行同时说了"在跑"和"没在跑"。
  // 判据：**有应答（任何码）就证明那个口上有人在** ⇒ 那是"在跑"，只是它自己说不健康。
  await withServer(
    async (call) => {
      const payload = JSON.parse((await call('GET', '/status')).body)
      assert.equal(payload.im.health, 500, '非 200 照给 —— 那是证据（页面据此说"在跑，但它自己说不健康"）')
    },
    { httpCode: async () => '500' },
  )
})

statusCase('im 读数：连不上时 health 是 null，不是 0', async () => {
  // `curl` 连不上给 `000` ⇒ parseInt 得 0。0 不是 HTTP 状态码，透出去页面会写
  // 「没在跑（HTTP 0）」——把"读不到"说成了一个具体读数（实测踩过）。
  await withServer(
    async (call) => {
      const payload = JSON.parse((await call('GET', '/status')).body)
      assert.equal(payload.im.health, null, '连不上就没读数，别给 0')
    },
    { httpCode: async () => '000' },
  )
})

statusCase('起口子时给它的环境：白名单 ＋ CELIRA_HOME_DIR，家的凭据一个都不进', () => {
  // TASK_59：口子由 `up` 拉起 ⇒ "它自己的环境里有什么"第一次成了要管的机械事实。
  const env = serveEnv(
    {
      PATH: '/usr/bin',
      SystemRoot: 'C:/Windows',
      DEEPSEEK_API_KEY: 'sk-should-never-pass',
      CELIRA_IM_BACKEND_AUTH: 'backend-should-never-pass',
      CELIRA_LOCAL_CLIENT_HOST_TOKEN: 'token-should-never-pass',
      HUMAN_TOKEN: 'clr_hs_should-never-pass',
      HOME: '/home/me',
    },
    'D:/celira-home',
  )
  assert.equal(env.CELIRA_HOME_DIR, 'D:/celira-home', '它得知道看哪个家')
  assert.equal(env.PATH, '/usr/bin', '操作系统必需品要留下（PowerShell/netstat 靠它）')
  assert.equal(env.SystemRoot, 'C:/Windows')
  for (const key of [
    'DEEPSEEK_API_KEY',
    'CELIRA_IM_BACKEND_AUTH',
    'CELIRA_LOCAL_CLIENT_HOST_TOKEN',
    'HUMAN_TOKEN',
    'HOME',
  ]) {
    assert.ok(!(key in env), `${key} 不许进口子的环境`)
  }
})

// TASK_61 删掉的用例：`CORS：只给本机来源回头`。
//
// 为什么删而不是留着：同源代转之后**没有任何消费者**了 —— 页面不再直连这个端口
// （发布形态走 `serve_client.mjs` 的 `/home-status` 代理、源码形态走 vite preview 代理），
// 同源就没有跨源可言。判据是"不留没人消费的活代码"；口子仍只绑 `127.0.0.1`，
// 可达面由**绑定**决定、不由响应头决定，所以没有回退。

statusCase('同源门之外：口子自己不再吐任何 CORS 头（TASK_61 收口）', async () => {
  // 反向判据：这里**故意**带一个本机 Origin，头也**不该**出现。
  // 留这一条是因为"删掉一个面"必须可判错 —— 只删实现不带判据的话，
  // 将来谁把 `...cors` 加回来也没人拦得住。
  await withServer(async (call) => {
    const res = await call('GET', '/status', { Origin: 'http://localhost:5173' })
    assert.equal(res.status, 200, '读数本身照常给')
    assert.equal(res.headers['access-control-allow-origin'], undefined, '不该再有 CORS 头')
    assert.equal(res.headers.vary, undefined, '也不该留 vary: origin')
  })
})

// —— 2026-09-25：来源守卫（模式一）与"模式二不被它卡" ——

statusCase('模式一：Host 非 loopback ⇒ 403 origin_not_allowed（只绑 loopback 挡不住 DNS rebinding）', async () => {
  // 这一口的读数里有**是谁、Agent 的准入、Agent 的经历投影**：可达面靠绑定，但"浏览器眼里同源"
  // 的那种页面（rebinding）只有 `Host` 认得出来。判据要正反都给：
  await withServer(async (call) => {
    const denied = await call('GET', '/status', { host: 'evil.example' })
    assert.equal(denied.status, 403)
    assert.equal(JSON.parse(denied.body).error, ORIGIN_NOT_ALLOWED)

    // 负向对照：两种本机写法照常放行（端口随便）。
    assert.equal((await call('GET', '/status', { host: '127.0.0.1:5190' })).status, 200)
    assert.equal((await call('GET', '/status', { host: 'localhost:5190' })).status, 200)
    assert.equal((await call('GET', '/healthz', { host: '[::1]:5190' })).status, 200)
    // Origin 有就必须是 loopback（与配置门同一道、同一份实现）。
    assert.equal((await call('GET', '/status', { Origin: 'https://evil.example' })).status, 403)
  })
})

statusCase('模式二：远程浏览器（Host 非 loopback）带钥匙 ⇒ 200（钥匙才是那一边的闸，不是 Host）', async () => {
  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-status-remote-'))
  const key = homeKey.generateKey()
  homeKey.writeKeyDigest(homeDir, homeKey.hashKey(key))
  try {
    await withServer(
      async (call) => {
        const authorized = { host: '192.168.1.20:5190', authorization: `Bearer ${key}` }
        assert.equal((await call('GET', '/healthz', authorized)).status, 200)
        const read = await call('GET', '/status', authorized)
        assert.equal(read.status, 200)
        // 远程那一份要能被**浏览器**读走：模式二的响应带 CORS 头（模式一不带，见上一条）。
        assert.equal(read.headers['access-control-allow-origin'], '*')
        // 负向对照：同一套 Host，没有钥匙 ⇒ 401（不是 403）。
        const noKey = await call('GET', '/status', { host: '192.168.1.20:5190' })
        assert.equal(noKey.status, 401)
        assert.equal(JSON.parse(noKey.body).error, homeKey.UNAUTHORIZED)
      },
      { serverOptions: { access: { mode: 'remote', homeDir }, ttlMs: 1 } },
    )
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

// —— TASK_72 ③：短 TTL 与开机预热（判据落在三个读数上：spawn 几次 / readAt 变没变 / stale 哪一档）——

/**
 * 造一个**异步**的 `spawn` 替身（形状与 `node:child_process` 的 `spawn` 一致：立刻交回一个
 * 带 `stdout` / `stderr` 流与 `close` 事件的孩子；**数据先到、`close` 后到**，与真进程同序）。
 *
 * ⚠ 为什么替身必须是异步形状（2026-09-24 收口刀）：探测链从 `spawnSync` 换成了 `spawn` ——
 * 替身**跟着实现走**。给异步实现喂一个同步形状的替身，只会得到一堆假读数
 * （`done.stdout` 变成一个流对象 ⇒ 文本拼成 `[object Object]` ⇒ 恒判 `unreadable`）。
 *
 * `delayMs` 把"这一次探测很慢"变成**可注入**的：本刀那条判据本体就是靠它把
 * "第二个请求的等待 ≠ 探测时长"变成可判的（真家一次探测 ~3.7 秒，不可能靠真跑二进制量）。
 */
function fakeSpawn(output, { delayMs = 0, code = 0 } = {}) {
  const child = new EventEmitter()
  child.stdout = new PassThrough()
  child.stderr = new PassThrough()
  const deliver = () => {
    child.stdout.end(output)
    child.stderr.end('')
    // `close` 等两股流都流完 —— 否则实现可能在数据到齐之前就收结论（真进程不是这个顺序）。
    let pending = 2
    const done = () => {
      if (--pending === 0) child.emit('close', code)
    }
    child.stdout.on('end', done)
    child.stderr.on('end', done)
  }
  if (delayMs > 0) setTimeout(deliver, delayMs)
  else setImmediate(deliver)
  return child
}

/** 一个**起不来**的孩子：`spawn` 的失败是 `error` 事件，不是抛。 */
function failingSpawn() {
  const child = new EventEmitter()
  child.stdout = new PassThrough()
  child.stderr = new PassThrough()
  setImmediate(() => child.emit('error', new Error('spawn ENOENT')))
  return child
}

/** 计数的 `spawn` 替身：交回一份**合法**读数（`GOOD`），并记下被叫了几次。 */
function countingSpawn(options = {}) {
  const calls = { n: 0 }
  return {
    calls,
    spawn: () => {
      calls.n += 1
      return fakeSpawn(GOOD, options)
    },
  }
}

/** 等后台刷新落地（`warmStatus` / 过期后的刷新都是 fire-and-forget）。 */
const settle = () => new Promise((resolve) => setTimeout(resolve, 25))

statusCase('TTL 内：第二次请求**不再 spawn**，且 readAt 逐字不变（缓存 ≠ 当下）', async () => {
  const { calls, spawn } = countingSpawn()
  let clock = 1_000_000
  await withServer(
    async (call) => {
      const first = JSON.parse((await call('GET', '/status')).body)
      clock += 5_000 // 仍在 TTL 内
      const second = JSON.parse((await call('GET', '/status')).body)

      assert.equal(calls.n, 1, 'TTL 内不许再探一次（那正是"每次进来等 3.7 秒"的来源）')
      assert.equal(second.readAt, first.readAt, '缓存命中时 readAt 必须是**那次读数**的时刻')
      assert.equal(second.stale, false, 'TTL 内不算陈旧')
      assert.equal(second.agent.state, first.agent.state, '读数内容照旧')
    },
    { statusExe: __filename, spawn, serverOptions: { ttlMs: STATUS_TTL_MS, now: () => clock } },
  )
})

statusCase('过 TTL：先把旧那份给出去（stale=true）并**在后台刷新**，下一问就是新的', async () => {
  const { calls, spawn } = countingSpawn()
  let clock = 2_000_000
  await withServer(
    async (call) => {
      const first = JSON.parse((await call('GET', '/status')).body)
      assert.equal(first.stale, false, '刚取回来的那一份不是缓存')

      clock += STATUS_TTL_MS + 1
      const second = JSON.parse((await call('GET', '/status')).body)
      assert.equal(second.stale, true, '过了窗口就必须标注"这是缓存读数"')
      assert.equal(second.readAt, first.readAt, '标注归标注，时刻仍是**那次读数**的时刻')

      await settle()
      assert.equal(calls.n, 2, '后台刷新真的发生了（不是只标注、不刷新）')
      const third = JSON.parse((await call('GET', '/status')).body)
      assert.equal(third.stale, false, '刷新之后就不再是陈旧读数')
      assert.notEqual(third.readAt, first.readAt, '刷新过之后 readAt 是新的时刻')
      assert.equal(calls.n, 2, '这一问命中刷新后的缓存 ⇒ 不再多探一次')
    },
    { statusExe: __filename, spawn, serverOptions: { ttlMs: STATUS_TTL_MS, now: () => clock } },
  )
})

statusCase('★ 并发：第二个请求的等待**不等于**探测时长（慢探测不许按住事件循环）', async () => {
  // 判据本体（2026-09-24 收口刀）：TTL 过期后那次"后台刷新"以前走 `spawnSync` ⇒ 它把事件循环
  // **按住**整个探测时长，落在窗口里的**任何**请求都只能干等（真机实测：389ms 的窗口里，
  // 第二/三/四个请求分别等了 317 / 226 / 134ms）。
  // 这里把探测做成**可注入的慢**，再问一句：窗口里来的第二个请求等了多久？
  const PROBE_MS = 400
  const { calls, spawn } = countingSpawn({ delayMs: PROBE_MS })
  let clock = 5_000_000
  await withServer(
    async (call) => {
      // 冷启动那一问会 await 探测（**既有行为**，不是本刀要治的）——先把它跑掉，拿到暖缓存。
      await call('GET', '/status')
      assert.equal(calls.n, 1)
      clock += STATUS_TTL_MS + 1 // 过 TTL
      const t0 = Date.now()
      const first = JSON.parse((await call('GET', '/status')).body)
      const firstMs = Date.now() - t0
      assert.equal(first.stale, true, '过期那一次给的是旧读数（并如实标注）')
      // 第二个请求：正落在后台探测**进行中**的窗口里。
      const t1 = Date.now()
      const second = JSON.parse((await call('GET', '/status')).body)
      const secondMs = Date.now() - t1
      assert.equal(second.stale, true, '窗口里这一问照旧立刻拿旧读数')
      assert.equal(calls.n, 2, '窗口里这一问不该再探一次（并发合并照旧）')
      // ⚠ 阈值取**探测时长的一半**（相对量，不是秒表上的绝对值）：异步实现实测 1~5ms，
      //    而"第二个请求跟着一起等探测"那一版会等于探测时长 ⇒ 这条判据就在这两态之间划线。
      assert.ok(
        secondMs < PROBE_MS / 2,
        `第二个请求等了 ${secondMs}ms —— 它不该等于别人的探测时长（${PROBE_MS}ms）`,
      )
      assert.ok(
        firstMs < PROBE_MS / 2,
        `触发刷新的那一问也等了 ${firstMs}ms —— 它拿的是旧读数，同样不该等探测`,
      )
    },
    { statusExe: __filename, spawn, serverOptions: { ttlMs: STATUS_TTL_MS, now: () => clock } },
  )
})

statusCase('冷启动没缓存时**等**一次探测（宁可等一下，也不给一份不存在的读数）', async () => {
  const { calls, spawn } = countingSpawn()
  await withServer(
    async (call) => {
      const payload = JSON.parse((await call('GET', '/status')).body)
      assert.equal(calls.n, 1)
      assert.equal(payload.stale, false, '这一份就是刚取回来的')
      assert.equal(payload.agent.state, 'loaded')
    },
    { statusExe: __filename, spawn, serverOptions: { ttlMs: STATUS_TTL_MS, now: () => 3_000_000 } },
  )
})

statusCase('开机预热：warmStatus 之后，页面第一次问就命中缓存（首屏不必等）', async () => {
  const { calls, spawn } = countingSpawn()
  await withServer(
    async (call, server) => {
      server.warmStatus()
      await settle()
      const payload = JSON.parse((await call('GET', '/status')).body)
      assert.equal(calls.n, 1, '预热那一次已经探过；页面这一问不该再探')
      assert.equal(payload.stale, false, '预热取的读数在 TTL 内，不算陈旧')
    },
    { statusExe: __filename, spawn, serverOptions: { ttlMs: STATUS_TTL_MS, now: () => 4_000_000 } },
  )
})

// —— TASK_72 ④：Agent 的经历的机械投影（**白名单 = 这一件的命门**）——

/**
 * 一份**真实形状**的经历（照 `trace.ndjson` 的实际字段抄）：那些 `observed` / `model-response`
 * / `run-input` 行**带着正文**（`params` / `blocks` / `text` / `payload`）——正是投影必须挡掉的东西。
 */
const TRACE_ROWS = [
  { kind: 'run-start', run: 'host-1-1-1', seq: 1 },
  { kind: 'run-input', run: 'host-1-1-1', seq: 2, text: '主人的问题原文' },
  {
    kind: 'injection',
    run: 'host-1-1-1',
    seq: 3,
    turn: 1,
    stage: 'run-start',
    blocks: [{ label: 'x', text: '注入正文' }],
  },
  {
    kind: 'model-response',
    run: 'host-1-1-1',
    seq: 4,
    turn: 1,
    decision: 'act',
    payload: '{"reasoning":"推理原文"}',
    content: '正文',
  },
  {
    kind: 'action-selected',
    run: 'host-1-1-1',
    seq: 5,
    turn: 1,
    index: 0,
    action: 'notes_create',
    params: [{ label: 'title', text: 'p4-marker' }],
  },
  {
    kind: 'observed',
    run: 'host-1-1-1',
    seq: 6,
    turn: 1,
    index: 0,
    action: 'notes_create',
    state: 'success',
    params: [{ label: 'title', text: 'p4-marker' }],
    blocks: [{ label: 'created', text: 'note-1' }],
  },
  { kind: 'ending', run: 'host-1-1-1', seq: 7, subject: 'completed' },
  { kind: 'sealed', run: 'host-1-1-1', seq: 8 },
]

statusCase('经历投影：**白名单本身**不许含任何带正文的字段名（点名守卫）', () => {
  // ⚠️ 为什么单独一条：`params` / `blocks` 是**数组**，就算误加进白名单也会被"只收标量"那条
  // 顺手挡住 —— 于是"加进去"这件事**不会红**（实测：我第一次的负向验证就是这么漏过去的）。
  // 真正危险的是**字符串**型字段（`text` / `content` / `payload`：宿主输入、模型原文与推理）
  // ⇒ 这条按**名字**点名，谁把它们加进白名单就红。
  for (const name of ['params', 'blocks', 'text', 'content', 'payload', 'extra-actions']) {
    assert.ok(!EXPERIENCE_FIELDS.includes(name), `${name} 不许在白名单里（它带正文）`)
  }
})

statusCase('经历投影：只过白名单字段——params / blocks / text / payload 一条都不许过桥', () => {
  const rows = TRACE_ROWS.map(projectExperience)
  const keys = new Set(rows.flatMap((row) => Object.keys(row)))
  for (const forbidden of ['params', 'blocks', 'text', 'content', 'payload']) {
    assert.ok(!keys.has(forbidden), `${forbidden} 绝不许进这一面（它带正文）`)
  }
  for (const wanted of ['action', 'state', 'turn', 'kind', 'seq']) {
    assert.ok(keys.has(wanted), `${wanted} 是机械读数，要留下`)
  }
  // 白名单是**逐条过**的：带正文的那几行也照常投影出机械字段
  const observed = rows.find((row) => row.kind === 'observed')
  assert.deepEqual(observed, {
    seq: 6,
    run: 'host-1-1-1',
    turn: 1,
    index: 0,
    kind: 'observed',
    action: 'notes_create',
    state: 'success',
  })
})

statusCase('经历投影：值不是字符串/数字的一律丢（数组、对象、null 都不进）', () => {
  const projected = projectExperience({
    kind: 'observed',
    seq: 9,
    turn: null,
    action: { nested: true },
    state: ['a'],
    whatever: 'x',
  })
  assert.deepEqual(projected, { kind: 'observed', seq: 9 }, '只有标量、且在白名单里的才留下')
})

statusCase('读经历：文件不在 ⇒ 说"这一次没有可读的"（不冒充"Agent 没有经历"）', () => {
  const got = readAgentExperience({ traceFile: 'D:/nowhere/definitely-not-here.ndjson' })
  assert.equal(got.state, 'missing')
  assert.ok(!('rows' in got), '读不到时不许给一个空 rows 冒充"零条"')
})

statusCase('读经历：真文件 ⇒ 只取尾部、只给机械字段、坏行跳过不整体失败', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-experience-'))
  const traceFile = path.join(dir, 'trace.ndjson')
  try {
    // ⚠ **今天的行形状**：`<JSON>\t<8 位十六进制>`（2026-10-06 起每行带校验和，见
    // `ledger_line.js`）。从前这里写的是**加校验和之前**的行 ⇒ 这条用例一直绿，而真实账本
    // 一行都读不出来（真机 2026-10-14：读数成"她没有经历"）。
    const signed = TRACE_ROWS.map((row, index) => `${JSON.stringify(row)}\t${(index + 1)
      .toString(16)
      .padStart(8, '0')}`)
    // 末行故意是**半行**（模拟"尾部窗口正好切在行中间"）；再给一行**校验列读不出来**的
    // （制表符之后不是十六进制）——两种坏行都跳过，好的照给。
    const lines = [
      ...signed,
      '{"half":',
      `${JSON.stringify({ kind: 'sealed', run: 'host-1-1-1', seq: 99 })}\tZZZ`,
    ]
    fs.writeFileSync(traceFile, `${lines.join('\n')}\n`, 'utf8')
    const got = readAgentExperience({ traceFile, limit: 3 })
    assert.equal(got.state, 'loaded')
    assert.equal(got.rows.length, 3, '只要最后 3 条')
    assert.equal(got.truncated, true, '截断了就要说出口')
    assert.equal(got.rows[got.rows.length - 1].kind, 'sealed', '坏的末行被跳过，前一条照给')
    const anyBody = got.rows.some(
      (row) => 'params' in row || 'blocks' in row || 'payload' in row || 'text' in row,
    )
    assert.ok(!anyBody, '真读一遍也不许漏正文')
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

statusCase('读经历：**旧记录**（加校验和之前、行尾没有那一列）照读 —— 两种形状都认', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-experience-old-'))
  const traceFile = path.join(dir, 'trace.ndjson')
  try {
    fs.writeFileSync(
      traceFile,
      `${TRACE_ROWS.map((row) => JSON.stringify(row)).join('\n')}\n`,
      'utf8',
    )
    const got = readAgentExperience({ traceFile, limit: 3 })
    assert.equal(got.state, 'loaded')
    assert.equal(got.rows.length, 3, '旧行没有校验列，但正文仍是整行')
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// 刀2b C：起源映射（闭集）与「这是哪个 Agent 的账本」
// ---------------------------------------------------------------------------

// 三句起源定稿句**逐字**抄自 `agent/src/os.rs` 的三个公开常量。
const ORIGIN_BELL =
  'This activity was started by the doorbell: the backend event stream signaled that new events are available in the im app.'
const ORIGIN_TIMER = 'This activity was started by the timer: an alarm set in this environment has arrived.'
const ORIGIN_MANUAL =
  'This activity was started manually by the host. No external event triggered it.'

statusCase('起源映射：三句定稿句各有稳定取值（逐字比，不是模糊匹配）', () => {
  const row = (text) => ({
    kind: 'injection',
    stage: 'run-start',
    blocks: [
      { label: 'notification-bar', text: '别的东西' },
      { label: 'activity-origin', text },
    ],
  })
  assert.equal(projectOrigin(row(ORIGIN_BELL)), 'bell')
  assert.equal(projectOrigin(row(ORIGIN_TIMER)), 'timer')
  assert.equal(projectOrigin(row(ORIGIN_MANUAL)), 'manual')
  // 认不出的句子 ⇒ unknown（**不猜**，也不塞进最近的那一档）。
  assert.equal(projectOrigin(row('This activity was started by something else entirely.')), 'unknown')
  // 三句都记在口子那份闭集里（改一句就得同时改两边）。
  assert.equal(ORIGIN_SENTENCES.length, 3)
})

statusCase('起源映射：不是起源行就**没有这一格**（不加字段，不是加一个 null）', () => {
  // 别 stage 的注入行
  assert.equal(projectOrigin({ kind: 'injection', stage: 'turn', blocks: [] }), null)
  // 别的 kind（实测算过：那句话在 observed / model-response 里也出现过，那不算起源）
  assert.equal(
    projectOrigin({ kind: 'observed', blocks: [{ label: 'activity-origin', text: ORIGIN_BELL }] }),
    null,
  )
  // 是起源行但没有那一块
  assert.equal(projectOrigin({ kind: 'injection', stage: 'run-start', blocks: [] }), null)
  assert.equal(projectOrigin({ kind: 'injection', stage: 'run-start' }), null)
})

statusCase('起源映射：投影**只带稳定取值，正文一个字节都不进**', () => {
  const projected = projectExperience({
    seq: 4517,
    run: 'host-1-2-3',
    kind: 'injection',
    stage: 'run-start',
    blocks: [{ label: 'activity-origin', text: ORIGIN_BELL }],
  })
  assert.equal(projected.origin, 'bell')
  assert.ok(!('blocks' in projected), 'blocks 是正文，绝不进口')
  assert.ok(
    !JSON.stringify(projected).includes('doorbell'),
    `投影里不许出现起源正文：${JSON.stringify(projected)}`,
  )
  // 正对照：非起源行的投影里**没有** origin 这一格。
  assert.ok(!('origin' in projectExperience({ seq: 1, run: 'r', kind: 'sealed' })))
})

statusCase('Agent 的身份：读环境根的记录（只有 id，没有名字 —— 名字不在本机）', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-identity-'))
  try {
    assert.equal(readEnvIdentity(dir), null, '文件不在 ⇒ 不给这一格（不编一个 id）')
    fs.writeFileSync(path.join(dir, 'identity.json'), '{"agent":"agent-7"}', 'utf8')
    assert.equal(readEnvIdentity(dir), 'agent-7')
    // 形状不对 ⇒ null（不猜）
    fs.writeFileSync(path.join(dir, 'identity.json'), '{"agent":123}', 'utf8')
    assert.equal(readEnvIdentity(dir), null)
    fs.writeFileSync(path.join(dir, 'identity.json'), '{不是 JSON', 'utf8')
    assert.equal(readEnvIdentity(dir), null)
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

// —— 2026-10-01 第六刀：**多支的读代价不许线性相加** ——
//
// 症状（第一性原理）：`buildStatusPayload` 逐支构造视图，而每支视图要 spawn 一次
// `agent_host_status`。串行 ⇒ 三支住户的一次刷新 = 三次探测**相加**（真家一次探测 ~3.7 秒，
// 三支就是十几秒）。这条与文件里既有的那条纪律是同一句：
// **"一个只读口的并发等待时间不许等于别人的探测时长"**。
//
// 判据用**可注入的慢探测**（`delayMs`）把"重叠"变成机械可判：三支各 80ms ⇒ 串行 ≈240ms、
// 并行 ≈80ms；再验三次 spawn 的起点落在同一个窗口里（重叠的直接证据）。

statusCase('★ 多支：三支的探测**并行**（总等待 ≈ 一次探测，不是三次相加）', async () => {
  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-status-many-'))
  const startedAt = []
  const spawn = () => {
    startedAt.push(Date.now())
    return fakeSpawn(GOOD, { delayMs: 80 })
  }
  try {
    const list = [1, 2, 3].map((n) => {
      const envDir = path.join(homeDir, 'agents', `agent-${n}`)
      fs.mkdirSync(envDir, { recursive: true })
      fs.writeFileSync(path.join(envDir, 'identity.json'), JSON.stringify({ agent: `agent-${n}` }), 'utf8')
      return { agentId: `agent-${n}`, envDir, slot: n, roomId: `room-${n}`, legacy: false, mismatch: null }
    })
    const t0 = Date.now()
    const payload = await buildStatusPayload({
      homeDir,
      layout: P.detectLayout(),
      imListen: '127.0.0.1:59421',
      residents: () => list,
      spawn,
      proc: { httpCode: async () => '000', trustedLedgerPid: () => null, hostOrphanPids: () => [] },
    })
    const elapsed = Date.now() - t0
    assert.equal(payload.residents.length, 3, '一支一份')
    assert.equal(startedAt.length, 3, '三支各探一次')
    assert.ok(elapsed < 200, `三支的探测要**重叠**：实测 ${elapsed} ms（串行会是 ~240ms）`)
    assert.ok(
      Math.max(...startedAt) - Math.min(...startedAt) < 40,
      `三次 spawn 的起点要落在同一个窗口里：${startedAt.join(',')}`,
    )
    // 顺序照旧：读数按住户表顺序给（并行不许把顺序打乱）。
    assert.deepEqual(payload.residents.map((r) => r.agentId), ['agent-1', 'agent-2', 'agent-3'])
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

test('（用例计数）', () => {
  assert.ok(cases > 0, '状态口用例数必须大于零')
  console.log(`      status-server 用例数：${cases}`)
})
