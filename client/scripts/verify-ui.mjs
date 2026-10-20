/**
 * UI 结构验证（页面级真机断言）。
 *
 * 与 `screenshot-catalog.mjs` 的分工：那支管**视觉回归**（像素 diff 对基线，只截
 * 陈列室的合成状态节）；这支管**结构判据**——"导航是不是分了那两组""那个入口在不在"
 * "按钮该不该出现"。两者互补：结构断言先挂，视觉 diff 才有意义（像素对了但少了一个
 * 入口也是错）。
 *
 * 为什么需要它：仓库只有 `node --test` 纯逻辑测试（无 jsdom / testing-library），
 * 所以"页面级行为"此前**没有任何自动化覆盖**——只能靠人点。这支脚本把那次真机
 * 验证（TASK_52 复核时手写的）固化成可重复的门。**不引入新依赖**：playwright
 * 本来就在 devDependencies 里（`screenshot-catalog.mjs` 与本脚本在用）。
 *
 * 只有一条通道（core 直连）：
 *   node scripts/verify-ui.mjs --core http://localhost:5173 --token <Human token>
 *
 * 退出码：0 = 全部通过；1 = 有断言失败；2 = 用法/环境错误。
 *
 * ## ⚠ 环境前置（2026-09-26 真机实跑后补齐 —— 这一节以前**一个字都没有**）
 *
 * 这支脚本**很久没被跑过**（`npm run verify` 不含它），所以它的前置一直没人写下来。
 * 2026-09-26 第一次实跑，20 条红里**只有 1 条是代码问题**，其余全是环境 —— 而当时
 * 从输出上**分不出**这两者。跑之前先满足这些：
 *
 * 1. **页面要是构建产物，不是 `vite dev`**。`chunk 拿不到 ⇒ 渲染错误边界`那一条
 *    靠拦截 `/assets/*.js` 复现"宿主换版后旧标签页取旧 chunk"——dev 服务器不发这种
 *    chunk ⇒ 它**永远拦到 0 个**。用 `npm run build && npm run preview -- --port 4173`
 *    再把 `--core` 指到 4173。（`vite.config.ts` 的 proxy 段 `dev`/`preview` 共用，两边都通。）
 * 2. **世界要"喂过"**，不能是刚 `bootstrap-human` 的空世界：
 *    - **至少一个 Agent** —— 没有的话脚本从前会**崩在半路**（`locator.click` 超时），
 *      现在会在那里停下并报"这个账号里没有 Agent"（exit 2）；
 *    - **至少一个房间** —— 否则「进房间后输入区挂得上来」判不动；
 *    - **一个含 ≥2 个 Human 的群房** —— 「@ 候选里含 Human」判不动；
 *    - **名下有至少一条接入绑定** —— 「凭据读不到」那两条判不动。
 *    造法（都走公开协议，不改库）：`POST /v1/agents`、`POST /v1/direct-rooms`、
 *    `POST /v1/groups` + `POST /v1/rooms/{id}/members`（被拉的人得先是**已接受联系人**：
 *    `POST /v1/contacts/requests` → 对方 `POST /v1/contacts/<发起人 id>/accept`）、
 *    `POST /v1/agents/{id}/backend-bindings`。
 *    ⚠ **没有活动记录的世界**（Agent 从没被唤醒过）是允许的：「活动记录」那一条会报
 *    **不适用**（没有可判的对象），不算失败；要连它一起验，得让那支 Agent 真跑一场
 *    （那是 `scripts/home/probe-loop.js` 的活，且要模型钥匙）。
 *    **现成的一条路**：`node scripts/home/probe-ui.js` —— 起临时家、铸 token、按上面这份
 *    清单喂饱、端构建产物、跑本脚本、删掉临时家（**全程不碰真家**）。
 * 3. **模型配置门开着**才能判配置页那一族（10 条）：`node home.js config --serve`
 *    （默认 5191）。**没开不算失败** —— 页面如实降级是对的，那 10 条报 `N/A`（不适用），
 *    既不计通过也不计失败，但**结尾会单独列出来**（仓库纪律：不适用要明说，绝不假装通过）。
 * 4. **本机状态口**（默认 5190）**不必开** —— 关着时三个房间走"如实降级"那一态，
 *    断言按 `gateOpen` 分支判，一样能绿。
 *
 * 三态输出：`PASS` / `FAIL` / **`N/A`（环境没就绪，这一轮没验）**。
 *
 * 纪律：
 * - **凭据只从 argv 进**：token / capability 不写文件、不进日志、不出现在输出里。
 * - 断言失败要**说清是哪一条**（带上那屏的正文片段），不做"某处不对"这种话。
 * - 回环请求一律绕 IDE 代理：脚本自己设 `no_proxy`（浏览器侧用 `--no-proxy-server`
 *   不方便，这里靠 Chromium 启动参数与进程 env 双保险）。
 *
 * ## ⚠ 页内异常记录器（2026-10-03 —— 修一处假绿）
 *
 * 本机唯一可用的引擎（CloakBrowser Chromium 146）**不发 CDP 的 `Runtime.exceptionThrown`**
 * （裸 CDP 会话显式 `Runtime.enable` 也一条收不到；同一会话的 `Log` 域正常）⇒
 * `page.on('pageerror')` 里**页面自己抛的异常永远不来** ⇒ 「零未捕获异常」这类断言在本机上
 * **恒真**。修法：`page.addInitScript` 在页面里装记录器（`error` / `unhandledrejection` →
 * `window.__celiraProbeErrors`），判分前 `page.evaluate` 读回，并入「进房间」那条断言。
 *
 * 口径（与 `probe.mjs` 同款，两边同步改）：
 * - **读不到 ≠ 没有**：记录器读不回来时，这一条按「不适用」如实降级（结尾会单独列出来），
 *   绝不当成通过；
 * - **与 CDP 事件的去重**：将来换到会发 `Runtime` 事件的引擎时两条路都在，同一桩异常
 *   只计一条（剥通道前缀与 `Uncaught [(in promise)]`、`Error:` 前缀后逐字比较，记录器为准）；
 * - ⚠ **同一根因对 console 类消息仍然成立**：引擎同样不发 `Runtime.consoleAPICalled` ⇒
 *   `page.on('console')` 里**页面自己**的 console 消息也收不到（只有浏览器自己的消息来，
 *   走 `Log` 域）。本刀只修「未捕获异常」这一条；基于 console 的其它断言**在这台机器上仍
 *   不构成证据**（页内包一层 `console.error` 是可选路，代价是改了被观测页面的行为，另议）。
 */

import { chromium } from 'playwright'
import { createRequire } from 'node:module'
import path from 'node:path'
// 内行词表：**唯一权威**（`scripts/jargon.mjs`），与 `probe.mjs` 的 `sweep` 共用同一份。
import { leakedJargon } from './jargon.mjs'
// ★ **门判据的共用表**（唯一权威位置）：这一半量**源码形态**的门
// （`node home.js doors`）；产品形态那一半在 `verify-ui-app.mjs` 里跑同一张表。
import {
  LOCAL_ORIGIN,
  describeDoorCases,
  doorCasesSayingWrongThing,
  runDoorCases,
} from './door-cases.mjs'

// ---------------------------------------------------------------------------
// 参数
// ---------------------------------------------------------------------------

const USAGE = `用法:
  node scripts/verify-ui.mjs --core <base> --token <Human token>
也可用环境变量（便于挂进 npm script / CI）：
  CELIRA_UI_BASE=<base>  CELIRA_UI_TOKEN=<token>
可选:
  --headed        显示浏览器窗口（默认无头）
  --keep-open     结束后不关浏览器（调试用）

只有一条通道：**core 直连**。曾经有一条 --host（本机 Host 托管）——
2026-09-20 起 Host 全托管路径**已退役**（docs/client/LOCAL_CLIENT_HOST.md §0），
那条分支现实中起不来、也就永远没有读数，已删。`

function parseArgs(argv) {
  const out = { base: null, token: null, home: null, headed: false, keepOpen: false }
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i]
    if (arg === '--core') {
      out.base = (argv[i + 1] || '').replace(/\/+$/, '')
      i += 1
    } else if (arg === '--token') {
      out.token = argv[i + 1] || ''
      i += 1
    } else if (arg === '--home') {
      // ★ **那个家的目录**（2026-10-12）：给出来才跑"门判据共用表"（从它的 `home.conf`
      //   读两个口的端口）。不给 ⇒ 那一段报不适用（本门禁仍照旧跑其它各段）。
      out.home = argv[i + 1] || null
      i += 1
    } else if (arg === '--headed') {
      out.headed = true
    } else if (arg === '--keep-open') {
      out.keepOpen = true
    } else if (arg === '--help' || arg === '-h') {
      console.log(USAGE)
      process.exit(0)
    }
  }
  // 命令行优先、环境变量兜底。凭据**只从这两个入口进**：不读文件、不落盘、不进日志。
  out.base = out.base || (process.env.CELIRA_UI_BASE || '').trim().replace(/\/+$/, '') || null
  out.token = out.token || process.env.CELIRA_UI_TOKEN?.trim() || null
  return out
}

const args = parseArgs(process.argv.slice(2))
if (!args.base || !args.token) {
  console.error(USAGE)
  process.exit(2)
}

// ---------------------------------------------------------------------------
// 断言脚手架
// ---------------------------------------------------------------------------

const results = []
/**
 * `ok` **三态**：`true` 通过 / `false` 失败 / **`null` 不适用**。
 *
 * ⚠ **为什么要有第三态（2026-09-26，本次真机实跑加的）**：有些断言**只有在某个环境
 * 就绪时才判得动** —— 模型配置门开着、世界里有一个含 ≥2 Human 的群房、账号里有 Agent、
 * 名下有接入绑定……从前它们一律记 FAIL，于是「我这台机器没起那个口」看起来和
 * 「代码坏了」**一模一样**。本次实跑 20 条红里有 12 条属于这一类，得逐条读正文才分得清。
 *
 * 仓库纪律：「本平台不适用时**明确报"不适用"，绝不假装通过**」
 * ⇒ 它既**不是 PASS**（那才是假装通过），也**不该算 FAIL**（那不是代码的问题）。
 * 它单独计数、单独打印，并在结尾**明确列出"这些这一轮没验"**。
 *
 * ⚠ **2026-10-05 扫了一遍"前置"那一族**：本文件里"环境没就绪 ⇒ 判不动"的前置断言，
 * 原先**两种口径并存** —— 5 处写 `null`（N/A），6 处写成 `false`（FAIL）。后果就是
 * 上面那段话说的病**只治好了一半**：真机实跑时"这个世界没有含 ≥2 Human 的群房"
 * 与"代码坏了"在输出上仍然长得一样。现在**一律 `null`**；判据一句话：
 * **这条断言要的输入不在场（账号里没 Agent / 没那个房 / 没有绑定）⇒ N/A；
 * 输入在场而东西不对 ⇒ FAIL。** 而"读不到"本身（如 `/v1/me` 没回话）是**第三种**：
 * 它不是"环境没就绪"，是这一轮真的坏了 ⇒ 照旧 `false`。
 */
function check(name, ok, detail = '') {
  results.push({ name, ok })
  const tag = ok === null ? 'N/A ' : ok ? 'PASS' : 'FAIL'
  console.log(`${tag}  ${name}${detail ? `  — ${detail}` : ''}`)
}

/** 屏上正文的一小段，用在失败信息里（别把整页倒出来）。 */
function snippet(text, n = 160) {
  return String(text || '').replace(/\s+/g, ' ').trim().slice(0, n)
}

/**
 * 等**Agent 的主页**落到终态（两段都读完 / 如实降级），最多 25 秒。
 *
 * 为什么不能睡固定时长：这一页有两个**过渡态**——先是壳层的「正在准备连接…」（会话自举），
 * 再是状态口那一次「正在读取运行情况…」（每次探测要 **3~4 秒**：spawn 读整个环境账本 +
 * 探一次 im 健康）。睡固定 4 秒会卡在过渡态上，把「读数慢」误报成「页面空白」（实测踩过）。
 * 等一个**状态**，别等一个时长。
 *
 * 终态判据（**两态都不许空白**）：或读到 Agent 的处境，或如实说"本机状态口未开"。
 * 刀2a 起这一页是**Agent 的主页**（今天为「资料 + 机器」两段），不再是"运行状态页"。
 */
async function waitForAgentHomeText(page) {
  const deadline = Date.now() + 25000
  let text = ''
  while (Date.now() < deadline) {
    text = await page.locator('body').innerText()
    const settled =
      text.includes(AGENT_HOME_UNREACHABLE_TITLE) ||
      (text.includes('运行') && text.includes('当前状态')) ||
      text.includes('没有这个 Agent') ||
      // ★ 2026-10-20：这里从前认的是「未接入」—— 那是 `agentMachineReadout` **2026-10-11 之前**
      // 的措辞（现在那一档是带作用域的「能读到的执行端都没有报它在跑」）⇒ 这一格早就恒不命中。
      // 现在认「机器」那一段的四态里**说得出话**的那三句（住址 / 说不准 / 都没报它）。
      text.includes('它住在') ||
      text.includes('没报上上面跑的是谁') ||
      text.includes('能读到的执行端')
    if (settled) return text
    const transient = text.includes('正在读取') || text.includes('正在准备连接')
    // 别的形状（不是这一页 / 抛错了）立刻返回，交给断言把原文报出来。
    if (!transient) return text
    await page.waitForTimeout(400)
  }
  return text
}

/**
 * 「读不到」那张卡的标题（家口远程访问刀：卡按"读不到<那一面>"起名）。
 * ⚠ 与 `components/celira/Runtimes.tsx` 的 `UnreachableCard` **逐字对齐**：
 * 改那边的标题就要同步这里（守卫写死被守卫对象的文案 ⇒ 改名即静默失效）。
 *
 * ⚠ 2026-09-26 修陈旧：原注释指 `components/celira/AgentRooms.tsx` —— **那个文件已不存在**
 * （2026-09-25 组织轴刀把三个房间从 Agent 主页搬到了机器页 `/app/runtimes/:entryId`，
 * 组件收进 `Runtimes.tsx`）。
 */
const AGENT_HOME_UNREACHABLE_TITLE = '读不到运行情况'
const AGENT_MIND_UNREACHABLE_TITLE = '读不到模型配置'

// 期望的一级入口（与 `src/components/celira/navGroups.ts` 对齐；改了那边要同步这里）
// ⚠ 2026-09-29：执行端组补了第二个入口「我的 Agent」⇒ 这里必须同步 ——
// `tests/nav-groups.test.ts` 有一条测试把两份清单锁在一起（改了 navGroups 忘了这里 ⇒ 那里红）。
// ⚠ 2026-10-03：这份清单量的是**真实世界**的入口（本脚本要一个跑着的 core + 一支 Human token）
// —— 真实世界下 `发现`（`/app/discover`）**不进导航**：它 `side: 'demo'`，点进去只有
// "尚未提供"（判据在 `routeRegistry.reachableInRealWorld`，过滤在 `navGroupsFor`）。
// 世界契约（Directory）落地后它自动回来，这里跟着改（上面那条锁会红着提醒）。
const EXPECTED_NAV = ['消息', '联系人', '我的执行端', '我的 Agent']
const EXPECTED_GROUPS = ['世界', '执行端']

/**
 * ★ 人话自检的词表（2026-09-22 主人裁决：设计词汇给设计者，界面语言给用户）。
 * 这些内行词**一个都不许出现在用户可见的文本里**。本刀（前端填肉）起从"只扫 Agent 的主页"
 * 扩成**每一个新做的面都扫一遍** —— 新写的文案多一句，漏一个词的概率就多一分。
 * ⚠ **2026-09-26 词表搬走了**：它现在住在 `scripts/jargon.mjs`，本脚本**import** 它。
 *
 * 为什么搬：这份表原先**只有本脚本一个执行者**，而本脚本要真机 core + token + 喂过的世界、
 * **不在 `npm run verify` 里**、并且很久没被跑过 ⇒ 实测漏了 **13 处**内行词上屏
 * （前端大整改 B2 一次扫出来的）。现在同一个词表由 `probe.mjs` 的 `sweep`（在 `verify` 里、
 * 逐条渲染、不需要真机）与**本脚本**（真机、带 token）**共用** —— 一份权威，两个执行者。
 * ⚠ **别把词表抄回来**：抄回来就是两个权威，它们一定会漂。
 */

/**
 * ★ 称谓反向词表（2026-09-23 称谓刀）。三替法：**铬件/短标签去掉主语**；句中泛指用
 * `Agent`（大写 A）；页面在手知道名字时用名字。这三条治的是同一个病：
 * 用「Agent」当**泛指**或**铬件标签**（指代不明），以及把小写 `agent` 摆上屏。
 *
 * ⚠ 只钉**这几个形状**，不把「Agent」整个拉黑 —— 房间名「家」、句中指代**具体那个
 * Agent**（如「它不在这台机器上跑」所在的段落）本来就可能合理地用第三人称。
 * 所以词表取的是**具体的病句**，不是主语本身。
 */
const BANNED_SUBJECT = [
  'Agent 正在用', // 徽标 → 「正在使用」
  'Agent 没有', // 「Agent 没有日子」这类 → 读不到 ≠ 它停了
  // ⚠ **2026-09-26 删掉了一条自相矛盾的词条**：这里原有一条 `'没有这个 Agent'`，
  //    而它的注释写的是 `// → 「没有这个 Agent」` —— **替换成它自己**，注释本身是坏的。
  //    更要命的是**同一份脚本第 700 多行那条断言要求正文必须含这句**
  //    （「找不到 Agent 时，页头铬件是「Agent 主页」」），而 `README.md` 也把它写成
  //    有意为之（「认不出时如实说「没有这个 Agent」」）⇒ **词表是错的一方**。
  //    本次真机实跑把它暴露出来：两条断言一红一绿，红的那条在骂绿的那条。
  //    判据收窄回它真正要治的形状：**用「Agent」当泛指或铬件标签**——
  //    「这个 Agent」是**指示代词指一个具体（只是名字不在手）的 Agent**，不是泛指。
  '正在读取 Agent 的', // 过渡态模板里的指代
  'Agent 的主页', // 铬件标签 → 「Agent 主页」（找不到时名字本就不在手，不能「Agent 的 X」）
]
const leakedSubject = (text) => BANNED_SUBJECT.filter((word) => text.includes(word))

/**
 * ★ 假路径反向词表（2026-09-24 诚实刀）：这两句曾经指着一扇**不存在的门**。
 *
 * 为什么是假路径：v1 没有给一个**已存在的** Agent 安家的路径（`home.js` 只有
 * `up|status|down|config|issue-home-key`），而模型绑定是"**某台机器上那个 Agent**"的事
 * —— 一个没有家的 Agent 在 Agent 的模型房间就是「未接入」，根本绑不上。
 * ⇒ 这两句**一个都不许再上屏**（`/app/agents` 列表页）。
 *
 * ⚠ 与 `JARGON` / `BANNED_SUBJECT` 同款：只钉这两条**具体的假路径**，不是整个话题。
 * ★ **2026-10-11 起「设置 → 模型配置」这句话在任何地方都是假的**：那一页已经**按机器寻址**
 * （`/app/runtimes/:entryId/config`），设置目录里不再有它 —— 所以这句话**不再有"仍然是真的"
 * 那一处**，禁的是全仓（从前那句括号里写的是"对住在这台机器上的 Agent 仍然是真的"，
 * 那是 2026-10-11 之前的事）。
 */
const BANNED_FALSE_PATH = ['在这台机器上安家', '设置 → 模型配置']
const leakedFalsePath = (text) => BANNED_FALSE_PATH.filter((phrase) => text.includes(phrase))

/**
 * ★ 设置目录的**组标题**（刀3，2026-09-24）：设置侧栏曾渲染出「家」「家」**连着两行**
 * —— 组标题是「家」、组里唯一条目也叫「家」（`pages/settings/SettingsLayout.tsx` 的
 * `NAV_GROUPS`）。「家」的语义是**Agent 住的那台机器**（执行端），组标题该说它**是什么**
 * （你的机器），不该与唯一条目同名。
 *
 * ⚠ **2026-10-12：「家」那一项退役**（机器清单只留一个门 —— 清单、加一台、管一台都在
 * `/app/runtimes` 那一侧）⇒ 这一组现在只剩「本机服务」一条。判据仍是"组标题不许与唯一条目
 * 同名"：组标题说它**是什么**（你的机器），条目说**这一页干什么**。
 */
const SETTINGS_GROUP_TITLE_EXPECTED = '你的机器'
const SETTINGS_GROUP_TITLE_BANNED = '家'
/** 组标题里那些**说错话**的（判据本体；正反例共用它，正对照才有意义）。 */
const titlesSayingWrongThing = (titles) =>
  titles.filter((title) => title === SETTINGS_GROUP_TITLE_BANNED)

/**
 * ★ 设置目录的**轴**（2026-10-04 轴修；2026-10-11 第三刀再修）。
 *
 * 2026-10-03 之前，「模型配置」与世界事实同挤在一组，组标题叫 **「Agent 与服务器」**
 * —— 那把**机器事实**与**世界事实**混成一组，与主导航那条轴不一致。
 * 那次裁决：那一组拆成「**这个世界**」与「**你的机器**」。
 *
 * ★ **2026-10-11 第二刀**：「模型配置」**整个搬出设置目录**（按机器寻址，
 * `/app/runtimes/:entryId/config`）—— 它归「你的机器」这条轴**判得对**，但不该住在
 * **没有机器上下文**的地址里。
 *
 * ★ **2026-10-11 第三刀（本条）**：`/settings/server` 从「这个世界」挪进「**你的机器**」，
 * 名字从「服务器」改成「**本机服务**」，那一页也收成一个主语（去掉两行世界侧读数）。
 * ⇒ 「这个世界」这一组**空了 ⇒ 删掉**（设置区里世界侧只有"你连哪儿"，它在「通用」里）。
 *
 * ★ **2026-10-12 第四刀（本条）**：「家」那一项**退役**（它是同一份机器清单的第二个门）——
 * 这一组现在只剩「本机服务」一条；清单侧的动作（加一台 / 管一台）都在 `/app/runtimes` 那一侧。
 *
 * ⚠ **这条断言量的是"目录里有没有这两条、在哪一组"** —— 它是**渲染后的正文**级读数，
 * 只证明"目录照着这份表长出来了"。**"这条该归哪一组"本身**由
 * `tests/settings-axis.test.ts` 顺着 `routeRegistry` 的 `planes` 机械钉住
 * （2026-10-11 加：旧判据只认字符串 ⇒ 把一条 `planes: ['home']` 的页挪进 world 组照样绿，
 * 而那一刀真撞上了这件事）。
 */
const SETTINGS_AXIS = [{ item: '本机服务', group: '你的机器' }]
/** 那个把两轴混在一起的**旧组标题**：它回来了就是轴又错了。 */
const SETTINGS_GROUP_TITLE_RETIRED = 'Agent 与服务器'
/** 轴说错话的那些（判据本体；正反例共用它，正对照才有意义）。 */
const axisSayingWrongThing = (groups) =>
  SETTINGS_AXIS.filter(({ item, group }) => {
    const owner = (groups ?? []).find((entry) => (entry?.items ?? []).includes(item))
    return !owner || owner.title !== group
  })

/** 小写 `agent`（拉丁）上屏：中文界面里该写 `Agent`。只在**拉丁词边界**上判，不误伤代码/标识。 */
const leakedLowercaseAgent = (text) => (/\bagent\b/.test(text) ? ['agent'] : [])

/**
 * ⚠ 下面三个常量**照抄**自 `src/lib/renderFailure.ts`（本脚本是 `.mjs`，引不到仓内的
 * `.ts`）。**改那边就要同步改这里** —— 否则"chunk 拿不到要给降级页"那三条断言会
 * **静默失效**（TASK_58 吃过一次这个形状：守卫写死了被守卫对象的文案，改名后守卫哑火）。
 */
const RELOAD_LABEL = '重新加载'
const CHUNK_FAILURE_TITLE = '页面已更新，需要重新加载'
const RENDER_FAILURE_TITLE = '这个页面出错了'

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

const browser = await chromium.launch({
  headless: !args.headed,
  args: ['--no-proxy-server'],
  executablePath: process.env.CELIRA_BROWSER_EXECUTABLE?.trim() || undefined,
})
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })

const consoleErrors = []
page.on('pageerror', (error) => consoleErrors.push(`pageerror: ${error.message}`))
page.on('console', (message) => {
  if (message.type() !== 'error') return
  // 带上来源 URL。只报"有 N 条错误"而不给 URL 等于没法诊断 —— 实测踩过：看到
  // 「5 条 404」却不知道是哪个资源，只能重跑一轮。⚠ 本刀那条"chunk 拿不到"的断言
  // 会**故意**造出 404（拦下几个 JS），所以这里的计数含那几条预期内的。
  const where = message.location()?.url
  consoleErrors.push(where ? `${message.text()} ← ${where}` : message.text())
})

/**
 * ★ 页内异常记录器（2026-10-03）：为什么必须用它、去重口径、console 类消息为何仍未覆盖，
 * 见文件头「页内异常记录器」。与 `probe.mjs` 同形（那边挂 context，这边单页）——**两边同步改**。
 * 记录器账本**每个文档一份**（每次导航重新开始）：下面「进房间」那条断言要的正是
 * "进房间那一刻"这一份，不是整场会话的累计。
 */
const IN_PAGE_ERROR_RECORDER = () => {
  const bucket = []
  Object.defineProperty(window, '__celiraProbeErrors', { value: bucket, configurable: true })
  window.addEventListener('error', (event) => {
    bucket.push(`error: ${event?.message ?? String(event?.error ?? event)}`)
  })
  window.addEventListener('unhandledrejection', (event) => {
    const reason = event?.reason
    bucket.push(`unhandledrejection: ${String(reason?.message ?? reason)}`)
  })
}
// 在任何导航之前装上（本脚本只有这一页，一次装够）。
await page.addInitScript(IN_PAGE_ERROR_RECORDER)

/** 读回页内记录器（读不到返回 null —— 「读不到 ≠ 没有」，调用方不许当成零异常）。 */
async function readRecordedErrors() {
  return page
    .evaluate(() => (Array.isArray(window.__celiraProbeErrors) ? window.__celiraProbeErrors.slice() : null))
    .catch(() => null)
}

/** 归一化口径（去重只认它）：剥通道前缀与 `Uncaught [(in promise)]`、`Error:` 前缀。 */
function normalizeUncaughtLine(line) {
  return String(line)
    .replace(/^(?:pageerror|error|unhandledrejection):\s*/, '')
    .replace(/^Uncaught(?:\s+\(in promise\))?\s*/, '')
    .replace(/^Error:\s*/, '')
    .trim()
}

/**
 * 合并两条来路并去重：记录器那份为准，CDP 流里逐条对得上的丢掉、对不上的补进来 ——
 * 同一条异常只算一条（将来换到会发 `Runtime` 事件的引擎时不许重复计数）。
 */
function mergeUncaughtExceptions(cdpErrors, recordedErrors) {
  const pool = recordedErrors.map(normalizeUncaughtLine)
  const unmatched = []
  for (const line of cdpErrors) {
    const index = pool.indexOf(normalizeUncaughtLine(line))
    if (index >= 0) {
      pool.splice(index, 1)
      continue
    }
    unmatched.push(line)
  }
  return [...recordedErrors, ...unmatched]
}

/**
 * 运行状态那一次请求**落在哪个源上**（TASK_61 的判据来源）。
 *
 * 为什么盯响应而不盯页面文字：页面文字只能证明"读到了状态"，证明不了**它怎么读的** ——
 * 编死的 `http://127.0.0.1:5190` 同样能读出真实读数。而本刀的全部主张是
 * 「页面只问**同源**的 `/home-status/*`，由代理代转 ⇒ 页面永远不知道端口」。
 * ⇒ 判据必须落在**请求本身**上：响应 URL 的 path 得是 `/home-status/…`、origin 得是页面自己的。
 * 也正因为如此，"有人把它改回绝对地址"会让这里**一条都收不到**（path 不再是 `/home-status/`），
 * 于是断言自然变红 —— 负对照是**内建**的，不用另写。
 */
const pageOrigin = new URL(args.base).origin
const statusCalls = []
page.on('response', (response) => {
  let parsed
  try {
    parsed = new URL(response.url())
  } catch {
    return
  }
  if (!parsed.pathname.startsWith('/home-status/')) return
  statusCalls.push({ origin: parsed.origin, path: parsed.pathname, status: response.status() })
})

/**
 * 本会话**已经请求过**的 JS 模块（供末尾"chunk 拿不到要先说话"那条断言用）。
 * 那条断言只放行这里面的 URL，其余 `/assets/*.js` 一律 404 ⇒ 精确复现
 * 「宿主换版后，旧标签页去取旧 chunk」那条路。
 */
const loadedJs = new Set()
page.on('request', (request) => {
  const url = request.url().split('?')[0]
  if (/\/assets\/.*\.js$/.test(url)) loadedJs.add(url)
})

/**
 * 本会话**打开过**哪些房间（末尾那几条读数判据要挑一个"从没打开过"的房：
 * 缓存与内存里都没有它的消息，于是"读失败"这一支不会被旧消息掩盖）。
 */
const visitedRoomIds = new Set()
page.on('request', (request) => {
  const matched = /\/v1\/rooms\/([^/?]+)\/messages/.exec(request.url())
  if (matched) visitedRoomIds.add(matched[1])
})

/**
 * 打不开页面（base 写错 / 服务没起）不是"某个断言失败"，而是**环境没准备好** ——
 * 要单独说清并给一个不同的退出码，否则会被当成"UI 有毛病"去查产品。
 */

/**
 * 等**降级页**到场（最多 15 秒）。与上面两支同款纪律：**等状态不等时长** ——
 * 降级页一到就返回；别的形状（正常渲染的页、空白）也立刻返回，交给断言把原文报出来。
 * 整页导航后壳层还有一次会话自举，所以不能睡固定时长。
 */
async function waitForBoundaryText(page) {
  const deadline = Date.now() + 15000
  let text = ''
  while (Date.now() < deadline) {
    text = await page.locator('body').innerText()
    if (text.includes(RELOAD_LABEL)) return text
    const transient = text.includes('正在准备连接') || text.includes('正在读取')
    if (!transient && text.trim().length > 0) return text
    await page.waitForTimeout(400)
  }
  return text
}

async function goto(url, note) {
  try {
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 20000 })
  } catch (error) {
    console.error(`\n环境错误：打不开 ${url}（${note}）`)
    console.error(`  ${error.message.split('\n')[0]}`)
    console.error('  ⇒ 先确认服务在跑、base 写对了；本脚本自己不做起服务的事。')
    if (!args.keepOpen) await browser.close()
    process.exit(2)
  }
}

try {
  // ---- 进门：core 直连（唯一通道）----
  await goto(`${args.base}/`, '直连入口面')
  await page.waitForTimeout(2500)
  // ⚠ **2026-10-05 修"门禁只能跑一次"**：这条门禁的浏览器 profile 会**跨次留存** —— 上一次跑留下的
  //    会话与偏好还在里面（**会话不只在 localStorage：客户端还把它存进 IndexedDB**）⇒ 这一次页面
  //    带着一支过期的会话去连、拿到 404、**卡在品牌启动页**，于是下面"填 token"永远等不到登录页，
  //    整条门禁 30 秒超时退出，看着像"环境没就绪"。实测：第一次跑全绿，第二次就起不来 ——
  //    而"改用户可见面时必须跑它"最需要它的正是第二次。**门禁必须从已知状态开始**：
  //    两处存储都清掉再重载，登录页才会如期出现。
  await page.evaluate(async () => {
    window.localStorage.clear()
    const dbs = (await indexedDB.databases?.()) ?? []
    for (const db of dbs) if (db.name) indexedDB.deleteDatabase(db.name)
  })
  await page.reload({ waitUntil: 'domcontentloaded' })
  await page.waitForTimeout(2500)
  const tokenField = page.getByLabel('Bearer token')
  if ((await tokenField.count()) > 0) {
    await tokenField.fill(args.token)
    await page.getByRole('button', { name: '连接', exact: true }).click()
    await page.waitForTimeout(4500)
  } else {
    console.log('  （没有登录页：这一颗浏览器本来就有会话 —— 直接用命令行给的这支 token 继续）')
  }
  // ⚠ **2026-09-29 修假绿**：这一条原先只写「正文里不含『连接你的世界』」—— 而**品牌启动页
  //    也不含那句话** ⇒ 登录后卡在 `/splash` 时它照样绿。真机实测：当时就是这个状态，
  //    本脚本 83/83 全绿，而人从登录页**根本进不去应用**（只有手敲 /app/chat 才进得去）；
  //    紧接着下面 `goto('/app/chat')` 又硬跳走，把问题彻底盖住。
  //    改成**正面断言落点**：真的落进壳层（URL 在 /app 下 ＋ 主导航在场）。
  const landingPath = new URL(page.url()).pathname
  const shellNav = await page.locator('nav[aria-label="主导航"]').count()
  const landing = await page.locator('body').innerText()
  check(
    '直连登录成功：真的落进应用（不是停在登录页 / 品牌启动页）',
    landingPath.startsWith('/app') && shellNav > 0 && !landing.includes('连接你的世界'),
    `pathname=${landingPath} · 主导航=${shellNav} · ${snippet(landing)}`,
  )

  // ---- 壳：导航栏只挂在 /app 壳里（Splash 上没有）----
  await goto(`${args.base}/app/chat`, '壳层首页')
  await page.waitForTimeout(4000)

  const groups = await page
    .locator('[role="group"]')
    .evaluateAll((els) => els.map((el) => el.getAttribute('aria-label')))
  check(
    `导航分为两组（${EXPECTED_GROUPS.join(' / ')}）`,
    EXPECTED_GROUPS.every((label) => groups.includes(label)),
    `实际 ${JSON.stringify(groups)}`,
  )

  const navLabels = await page
    .locator('nav a[aria-label]')
    .evaluateAll((els) => els.map((el) => el.getAttribute('aria-label')))
  const uniqueNav = [...new Set(navLabels)]
  const missing = EXPECTED_NAV.filter((label) => !uniqueNav.includes(label))
  check('一级入口齐全', missing.length === 0, missing.length ? `缺 ${JSON.stringify(missing)}` : `实际 ${JSON.stringify(uniqueNav)}`)
  check(
    '「家」组只剩一个一级入口：运行状态 / Agent 的经历 / 模型配置 都不再是一级入口（刀2a）',
    !['运行状态', 'Agent 的经历', '模型配置', '模型服务'].some((label) => uniqueNav.includes(label)),
    `实际 ${JSON.stringify(uniqueNav)}`,
  )

  // ---- 我的：**人自己的名字与头像**（表达权第四刀，2026-10-17 主人批准）----
  //
  // 判据：真实世界里这一页必须给得出**两个入口** ——
  // 「改名字」（`display_name` 是身份事实 ⇒ **状态写**：无版本号、无幂等键）
  // 与「加头像 / 换头像」（呈现版本链 ⇒ 带 base_version ＋ 幂等键）。
  // 它们**不合成一个"保存资料"**：两件事、两个入口（一个端点说一件事）。
  //
  // ⚠ 演示通道（未连接）**不摆这两块**（没有世界可写）—— 而这条门禁本来就跑在
  //   临时世界的**真 token** 上，所以这里判"在不在"是有效的。
  await goto(`${args.base}/app/profile`, '我的')
  await page.waitForTimeout(2000)
  const meText = await page.locator('body').innerText()
  check(
    '★ 我的：改名字 ＋ 加/换头像两个入口都在（人自己的资料可改写）',
    meText.includes('改名字') && (meText.includes('加头像') || meText.includes('换头像')),
    snippet(meText, 200),
  )
  check(
    '★ 我的：自己的脸借**房间成员路径**读（不新开读取面）—— 页面必须如实说清能不能看到',
    meText.includes('房间成员') || meText.includes('还没有任何房间'),
    snippet(meText, 200),
  )
  // ⚠ 2026-10-20：这格原先是一颗**没有 onClick 的 `Copy` 图标**（看着能复制、按了什么也不发生）。
  //    加人/加 Agent 在这个世界里唯一的握手就是"对方把这串 ID 给你"，所以它是真按钮才算数。
  const copyIdButtons = await page.getByRole('button', { name: '复制身份 ID' }).count()
  check(
    '★ 我的：「身份 ID」旁边那颗复制是**真按钮**（不是画上去的图标）',
    copyIdButtons === 1,
    `复制按钮=${copyIdButtons}`,
  )

  // ---- 我的 Agent：**列表**（刀2a 起点一个 Agent ⇒ Agent 的主页；填肉刀起每行是三格读数）----
  await goto(`${args.base}/app/agents`, '我的 Agent 列表')
  await page.waitForTimeout(4500)
  const listText = await page.locator('body').innerText()
  // 住址那一格（`machineMatchLabel` 三态）＋ 模型 ＋ 最近活动。
  // ⚠ 这一条**不许等一个具体取值**：状态口没开时那几格本来就说「不知道」/「未知」，
  //    而那是**终态**、不是加载中（判据在 `api/agentRowStatus.ts`，有离线用例）。
  //    所以这里判的是"那几格在不在"，并把**实际读到的**住址打进 detail（真实读数）。
  // ★ 2026-10-20（T2 ②）：住址那一格从「在本机运行 / 未接入 / 状态未知」改成
  //    「在这台机器上 / 不在这台机器上 / 不知道在哪台机器上」—— 它**只答住址**，
  //    "在不在跑"只归机器那一句（同一行两个答案不许并排）。⚠ 顺序有讲究：
  //    `不在这台机器上` 里含着 `在这台机器上` 六个字，所以更长的写法必须排在前面
  //    （`find` 取第一个命中的）。
  const ONLINE_LABELS = ['不在这台机器上', '不知道在哪台机器上', '在这台机器上']
  const onlineSeen = ONLINE_LABELS.find((label) => listText.includes(label)) ?? ''
  // ★ 2026-10-20（T2 ④）：一份读数都没读回来时，那一行**收成一句**（原因 ＋ 出口），
  //    不再并列四个"不知道"。两种形状都合法 —— 这一条判的是"读数那一段摆得出来"。
  const collapsed = listText.includes('读不到它现在怎么样')
  check(
    '列表行读数：住址（三态之一）+ 模型 + 最近活动（或四格同因收成一句 ＋ 出口）',
    collapsed
      ? listText.includes('去我的执行端')
      : onlineSeen !== '' && listText.includes('模型：') && listText.includes('最近活动：'),
    collapsed
      ? `四格同因 ⇒ 收成一句 · ${snippet(listText, 200)}`
      : `住址那一格=「${onlineSeen}」· ${snippet(listText, 200)}`,
  )
  check(
    '★ 最近活动那一格不把原始行摆出来（`run=…` / `drives=…` 不许上屏）',
    !listText.includes('run=') && !listText.includes('drives='),
    snippet(listText, 200),
  )
  // 机器侧那一句（2026-10-06 加）：**它现在有没有在跑 ＋ 上次怎么样**。
  // 判据与文案在 `api/agentHealth.ts`（纯函数 ＋ 离线用例），这里只判"它在不在屏上"。
  // ⚠ 不许等一个具体取值（状态口没开时那一句本来就说「读不到」，而那是终态）。
  //
  // ⚠ **2026-10-10 补前提**：这一句**只对"这台机器承载的那支"有对象**——临时世界里那几支
  // Agent 若一支都不由本机承载，那一句**本来就不该出现** ⇒ 这时按本文件头那条纪律报
  // **不适用**，不是红。判据：屏上有没有**至少一行**是 claimed（住址那一格说「在这台机器上」）。
  // ★ 2026-10-20（T2 ①）：那一句的四档改成"主语是**它**"（`它正在跑` / `它现在没在跑（…）` /
  //    `它现在跑没跑读不到`）—— 从前的主语是宿主进程，与住址那一格并排就是两个相反的答案。
  const machinePremise = listText.includes('在这台机器上')
  check(
    '列表行有机器侧那一句：它现在有没有在跑 ＋ 上次怎么样',
    machinePremise
      ? ['它正在跑', '它现在没在跑', '它现在跑没跑读不到', '机器这一面读不到'].some((s) =>
          listText.includes(s),
        )
      : null,
    machinePremise
      ? snippet(listText, 200)
      : `这一轮没验：这台机器一支都没承载（屏上没有「在这台机器上」）⇒ 那一句本来就没有对象 · ${snippet(listText, 160)}`,
  )
  check(
    '★ 机器那一格不把口子的原始取值摆出来（`running` / `stopped` / `pid` 不许上屏）',
    !/\b(running|running-orphan|stopped)\b/.test(listText) && !listText.includes('pid'),
    snippet(listText, 200),
  )
  check(
    '列表行文案没有工程黑话（人话自检）',
    leakedJargon(listText).length === 0,
    leakedJargon(listText).length ? `漏了 ${JSON.stringify(leakedJargon(listText))}` : snippet(listText, 160),
  )

  // ---- 内联创建面板：两条路（**只点不写**）----
  //
  // ⚠ 打开、读、关掉 —— **不真创建**。真机门禁不该改主人的家：真建一个 Agent 多一条脏数据，
  //    而这几条判据要的是"面板挂得上、两条路各自是什么形状"，表单挂载即已可判。
  await page.getByLabel('添加 Agent').click()
  await page.waitForTimeout(400)
  const createText = await page.locator('body').innerText()
  const createInputs = await page.locator('input').count()
  check(
    '内联创建面板挂得上来：默认路「创建」+ 一句「创建后它不会回复 —— 还没有模型，也没有机器在跑它」+ 名字输入框',
    createText.includes('创建 Agent') &&
      createText.includes('创建后它不会回复 —— 还没有模型，也没有机器在跑它') &&
      createInputs >= 1,
    `输入框=${createInputs} · ${snippet(createText, 200)}`,
  )
  // ★ **2026-10-20（T1 ①③）：创建面板顶部是「三步清单」** —— 模型源 / 建一支 / 接上一台机器。
  // 目标句：新账号 + 一台空机器，从 `/app/agents` 出发、不查文档，就能看见"还差哪一步、各去哪"。
  // 判据两条：① 三步的名字与 ✓/✗ 在场；② **缺模型源那一步自带出口**（不是只印一句"它不会回话"）。
  // ⚠ 前提：模型那一格读得到（判据在 `api/firstRun.ts`：**读不到不摆**）——门没开就报不适用。
  const configDoorOpen = await page.evaluate(async () => {
    try {
      const res = await fetch('/home-config')
      return res.ok
    } catch {
      return false
    }
  })
  check(
    '★ 创建面板顶部：三步清单（模型源 / 建一支 / 接上一台机器）＋ 缺模型源那一步带出口',
    configDoorOpen
      ? ['模型源', '建一支', '接上一台机器', '✓', '✗'].every((word) => createText.includes(word)) &&
          createText.includes('去配模型源')
      : null,
    configDoorOpen
      ? snippet(createText, 260)
      : '⚠ 不适用：本机模型配置门没开（/home-config 不通）⇒ 那一格读不到，清单整块不该摆',
  )
  const localOption = page.locator('button', { hasText: '只创建本地' })
  const localDisabled = await localOption.isDisabled().catch(() => false)
  check(
    // ⚠ 2026-09-26 改判据：tooltip/徽标从「即将支持」改成「**尚未支持**」。
    // 理由是 R15 措辞纪律：**"尚未"是当下的事实，"即将"是对未来的承诺**
    // （见 `DESIGN_SYSTEM.md` 与 `AgentsListPanel.tsx` 的注释）。
    // 这条断言原先钉的是旧措辞 —— 前端大整改 B2 改了代码却没同步这里，
    // 于是它在真机上**第一次跑就红了**（本次实跑发现）。
    '内联创建：进阶路「只创建本地」是**禁用**态 + tooltip「尚未支持」（诚实降级，不承诺"即将"）',
    (await localOption.count()) === 1 &&
      localDisabled &&
      (await localOption.getAttribute('title')) === '尚未支持' &&
      createText.includes('尚未支持'),
    `count=${await localOption.count()} disabled=${localDisabled} tooltip=${await localOption.getAttribute('title')}`,
  )
  check(
    '内联创建：「入籍」这类设计词汇不上屏（用户语言里它叫「创建」）',
    !createText.includes('入籍'),
    snippet(createText, 160),
  )
  // ★ 假路径反向词表（2026-09-24 诚实刀）：列表页整页正文里，那两句**指着一扇不存在的门**
  //   的话一个都不许在场。先做一次**正对照**（词表本身要真会红），再扫真页面 ——
  //   防止"词表写了却恒不命中"这种装饰性覆盖（与称谓刀那条同一纪律）。
  const falsePathProbe = leakedFalsePath(`${createText}\n${BANNED_FALSE_PATH.join('\n')}`)
  check(
    '假路径词表正对照：注入词表自身的每一条都必须被抓到（否则难说是正对照）',
    BANNED_FALSE_PATH.every((phrase) => falsePathProbe.includes(phrase)),
    JSON.stringify(falsePathProbe),
  )
  const createFalsePath = leakedFalsePath(createText)
  check(
    '列表页不出现假路径：「在这台机器上安家」「设置 → 模型配置」一个都不在场',
    createFalsePath.length === 0,
    createFalsePath.length ? `漏了 ${JSON.stringify(createFalsePath)}` : '一个都没有 ✓',
  )
  check(
    '内联创建：**不跳走**（地址还停在列表页，不再指去「新的朋友」）',
    /\/app\/agents\/?$/.test(new URL(page.url()).pathname),
    page.url(),
  )
  await page.getByRole('button', { name: '取消' }).click()
  await page.waitForTimeout(400)
  const afterCloseText = await page.locator('body').innerText()
  check(
    '内联创建：「取消」把面板收起来（不留半开的界面）',
    !afterCloseText.includes('创建后它不会回复 —— 还没有模型，也没有机器在跑它') &&
      !afterCloseText.includes('只创建本地'),
    snippet(afterCloseText, 200),
  )

  // 点列表里的第一个 Agent ⇒ **Agent 的主页**（页面按 Agent 编址：`/app/agents/<id>`）
  // ⚠ 选择器换过了：列表行不再写「资料·运行·活动记录·模型」（那是刀2a 的旧文案，
  //    填肉刀换成了三格读数）⇒ 改用行按钮自己的**可访问名**（`打开 <名字> 的主页`）。
  const agentRow = page.getByRole('button', { name: /^打开 .* 的主页$/ })
  check(
    '列表里至少有一行（真机门禁需要这个账号里有 Agent）',
    (await agentRow.count()) > 0,
    `行数=${await agentRow.count()} · ${snippet(listText, 120)}`,
  )
  // ⚠ **2026-09-26 加这道前置闸（本次真机实跑踩到）**：原先这里只记一条 FAIL 就往下走，
  // 而紧接着的 `agentRow.first().click()` 在**零行**时会等 30 秒然后抛
  // `locator.click: Timeout 30000ms exceeded` —— 门禁脚本**崩在半路**，
  // 后面几十条断言一条都没跑，而且看到的是个栈，不是"缺什么"。
  // 现在：前置不满足就**明确报出来并以 2 退出**（脚本自己文档化的"用法/环境错误"码），
  // 把"这个账号里没有 Agent"直接说成原因，并给出怎么建。
  if ((await agentRow.count()) === 0) {
    console.error('\n前置不满足：这个账号里一个 Agent 都没有。')
    console.error('  本脚本从这一行往下（Agent 主页 / 运行 / 活动 / 模型 / 绑定 / 凭据 / @ 候选）')
    console.error('  全都要有 Agent 才跑得下去 —— 与其崩在半路，不如在这里停下说清楚。')
    console.error('  建一个：POST /v1/agents  {"display_name":"…"}（带 Bearer token 与 Idempotency-Key）')
    if (!args.keepOpen) await browser.close()
    process.exit(2)
  }
  await agentRow.first().click()
  await page.waitForTimeout(4500)
  check(
    '点进 Agent 的主页（地址变成 /app/agents/<id>）',
    /\/app\/agents\/[^/?#]+/.test(page.url()),
    page.url(),
  )

  const homeText = await waitForAgentHomeText(page)

  // ★★ 刀2（门禁两态收紧，2026-09-24）：三条判据由**口子的实际状态**说，不再用 `||` 把
  //    「渲染真实数据」与「如实降级」并列 —— 口子明明有数据、页面却是降级/空白时，旧判据
  //    照样绿（**健康检查的判据不许有"或"**）。
  //
  // 先探一次状态口（**从页面自己的 origin** 打，与页面走同一条路）：探到可用 JSON ⇒ 口子开着，
  // 页面**必须**渲染真实数据；探不到 ⇒ 口子关着，页面**必须**如实降级。
  // ⚠ 2026-09-28：这个区分现在只用来**给断言起名**（`gateLabel`）与决定"活动记录起源"
  //    那一块跑不跑 —— 三个房间的判据已改回**机器页自己的契约**（见下面那段注释）：
  //    "读数是不是你在看的那一支"不再决定房间该怎么渲染，"上面跑的是谁"由机器页自己
  //    如实说（那条「四态之一」的断言管它）。
  const agentIdInUrl = new URL(page.url()).pathname.split('/').filter(Boolean).pop() ?? ''
  const statusProbe = await page.evaluate(() =>
    fetch('/home-status/status')
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null),
  )
  const agentAgentIdOnMachine = statusProbe?.agent?.agentId ?? null
  const gateOpen = typeof statusProbe === 'object' && statusProbe !== null
  const gateClaimsAgent =
    gateOpen && (agentAgentIdOnMachine === null || agentAgentIdOnMachine === agentIdInUrl)
  const gateLabel = !gateOpen
    ? '口子关着'
    : gateClaimsAgent
      ? '口子开着·读数就是 Agent'
      : '口子开着·读数不是 Agent'

  // 主人三问（2026-09-22 裁决）在页面上有答案 —— 这三条就是刀2a 的验收。
  // ⚠ 2026-09-26 修陈旧：这里原本断言 Agent 主页有**四个**房间
  // （资料 / 运行 / 活动记录 / 模型）—— 而 2026-09-25 组织轴刀之后 Agent 主页
  // **只剩两间**（资料 / 服务），运行 / 活动 / 模型搬去了**机器页**
  // `/app/runtimes/:entryId`。旧断言在真机上**必然红**，且它从没被跑过
  // （`npm run verify` 不含本脚本）。三个房间的断言见下面 `runtimes` 那一段。
  //
  // ⚠ 2026-09-28 再修一次，这次是**假绿**（真机实跑发现）：上面那版改成
  //    `for (const room of ['资料','服务'])` 之后，判据是**裸子串** —— 而这一页正文里
  //    本来就有「**服务**器上的数据」「本机**服务**端」⇒ 「服务」那一条**恒绿**。
  //    服务层 2026-09-28 整体退役，`MyAgentsPage.tsx` 的 `AgentHome` 现在只渲染**一个**
  //    `SectionFrame` ⇒ 房间真的没了，而那条断言照样过（装饰性覆盖）。
  //    ⇒ 改成按**房间标题本体**判：`SectionFrame` 把分区名渲成 `section[id] > h2`，
  //      收这些标题，要求「资料」在场、**「服务」不在场**（退役的验收点正在这里）。
  // ★ **2026-10-20（T4：这一页从管理台改成"对象首页"）**：判据改成量**四段与顺序**：
  //   ①「机器」（首屏那句 health 的唯一出处）②「资料」③「接线（高级）」④「危险操作」，
  //   且「服务」仍然不在场（2026-09-28 服务层退役那条验收点不丢）。
  const roomTitles = await page
    .locator('section[id]')
    .evaluateAll((els) => els.map((el) => (el.querySelector('h2')?.textContent ?? '').trim()).filter(Boolean))
  const expectedRooms = ['机器', '资料', '接线（高级）', '危险操作']
  check(
    '★ Agent 的主页四段且顺序对：机器 → 资料 → 接线（高级）→ 危险操作（且「服务」不在场）',
    expectedRooms.every((title, index) => roomTitles[index] === title) && !roomTitles.includes('服务'),
    `房间标题=${JSON.stringify(roomTitles)} · ${snippet(homeText, 120)}`,
  )

  // ★ **2026-10-20（T4）首屏那两个动作**：页头就是「发消息」与「在世界里看」——
  // "去跟它说话"必须是**能点的一颗**（复用 `api/directRoom.ts` 那一处判据），
  // 而"它在世界里的样子"是去 `/agent/:id`（那一页才是唯一版面，这里不重画）。
  check(
    '★ 首屏第一动作：页头有「发消息」与「在世界里看」两颗，且「发消息」是首屏第一个动作按钮',
    (await page.getByRole('button', { name: '发消息' }).count()) === 1 &&
      (await page.getByRole('button', { name: '在世界里看' }).count()) === 1,
    snippet(homeText, 160),
  )
  // ★ **接线（高级）默认折叠**（判据在页内 `wiringOpen`）：这一支名下有生效的接入 ⇒ 收着、
  //   但折叠头**带读数**（"藏起来的是细节，不是事实"）。反例：默认态写成展开、或读数丢了。
  const wiringCollapsed = await page.locator('section#wiring').innerText()
  check(
    '★ 接线（高级）：有生效的接入时默认折叠 ＋ 折叠头带一行读数（1 条接入 / N 张凭证生效中）',
    wiringCollapsed.includes('条接入') && !wiringCollapsed.includes('解除接入'),
    snippet(wiringCollapsed, 200),
  )
  // 展开之后内容必须在（折叠不是删除）——这一条同时也是"要人动手的那一格没被藏死"的反证。
  const expandWiring = page.getByRole('button', { name: '展开' })
  const hadExpand = (await expandWiring.count()) > 0
  if (hadExpand) await expandWiring.first().click()
  await page.waitForTimeout(400)
  const wiringExpanded = await page.locator('section#wiring').innerText()
  check(
    '★ 接线（高级）：点「展开」之后接入 / 凭证的内容**真的在**（折叠不是删除）',
    wiringExpanded.includes('解除接入') && wiringExpanded.includes('凭证'),
    `展开按钮=${hadExpand} · ${snippet(wiringExpanded, 200)}`,
  )
  // 「机器」那一段的第二句（2026-10-06 加）：**住址之外**还要说"机器在不在跑 ＋ 上次怎么样"。
  // 判据与文案在 `api/agentHealth.ts`（纯函数 ＋ 离线用例），与「我的 Agent」列表**同一句**
  // —— 两处渲染同一句话，所以两处都要有人跑。
  // ⚠ **归属判据**（这一条是负对照，不是重复断言）：只有"这台机器报的就是它"时才该有这一句。
  //    说不准住哪台（`unknown-id`）或那台跑的是别人（`elsewhere`）时，把那台的账本读数摆上来
  //    就是**张冠李戴** ⇒ 那三档里这一句必须**不在场**。
  const machineSection = await page
    .locator('section#machine')
    .innerText()
    .catch(() => '')
  const machineClaimsThisAgent = gateOpen && agentAgentIdOnMachine === agentIdInUrl
  check(
    machineClaimsThisAgent
      ? 'Agent 主页「机器」那一段：住址 ＋ 它现在有没有在跑 ＋ 上次怎么样'
      : 'Agent 主页「机器」那一段：说不准是哪台时，不给别台的账本读数（负对照）',
    machineClaimsThisAgent
      ? ['它正在跑', '它现在没在跑', '它现在跑没跑读不到', '机器这一面读不到'].some((s) =>
          machineSection.includes(s),
        )
      : !machineSection.includes('上次：'),
    snippet(machineSection, 160),
  )
  // ★ **2026-10-14「选择先于接上」**：它**还没被接到这台机器上**时，"用哪一条模型"那一格
  //   也要在场 —— 从前它只在"某台机器报的就是它"（`claimed`）时才摆，于是接它时壳按
  //   "选哪条 = 花谁的钱，不替它决定"停下来，而"选"的那一格不摆：**两边互等**
  //   （主人当次撞上的就是这一格）。
  //   前提：本机那台的清单**读得到**（读不到就没得选，页面如实说"这一格读不到"）——
  //   那时这一条按**不适用**报（`null`），与人话门禁那条纪律同一档，不记 FAIL。
  //   判据本体（该指哪台机器）在 `api/agentMachine.ts` 的 `modelChoiceMachine`（纯函数＋用例），
  //   这里只量"那一格有没有上屏"。
  //   ⚠ **本夹具那支 Agent 是住户**（临时家里就它一支）⇒ 这一轮走的是**住户档**那一条分支；
  //      "还没接上"那一档由 `tests/agent-machine.test.ts` ＋ 门侧用例钉住（那要夹具里多一支
  //      没被接上的 Agent —— 那是装配的活，不在这里造）。
  const modelChoiceVisible = machineSection.includes('它用哪一条模型')
  const modelChoiceUnhomed = machineSection.includes('先给它选好用哪一条')
  const homeConfigReadable = await page.evaluate(async () => {
    try {
      return (await fetch('/home-config')).ok
    } catch {
      return false
    }
  })
  check(
    machineClaimsThisAgent
      ? 'Agent 主页「机器」那一段：这一支用哪条模型那一格在场（住户档，与机器页同一份读数）'
      : '★ 「选择先于接上」：还没接上这台机器时，选模型那一格也在场（接上就用它）',
    machineClaimsThisAgent ? modelChoiceVisible : homeConfigReadable ? modelChoiceUnhomed : null,
    machineClaimsThisAgent || homeConfigReadable
      ? snippet(machineSection, 200)
      : '⚠ 不适用：本机模型配置门没开（/home-config 不通）⇒ 没有可选的钥匙，那一格本来就不该摆',
  )

  // ★★ **2026-10-15「让它去哪台机器上跑」那一刀**：那一格要在场，而且**主语写在每一行上**。
  //
  // 判据（都能被反例打红）：① 那一格的标题在场；② 那句硬事实（**搬家**，V1：同一支同时只在
  // 一台机器上跑）在场 —— 少了它"接上第二台"会被读成"多开一份"；③ 夹具里这一支是**住户**
  // ⇒ 本机那一行必须说"**它现在在这台机器上跑**"，而**不摆**那颗按钮（摆一个无意义的按钮
  // 只会让人以为点了会再开一份）。⚠ 夹具只有一台机器（本机）⇒ "外地那一档"由
  // `tests/remote-attach.test.ts` 的纯函数用例钉住（那一档要两台机器才现得出来）。
  check(
    '★ 让它去哪台机器上跑：那一格在场、写着"搬家"那句硬事实，且**已经在跑的那台不摆按钮**',
    machineSection.includes('可以让它去哪台机器上跑') &&
      machineSection.includes('同时只在一台机器上跑') &&
      (machineSection.includes('它现在在这台机器上跑') || !machineClaimsThisAgent),
    snippet(machineSection, 240),
  )

  // ★★ **2026-10-14「选具体的模型」那一刀**：候选摆在脸上 ＋ 能选**型号 / 思考档**，
  //    而且**点一下真的写进去**（页 → 门 → 文件 → 页回来）。
  //
  // 判据四条（都能被一个反例打红）：① 候选那一行**写着型号**（不再靠点下拉才看得见有什么）；
  // ② 那两格（用哪个型号 / 思考档）在场；③ 换思考档之后**读数里就是新的**（真的落了盘）；
  // ④ 「看这家的型号…」问得到，且**连不上是读数**（如实说"没问到"，不画成"这家没有型号"）。
  //
  // ⚠ 夹具那台机器上**本来一条模型源都没有**（临时家）⇒ 这一段**先存一条**（走公开的配置门：
  //   `POST /home-config`）。endpoint 指向**一个刚放掉的端口** ⇒ 试连必然连不上，而
  //   **试连失败不拦保存**（门侧那条纪律）—— 全程只动**临时家**，一个字节都不碰真家。
  if (machineClaimsThisAgent && homeConfigReadable) {
    const closedPort = await (async () => {
      const net = createRequire(import.meta.url)('node:net')
      const probe = net.createServer()
      await new Promise((resolve) => probe.listen(0, '127.0.0.1', resolve))
      const port = probe.address().port
      await new Promise((resolve) => probe.close(resolve))
      return port
    })()
    const saved = await page.evaluate(
      async ([port, model]) => {
        const res = await fetch('/home-config', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            endpoint: `http://127.0.0.1:${port}/v1/chat/completions`,
            apiKey: 'sk-verify-ui-canary-1f42',
            model,
            contextWindow: 8192,
            name: '探针钥匙',
          }),
        })
        const payload = await res.json().catch(() => null)
        return { status: res.status, entryId: payload && payload.entryId ? payload.entryId : null }
      },
      [closedPort, 'probe-model-alpha'],
    )
    check(
      '★ 选模型那一刀：夹具里存得下一条模型源（后面三条的前提）',
      saved.status === 200 && saved.entryId !== null,
      `HTTP ${saved.status} · entryId=${saved.entryId}`,
    )
    await goto(`${args.base}/app/agents/${agentIdInUrl}`, 'Agent 主页（带一条模型源再看那一格）')
    const withSource = await page.locator('section#machine').innerText()
    check(
      '★ 选模型那一刀：候选**摆在脸上** —— 名字 ＋ **型号**都在那一格里（不用点下拉才知道有什么）',
      withSource.includes('探针钥匙') && withSource.includes('probe-model-alpha'),
      snippet(withSource, 220),
    )
    check(
      '★ 选模型那一刀：**用哪个型号** 与 **思考档** 两格在场（型号默认＝这条来源配的那个）',
      withSource.includes('用哪个型号') &&
        withSource.includes('思考档') &&
        withSource.includes('跟这条来源一样'),
      snippet(withSource, 220),
    )
    // ③ **真的写进去**：改思考档 ⇒ 读数里就是它（这一笔落在**临时家**的绑定文件上）。
    let switched = false
    try {
      await page.getByRole('button', { name: '改思考档…' }).first().click()
      await page.getByRole('menuitem', { name: 'max', exact: true }).first().click()
      await page.waitForTimeout(1200)
      switched = (await page.locator('section#machine').innerText()).includes('思考档：max')
    } catch {
      switched = false
    }
    check(
      '★ 选模型那一刀：换了思考档之后**读数里就是新的**（页 → 门 → 文件 → 页走通）',
      switched,
      snippet(await page.locator('section#machine').innerText().catch(() => ''), 200),
    )
    // ④ **"这条钥匙能调哪些型号"**：问得到，且连不上如实说（**不是**"这家没有型号"）。
    let asked = ''
    try {
      await page.getByRole('button', { name: '看这家的型号…' }).first().click()
      // 等它问完（`正在问…` 那一档过去）——门那边给供应商的窗口是 8 秒，这里给足。
      await page
        .getByRole('button', { name: '看这家的型号…' })
        .first()
        .waitFor({ state: 'visible', timeout: 12000 })
        .catch(() => {})
      await page.waitForTimeout(500)
      asked = await page.locator('section#machine').innerText()
    } catch {
      asked = ''
    }
    check(
      '★ 选模型那一刀：问"这条钥匙支持哪些型号" ⇒ 连不上时**如实说没问到**（不画成"没有型号"）',
      asked.includes('没能问到这家'),
      snippet(asked, 260),
    )
    // ④' ★ **机器页那一间的边界**（2026-10-15 主人裁：机器是地方，Agent 是人 —— 机器页只回答
    //    "有什么 / 谁在用"，**"换"永远是给人换**）。趁夹具里还有那条来源，去机器页量三条：
    //    ① 清单那列把**谁在用**写成名字（不再是一枚不点名的「正在使用」徽标）；
    //    ② 那一列**没有"换"的动作**（`换成这条` 那颗从前实际动的是每一支 —— 已撤）；
    //    ③ 换绑那颗动作**带着主语**（"换这一支…"），家级那颗「每一支都解除」也不在。
    //    ⚠ 判据是**否定式**的那两条：它们防的正是"没主语的动作又长回来"。
    await goto(`${args.base}/app/runtimes/local`, '执行端：本机那一页（看「模型」那一间的边界）')
    const modelRoom = await page.locator('section#model').innerText()
    check(
      '★ 机器页「模型」那一间：清单只回答"谁在用"（把名字写出来），**没有**没主语的「换成这条」',
      !modelRoom.includes('换成这条') &&
        !modelRoom.includes('正在使用') &&
        /探针钥匙[\s\S]{0,120}?在用/.test(modelRoom),
      snippet(modelRoom, 260),
    )
    check(
      '★ 机器页「模型」那一间：换绑那颗动作**带着主语**（"换这一支…"）；家级那颗「每一支都解除」不在',
      modelRoom.includes('换这一支…') &&
        !modelRoom.includes('每一支都解除') &&
        !modelRoom.includes('每一支都换'),
      snippet(modelRoom, 200),
    )
    await goto(`${args.base}/app/agents/${agentIdInUrl}`, '回 Agent 主页')
    // ★ **跑完撤干净**（真机门禁的硬线：夹具不许给后面的断言留改变过的世界）。
    //   先解绑（`PUT /home-binding` 空 source = 每一支都摘掉），再删那一条（被绑定挡住的删除会拒）。
    //   ⚠ 三个状态都**如实带出来**：这一条红过（第一次跑时删除没生效），而"没生效"必须是看得见的。
    const cleanup = await page.evaluate(async (entryId) => {
      const unbind = await fetch('/home-binding', {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ source: '' }),
      })
        .then((response) => response.status)
        .catch(() => 0)
      const removed = entryId
        ? await fetch(`/home-config?id=${encodeURIComponent(entryId)}`, { method: 'DELETE' })
            .then((response) => response.status)
            .catch(() => 0)
        : 0
      const payload = await fetch('/home-config')
        .then((response) => response.json())
        .catch(() => null)
      return {
        unbind,
        removed,
        entries: payload && Array.isArray(payload.entries) ? payload.entries.length : -1,
      }
    }, saved.entryId)
    check(
      '★ 选模型那一刀：跑完**撤干净**（模型源 0 条，后面的断言看不到我们动过的东西）',
      cleanup.entries === 0,
      `解绑=${cleanup.unbind} 删除=${cleanup.removed} 剩 ${cleanup.entries} 条`,
    )
  } else {
    check('★ 选模型那一刀（候选 / 型号 / 思考档 / 只读目录）', null, '⚠ 不适用：这一轮那支 Agent 不是住户，或配置门没开')
  }
  check(
    '「运行状态代表谁」：Agent 的主页写着 Agent 的名字，且不再自称「本机执行端」',
    homeText.includes('的主页') && !homeText.includes('本机执行端'),
    snippet(homeText, 120),
  )
  // ★ 人话自检（2026-09-22 主人裁决：设计词汇给设计者，界面语言给用户）：
  // 这些内行词**一个都不许出现在用户可见的文本里**（词表提到模块级，别的页也用它）。
  const leaked = leakedJargon(homeText)
  check(
    '人话：Agent 主页里没有内行词（清醒/唤醒/准入/收场/读数/管家/门铃/答话/在岗/标称窗口/线协议/摘除）',
    leaked.length === 0,
    leaked.length ? `漏了 ${JSON.stringify(leaked)} · ${snippet(homeText, 120)}` : '一个都没有 ✓',
  )

  // ★ 称谓刀（2026-09-23）：Agent 的主页 + 刚扫过的两处正文都不许出现**病句形状**。
  // 先做一次**正对照**（词表本身要真会红），再扫真页面 —— 防止"词表写了却恒不命中"
  // 这种装饰性覆盖。**正对照必须逐字覆盖词表里每一条**（否则新加的词可能恒不命中）。
  const subjectProbe = leakedSubject(`${homeText}\n${BANNED_SUBJECT.join('\n')}`)
  check(
    '称谓词表正对照：注入词表自身的每一条病句都必须被抓到（否则难说是正对照）',
    BANNED_SUBJECT.every((word) => subjectProbe.includes(word)),
    JSON.stringify(subjectProbe),
  )
  // ⚠ 2026-09-28 修假红（真机实跑发现）：扫之前先把**"找到"这一支的页头那一句**摘掉。
  //    页头在这一支是 `{名字} 的主页`，而名字本身可能以 `Agent` 结尾 —— 实测：Agent 叫
  //    「值班 Agent」时正文出现「值班 Agent 的主页」，裸子串 `Agent 的主页` 当场误报
  //    （而那正是**该有的**形状：页头本来就该带名字，见上面「运行状态代表谁」那条）。
  //    词表要治的是**名字不在手时的铬件标签** —— 那一支是"找不到"，由下面
  //    `notFoundSubject` 与那条锚定正则的断言单独判；摘掉页头**不会**放过别处的同一句。
  //    摘取做了两道保险（长度 ≤60、必须以「的主页」收尾），认不出就退回整页照扫。
  const foundHeader = (await page.getByText(/^[^\n]{1,60} 的主页$/).first().innerText().catch(() => '')) || ''
  const subjectScanText = foundHeader.endsWith('的主页') ? homeText.split(foundHeader).join('\n') : homeText
  const homeSubject = leakedSubject(subjectScanText)
  check(
    '称谓：Agent 的主页不出现「Agent 正在用 / 没有这个 Agent / 正在读取 Agent 的 / Agent 的主页」这类指代病句',
    homeSubject.length === 0,
    homeSubject.length ? `漏了 ${JSON.stringify(homeSubject)} · ${snippet(homeText, 120)}` : '一个都没有 ✓',
  )
  const homeLower = leakedLowercaseAgent(homeText)
  check(
    '称谓：Agent 的主页不出现小写 agent（中文界面里写 Agent）',
    homeLower.length === 0,
    homeLower.length ? `${snippet(homeText, 120)}` : '没有 ✓',
  )

  // 「资料」房间：撤销按**能力探测**在场；头像区改成**只读展示**。
  //
  // ⚠ **2026-09-29 主人裁**：这一页不再代笔 —— 契约允许 Owner 写（并集），但按「表达权归主体」
  // 资料由 Agent 自己写（`im_write_profile` / `im_write_avatar`）。判据因此从
  // 「头像区在场（含 2 MiB 那句上传提示）」翻成**一对**：头像读数**在场** ＋ 编辑入口**不在场**。
  // 只断"某串不在场"是本仓禁止的形状；两条合起来才是判据。
  // ★ **2026-10-20（T4）**：撤销 Agent 现在在页尾「危险操作」那一节里（不可恢复 ⇒ 一次误点代价太大）。
  //   判据从"正文里有没有那串"改成"它在不在那一节里"——后者才说明它被放在了该在的地方。
  const dangerSection = await page.locator('section#danger').innerText().catch(() => '')
  check(
    '★ 「撤销 Agent」在页尾「危险操作」那一节里（不在首屏、也不再挨着资料）',
    dangerSection.includes('撤销 Agent') && dangerSection.includes('再次点击确认撤销') === false,
    snippet(dangerSection, 160),
  )
  check('「撤销 Agent」按钮可见', dangerSection.includes('撤销 Agent'), snippet(homeText))
  const avatarShown = homeText.includes('头像')
  const profileEditable = homeText.includes('保存资料') || homeText.includes('选择图片')
  check(
    '资料区只读：头像读数在场，且没有代笔编辑入口（2026-09-29 裁决）',
    avatarShown && !profileEditable,
    `头像在场=${avatarShown} · 编辑入口在场=${profileEditable} · ${snippet(homeText, 120)}`,
  )

  // ---- fixtures 纪律：真实世界守卫（TASK_56/58）----
  //
  // 断法（TASK_58 纪律：断"在场的东西"）：**正向锚点** = 同页该在场的「所有权」标题；
  // 反向标记取**只可能来自 fixtures** 的串 —— 用模型 label `deepseek-v4-flash`，
  // **不**用供应商名 `DeepSeek`（真实用户可能真在用它 ⇒ 会假红）。
  check(
    '真实世界守卫：Agent 的主页不露 fixture 模型（「模型」卡已随刀2a 撤下）',
    homeText.includes('所有权') && !homeText.includes('deepseek-v4-flash'),
    `所有权在场=${homeText.includes('所有权')} · fixture 模型在场=${homeText.includes('deepseek-v4-flash')} · ${snippet(homeText, 80)}`,
  )

  // ══════════════════════════════════════════════════════════════════════════
  // 三个房间（运行 / 活动 / 模型）—— **2026-09-26 搬到机器页**（本次真机实跑发现）
  //
  // 组织轴刀（2026-09-25）把它们从 **Agent 主页** 搬到了 **机器页**
  // `/app/runtimes/:entryId`，而**这一段断言没有跟着搬** ⇒ 它自那天起就是红的。
  // 本次实跑证实：三条全红，而 Agent 主页那边「资料」「服务」两间是绿的。
  //
  // 判据本身**一字未变**（还是"由口子的状态说该断哪一态"，不是两态并列），
  // 变的只是**读哪一页的正文**：`homeText`（Agent 主页）→ `machineText`（机器页）。
  //
  // 本机条目 id 是固定的 `'local'`（`api/homes.ts` 的 `LOCAL_HOME_ID`，预置、删不得），
  // 所以直接深链接进去，不靠点行（行按钮没有 aria-label，按文本选不稳）。
  // ══════════════════════════════════════════════════════════════════════════
  await goto(`${args.base}/app/runtimes/local`, '执行端：本机那一页')
  await page.waitForTimeout(4000)
  // ★ **活动明细默认收起**（2026-10-12 主人裁：不问也该知道的留在页面上，问了才知道的交给问询）——
  //   下面那一族断言量的是**卡片本身**（来源列 / 结果 / 8 秒落地 / 不露正文）⇒ 先展开再读正文。
  //   ⚠ 展开**不是**为了让断言好过：不展开时页面上确实没有那些字（那正是"默认收起"的意思）；
  //   这一族要判的是"展开之后卡片对不对"，所以展开是这一族断言的前置动作。
  const activitySummary = page.locator('summary', { hasText: '最近的活动明细' })
  const summaryCount = await activitySummary.count()
  if (summaryCount > 0) {
    await activitySummary.first().click()
    await page.waitForTimeout(600)
  }
  const disclosureOpen = await page.evaluate(
    () => [...document.querySelectorAll('details')].some((node) => node.open),
  )
  const machineText = await page.locator('body').innerText()
  check(
    `机器页两间在场（运行 / 模型）＋「活动」明细在「运行」那一间里（下钻；摘要 ${summaryCount} 个 · 展开=${disclosureOpen}）`,
    ['运行', '模型'].every((room) => machineText.includes(room)) &&
      machineText.includes('最近的活动明细') &&
      (machineText.includes('结果：') ||
        machineText.includes('这一段记录里没有活动') ||
        machineText.includes('读不到活动记录')),
    snippet(machineText, 220),
  )

  // ★ **缺省 ≠ 读不到**（2026-10-11）：机器卡从前只看 `agentId === null` ⇒ 一个**还没有 Agent 的
  //   新家**（第一次那条路）会被说成「这台机器没报上上面跑的是谁（**可能是记录读不出来，或口子
  //   版本旧**）」，而同一页「运行」那一间明明写着"还没有 Agent"——**同屏自相矛盾**。
  //
  //   ⚠ 判据写成**状态无关**的一句：那两句**不许同时出现**（谁对谁错取决于这个家有没有住户，
  //   而那件事探针说了不算）。三态本身（缺省 / 老形态 / 读不到 id）由**纯判据**钉住
  //   （`tests/agent-row-status.test.ts` 那一组）；这条真机断言量的是**接线**：卡片与「运行」
  //   那一间说的是同一件事。
  check(
    '★ 机器卡与「运行」那一间不许自相矛盾：说了"没报上上面跑的是谁"就不许同时说"还没有 Agent"',
    !(machineText.includes('没报上上面跑的是谁') && machineText.includes('还没有 Agent')),
    snippet(machineText, 160),
  )

  // ★ **第一次那条路的第一步在场**（2026-10-11）：这一轮的临时家里**没有模型源**
  //   （探针从不写它 —— 它只读配置页），所以那张卡**必须在场**，而且必须排在三个房间**之前**
  //   （判据：**动作在读数之前** —— 三个房间回答"现在怎么样"，第一次来的人要的是"我该做什么"）。
  //   ⚠ 哪天探针开始给临时家塞模型源，这条会红 —— 那时要改的是**探针的前提**，不是放宽断言。
  const firstStepGeometry = await page.evaluate(() => {
    const card = document.querySelector('[data-first-model-step]')
    const run = document.getElementById('run')
    if (!card) return null
    return {
      cardTop: Math.round(card.getBoundingClientRect().top),
      runTop: run ? Math.round(run.getBoundingClientRect().top) : null,
      text: String(card.innerText ?? '').replace(/\s+/g, ' ').slice(0, 120),
    }
  })
  check(
    '★ 第一次那条路的第一步在场，且排在三个房间**之前**（动作在读数之前）',
    firstStepGeometry !== null &&
      firstStepGeometry.runTop !== null &&
      firstStepGeometry.cardTop < firstStepGeometry.runTop,
    firstStepGeometry
      ? `卡 top=${firstStepGeometry.cardTop}px · 运行 top=${firstStepGeometry.runTop}px · ${firstStepGeometry.text}`
      : '页面上找不到那张卡（`[data-first-model-step]`）—— 这一轮的家里没有模型源，它本该在场',
  )

  // ★ **落点带锚点要真的滚到位**（2026-10-11 加）：第一次那条路的落点是
  //   `/app/runtimes/local#model`，判据**不是**"地址里有 `#model`"，而是**几何**。
  //
  // ⚠ **判据长什么样，是量出来的**（第一版判错了，记在这里）：第一版要求"目标在视口上半"，
  //   而「模型」是**最后一间** ⇒ 容器滚到底也顶不到上半（实测 `scrollTop=348 = 上限`）⇒ 假红。
  //   现在两条：
  //   ① 拿**中间那一间**（`#activity`）量**对齐** —— 它够得着，所以这一条真能分辨
  //      "滚一次就收工"（旧写法：滚了 171 却因上面几节长高而没对齐）与"盯住它"；
  //   ② 拿落点 `#model` 量**在视野里** —— 最后一间够不着顶部是物理事实，不该判它红。
  //   ⚠ **视口要调矮**（2026-10-11 注入旧实现实测出来的）：这一页只比视口高一点点，
  //     长视口下"滚到底"就等于"到了" ⇒ "滚一次就收工"与"盯住它"读数**一模一样**（两条都假绿）。
  //     调矮之后"对齐"才成为可能，这条判据才真的在量东西。
  const defaultViewport = page.viewportSize()
  await page.setViewportSize({ width: 1280, height: 480 })
  await goto(`${args.base}/app/runtimes/local#model`, '机器页带锚点（#model，第一次那条路的落点）')
  await page.waitForTimeout(4000)
  const anchorGeometry = await page.evaluate(() => {
    const anchorOf = () => {
      let node = document.getElementById('model')?.parentElement ?? null
      while (node && node !== document.body) {
        if (/(auto|scroll)/.test(getComputedStyle(node).overflowY)) return node
        node = node.parentElement
      }
      return null
    }
    const scroller = anchorOf()
    const box = (id) => {
      const el = document.getElementById(id)
      if (!el) return null
      const rect = el.getBoundingClientRect()
      return { top: Math.round(rect.top), bottom: Math.round(rect.bottom) }
    }
    return {
      viewport: window.innerHeight,
      scrollerTop: scroller ? Math.round(scroller.getBoundingClientRect().top) : null,
      scrollTop: scroller ? Math.round(scroller.scrollTop) : -1,
      maxScrollTop: scroller ? Math.round(scroller.scrollHeight - scroller.clientHeight) : -1,
      run: box('run'),
      activity: box('activity'),
      model: box('model'),
    }
  })

  await goto(`${args.base}/app/runtimes/local#activity`, '机器页带**旧锚点**（#activity ⇒ 2026-10-12 起映射到「运行」那一间）')
  await page.waitForTimeout(4000)
  const activityGeometry = await page.evaluate(() => {
    // ⚠ 2026-10-12：「活动」不再是一个房间（明细降为「运行」那一间里的下钻）⇒ 旧锚点
    //    `#activity` 由 `RuntimesPage` 的 `LEGACY_ANCHORS` 映射到 `#run`。这里量**落点那一间**
    //    （`#run`）—— 判据不变：**落到的那一间要贴住顶部**（分辨"滚一次就收工"）。
    let node = document.getElementById('run')?.parentElement ?? null
    while (node && node !== document.body) {
      if (/(auto|scroll)/.test(getComputedStyle(node).overflowY)) break
      node = node.parentElement
    }
    const scroller = node && node !== document.body ? node : null
    const rect = document.getElementById('run')?.getBoundingClientRect() ?? null
    return {
      scrollerTop: scroller ? Math.round(scroller.getBoundingClientRect().top) : null,
      scrollTop: scroller ? Math.round(scroller.scrollTop) : -1,
      maxScrollTop: scroller ? Math.round(scroller.scrollHeight - scroller.clientHeight) : -1,
      activityTop: rect ? Math.round(rect.top) : null,
    }
  })
  const activityAligned =
    activityGeometry.activityTop !== null &&
    activityGeometry.scrollerTop !== null &&
    Math.abs(activityGeometry.activityTop - activityGeometry.scrollerTop) <= 48
  check(
    '★ 落点 `#activity`（旧锚点）⇒ 落到「运行」那一间、且**对齐到顶部**（这一条分辨"滚一次就收工"）',
    activityAligned,
    `运行 top=${activityGeometry.activityTop}px · 容器 top=${activityGeometry.scrollerTop}px · ` +
      `scrollTop=${activityGeometry.scrollTop}/max=${activityGeometry.maxScrollTop} ⇒ ` +
      `${activityAligned ? '已对齐' : activityGeometry.scrollTop >= activityGeometry.maxScrollTop - 2 ? '已到底但**没对齐**（内容长高把它顶下去了）' : '既没对齐也没到底'}`,
  )

  check(
    '★ 落点 `/app/runtimes/local#model` ⇒ 「模型」那一间在**视野里**（最后一间够不着顶部是物理事实）',
    anchorGeometry.model !== null &&
      anchorGeometry.model.top < anchorGeometry.viewport &&
      anchorGeometry.model.bottom > 0,
    anchorGeometry.model
      ? `模型 ${anchorGeometry.model.top}..${anchorGeometry.model.bottom}px（视口高 ${anchorGeometry.viewport}）· ` +
        `运行 top=${anchorGeometry.run?.top} · scrollTop=${anchorGeometry.scrollTop}/max=${anchorGeometry.maxScrollTop}`
      : '页面上找不到 #model 那一间',
  )
  await page.setViewportSize(defaultViewport)
  check(
    '机器页说得出「上面跑着谁」（四态之一，不压成两态）',
    ['上面跑的是', '它没报上上面跑的是谁', '不在这个世界里', '读不到你的 Agent 列表'].some((s) =>
      machineText.includes(s),
    ),
    snippet(machineText, 140),
  )

  // ---- ★ 门判据：跑那一份**共用表**（2026-10-12）-------------------------------
  //
  // 表在 `client/scripts/door-cases.mjs`（**唯一权威位置**）：源码形态的门
  // （`node home.js doors`）与壳各有一份实现（两种形态各需要一个宿主，合不成一份）
  // ⇒ 判据落在"**同形**"上。产品形态那一半在 `verify-ui-app.mjs` 里跑**同一张表**：
  // 任何一条两边不同形，就有一边红。
  // ⚠ 2026-10-11 真撞过一次不同形（来源那道与路由谁先判两边反着）⇒ 那天产品形态门禁
  //   量到的其实是源码形态的门，看着像代码坏了。
  // ⚠ 要知道两个口开在哪 ⇒ 需要 `--home <家目录>`（从它的 `home.conf` 读端口）；
  //   没给就报不适用（本门禁其它各段照旧跑）。
  if (args.home) {
    const conf = createRequire(import.meta.url)('../../scripts/home/conf.js')
    const confFile = path.join(args.home, 'home.conf')
    const doorResults = await runDoorCases(
      {
        status: `http://127.0.0.1:${conf.confGet(confFile, 'status_port', '5190')}`,
        config: `http://127.0.0.1:${conf.confGet(confFile, 'config_port', '5191')}`,
      },
      LOCAL_ORIGIN,
    )
    const doorWrong = doorCasesSayingWrongThing(doorResults)
    check(
      '★ 门的六条判据（共用表，量的是**源码形态**的门；产品形态跑同一张表）',
      doorWrong.length === 0,
      doorWrong.length ? doorWrong.join(' · ') : describeDoorCases(doorResults),
    )
  } else {
    check('门判据（共用表）：这一轮没给 `--home` ⇒ 不知道两个口开在哪', null, '不适用')
  }

  // ---- ★ 一级入口在移动端**不是二级页**（2026-10-11 立）-------------------------
  //
  // 判据：导航里的一级入口在移动端**保留 TabBar**；二级页（一间房 / 某一支 Agent）才沉浸
  // 全屏、TabBar 让位。从前外壳那条判据写的是 `startsWith('/app/agents')` ⇒ **清单本身**
  // 也被当成二级 ✗（TabBar 没了，页面还顶着「返回我的」）。
  // ⚠ 只能在手机视口下量：默认 1440×900 下**桌面图标栏也在场**，两种形状分不开。
  //   所以临时缩视口、量完复原（与上面锚点那一段同一手法）。
  // ⚠ 外壳里有两个 `nav[aria-label="主导航"]`（桌面栏 ＋ 移动 TabBar）⇒ 用 `:visible` 数**在场**的那个。
  const desktopViewport = page.viewportSize()
  await page.setViewportSize({ width: 390, height: 844 })
  const visibleMainNav = () => page.locator('nav[aria-label="主导航"]:visible').count()
  await goto(`${args.base}/app/agents`, '我的 Agent（一级入口）')
  await page.waitForTimeout(1200)
  const listTabBar = await visibleMainNav()
  await goto(`${args.base}/app/runtimes`, '我的执行端（一级入口）')
  await page.waitForTimeout(1200)
  const runtimesTabBar = await visibleMainNav()
  check(
    '★ 移动端：两个**一级入口**都保留 TabBar（从前 `/app/agents` 被当成二级页 ⇒ TabBar 让位）',
    listTabBar === 1 && runtimesTabBar === 1,
    `我的 Agent=${listTabBar} · 我的执行端=${runtimesTabBar}`,
  )
  // 正对照：**二级页** TabBar 让位 —— 否则上面那条可能只是"TabBar 永远在场"。
  // ⚠ 用一条**房地址**（`/app/chat/<id>`）：外壳那条判据是**纯路径**的（`navGroups.ts`），
  //   与"这间房里有没有东西"无关 ⇒ 这一条量的是铬件，不是房里的读数（房间里是不是真有东西
  //   由上面那一族断言管）。
  await goto(`${args.base}/app/chat/probe-chrome-only`, '房地址（二级页，只量铬件）')
  await page.waitForTimeout(1200)
  const roomTabBar = await visibleMainNav()
  check('★ 移动端正对照：二级页（房地址）TabBar 让位', roomTabBar === 0, `那一页=${roomTabBar}`)
  // ★ **某一台机器**那一页也在一级入口下面那一层（2026-10-11 补：它从前自带返回却保留
  //   TabBar，与"一间房 / 一个联系人 / 某一支 Agent"不一致）。
  await goto(`${args.base}/app/runtimes/local`, '机器页（二级页）')
  await page.waitForTimeout(1500)
  const machineTabBar = await visibleMainNav()
  check('★ 移动端：某一台机器那一页也让位（同族详情页，自带返回出口）', machineTabBar === 0, `那一页=${machineTabBar}`)
  // ⚠ **把页面放回原处**：这一段后面（活动记录那一族）读的是**当前页**的正文，
  //   而我为了量移动端把浏览器带去了别的地址 —— 探针的段落是**有顺序**的。
  await goto(`${args.base}/app/runtimes/local`, '机器页（放回原处）')
  await page.waitForTimeout(800)
  await page.setViewportSize(desktopViewport)

  // ⚠ **活动明细默认收起**（2026-10-12）⇒ 上面那次"放回原处"是一次**重新加载**，展开状态不跟着走；
  //   而下面这一族读的是**当前页**的正文（还有一条 8 秒落地的 `waitForFunction`）⇒ 先再展开一次。
  const activitySummaryAgain = page.locator('summary', { hasText: '最近的活动明细' })
  if ((await activitySummaryAgain.count()) > 0) {
    await activitySummaryAgain.first().click()
    await page.waitForTimeout(600)
  }

  // ---- 运行 / 活动 / 模型：**判据由口子的状态说**（刀2 收紧，不再两态并列）----
  //
  // （旧版把"渲染真实数据"与"如实降级"用 `||` 并列：初版只钉死"未开"那一态，起口之后它
  //   立刻假红 —— 但反过来，口子开着而页面降级时它**照样绿**，那正是这一刀要治的。）
  const agentHomeLoaded = machineText.includes('当前状态')
  const agentHomeDegraded = machineText.includes(AGENT_HOME_UNREACHABLE_TITLE)
  // ⚠ 2026-09-28 修陈旧（真机实跑发现）：这一支原先断言正文含「此 Agent 未接入」——
  //    而那个串**在产品源码里已经不存在了**（2026-09-26 前端大整改删掉；`git log -S` 可证，
  //    它只在门禁脚本里活着）⇒ 只要口子开着、读数又不是"你刚才看的那一支"，这三条**恒红**。
  //    判据改回机器页自己的契约：**这一页的主语是机器**（2026-09-25 组织轴刀把三个房间
  //    从 Agent 主页搬到这里）⇒ 口子有读数就必须渲染机器自己的读数；至于"上面跑的是谁"，
  //    由上面那条「四态之一」的断言单独管（它认「不在这个世界里」那几句）。
  check(
    `运行房间（${gateLabel}）：${
      gateOpen ? '口子有数据 ⇒ 机器页必须渲染真实数据（当前状态）' : '必须如实降级（读不到运行情况）'
    }`,
    gateOpen ? agentHomeLoaded : agentHomeDegraded,
    snippet(machineText),
  )
  check('运行房间说人话：待命/已暂停这些词在，而不是「在岗（准入 …）」', !machineText.includes('准入'))
  const daysRendered =
    machineText.includes('次活动') ||
    machineText.includes('这一段记录里没有活动') ||
    machineText.includes('这次没有读到数据')
  const daysDegraded = machineText.includes('读不到活动记录')
  check(
    `活动记录（${gateLabel}）：${
      gateOpen
        ? '口子有数据 ⇒ 必须以「一次活动」为单位渲染（或如实说这一段没有活动）'
        : '必须如实降级（读不到活动记录）'
    }`,
    gateOpen ? daysRendered : daysDegraded,
    snippet(machineText),
  )
  // 刀2b C②：起源列（口子把起源映射成稳定取值 ⇒ 页面说人话）。
  // 只有"读到了活动"那一态才要求它（降级 / 未接入那一态当然没有）。
  //
  // ★★ **这一条踩过"等时长"的坑**（2026-09-23）：活动记录那一列是**异步**拉来的，
  //    固定 `waitForTimeout` 之后它可能还在加载 ⇒ **同一天两次跑，一次过一次红**。
  //    改成两步：① 等它真的落地（等一场渲染出来，或它如实说这一段没有活动）；
  //    ② 判"该不该有来源那一行"**由数据说**（窗口里有没有带起源的那一行），
  //       没有就断个正向锚点（那一场确实渲染出来了），并在名字里说清这一轮没有可判的起源。
  if (gateClaimsAgent && daysRendered) {
    /**
     * ⚠ **2026-10-16 消除读数竞态**：从前这里"等条件成立"与"取正文"是**两次读**，
     * 而活动记录那一列是异步拉来的、还会重渲染（`<details>` 收起时正文不进 `innerText`）
     * ⇒ 两次读之间那一段可能被换掉：**等到了**，紧接着**读到的却没有**（实测三次里红两次，
     * 红的是读数不是产品）。现在让浏览器在**条件成立的那一瞬间**把正文一起交回来 ——
     * 一个瞬间只读一次，这一类抖从根上没有了。
     */
    const settledText = await page
      .waitForFunction(
        () => {
          const text = document.body.innerText
          return (
            text.includes('来源：') ||
            text.includes('结果：') ||
            text.includes('这一段记录里没有活动')
          )
            ? text
            : false
        },
        null,
        { timeout: 8000 },
      )
      .then((handle) => handle.jsonValue())
      .catch(() => null)
    const settled = typeof settledText === 'string'
    const daysText = settled ? settledText : await page.locator('body').innerText()
    check('活动记录在 8 秒内落地（等到一场渲染出来，或它如实说没有活动）', settled, snippet(daysText, 160))

    // 窗口里有没有**带起源**的那一行：由门的读数说（不是猜）。
    //
    // ⚠ **2026-10-16 改成当场再读一次门**：从前这里复用**本轮开头**那一次快照
    //    （`statusProbe`），而页面读的是**当下**的账本 —— 两个不同时刻的读数比，
    //    就是拿移动靶当判据：快照里那几行带起源、页面此时渲染的是**更新**的那几行
    //    （它的起源页面还没拿到）⇒ 页面明明照契约说了，却判红 ✗（实测在两种红之间来回跳）。
    //    现在两边取**同一时刻**：先读页面正文，紧接着读一次门，用**那一次**的 rows 判"该不该有来源"。
    const freshRows = await page
      .evaluate(() =>
        fetch('/home-status/status')
          .then((r) => (r.ok ? r.json() : null))
          .catch(() => null),
      )
      .then((payload) => payload?.experience?.rows ?? [])
      .catch(() => [])
    const rows = Array.isArray(freshRows) && freshRows.length > 0 ? freshRows : (statusProbe?.experience?.rows ?? [])
    const attributable = rows.some((row) => row && typeof row.origin === 'string' && row.origin)
    /**
     * ⚠ **第三档**（2026-10-08 补）：这个世界里**一场活动都没有**。
     *
     * 那时"这一场渲染出来了"这个正锚**没有可判的对象** —— 按本文件顶上的三态判据
     * （"这条断言要的输入不在场 ⇒ N/A"），它该**报不适用**，而不是判失败；
     * 上面那个 `waitForFunction` 本来就把「这一段记录里没有活动」当**合法终态**收下。
     * 有活动的世界照旧判（有起源判「来源」列、没起源判"渲染出来了"），牙齿一颗没掉。
     */
    const noRecords = daysText.includes('这一段记录里没有活动')
    /**
     * ⚠ **2026-10-16 修陈旧输入的抖**（实测三次里红两次，红的是**判据的输入**不是产品）：
     *
     * `attributable` 读的是**早先那一次探针的快照**（`statusProbe.experience.rows`），而页面
     * 读的是**当下的账本** —— 两次读之间那一场刚落下来（它带起源）时，就会出现
     * "探针说没有起源、页面却写着来源" ✗ ⇒ 兜底锚点从前只认 `结果：`，于是**页面明明说对了**
     * 也判红。修法**不放松牙齿**：读数说有起源 ⇒ 页面必须写出合法的来源；读数说没有而
     * **页面自己写出来了** ⇒ 那也是合格的正锚（这一条要的本来就是"说清为什么开始"）。
     */
    const pageSaysOrigin = /来源：(有新消息|它自己约的时间到了|手动开始|不知道)/.test(daysText)
    const renderAnchor = daysText.includes('结果：') || daysText.includes('来源：')
    check(
      attributable
        ? '活动记录有「来源」列（窗口里那一场带着起源 ⇒ 页面必须说清为什么开始）'
        : pageSaysOrigin
          ? '活动记录：窗口里那一场**写着来源**（这一次的探针快照没给起源 ⇒ 断"来源"作正锚）'
          : noRecords
            ? '活动记录：这个家里还没有任何活动 ⇒ 这一轮没有可判的「来源」（不是失败）'
            : '活动记录：窗口里没有带起源的那一场 ⇒ 这一轮没有可判的「来源」（断"这一场渲染出来了"作正锚）',
      attributable || pageSaysOrigin ? pageSaysOrigin : noRecords ? null : renderAnchor,
      snippet(daysText, 200),
    )
  }

  // 展开每一场的详情 ⇒ **仍然只能看到机械字段**（绝不给正文）。
  // ⚠ 收起时 `<details>` 的内容不进 `innerText` ⇒ 必须**真展开**再读，否则这条是空判据。
  await page.evaluate(() => {
    for (const node of document.querySelectorAll('details')) node.open = true
  })
  await page.waitForTimeout(400)
  const expanded = await page.locator('body').innerText()
  /**
   * ⚠ **2026-10-16 让这条断言自己点名**：从前它的读数是"整页正文"（`snippet(expanded)`），
   * 红了只知道"有某个词出现了"，得再跑一次才知道是哪个 ✗ —— 现在把命中的词列出来
   * （判据一字未改：四个词一个都不许出现在展开后的正文里）。
   */
  const leakedBodyWords = ['params', 'blocks', 'payload', 'reasoning'].filter((word) =>
    expanded.includes(word),
  )
  check(
    '活动记录展开详情后仍不露正文类字段（params / blocks / payload / reasoning）',
    leakedBodyWords.length === 0,
    `命中 ${JSON.stringify(leakedBodyWords)} · ${snippet(expanded)}`,
  )

  // ---- 模型房间：本机那一格的模型配置读数（刀2 起**判据由口子的状态说**）----
  //
  // ⚠ 刀2b 换了徽章的词（**已指定 / 没指定 / 读不出来**）：那一格里"配置"已经分不清
  // "清单里有几条"与"Agent 指定了哪条"，而页面现在说的是后者。刀2 换掉的是**判据的形状**：
  // 不再是"渲染读数**或**如实降级"（两态并列），而是由口子开着/关着决定该断哪一态。
  const mindReadout =
    machineText.includes('已指定') || machineText.includes('没指定') || machineText.includes('读不出来')
  const mindDegraded = machineText.includes(AGENT_MIND_UNREACHABLE_TITLE)
  check(
    `模型房间（${gateLabel}）：${
      gateOpen
        ? '口子有数据 ⇒ 必须渲染那台机器上的读数（已指定 / 没指定 / 读不出来）'
        : '必须如实降级（读不到模型配置）'
    }`,
    gateOpen ? mindReadout : mindDegraded,
    snippet(machineText),
  )
  check('模型房间不显示任何密钥内容', !/sk-|clr_hs|Bearer/.test(machineText))

  // ★★ 2026-09-23 补：**加载中 / 找不到**那一支的页头也要扫**（这次真逃过门禁的那个分支）。
  // 上面那条扫的是**已落地**的正文（页头是 `{名字} 的主页`，带名字）。而 `!agent` 那支的
  // 页头是**没有名字的铬件标签**——它只在这一支渲染，且**转瞬即逝**（实测 <250ms 就落地）。
  // ⇒ 靠"等落地再读正文"永远扫不到它 ⇒ 必须**只读页头那一个元素**（不读整页正文）。
  // 用**找不到的 id** 把这一支钉在屏上（这是它唯一稳定可达的入口）。
  // ⚠ 放在**本组断言的最后**：它把页面留在"找不到"那一页，后面的分节各自会重新 `goto`，
  //   若插在中间会把后续断言全带偏（实测：活动记录两条当场红）。
  await goto(`${args.base}/app/agents/00000000-0000-0000-0000-000000000000`, 'Agent 主页（找不到）')
  await page.waitForTimeout(1200)
  const notFoundText = await page.locator('body').innerText()
  const chromeHeader = (await page.getByText(/^(Agent 主页|Agent 的主页)$/).first().innerText().catch(() => '')) || ''
  check(
    '找不到 Agent 时，页头铬件是「Agent 主页」（名字本就不在手，不能「Agent 的主页」）',
    notFoundText.includes('没有这个 Agent') && chromeHeader === 'Agent 主页',
    `正文含"没有这个 Agent"=${notFoundText.includes('没有这个 Agent')} · 页头=「${chromeHeader}」`,
  )
  const notFoundSubject = leakedSubject(notFoundText)
  check(
    '找不到那一支的正文也不出现称谓病句（含「Agent 的主页」）',
    notFoundSubject.length === 0,
    notFoundSubject.length ? `漏了 ${JSON.stringify(notFoundSubject)}` : '一个都没有 ✓',
  )

  // ---- 旧地址不 404：`/app/runtime`、`/app/experience` 重定向到**执行端机器列表** ----
  //
  // ⚠ **2026-09-26 改判据（本次真机实跑发现它早已过期）**：这一段原先断言"重定向到
  // 「运行」/「活动记录」**房间**"——那是**组织轴刀之前**的形状（那时它落到 Agent 主页的
  // 四个房间里）。2026-09-25 组织轴刀把落点改成 `/app/runtimes`（**机器列表**），
  // 而**这一条没有跟着改** ⇒ 它自那天起就是红的，只是一直没人跑这支脚本
  // （审计 P1 猜的正是这件事：「`npm run verify:ui` 可能已经是红的」）。
  //
  // 现在断的是**当前事实**：不 404、且落到执行端那一页（列表标题「我的执行端」）。
  // 同一次整改还把重定向改成**不带 hash**（列表页没有房间可滚，H12）——
  // 旧锚点只在带 entryId 的地址上有效，所以这里**不再**断"滚到哪一间"。
  for (const legacy of ['/app/runtime', '/app/experience']) {
    await goto(`${args.base}${legacy}`, `旧地址 ${legacy}`)
    await page.waitForTimeout(3500)
    const text = await page.locator('body').innerText()
    check(
      `${legacy} 不 404（重定向到执行端机器列表）`,
      !text.includes('404') && text.includes('我的执行端'),
      snippet(text, 120),
    )
  }

  // ---- TASK_61：那一次读数是**同源**打出去的（页面不知道端口）----
  check(
    '家/日子读的是同源门 /home-status/*（TASK_61）',
    statusCalls.length > 0 && statusCalls.every((call) => call.origin === pageOrigin),
    `pageOrigin=${pageOrigin} calls=${JSON.stringify(statusCalls.slice(0, 3))}`,
  )
  if (agentHomeLoaded) {
    // 只有"读到了真实读数"那一态才要求 200：口子关着时页面本来就走降级文案，
    // 那一次的 502 是**如实**的，不是故障。
    check(
      '同源门那一次真的取到了读数（200）',
      statusCalls.some((call) => call.status === 200),
      JSON.stringify(statusCalls.slice(0, 3)),
    )
    check('家读数里没有凭据字样', !/clr_hs|Bearer|DEEPSEEK|BACKEND_AUTH/.test(homeText))
  }

  // ---- 真实世界守卫：fixture 驱动页在真实世界里必须诚实缺省（TASK_56）----
  //
  // 这一条把「判据是**是不是真实世界**」钉进未来。原先这四页只被
  // `mode === 'host'` 拦着 ⇒ core 直连（**也是真实世界**）一路放行，
  // 真实用户进「发现」看到 fixtures 假居民、点「打招呼」跳进不存在的房间。
  //
  // 标记取 fixture 目录里**只可能来自 fixtures** 的三个串（人名 + fixture 房间 id），
  // 不取 `Inori`：真实用户的名字可能就叫这个，那会造成假红。
  const FIXTURE_MARKERS = ['Celira 向导', 'Aria', 'r-guide']
  const HONEST = '本页面尚未提供真实数据'
  const guardTexts = {}
  for (const [pathname, label] of [
    ['/app/discover', '发现（壳层一级入口）'],
    ['/gallery', '陈列室'],
  ]) {
    await goto(`${args.base}${pathname}`, label)
    await page.waitForTimeout(2500)
    const text = await page.locator('body').innerText()
    guardTexts[pathname] = text
    const leaked = FIXTURE_MARKERS.filter((marker) => text.includes(marker))
    check(
      `真实世界守卫：${label} 不给 fixtures（诚实缺省）`,
      text.includes(HONEST) && leaked.length === 0,
      leaked.length > 0 ? `漏出 fixtures：${JSON.stringify(leaked)}` : snippet(text, 90),
    )
  }

  // ⚠ **2026-09-26：`/agent/:participantId` 已不在守卫之下**（重写成世界侧参与者页，
  // 见 `api/routeRegistry.ts` 与 `tests/real-world.test.ts` 的移除记账）。
  // 它原先在这一圈里，断的是"撞诚实门"——那正是审计 H9 那条硬伤（真实世界有三处入口
  // 链到它，用户点得到、点进去撞门）。现在它必须**自己**诚实：拿一个 fixtures 的 id
  // 去问真实世界，应当说「没有这位参与者的资料」，而**不是**渲染门禁页、也不许漏出 fixtures。
  {
    const pathname = '/agent/p-inori'
    await goto(`${args.base}${pathname}`, '参与者页（世界侧）')
    await page.waitForTimeout(2500)
    const text = await page.locator('body').innerText()
    const leaked = FIXTURE_MARKERS.filter((marker) => text.includes(marker))
    check(
      '真实世界：参与者页自己说真话（不给 fixtures，也不再撞门）',
      leaked.length === 0 && !text.includes(HONEST),
      leaked.length > 0 ? `漏出 fixtures：${JSON.stringify(leaked)}` : snippet(text, 90),
    )
  }
  // 演示通道的开关（「陈列室」那颗按钮）在真实世界**不该存在**：陈列室已被守卫拦住，
  // 给了入口就是给一条死链。这条同时证明 DemoSwitcher 的判据也换成了「真实世界」。
  check(
    '真实世界不出现演示通道开关（陈列室入口）',
    !(guardTexts['/app/discover'] ?? '').includes('陈列室'),
    snippet(guardTexts['/app/discover'] ?? '', 90),
  )

  // ---- 进一个真房间：输入区要挂得上来，且**零未捕获异常**（TASK_56）----
  //
  // 为什么必须有这一步：本刀改之前，这支脚本只走到「未选中的聊天页」，
  // 而 `MessageInput` 里有一条**无条件**的 fixtures 读取
  // （`getParticipant(myAgentConfig.agentId)` = 'a-courier'），
  // 它**只在输入区挂上来时才求值** ⇒ 当时那一轮全绿是**假绿**：
  // 真实用户一选中房间，整页就崩（`unknown participant: a-courier`，实测抓到）。
  // ⇒ 判据必须落在「输入区在不在」上，而不是「聊天页有没有打开」。
  await goto(`${args.base}/app/chat`, '聊天首页（进房间）')
  await page.waitForTimeout(4500)
  // 会话行在真实数据下是**虚拟列表**（`ConversationList` 用 rowVirtualizer 渲染
  // `div[data-index]`，不是 ul/li —— 这一条是探 DOM 量出来的，别照源码里的 `<ul>` 猜）；
  // 没数据时才走别的分支，所以两种形状都试。
  let roomRow = page.locator('[data-index] button').first()
  if ((await roomRow.count()) === 0) roomRow = page.locator('ul li button').first()
  if ((await roomRow.count()) > 0) {
    await roomRow.click()
    await page.waitForTimeout(5000)
  }
  const composerCount = await page.getByLabel('添加附件').count()
  const renderErrors = consoleErrors.filter((line) => line.startsWith('pageerror'))
  // ★ 2026-10-03：页内记录器读回，接到「零未捕获异常」上（见文件头「页内异常记录器」；
  // 它是这条判据在本机引擎上的唯一来路）。三态：输入区没挂上来 ⇒ 红；挂上来了但记录器
  // 读不到 ⇒ 不适用（这一半没验，结尾会列出来）——**不许**当成零异常。
  const recordedErrors = await readRecordedErrors()
  const uncaught = recordedErrors === null ? null : mergeUncaughtExceptions(renderErrors, recordedErrors)
  const uncaughtOk = uncaught === null ? null : uncaught.length === 0
  check(
    '进房间后输入区挂得上来（真实世界不崩）',
    composerCount > 0 ? uncaughtOk : false,
    `输入区=${composerCount} 未捕获异常=${uncaught === null ? '读不到（本轮降级，不许当成 0）' : uncaught.length}${
      uncaught !== null && uncaught.length > 0 ? ` · ${snippet(uncaught[0], 120)}` : ''
    } · ${snippet(await page.locator('body').innerText(), 90)}`,
  )

  // ---- 会话行的预览与时间：**历史到手之后必须补上**（2026-09-29 真机实测修）----
  //
  // 原先 `store.refreshRooms` 只在**连接那一刻**投影一次（那时任何房间的历史都还没加载 ⇒
  // 预览与时间必然是空的），而"进入房间 → 历史到手"**不再投影** ⇒ 时间线里明明有消息，
  // 左边那一行还挂着「还没有消息」，要等到别的事情（比如在另一间房发一条）再跑一次
  // `refreshRooms` 才补上。真机实测：5 条消息在屏上，行里写着「还没有消息」。
  //
  // 判据只在**这个房间真的有非 system 消息**时才判得动 —— 空房说「还没有消息」是对的，
  // 所以先直接问服务器要一次事实（只读，不写任何东西）。
  const openedRoomId = new URL(page.url()).pathname.split('/').filter(Boolean).pop() ?? ''
  const historyProbe = openedRoomId
    ? await fetch(`${args.base}/v1/rooms/${openedRoomId}/messages?limit=20`, {
        headers: { authorization: `Bearer ${args.token}` },
      })
        .then((r) => (r.ok ? r.json() : null))
        .catch(() => null)
    : null
  const realMessageCount = Array.isArray(historyProbe?.messages) ? historyProbe.messages.length : null
  if (realMessageCount === null) {
    check('会话行预览：这一轮没验（读不到这个房间的历史）', null, `房间=${openedRoomId || '(没进房间)'}`)
  } else if (realMessageCount === 0) {
    check('会话行预览：这一轮没验（这个房间本来就没有消息）', null, `房间=${openedRoomId}`)
  } else {
    const rowText = (await roomRow.count()) > 0 ? await roomRow.innerText() : ''
    check(
      '★ 会话行补上了预览（历史已在手时不许还说「还没有消息」）',
      rowText.includes('还没有消息') === false && rowText.trim().length > 0,
      `行内=${JSON.stringify(rowText.replace(/\n/g, ' · '))} · 服务器上这个房间有 ${realMessageCount} 条`,
    )
  }

  // ---- 「对方读到这儿 / 还没读」那一格（2026-10-10；契约 `CORE_V1_READ_RECEIPT.md` §2）----
  //
  // 这一格是**投影**：服务端在 `direct` 房那一行给 `peer_read`，客户端把它挂在
  // **我最后一条气泡下面**（2026-10-20 主人真机点名后从头部挪下来——头部那三行里
  // 「在线」说的是对方，「已读」说的是"我这条消息的结局"，主语混在一格里）。判据两条：
  //
  // ① `direct` 房、**我在里面说过话** ⇒ 我最后那条气泡下出现「还没读 / 读到这儿」；
  // ② **不出现**也是判据：群房里那两个词**一个都不许有**（"没有答案"与"答案是未读"
  //    必须分得开——缺省不当零）。
  //
  // ⚠ 为什么这一条要**先自己发一条**：那一格只在"我在这间房说过话"时成立——不发，
  //    它**本来就不该出现**，断言会变成空转（假绿）。
  // ⚠ 这一轮只验 **`未读`** 那一半：翻到 `已读` 要**推进那支 Agent 的读游标**
  //    （BackendAuth，那支凭据不在本脚本手里）。状态机那一半由服务端用例钉着
  //    （`room_store_tests::peer_read_tells_whether_the_other_side_read_my_last_message`）。
  const headerText = await page.locator('body').innerText()
  const roomListProbe = await fetch(`${args.base}/v1/rooms`, {
    headers: { authorization: `Bearer ${args.token}` },
  })
    .then((r) => (r.ok ? r.json() : null))
    .catch(() => null)
  const openedEntry = Array.isArray(roomListProbe?.rooms)
    ? roomListProbe.rooms.find((row) => row.room?.id === openedRoomId)
    : null
  const meProbe = await fetch(`${args.base}/v1/me`, {
    headers: { authorization: `Bearer ${args.token}` },
  })
    .then((r) => (r.ok ? r.json() : null))
    .catch(() => null)
  const myId = meProbe?.participant?.id ?? meProbe?.id ?? null
  // ⚠ 判据落在**那一格自己**上（`data-peer-read` 钩子），**不拿整页正文做正则**：
  //    2026-10-10 实测踩到——探针自己发的那条消息正文里就含那一格当时的两个词，
  //    于是左栏预览把它带进了群房那一页，两条断言**同时**变成假绿/假红。
  const peerReadCell = page.locator('[data-peer-read]')
  if (!openedEntry || openedEntry.room.kind !== 'direct' || !myId) {
    check(
      '★ 读到这儿/还没读那一格：这一轮没验（打开的不是 direct 房，或读不到我是谁）',
      null,
      `房=${openedRoomId || '(没进房间)'} kind=${openedEntry?.room?.kind ?? '读不到'}`,
    )
  } else {
    const spokeAlready = (historyProbe?.messages ?? []).some(
      (m) => (m.actor?.id ?? m.actor_id) === myId,
    )
    let sendStatus = null
    if (!spokeAlready) {
      // 走**公开协议**发一条（临时世界；判据要的是"这一格成立"的那个前提）。
      // ⚠ 正文**不许**含那一格的词（否则它会被别处读到，污染断言——实测过）。
      sendStatus = await fetch(`${args.base}/v1/rooms/${openedRoomId}/messages`, {
        method: 'POST',
        headers: {
          authorization: `Bearer ${args.token}`,
          'content-type': 'application/json',
          'Idempotency-Key': `probe-peer-read-${Date.now()}`,
        },
        body: JSON.stringify({ body: '探针：这一条用来造出那个前提' }),
      })
        .then((r) => r.status)
        .catch(() => 'fetch-failed')
    }
    // 那一格是**拉取式**（对方读不产生事件）⇒ 等这一页那支 5 秒定时器拉一次单房。
    await page.waitForTimeout(7000)
    const cellCount = await peerReadCell.count()
    const cellValue = cellCount > 0 ? await peerReadCell.first().getAttribute('data-peer-read') : null
    const cellText = cellCount > 0 ? await peerReadCell.first().innerText() : ''
    // 三个诊断读数：消息到底发出去没有 · 房里现在有几条 · **对端有没有 active 周期**
    // （`peer_read` 为 null 的三个原因里，后两个是"我没说过话"与"对端不是 active 成员"）。
    const afterHistory = await fetch(`${args.base}/v1/rooms/${openedRoomId}/messages?limit=20`, {
      headers: { authorization: `Bearer ${args.token}` },
    })
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null)
    const afterCount = Array.isArray(afterHistory?.messages) ? afterHistory.messages.length : '?'
    const mineAfter = Array.isArray(afterHistory?.messages)
      ? afterHistory.messages.filter((m) => (m.actor?.id ?? m.actor_id) === myId).length
      : '?'
    const membersProbe = await fetch(`${args.base}/v1/rooms/${openedRoomId}/members`, {
      headers: { authorization: `Bearer ${args.token}` },
    })
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null)
    const peerId = openedEntry.peer?.id ?? null
    const peerPeriods = Array.isArray(membersProbe?.members)
      ? membersProbe.members.filter((m) => m.participant?.id === peerId)
      : []
    const peerActive = peerPeriods.some((m) => m.left_at === null || m.left_at === undefined)
    check(
      '★ direct 房里、我说过话 ⇒ **我最后那条气泡下**出现那一格（服务端给、客户端原样渲染；文案与取值一致）',
      cellCount === 1 && (cellValue === 'read' || cellValue === 'unread') &&
        cellText === (cellValue === 'read' ? '读到这儿' : '还没读'),
      `那一格=${cellCount} 取值=${cellValue ?? '无'} 文案=${JSON.stringify(cellText)} · ` +
        `服务端这一行给的 peer_read=${JSON.stringify(openedEntry.peer_read ?? null)} · ` +
        `发消息=${sendStatus ?? '（房里本来就有我说的话，没发）'} · 房里消息=${afterCount} 其中我的=${mineAfter} · ` +
        `对端=${peerId ?? '无'} 周期=${peerPeriods.length} 有active=${peerActive} · ` +
        `房=${openedRoomId} kind=${openedEntry.room.kind}`,
    )
    check(
      '★ 那一格说的是**读位置**，不是"对方看到了"（文案不许写成后者）',
      cellCount === 1 && !cellText.includes('看到'),
      `那一格=${cellCount} 文案=${JSON.stringify(cellText)}`,
    )

    // ---- 那一格的**另一半**：对方真读了 ⇒ 当场变「已读」（2026-10-10）----
    //
    // 上面那半证明"服务端给了、客户端渲染了"；这一半证明**它跟着读位置动**：
    // 用那支 Agent 的 BackendAuth 凭据推进它的读游标（`PUT …/read-cursor`，切片 M1 的既有端点），
    // 等这一页那支 5 秒定时器拉一次 ⇒ 那一格必须从 `unread` 变成 `read`、文案变「读到这儿」。
    //
    // ⚠ **必须进夹具那间房**（`CELIRA_PROBE_DIRECT_ROOM_ID`）：凭据属于夹具那支 Agent，
    //    而"打开的第一行"可能是**别的房**（它的对端是另一支 Agent）⇒ 推进读游标会 404（实测踩过）。
    // ⚠ 没有那支凭据 / 那间房时报**不适用**（不算通过、也不算失败）——它们是环境给的东西。
    // ⚠ 这一半同时是**刷新那条**的判据：那一格是拉取式（对方读不产生事件），
    //    若定时器没拉单房，这里会**停在 unread** ⇒ 红。
    const agentCredential = process.env.CELIRA_PROBE_AGENT_CREDENTIAL || null
    const fixtureRoomId = process.env.CELIRA_PROBE_DIRECT_ROOM_ID || null
    if (!agentCredential || !fixtureRoomId) {
      check(
        '★ 对方真读了 ⇒ 那一格当场变「已读」：这一轮没验（夹具没交出那支凭据 / 那间房）',
        null,
        `凭据=${agentCredential ? '在场' : '缺'} 房=${fixtureRoomId ?? '缺'}`,
      )
    } else {
      await goto(`${args.base}/app/chat/${fixtureRoomId}`, '夹具那间 direct 房（判「已读」那一支）')
      await page.waitForTimeout(5000)
      // 先确保"我说过话"这个前提成立（那一格只在问题成立时出现）。
      const roomHistory = await fetch(`${args.base}/v1/rooms/${fixtureRoomId}/messages?limit=20`, {
        headers: { authorization: `Bearer ${args.token}` },
      })
        .then((r) => (r.ok ? r.json() : null))
        .catch(() => null)
      let lastMine = Array.isArray(roomHistory?.messages)
        ? [...roomHistory.messages].reverse().find((m) => (m.actor?.id ?? m.actor_id) === myId)
        : null
      if (!lastMine) {
        const sent = await fetch(`${args.base}/v1/rooms/${fixtureRoomId}/messages`, {
          method: 'POST',
          headers: {
            authorization: `Bearer ${args.token}`,
            'content-type': 'application/json',
            'Idempotency-Key': `probe-peer-read-b-${Date.now()}`,
          },
          body: JSON.stringify({ body: '探针：这一条用来判「已读」那一支' }),
        })
          .then((r) => (r.ok ? r.json() : null))
          .catch(() => null)
        lastMine = sent && sent.message ? sent.message : null
        await page.waitForTimeout(7000)
      }
      const beforeCell = await peerReadCell.first().getAttribute('data-peer-read').catch(() => null)
      const advanced = lastMine
        ? await fetch(`${args.base}/v1/backend/rooms/${fixtureRoomId}/read-cursor`, {
            method: 'PUT',
            headers: {
              authorization: `Bearer ${agentCredential}`,
              'content-type': 'application/json',
            },
            body: JSON.stringify({ message_id: lastMine.id }),
          })
            .then((r) => r.status)
            .catch(() => 'fetch-failed')
        : 'no-message'
      await page.waitForTimeout(7000)
      const afterCount2 = await peerReadCell.count()
      const afterValue =
        afterCount2 > 0 ? await peerReadCell.first().getAttribute('data-peer-read') : null
      const afterText2 = afterCount2 > 0 ? await peerReadCell.first().innerText() : ''
      check(
        '★ 对方真读了 ⇒ 那一格当场变「读到这儿」（那一格跟着读位置动，且定时拉取真的在拉）',
        afterValue === 'read' && afterText2 === '读到这儿',
        `推进读游标=${advanced}（推的是 ${lastMine ? lastMine.id : '无'}）· 推前=${beforeCell ?? '无'} · ` +
          `那一格=${afterCount2} 取值=${afterValue ?? '无'} 文案=${JSON.stringify(afterText2)}`,
      )
    }
  }
  // ② 不出现那一半：群房里**那一格整个不在**（它只在 direct 房回答这个问题）。
  const groupRoomId = Array.isArray(roomListProbe?.rooms)
    ? (roomListProbe.rooms.find((row) => row.room?.kind === 'group')?.room?.id ?? null)
    : null
  if (!groupRoomId) {
    check('★ 群房里不出现那一格：这一轮没验（这个家里没有群房）', null, '')
  } else {
    await goto(`${args.base}/app/chat/${groupRoomId}`, '群房（那一格不该出现）')
    await page.waitForTimeout(6000)
    check(
      '★ 群房里**那一格整个不在**（缺席＝这类房不回答这个问题，不是"未读"）',
      (await peerReadCell.count()) === 0,
      `群房那一格=${await peerReadCell.count()}`,
    )
    await goto(`${args.base}/app/chat/${openedRoomId}`, '回到刚才那间 direct 房')
    await page.waitForTimeout(2000)
  }

  // ---- 候选去重：同一支 Agent 不许列两行（2026-09-29 真机实测修）----
  //
  // 真机实测：小满**既是** accepted 联系人**又是**自有 Agent（批次三允许的常态），
  // `[...contacts, ...ownedAgents]` 不去重 ⇒ 「发起新会话」与「邀请成员」各列出两行同一个 id
  // （React key 也重复），而且下面 `start()` 里先 `contacts.find(...)` 命中 ⇒ 一支 Agent 被送进
  // `POST /v1/human-direct-rooms`（只收 Human）⇒ 404、界面红字「找不到这位用户」。
  // 判据按 **id** 数（`data-candidate-id`）—— 同名不同人不会误判。
  await goto(`${args.base}/app/chat`, '发起新会话（候选去重）')
  await page.waitForTimeout(3500)
  await page.getByRole('button', { name: '发起新会话' }).click()
  await page.waitForTimeout(1500)
  const candidateIds = await page
    .locator('[role="dialog"] button[data-candidate-id]')
    .evaluateAll((els) => els.map((el) => el.getAttribute('data-candidate-id') ?? ''))
  if (candidateIds.length === 0) {
    check('候选去重：这一轮没验（这一页没有可选的候选）', null, '候选行=0')
  } else {
    check(
      '★ 建房候选里没有重复行（同一支 Agent 只列一次）',
      new Set(candidateIds).size === candidateIds.length,
      `候选 ${candidateIds.length} 行 · 去重后 ${new Set(candidateIds).size} 个 id`,
    )
  }
  await page.keyboard.press('Escape')
  await page.waitForTimeout(400)

  // ---- 「新的朋友」= **一个输入框**（2026-10-20：按 kind 分叉的两个页签 + 「创建 Agent」都撤走）----
  //
  // 判据来自契约，不来自喜好：`POST /v1/contacts/requests` 的字段是 **`participant_id`**，
  // 目标"可以是任意 active 主体"（`CORE_V1_CONTACT_REVERSIBILITY.md` §5.2）。所以：
  //   ① 页面上不许再有「添加真人 / 添加 Agent」—— 那是客户端把世界拆掉的分叉装回来；
  //   ② 只有一个输入框，且明说 Human 与 Agent 同一串；
  //   ③ 贴一串**不是 id** 的东西 ⇒ 当场说清，不发请求（真机门禁不写，见上面那条纪律）。
  // ⚠ 「添加 Agent」现在是**创建所有权**，住在 `/app/agents` 右上角那颗「添加」里 ——
  //   这一页不该再有它（同一件事两个入口，迟早说两句不同的话）。
  await goto(`${args.base}/app/contacts/new`, '新的朋友（一个输入框）')
  await page.waitForTimeout(1500)
  const newContactText = await page.locator('body').innerText()
  const kindTabs =
    (await page.getByRole('button', { name: '添加真人' }).count()) +
    (await page.getByRole('button', { name: '添加 Agent' }).count())
  check(
    '★ 「新的朋友」不按 kind 分页签（世界只认一串 participant_id）',
    kindTabs === 0,
    `kind 页签=${kindTabs}`,
  )
  check(
    '「新的朋友」说清 Human 与 Agent 用同一串 ID',
    newContactText.includes('Human 与 Agent 都用这一串'),
    snippet(newContactText, 200),
  )
  const contactInputs = await page.locator('input').count()
  check('「新的朋友」只有一个输入框（没有第二个名字输入框 / 创建表单）', contactInputs === 1, `输入框=${contactInputs}`)
  await page.locator('#contact-id').fill('张三')
  await page.waitForTimeout(400)
  const junkText = await page.locator('body').innerText()
  check(
    '★ 贴一串不是 ID 的东西 ⇒ 当场说清（"名字在这里查不到人"，且不发请求）',
    junkText.includes('这不像一串身份 ID'),
    snippet(junkText, 200),
  )

  // ---- 设置区：模型配置**搬出去了**（2026-10-11 按机器寻址）+ 新的服务器节 ----
  //
  // ⚠ **判据翻转（2026-10-11）**：这里原有一条「设置区列出「模型配置」一级入口」
  // （2026-09-23 裁决「高频配置回设置一级」）。那一刀的前提被换掉了：模型配置**按机器寻址**
  // （`/app/runtimes/:entryId/config`）—— 一个"配某台机器"的页面住在没有机器上下文的设置区里，
  // 就只能自己长一行"配哪台机器"的选择器（**同一个问题两个答案**）。
  // ⇒ 入口归**机器页的「模型」间**（那里本来就有"去配置模型"），设置目录里**不该再有它**。
  // 判据落在**目录条目那一层**（与下面轴那条同源读数）—— 别拿整页正文正则，那会被别处文字污染。
  await goto(`${args.base}/settings`, '设置')
  await page.waitForTimeout(2500)
  const settingsText = await page.locator('body').innerText()
  // ⚠ **判据翻转（2026-10-05）**：这里原有一条「设置区新增『Agent 与服务器』分组
  // （模型配置 + 服务器两项）」—— 那一组已被 2026-10-04 的轴修**拆掉**
  // （「模型配置」搬进「你的机器」、中间那组改名「这个世界」），于是那条断言恒假。
  // 现在它翻成**量轴**（`SETTINGS_AXIS` / `axisSayingWrongThing`，判据在常量那一节）。
  // ★ 刀3（2026-09-24）：设置目录栏的**组标题**那一层 —— 取得到，且不得与唯一条目同名。
  //   组标题 = 每个组容器的**第一个 div**（`nav[aria-label="设置目录"]` 的直接子元素）。
  //   ⚠ 2026-10-05：同一次取值顺带把**组里的条目**也取回来（轴那条判据要用它）——
  //   两件事同源（同一份 DOM），分两次 evaluate 就是两份可能漂的读数。
  const settingsGroups = await page.evaluate(() => {
    const nav = document.querySelector('nav[aria-label="设置目录"]')
    if (!nav) return null
    return [...nav.children].map((group) => ({
      title: (group.querySelector('div')?.innerText ?? '').trim(),
      items: [...group.querySelectorAll('button')].map((button) => button.innerText.trim()),
    }))
  })
  const settingsTitles = Array.isArray(settingsGroups)
    ? settingsGroups.map((group) => group.title)
    : null
  const settingsItems = Array.isArray(settingsGroups)
    ? settingsGroups.flatMap((group) => group.items)
    : null
  // ★ **判据翻转（2026-10-11）**：见上面那段 —— 「模型配置」整个搬出设置目录
  //   （按机器寻址，入口归机器页的「模型」间）。
  check(
    '★ 设置目录里**不再**有「模型配置」（2026-10-11：它按机器寻址，入口归机器页的「模型」间）',
    Array.isArray(settingsItems) && !settingsItems.includes('模型配置'),
    `条目=${JSON.stringify(settingsItems)}`,
  )
  // 正对照：判据本身要真会红 —— 把两条塞回那个旧组，必须被抓到（两条都要）。
  const axisProbe = axisSayingWrongThing([
    { title: SETTINGS_GROUP_TITLE_RETIRED, items: ['家', '服务器'] },
  ])
  check(
    `设置目录的轴正对照：把「家」「服务器」塞回「${SETTINGS_GROUP_TITLE_RETIRED}」必须被抓到（否则难说是正对照）`,
    axisProbe.length === SETTINGS_AXIS.length,
    JSON.stringify(axisProbe),
  )
  // 判据本体：**取得到**（选择器非空 ⇒ 这条不是空判据）＋ 两条各归各组
  // ＋ 那个把两轴混在一起的旧组标题不许回来。
  check(
    `设置目录的**轴**与主导航同一条（${SETTINGS_AXIS.map(({ item, group }) => `${item}∈${group}`).join(' · ')}；旧组标题「${SETTINGS_GROUP_TITLE_RETIRED}」不许回来）`,
    Array.isArray(settingsGroups) &&
      axisSayingWrongThing(settingsGroups).length === 0 &&
      // ⚠ **别按源码里的大小写断文案**：组标题那一条有 CSS `uppercase`，`innerText` 给的是
      // **渲染后**的「AGENT 与服务器」。所以这里不分大小写，也容忍中间那个空格。
      !/agent\s*与服务器/i.test(settingsText),
    `组=${JSON.stringify(settingsGroups)}`,
  )
  // 正对照：判据本身要真会红 —— 把「家」当组标题喂进去，必须被抓到。
  const titleProbe = titlesSayingWrongThing([SETTINGS_GROUP_TITLE_BANNED])
  check(
    `组标题词表正对照：把「${SETTINGS_GROUP_TITLE_BANNED}」当组标题必须被抓到（否则难说是正对照）`,
    titleProbe.length === 1,
    JSON.stringify(titleProbe),
  )
  // 判据本体（一条）：**取得到**（选择器非空 ⇒ 这条不是空判据）＋ 组标题是「你的机器」
  // ＋ 组标题里没有「家」。三项合一条，是为了让"组标题改回「家」"这件事**只红这一条**。
  check(
    `设置目录的**组标题**是「${SETTINGS_GROUP_TITLE_EXPECTED}」、且不得是「${SETTINGS_GROUP_TITLE_BANNED}」（条目「${SETTINGS_GROUP_TITLE_BANNED}」照旧在）`,
    Array.isArray(settingsTitles) &&
      settingsTitles.length >= 3 &&
      settingsTitles.includes(SETTINGS_GROUP_TITLE_EXPECTED) &&
      titlesSayingWrongThing(settingsTitles).length === 0,
    `组标题=${JSON.stringify(settingsTitles)}`,
  )

  // ---- 本机服务页（★ 2026-10-11 收成**一个主语**）----
  //
  // 这一页从前叫「服务器」，还混着两行**世界侧**读数（服务器地址 / 账号）✗：同一页两个主语，
  // 而导航里它挂在「这个世界」组下 —— 可它在 `routeRegistry` 里一直写着 `planes: ['home']`。
  // 现在它只留**这台机器**的两行；那两行世界侧读数的权威在「通用」（地址在那里可编辑）。
  await goto(`${args.base}/settings/server`, '本机服务')
  await page.waitForTimeout(3000)
  const serverText = await page.locator('body').innerText()
  check(
    '本机服务页：两行都在（本机服务端 / 这台机器上的活动）',
    serverText.includes('本机服务端') && serverText.includes('这台机器上的活动'),
    snippet(serverText, 240),
  )
  check(
    '★ 本机服务页**不再复述世界侧**：这一页不出现「服务器地址」（它的权威在「通用」）',
    !serverText.includes('服务器地址'),
    snippet(serverText, 200),
  )
  check(
    '★ 本机服务页：活动那一行**带主语**，并给出去「通用」的路',
    serverText.includes('这台机器上的活动') && serverText.includes('你连的世界'),
    snippet(serverText, 200),
  )
  check(
    '本机服务页文案没有工程黑话（人话自检）',
    leakedJargon(serverText).length === 0,
    leakedJargon(serverText).length ? `漏了 ${JSON.stringify(leakedJargon(serverText))}` : snippet(serverText, 160),
  )

  // ---- 通用页：世界侧那两样（地址 / 账号）的**权威**在这里 -----------------------
  //
  // 这条断言从前挂在「服务器」那一页上（那一页当时复制了一份地址读数）⇒ 同一个事实两个位置。
  // 现在它随那个事实回到权威页：**地址显示的就是"现在连的是哪儿"** —— 包括"就是这一页"
  // 那一档（输入框空着看不出它到底连哪儿）。
  await goto(`${args.base}/settings/general`, '通用')
  await page.waitForTimeout(1500)
  const generalText = await page.locator('body').innerText()
  check(
    '通用页：说得出**现在连的是哪儿**（用户填的地址，或「本页面所在的地址」+它）',
    generalText.includes('本页面所在的地址') || /https?:\/\//.test(generalText),
    snippet(generalText, 200),
  )

  // ---- 机器清单**只留一个门**（2026-10-12）：加在清单页、管在那台机器自己那一页 ------
  //
  // 为什么必须有一条**真机**断言：那三句后果 ＋ 两次点击确认式是**行为**，源级守卫
  // （`tests/home-removal.test.ts`）只能证明"源码里写着"—— 证明不了它真的渲染出来、真的要点两次。
  // 判据一句话：**把"这台浏览器不再记它"读成"它进不来了"，代价在安全上**
  // （那台机器上那把钥匙仍然有效；钥匙的失效只有 CLI 一条路）。
  //
  // 手法：**用表单真的加一条形状合法但连不上的家** —— 地址用 `https://127.0.0.1` ＋ 端口填 `1`
  // ⇒ 立刻 `ECONNREFUSED`，不会让这一格挂满状态口那 12 秒超时（那是这条断言唯一的不稳定源）。
  // 收尾把它移除 ⇒ 不留残留（下面的模型配置那一族会被"家里有几台"影响，所以**顺序有意义**）。
  //
  // ⚠ 2026-10-12 形状变了：从前"加"住在 `/settings/homes`（同一份清单的**第二个门**）⇒
  //   这一族是"带意图过去、在那一页加、在那一页移除"。现在**清单只有一处**：
  //   ① 加：清单页那颗「添加」**就地**开表单（不再 navigate 走人）；
  //   ② 管（钥匙 / 移除）：进**那台机器自己那一页** ⇒ 「这台机器的设置」就地展开。
  await goto(`${args.base}/app/runtimes`, '我的执行端（清单）')
  await page.waitForTimeout(2000)
  await page.getByRole('button', { name: '添加', exact: true }).click()
  const nameField = page.getByLabel('名字', { exact: true })
  await nameField.waitFor({ state: 'visible', timeout: 8000 }).catch(() => {})
  check(
    '★ 清单页那颗「添加」**就地**开表单（不再把人送去设置里那一页）',
    (await nameField.isVisible().catch(() => false)) && page.url().endsWith('/app/runtimes'),
    `表单在场=${await nameField.count()} · url=${page.url()}`,
  )

  const probeHomeName = '探针：不存在的家'
  await nameField.fill(probeHomeName)
  await page.getByLabel('家地址', { exact: true }).fill('https://127.0.0.1')
  await page.getByLabel('状态口端口', { exact: true }).fill('1')
  await page.getByLabel('配置门端口', { exact: true }).fill('1')
  await page.getByLabel('家钥匙', { exact: true }).fill('probe-key-not-a-real-key')
  await page.getByRole('button', { name: '保存', exact: true }).click()
  await page.waitForTimeout(2500)

  const machineListText = await page.locator('body').innerText()
  check(
    '★ 加进来的那台**出现在清单里**（清单是唯一的门 ⇒ 加完就该在这一页看得见）',
    machineListText.includes(probeHomeName),
    snippet(machineListText, 160),
  )

  // 进**那台机器自己那一页**（清单行点它的名字），在那里管它。
  await page.getByText(probeHomeName, { exact: true }).first().click()
  await page.waitForTimeout(2000)
  await page.getByRole('button', { name: '这台机器的设置' }).click()
  await page.waitForTimeout(400)
  const adminText = await page.locator('body').innerText()
  const removeButton = page.getByRole('button', { name: `移除 ${probeHomeName}` })
  const deleteNamed = await page.getByRole('button', { name: `删除 ${probeHomeName}` }).count()
  check(
    '★ 机器页：管理面就地展开，那一条有「移除」按钮，而它的名字**不是**「删除」',
    (await removeButton.count()) === 1 && deleteNamed === 0 && adminText.includes('已配钥匙'),
    `移除=${await removeButton.count()} · 删除=${deleteNamed} · ${snippet(adminText, 160)}`,
  )

  await removeButton.click()
  await page.waitForTimeout(400)
  const armedText = await page.locator('body').innerText()
  check(
    '★ 第一次点击只是**把后果摆出来**：那台机器上那把钥匙仍然有效 ＋ 真的出口（CLI）',
    armedText.includes('那台机器上那把钥匙仍然有效') && armedText.includes('issue-home-key'),
    snippet(armedText, 200),
  )
  const confirmRemoveButton = page.getByRole('button', { name: `再次点击确认移除 ${probeHomeName}` })
  check(
    '★ 第一次点击**没有**把它移掉（按钮变成「再次点击确认移除」）',
    (await confirmRemoveButton.count()) === 1,
    `确认按钮=${await confirmRemoveButton.count()} · ${snippet(armedText, 120)}`,
  )

  await confirmRemoveButton.click()
  await page.waitForTimeout(1500)
  // 移除之后回清单：那一条没了，且不留残留。
  await goto(`${args.base}/app/runtimes`, '我的执行端（移除之后）')
  await page.waitForTimeout(2000)
  const afterRemovalText = await page.locator('body').innerText()
  check(
    '★ 第二次点击才真的移除（清单里那一条没了）',
    !afterRemovalText.includes(probeHomeName),
    snippet(afterRemovalText, 140),
  )

  // ★ **本机那条删不得**：进本机那一页展开管理面 ⇒ 它在场（"页面直接读"）且**没有**移除按钮。
  await goto(`${args.base}/app/runtimes/local`, '本机那一页')
  await page.waitForTimeout(2000)
  await page.getByRole('button', { name: '这台机器的设置' }).click()
  await page.waitForTimeout(400)
  const localAdminText = await page.locator('body').innerText()
  const localRemove = await page.getByRole('button', { name: /移除/ }).count()
  check(
    '★ 机器页：本机那条的管理面在场（页面直接读），且它**没有**移除按钮（预置那条移除不得）',
    localAdminText.includes('这台机器，页面直接读') && localRemove === 0,
    `移除按钮数=${localRemove} · ${snippet(localAdminText, 140)}`,
  )

  // ---- 模型配置：**按机器寻址**（2026-10-11）---------------------------------
  //
  // 判据：**配置跟着它的对象走** —— 这一页管的是**某台机器**上的钥匙清单（清单落
  // `<家>/model_sources.json`），所以地址里点名那台机器（`/app/runtimes/:entryId/config`），
  // 而不是住在"设置"里。旧地址保留成**别名**（重定向）：带 `?home=<id>` 落那台；
  // 没有上下文落执行端列表（**不猜是哪台** —— 与 `/app/runtime` 同款）。
  //
  // ⚠ 顺序有意义：先把别名两条出口验掉，**最后**停在新地址上 —— 下面那一族断言（左栏常驻 /
  //    预设 / 自定义降级…）读的是 `providersText`，必须取自**新地址那一页**。
  await goto(`${args.base}/settings/providers`, '旧地址（没有机器上下文）')
  await page.waitForTimeout(1500)
  check(
    '★ 旧地址 /settings/providers（无上下文）⇒ 落**执行端列表**，不猜是哪台',
    page.url().endsWith('/app/runtimes'),
    `落在 ${page.url()}`,
  )
  await goto(`${args.base}/settings/providers?home=local`, '旧地址（带 ?home=local）')
  await page.waitForTimeout(1500)
  check(
    '★ 旧地址 /settings/providers?home=local ⇒ 落到**那台机器**的配置页（老书签不破）',
    page.url().includes('/app/runtimes/local/config'),
    `落在 ${page.url()}`,
  )

  // ★ **认不出那台机器 ⇒ 如实说，绝不回落本机**（2026-10-11 修的那条）：地址里点名一台
  //   不在列表里的机器时，旧实现 `?? localHomeEntry()` 会**默默去配本机** —— 而用户以为
  //   自己在配那台。判据两件事：① 说得出「没有这台执行端」；② **配置面板一个都不在场**
  //   （回落本机会把它整块画出来 —— 所以"没有 DeepSeek"就是"没画面板"的硬证据）。
  await goto(
    `${args.base}/app/runtimes/00000000-0000-0000-0000-000000000000/config`,
    '点名一台不存在的机器',
  )
  await page.waitForTimeout(1500)
  const missingMachineText = await page.locator('body').innerText()
  check(
    '★ 地址里那台机器不在列表里 ⇒ 如实说「没有这台执行端」，且**不回落本机**（不画配置面板）',
    missingMachineText.includes('没有这台执行端') && !missingMachineText.includes('DeepSeek'),
    snippet(missingMachineText, 160),
  )

  await goto(`${args.base}/app/runtimes/local/config`, '某台机器的模型配置（按机器寻址）')
  await page.waitForTimeout(2500)
  const providersText = await page.locator('body').innerText()
  check(
    '模型配置：新地址 `/app/runtimes/:entryId/config` 可达（本机那一台）',
    providersText.length > 40 && !providersText.includes('404'),
    snippet(providersText, 100),
  )
  check(
    '★ 配置页**不再有「配哪台机器」那一行**（地址就是那台机器 —— 同一个问题不许两个答案）',
    !providersText.includes('配哪台机器'),
    snippet(providersText, 140),
  )

  /**
   * **模型配置门开着吗**（2026-09-26 加）。
   *
   * 门关着（`/home-config` 不通：没起 `node home.js config --serve`）时，
   * 页面**如实降级** —— 正文里说「读不到…的配置」，面板根本不渲染。
   * 那是**正确行为**（下面「清单为空时如实说空」那条会绿），但下面这一族
   * **面板类**断言就**没有可判对象**了。
   *
   * 从前它们一律记 FAIL ⇒「我这台机器没起那个口」和「代码坏了」长得**一模一样**。
   * 本次真机实跑 20 条红里，10 条是这一种。现在按仓库纪律报**不适用**：
   * 既不是 PASS（那才是假装通过），也不算 FAIL（那不是代码的问题）。
   *
   * ⚠ 定义必须**在使用之前**（第一次用就在下面几行）—— 上一版放晚了，
   *    真机上当场 `ReferenceError: Cannot access 'panelCheck' before initialization`。
   */
  const configGateOpen = !/读不到[^\n]{0,16}的配置/.test(providersText)
  const panelCheck = (name, ok, detail = '') =>
    check(
      name,
      configGateOpen ? ok : null,
      configGateOpen ? detail : '⚠ 不适用：本机模型配置门没开（/home-config 不通）⇒ 页面如实降级，面板不在场',
    )

  // ---- 配置页的**接线**（刀2b；2026-09-23 二次回炉后换了承担者）----
  //
  // ⚠ 为什么这几条必须落在真机：清单那套语义（新建 / 改一条不动绑定 / 409 被绑定挡住 / 解绑）
  // 已经在 `node --test` 里钉住了，但"页面上那颗按钮有没有接到那条路上"单测看不见 ——
  // 那正是「判据落在外壳而目标在内层」的反面。
  // ⚠ 这里**只点不产生写动作**的那几下（切一家 / 看输入框类型），
  // 因此**不会动这台机器上的任何配置** —— 真机门禁不该改主人的家。
  //
  // ⚠ 回炉换掉了 `StatusBar`（那条"清单 N 条 · Agent 正在用"的横条没有了）：**状态读数搬到了
  //    左栏每一行上**（「Agent 正在用」/「已保存」），所以判据跟着搬到那里 —— 强度不变。
  panelCheck(
    '配置页：Agent 用的是哪一条 / 清单有没有 —— 读数在场（左栏那份清单或面板头的"谁在用"）',
    // ⚠ 2026-09-23 称谓刀把徽标文案从「Agent 正在用」改成「正在使用」，这条词表当时**漏改了**
    //    （靠 `已保存` 侥幸过了一次）⇒ 现在补上。**约束不变、强度不减**：认的词只增不减。
    // ⚠ 2026-10-15「模型配置归机器侧」那一刀：承载者换成**左栏那份清单**（「这台机器上的钥匙
    //    （N）」/「还没配好的钥匙」）与面板头的**"谁在用"读数**（「X 在用」/「没人用」）——
    //    旧词照旧认（**只增不减**），新词补上。
    /清单 \d+ 条/.test(providersText) ||
      /这台机器上的钥匙（\d+）|这台机器上还没有配好的钥匙/.test(providersText) ||
      /正在使用|Agent 正在用|已保存|还没配|在用|没人用/.test(providersText) ||
      providersText.includes('还没有任何模型源'),
    snippet(providersText, 160),
  )
  check(
    '配置页：清单为空时如实说空（不是画一份假配置）',
    !/清单 0 条/.test(providersText) || providersText.includes('还没有任何模型源'),
    snippet(providersText, 140),
  )

  // ---- 点谁配谁（2026-09-23 二次回炉）：左栏常驻 / 点一行就配那一家 ----
  //
  // ⚠ **只点不写**（本仓真机门禁的硬线）：全程**不点保存那颗**，
  //    **也不点「获取模型列表」** —— 那一下会真的向供应商发一次带钥匙的请求。
  //    要判的是"东西挂没挂上来"，不是"那家通不通"（那要主人自己的钥匙，见报告 §验收）。
  const PRESET_TEXT = ['DeepSeek', 'OpenAI', '硅基流动', 'OpenRouter', '自定义']
  const PRESET_DISCLAIMER_TEXT = '不代表已经配好'
  const ENDPOINT_PLACEHOLDER = 'https://…/v1/chat/completions'

  // ★★ **2026-10-15「模型配置归机器侧」那一刀**：左栏换成了**这台机器上的钥匙**，
  //    预设表只在「加一把钥匙」那一步出现。判据跟着换承载者，**承诺一条不少**：
  //    ① 这一页的主语是**机器**（左栏是这台机器上的钥匙，不是"世上还有哪些家"）；
  //    ② 预设表**仍然在**、仍然带那句「不代表已经配好」——只是它现在住**加一把**那一步里；
  //    ③ 「加一条」这种**先声明再加**的动作照旧不在场（点左栏一条就是配它）。
  const landingText = await page.locator('body').innerText()
  check(
    '★ 配置页：左栏的主语是**这台机器**（「这台机器上的钥匙」＋「加一把钥匙」）—— 不是供应商目录',
    landingText.includes('这台机器上的钥匙') && landingText.includes('加一把钥匙'),
    snippet(landingText, 240),
  )
  panelCheck(
    '★ 页面进来就有落脚点：右侧已经是一条钥匙（不是一张空表单 / 不是"请先选"）',
    (await page.locator('aside button[aria-pressed="true"]').count()) <= 1,
    `高亮行数=${await page.locator('aside button[aria-pressed="true"]').count()}`,
  )
  check(
    '★ 「加一条」那颗按钮**不在场了**（旧版就是"点左栏就是加"，不是先声明再加）',
    (await page.getByRole('button', { name: '加一条', exact: true }).count()) === 0,
    `count=${await page.getByRole('button', { name: '加一条', exact: true }).count()}`,
  )
  // 预设表**只在"加一把钥匙"那一步**出现 —— 点开它，那些家与那句明示就在场。
  // ⚠ 同一动作有两个出口（左栏那颗 ＋ 空清单时右侧那句话下面那颗）⇒ 这里**点名左栏**
  //   （探针不许靠"页面上只有一颗"来选中它）。
  await page.getByRole('complementary').getByRole('button', { name: '加一把钥匙' }).click()
  await page.waitForTimeout(300)
  const presetListText = await page.locator('body').innerText()
  check(
    '★ 预设表挪进了「加一把钥匙」那一步：那些家 ＋ 那句「预设 ≠ 已配置」都在场（模板库 ≠ 机器事实）',
    PRESET_TEXT.every((name) => presetListText.includes(name)) &&
      presetListText.includes(PRESET_DISCLAIMER_TEXT),
    `缺 ${JSON.stringify(PRESET_TEXT.filter((name) => !presetListText.includes(name)))} · 明示=${presetListText.includes(PRESET_DISCLAIMER_TEXT)}`,
  )

  // 标记（ProviderMark）：左栏每家一个**真 logo**（来自 LobeHub lobe-icons，
  // 见 `src/assets/provider-logos/NOTICE.md`）；「自定义」不是供应商，走首字母。
  // 判据落在"**每一行都真的画出来了**"：漏一家 ⇒ 静默退成首字母（看着像没做完）；
  // 注入成空 svg（raw 串没进去）⇒ `filled` 那条会红。
  const markReadings = await page.evaluate(() => {
    const rows = [...document.querySelectorAll('nav[aria-label="预设供应商"] button')]
    return {
      rows: rows.length,
      drawn: rows.filter((b) => b.querySelector('svg')).length,
      // 光有 <svg> 不算数 —— 一份只有 <title> 的**空壳**也会被算进去 ⇒ 要真有**绘制元素**
      painted: rows.filter(
        (b) =>
          b.querySelector('svg')?.querySelector(
            'path,rect,circle,ellipse,polygon,polyline,line,use',
          ) != null,
      ).length,
      // 标记是固定尺寸的小方块（28px）—— 量一条证明它真占了那块位
      markBox: (() => {
        const el = rows[0]?.querySelector('span[aria-hidden]')
        if (!el) return null
        const r = el.getBoundingClientRect()
        return { w: Math.round(r.width), h: Math.round(r.height) }
      })(),
    }
  })
  check(
    '★ 左栏每家都有**真 logo**（本仓内置矢量、运行时零请求）——只有「自定义」走首字母',
    markReadings.rows > 0 &&
      // 一行是「自定义」（它不是供应商，故意走首字母）
      markReadings.drawn === markReadings.rows - 1 &&
      markReadings.painted === markReadings.drawn &&
      markReadings.markBox?.w === 28,
    `行数=${markReadings.rows} 画出标记=${markReadings.drawn} 有绘制元素=${markReadings.painted} 标记尺寸=${JSON.stringify(markReadings.markBox)}`,
  )

  // 点「DeepSeek」⇒ 右侧就是这一家：地址是**替你填好的**、摆成**读数行**（不是输入框）。
  await page.getByRole('button', { name: /^DeepSeek/ }).click()
  await page.waitForTimeout(400)
  const presetFormText = await page.locator('body').innerText()
  const endpointInput = page.getByPlaceholder(ENDPOINT_PLACEHOLDER)
  // 预设那条路：接口地址**不摆输入框**（摆成读数行）—— 判据是"那个 placeholder 的输入框不在场"，
  // 且地址以 `<code>` 正文的形状**上屏**（比"输入框只读"更强：连可编辑的空壳都没有）。
  const presetEndpointBoxCount = await endpointInput.count()
  const presetFormInputs = await page.locator('input').count()
  const passwordFields = await page.locator('input[type="password"]').count()
  const endpointReadout = presetFormText.includes('api.deepseek.com')
  panelCheck(
    '点左栏一家 ⇒ 右侧就是**这一家**：地址已替你填好、摆成**读数行**（不是输入框）+ 密钥格',
    endpointReadout && presetEndpointBoxCount === 0 && passwordFields >= 1,
    `地址在正文=${endpointReadout} 地址输入框=${presetEndpointBoxCount} 输入框=${presetFormInputs} password=${passwordFields}`,
  )
  // ⚠ 2026-09-23 文案减重刀：底下那句讲解（"已经替你填好…这格不用你管"）**删了**。
  //   这个约束**换承载者，不是放宽**：「一眼看出这格不用填」= **只读地址行**
  //   （上面那条 check 断的：地址在正文里、且那格压根没有输入框）＋ **那枚中性徽标**（这里断）。
  //
  // ★★ **2026-10-16 主人当次："这种接口地址，为啥是绿的，看起来很不合理？"**
  //    从前这一行挂的是 `presence-online`（= **在线**那个绿）＋一个勾＋「已就绪」—— 而这一格
  //    **只说明"这格不用你填"**，没有验证过任何东西（对方通不通、钥匙对不对，一概未知）：
  //    绿＋勾＋"已就绪"在替一件**没发生的事**作证 ✗（同一屏上「没人用」还挂着，更自相矛盾）。
  //    判据（本仓早就写着）：**试连是读数不是判据**、**预设 ≠ 已配置** ⇒ 那身绿属于**读数**。
  //    ⇒ 这条断言现在有两颗牙：① 徽标说的是真事（「预设地址 · 不用你填」）；
  //       ② **含这个地址的盒子里不许出现"在线绿"**（不靠文案、直接量 DOM 的类名）。
  const endpointGreen = await page.evaluate(() => {
    const hit = [...document.querySelectorAll('code')].find((node) =>
      (node.textContent ?? '').includes('api.deepseek.com'),
    )
    if (!hit) return { found: false, green: [] }
    const green = []
    for (let node = hit; node && node.tagName !== 'SECTION'; node = node.parentElement) {
      for (const cls of node.classList) if (cls.includes('presence-online')) green.push(cls)
    }
    return { found: true, green }
  })
  panelCheck(
    '★ 预设那条路：地址格挂着**中性**徽标「预设地址 · 不用你填」，且**不许是"在线绿"**',
    presetFormText.includes('预设地址 · 不用你填') &&
      endpointGreen.found &&
      endpointGreen.green.length === 0,
    `徽标=${presetFormText.includes('预设地址 · 不用你填')} 含地址的盒子里出现的绿=${JSON.stringify(endpointGreen.green)}`,
  )
  panelCheck(
    '点左栏一家 ⇒ 面板头有**身份 + 状态徽标 + 保存**（动作集中右上角 —— 旧版那个形状）',
    // ⚠ 2026-10-15：徽标从「正在使用 / 已保存 / 还没配」换成**"谁在用"的读数**
    //    （「小欣 在用」/「没人用」/「还没配」）—— 判据认的词**只增不减**。
    presetFormText.includes('DeepSeek') &&
      /在用|还没配|已保存|正在使用|Agent 正在用/.test(presetFormText),
    snippet(presetFormText, 200),
  )
  const fetchButton = page.getByRole('button', { name: '获取模型列表' })
  panelCheck(
    '★ 「获取模型列表」按钮挂着且可用（**只看不点**：点了会真发一次请求）',
    (await fetchButton.count()) === 1 && (await fetchButton.isEnabled()),
    `count=${await fetchButton.count()} enabled=${await fetchButton.isEnabled().catch(() => '?')} · ${snippet(presetFormText, 160)}`,
  )
  panelCheck(
    '「获取模型列表」旁边明说这一步**不会保存任何东西**（探测 ≠ 保存）',
    presetFormText.includes('不会保存任何东西'),
    snippet(presetFormText, 160),
  )
  check(
    '配置页文案没有工程黑话（人话自检）',
    leakedJargon(presetFormText).length === 0,
    leakedJargon(presetFormText).length
      ? `漏了 ${JSON.stringify(leakedJargon(presetFormText))}`
      : snippet(presetFormText, 160),
  )

  // 「自定义」＝**降级通道仍在**：地址那格**回到输入框**（老表单流程没被这一刀删掉）。
  await page.getByRole('button', { name: /^自定义/ }).click()
  await page.waitForTimeout(400)
  const customEndpointEditable = await endpointInput.isEditable().catch(() => false)
  const customEndpoint = await endpointInput.inputValue().catch(() => '?')
  panelCheck(
    '自定义＝降级通道还在：地址**回到可编辑的输入框、且为空**（手填端点那条老路一字未删）',
    customEndpointEditable && customEndpoint === '',
    `可编辑=${customEndpointEditable} 地址=「${customEndpoint}」`,
  )
  const customInputs = await page.locator('input').count()
  const customPasswords = await page.locator('input[type="password"]').count()
  panelCheck(
    '自定义：表单字段齐全（输入框 ≥4、密钥那格是密码型，不回显）',
    customInputs >= 4 && customPasswords >= 1,
    `输入框=${customInputs} password=${customPasswords}`,
  )

  // 回到一家预设 ⇒ 界面是"切哪家配哪家"，没有"收起"这个概念（旧版也没有）。
  await page.getByRole('button', { name: /^DeepSeek/ }).click()
  await page.waitForTimeout(400)
  const afterSwitchText = await page.locator('body').innerText()
  // ⚠ 预设那条路**没有地址输入框**（是读数行）⇒ 判据读**渲染后的正文**里那个地址，
  // 别再拿 `inputValue()` 读（那个 input 只有「自定义」那条路才有）。
  const deepseekEndpointShown = afterSwitchText.includes('api.deepseek.com')
  panelCheck(
    '★ 换回一家：右侧跟着换（地址回到那家的），左栏高亮也回到它 —— 切换而不是收起',
    deepseekEndpointShown &&
      (await page.locator('nav[aria-label="预设供应商"] button[aria-pressed="true"]').count()) === 1,
    `正文里有地址=${deepseekEndpointShown}· ${snippet(afterSwitchText, 120)}`,
  )

  // ---- 渲染错误边界：chunk 拿不到时必须给降级页，不是白屏 ----
  //
  // 为什么这条只能落在真机上：前端每个路由都是 `lazy()`，而宿主换版
  // （`home.js down/up`）会换掉构建 hash ⇒ **已经开着的标签页**再去取旧 chunk 就是 404。
  // 那一刻的旧行为是"整棵 React 树被卸载"= 白屏、一个字都不说。这一刀把它收口成
  // 一句真话 + 一条出路（分类与文案见 `src/lib/renderFailure.ts`）。
  //
  // 做法：只放行**本会话加载过的** JS，其余 `/assets/*.js` 一律 404 —— 这就是那条路的
  // 精确复现；然后走 **SPA 内导航**（不是整页 `goto`，那会重载 document，是另一件事）
  // 去一条没访问过的懒路由，让它的 chunk 真的去请求。
  //
  // 记下"进拦截前"的控制台错误数：下面那次拦截会**故意**造出 404，末尾报错误总数时必须
  // 把它扣出来说清 —— 否则每次跑都看到"有 N 条错误"而误当成产品问题（实测踩过）。
  const consoleErrorsBeforeBoundary = consoleErrors.length

  await goto(`${args.base}/app/chat`, '聊天首页（错误边界前置）')
  await page.waitForTimeout(4000)
  const beforeBoundaryText = await page.locator('body').innerText()

  // 拦截：只放行**装拦截那一刻已经加载完**的 JS。整页导航时入口和它的静态依赖都在
  // 快照里，唯一"新"的请求就是这条路由自己那份懒 chunk ⇒ 精确复现「宿主换版后，
  // 旧标签页去取旧 chunk」那条路。
  //
  // ⚠ 必须用**快照**、不能直接读 `loadedJs`：`page.on('request')` 的执行先于 route
  // handler，直接读的话**本次请求自己已经被记进去了** ⇒ "新 chunk"会被误判成"已加载"
  // 而放行（实测踩过：`ContactsPage` 的 chunk 被放行、页面照常渲染，断言两头不落）。
  const allowedJs = new Set(loadedJs)
  const blockedJs = []
  const isJsAsset = (url) => /\/assets\/.*\.js(\?|$)/.test(url)
  await page.route(isJsAsset, (route) => {
    const url = route.request().url().split('?')[0]
    if (allowedJs.has(url)) return route.continue()
    blockedJs.push(url.split('/').pop())
    return route.fulfill({ status: 404, contentType: 'text/plain', body: '' })
  })

  await goto(`${args.base}/app/contacts`, '联系人（它的 chunk 被拦）')
  // 等**状态**不等时长：降级页一到就返回；别的形状（含空白）也立刻返回，交给断言报原文。
  const boundaryText = await waitForBoundaryText(page)
  await page.unroute(isJsAsset)
  // 这一段里"故意"造出来的错误数（就是上面那几条 404）——末尾读数要把它单列。
  const selfInflictedErrors = consoleErrors.length - consoleErrorsBeforeBoundary

  // 正向断言（TASK_58 起本仓不许断"某串不在场"）：降级页**在场**，且它平时**不在场**。
  check(
    'chunk 拿不到 ⇒ 渲染错误边界给降级页（不是白屏）',
    boundaryText.includes(RELOAD_LABEL) && !beforeBoundaryText.includes(RELOAD_LABEL),
    `拦下 ${blockedJs.length} 个 JS（${blockedJs.slice(0, 3).join(', ')}）· ${snippet(boundaryText, 110)}`,
  )
  check(
    '降级页认得出这是「文件拿不到」那一类，没混说成「页面出错」',
    boundaryText.includes(CHUNK_FAILURE_TITLE) && !boundaryText.includes(RENDER_FAILURE_TITLE),
    snippet(boundaryText, 130),
  )
  check(
    '降级页有可读正文（不是空白页）',
    boundaryText.trim().length > 20,
    `正文长度=${boundaryText.trim().length}`,
  )

  // ---- 关系面（拒绝 / 断交 / 拉黑 / 解除拉黑）：只看不点 --------------------------
  //
  // ⚠ **位置是有约束的：必须排在"拦 chunk"那一段之后。** 这一族要访问 `/app/contacts` 与
  // 资料卡，而那两条路由的懒 chunk 一旦被本会话加载过，前面那条 `chunk 拿不到` 就**拦不到
  // 任何东西**（实测踩过：本段原先在设置区之前 ⇒ `拦下 0 个 JS`、降级页不出现，两条断言假红）。
  // 脚本文件头的「环境前置」第 1 条说的就是这件事的另一半（dev 服务器不发 `/assets/*.js`）。
  //
  // ⚠ **只断言"画得出来、且把不可逆说清了"，绝不点下去**：断交与拉黑会**永久封存**双方那间房
  // （契约：段结束时双方成员周期同事务关闭），真机门禁不该去动真关系。动作本身的行为由
  // `tests/relation-actions.test.ts` 与真机手工演练负责。
  const contactsProbe = await fetch(`${args.base}/v1/contacts`, {
    headers: { authorization: `Bearer ${args.token}` },
  })
    .then((r) => (r.ok ? r.json() : null))
    .catch(() => null)
  const allContacts = Array.isArray(contactsProbe?.contacts) ? contactsProbe.contacts : []
  const acceptedPeer = allContacts.find((c) => c.status === 'accepted' && c.peer?.id)
  const incomingPeer = allContacts.find(
    (c) => c.status === 'pending' && c.direction === 'incoming' && c.peer?.id,
  )

  if (!acceptedPeer) {
    check('关系面：这一轮没验（这个世界没有 accepted 联系人）', null, `关系账 ${allContacts.length} 条`)
  } else {
    await goto(`${args.base}/app/contacts/${acceptedPeer.peer.id}`, '联系人资料卡（关系面）')
    await page.waitForTimeout(2500)
    // ⚠ `locator('main')` 在这一页命中**两个**（壳层一个、本页右栏一个）⇒ 实测 strict mode
    // 直接抛。取**最后一个**：资料卡渲染在右栏那个里。
    const cardText = await page.locator('main').last().innerText()
    check(
      '★ 资料卡给出「断交」与「拉黑」两个动作',
      cardText.includes('断交') && cardText.includes('拉黑'),
      snippet(cardText, 160),
    )
    // 不可逆的后果必须**写在按钮旁边**（常驻说明），不靠二次确认里的一句话：
    // 那三件事是契约原文（旧房永久封存 / 复合另建新房 / 不通知对端）。
    const irreversible = ['永久封存', '另建一间新房', '对方不会收到通知']
    const missing = irreversible.filter((phrase) => !cardText.includes(phrase))
    check(
      '★ 不可逆的后果写在按钮旁（永久封存 + 另建新房 + 不通知对端）',
      missing.length === 0,
      missing.length === 0 ? '三句都在' : `缺=${JSON.stringify(missing)}`,
    )
    check(
      '拉黑的后果说清了：同时就是断交、不影响群聊',
      cardText.includes('拉黑同时就是断交') && cardText.includes('不影响群聊'),
      snippet(cardText, 160),
    )
  }

  // 「拒绝」只在**真的有 incoming 申请**时才判得动（那是世界的状态，不是页面能造出来的）。
  if (!incomingPeer) {
    check('关系面：拒绝按钮这一轮没验（没有等我还价的申请）', null, 'pending+incoming 有 0 条')
  } else {
    await goto(`${args.base}/app/contacts`, '联系人（待确认行）')
    await page.waitForTimeout(2500)
    const pendingText = await page.locator('aside').innerText()
    check(
      '★ 「待确认」那一行拒绝与接受成对出现',
      pendingText.includes('拒绝') && pendingText.includes('接受'),
      snippet(pendingText, 160),
    )
  }

  // ─────────────────────────────────────────────────────────────────────────
  // 读数面真机判据（跨族收口刀 2 / 刀 4 / 刀 5 + mention 客户端第二步）
  //
  // 为什么加在这一段：这几条的**核心不是"页面长什么样"，而是"同一个处境下不许说另一句话"**
  // ——「读不到」与「确认没有」是两件事，页面必须分开说。纯函数用例已经钉住了裁决面
  // （`mentionTargets` / `credentials` / `agentBindings` / `chatFeedState`），
  // 这里钉的是**页面真的接上了那个裁决面**——那是纯函数测不到的一段。
  //
  // 手法：用 `page.route` **制造**读失败（abort），不去等它自然发生 —— 确定性、与网速无关。
  // 每条判据都配一句"另一句话**不在场**"（负对照内建：把裁决面换回旧写法，这一句立刻红）。
  //
  // ⚠ 这一段会**故意**造出若干条控制台错误（被 abort 的请求），末尾读数会把它们单列。
  // ─────────────────────────────────────────────────────────────────────────
  const errorsBeforeReadouts = consoleErrors.length

  /** 用**页面自己的源**打一次 Core：判据只读世界事实，不碰 UI 内部状态。 */
  const apiJson = (path, init) =>
    page.evaluate(
      async ([p, i, token]) => {
        const res = await fetch(p, {
          ...(i || {}),
          headers: { ...((i && i.headers) || {}), Authorization: `Bearer ${token}` },
        })
        // ⚠ 2026-10-20（T3 那一刀撞上）：`204 No Content` **是成功响应、但没有 JSON 正文**
        // ⇒ `res.ok` 为真时也不能无脑 `res.json()`（那一句会抛 `Unexpected end of JSON input`，
        // 整支门禁崩在半路）。这一格的用途只是"拿状态码 ＋ 可能有的正文"（撤销那条只要状态码）。
        let body = null
        if (res.ok) {
          try {
            body = await res.json()
          } catch {
            body = null
          }
        }
        return { status: res.status, body }
      },
      [path, init || null, args.token],
    )

  // ① 聊天首屏：**消息读失败** ⇒ 说"没读到"，**不许**说"这个会话还没有消息"
  //
  // 这一条正是刀 5 修的病：旧判据只有 `roomMessages.length === 0`，于是"读失败"被显示成
  // 「这个会话还没有消息，说一句开始吧。」—— 那句话会一直挂着，看起来像历史被清空。
  {
    const meRes = await apiJson('/v1/me')
    const agentsRes = await apiJson('/v1/agents')
    const firstAgent = agentsRes.body?.agents?.[0]?.agent?.id ?? null
    if (!meRes.body) {
      // ⚠ `/v1/me` 读不到**不是**"环境没就绪"：这一轮的 token 刚刚登录成功过
      // ⇒ 这是世界那一侧真的出了问题，该红（别混进 N/A 里假装"这一轮没验"）。
      check('首屏读数判据的前置：读得到 /v1/me', false, 'me 读不到')
    } else if (!firstAgent) {
      // 账号里没有 Agent ⇒ 这一条判不动（**不是**代码问题）。三态表的 `null` 就是为它准备的。
      check('首屏读数判据的前置：账号里至少一个 Agent（没有 ⇒ 这一轮没验）', null, 'agents=0')
    } else {
      // 挑一个**本会话从没打开过**的房间：它的消息既不在 store 内存里、也不在
      // IndexedDB 缓存里 ⇒ "读失败"这一支不会被旧消息掩盖。**优先不造数据**：
      // 只有"每个房间都打开过"时，才把这个房的消息缓存摘掉造一个"冷房"
      // ——一间房 = 一段关系（Core 批次二）之后，"同一对再建一间空房"这条路
      // 已经不存在（换 key 也回到原房），而且摘缓存**不新增任何世界事实**。
      const roomsRes = await apiJson('/v1/rooms')
      const allRooms = (roomsRes.body?.rooms ?? []).map((entry) => entry.room)
      let probeRoom = allRooms.find((room) => !visitedRoomIds.has(room.id))?.id ?? null
      if (!probeRoom) {
        probeRoom = allRooms[0]?.id ?? null
        if (probeRoom) {
          await page.evaluate(async (roomId) => {
            await new Promise((resolve) => {
              const req = indexedDB.open('celira-cache')
              req.onerror = () => resolve()
              req.onsuccess = () => {
                const db = req.result
                if (!db.objectStoreNames.contains('rooms')) return resolve()
                const tx = db.transaction('rooms', 'readwrite')
                tx.objectStore('rooms').delete(roomId)
                tx.oncomplete = () => resolve()
                tx.onerror = () => resolve()
              }
            })
          }, probeRoom)
          console.log(
            '注意：本会话每个房间都打开过 ⇒ 摘掉该房的消息缓存来造"冷房"（世界不新增事实）',
          )
        }
      }
      if (!probeRoom) {
        // 世界里一个房都没有 ⇒ 判不动（环境没就绪，不是代码问题）。
        check('首屏读数判据的前置：能挑到一个"没打开过"的房（或造一个空房）（没有 ⇒ 这一轮没验）', null, '两个来源都空')
      } else {
        const messagesRoute = /\/v1\/rooms\/[^/]+\/messages(\?|$)/
        await page.route(messagesRoute, (route) => route.abort())
        await goto(`${args.base}/app/chat/${probeRoom}`, '空房（消息口被拦）')
        await page.waitForTimeout(2500)
        const feedText = await page.locator('body').innerText()
        await page.unroute(messagesRoute)
        check(
          '首屏读不到 ⇒ 如实说「没读到」（不是沉默、也不是空态）',
          feedText.includes('没读到'),
          snippet(feedText, 200),
        )
        check(
          '★ 首屏读不到时**不许**说「这个会话还没有消息」（读不到 ≠ 确认没有）',
          !feedText.includes('这个会话还没有消息'),
          snippet(feedText, 200),
        )
      }
    }
  }

  // ② 绑定读数：**读失败** ⇒ 说「绑定还没读到」，**不许**说「还没有后端绑定」
  //
  // 刀 4 修的病：`bindingsByAgent[id] ?? []` 把"还没加载 / 读失败"说成"没有绑定"，
  // 并把创建表单递上去（而契约是同一 Agent 至多一个 active Binding）。
  {
    await goto(`${args.base}/app/agents`, '我的 Agent 列表（绑定读数判据）')
    const rowForBindings = page.getByRole('button', { name: /^打开 .* 的主页$/ })
    // 等**行**出现再数：`goto` 只保证 DOM 就绪，列表是异步拉的（第一版没等 ⇒ 假红）。
    await rowForBindings
      .first()
      .waitFor({ timeout: 8000 })
      .catch(() => {})
    if ((await rowForBindings.count()) === 0) {
      // 账号里没有 Agent ⇒ 判不动（环境没就绪，不是代码问题）。
      check('绑定读数判据的前置：账号里至少一个 Agent（没有 ⇒ 这一轮没验）', null, '行数=0')
    } else {
      const bindingsRoute = /\/v1\/agents\/[^/]+\/backend-bindings(\?|$)/
      await page.route(bindingsRoute, (route) => route.abort())
      await rowForBindings.first().click()
      await page.waitForTimeout(1200)
      // **重载**：store 是内存态，重载后从零开始 ⇒ 这一次读失败不会被上一轮读到的绑定掩盖。
      await page.reload({ waitUntil: 'domcontentloaded' })
      await page.waitForTimeout(2500)
      const bindText = await page.locator('body').innerText()
      await page.unroute(bindingsRoute)
      check('绑定读不到 ⇒ 如实说「绑定还没读到」', bindText.includes('绑定还没读到'), snippet(bindText, 200))
      check(
        '★ 绑定读不到时**不许**说「还没有后端绑定」（读不到 ≠ 没有）',
        !bindText.includes('还没有后端绑定'),
        snippet(bindText, 200),
      )
    }
  }

  // ③ 凭据读数：**读失败** ⇒ 说「凭据还没读到」，且**签发按钮禁用**（fail-closed）
  //
  // 刀 2 修的病：复合键两侧各写一遍 + 消费侧 `?? []` ⇒ 键一漂移就静默说"没有凭据"。
  {
    // ⚠ 上一条判据把页面留在了**Agent 的主页**上，这里必须先回列表（第一版没回 ⇒ 找不到行 ⇒ 假红）。
    await goto(`${args.base}/app/agents`, '我的 Agent 列表（凭据读数判据）')
    const agentRowForCredentials = page.getByRole('button', { name: /^打开 .* 的主页$/ })
    await agentRowForCredentials
      .first()
      .waitFor({ timeout: 8000 })
      .catch(() => {})
    if ((await agentRowForCredentials.count()) === 0) {
      // 账号里没有 Agent ⇒ 判不动（环境没就绪，不是代码问题）。
      check('凭据读数判据的前置：账号里至少一个 Agent（没有 ⇒ 这一轮没验）', null, '行数=0')
    } else {
      // 先正常进一次：绑定读得到 ⇒ 凭据区才渲染（它需要一条 active 绑定）。
      await agentRowForCredentials.first().click()
      await page.waitForTimeout(2000)
      // ★ 2026-10-20（T4）：接入 / 凭据搬进「接线（高级）」并**默认折叠**（有生效的接入时）⇒
      //   判据要的那几格在折叠区里，先按页面上那颗「展开」（按钮在就是证据：面板在，只是收着）。
      const expandForCredentials = page.getByRole('button', { name: '展开' })
      if ((await expandForCredentials.count()) > 0) await expandForCredentials.first().click()
      await page.waitForTimeout(400)
      const beforeCredentialsText = await page.locator('body').innerText()
      // 判据从"正文里有『凭证』"收紧成"这一支真的有一条生效的接入"（折叠头那句读数里
      // 「凭证」两个字一直在 ⇒ 旧的裸子串判据会**恒真**，那是装饰性覆盖）。
      if (!beforeCredentialsText.includes('条接入')) {
        // 「凭证」区需要**一条 active 绑定**才渲染 ⇒ 名下有绑定才判得动
        // （环境没就绪，不是代码问题；文件头那张前置表里写着这一条）。
        check('凭据读数判据的前置：这一页有「凭证」区（名下有接入绑定才判得动 ⇒ 没有即这一轮没验）', null, snippet(beforeCredentialsText, 200))
      } else {
        const credentialsRoute = /\/v1\/agents\/[^/]+\/backend-bindings\/[^/]+\/credentials(\?|$)/
        await page.route(credentialsRoute, (route) => route.abort())
        await page.reload({ waitUntil: 'domcontentloaded' })
        await page.waitForTimeout(2500)
        // 重载之后折叠态回到默认（有生效的接入 ⇒ 收着）⇒ 再展开一次才看得到那一格。
        const expandAfterReload = page.getByRole('button', { name: '展开' })
        if ((await expandAfterReload.count()) > 0) await expandAfterReload.first().click()
        await page.waitForTimeout(400)
        const credText = await page.locator('body').innerText()
        await page.unroute(credentialsRoute)
        check('凭据读不到 ⇒ 如实说「凭据还没读到」', credText.includes('凭据还没读到'), snippet(credText, 200))
        check(
          '★ 凭据读不到时不许签发（看不见既有凭据时签发可能多造一条 active 凭据）',
          (await page.getByRole('button', { name: '签发凭证' }).isDisabled().catch(() => false)) ||
            (await page.getByRole('button', { name: '轮换凭证' }).isDisabled().catch(() => false)),
          snippet(credText, 200),
        )
      }
    }
  }

  // ④ mention 候选：群房里**人类也在候选里**（mention 客户端第二步修的病：@ 不到人）
  //
  // 旧判据 `room.canMention = 房里有 agent 成员` + 发送时只留 agent 目标 ⇒
  // 纯人类群房完全 @ 不出人。这一条要求世界里有一个"含另一个人"的群房。
  {
    const meRes = await apiJson('/v1/me')
    const myId = meRes.body?.participant?.id ?? null
    const roomsRes = await apiJson('/v1/rooms')
    const groupRooms = (roomsRes.body?.rooms ?? [])
      .map((entry) => entry.room)
      .filter((room) => room.kind === 'group')
    let target = null
    for (const room of groupRooms) {
      const membersRes = await apiJson(`/v1/rooms/${room.id}/members`)
      const others = (membersRes.body?.members ?? [])
        .map((m) => m.participant)
        .filter((p) => p.kind === 'human' && p.id !== myId)
      if (others.length > 0) {
        target = { room, human: others[0] }
        break
      }
    }
    if (!target) {
      // 这个世界里没有"含另一个人"的群房 ⇒ 判不动（环境没就绪，不是代码问题）：
      // 纯人类群房是这一条**要的输入**，造它得先有两个已接受联系人（文件头那张前置表写着）。
      check(
        'mention 候选判据的前置：世界里有一个「含另一个人」的群房（没有 ⇒ 这一轮没验）',
        null,
        `群房数=${groupRooms.length}（需要一个含 ≥2 个 Human 的群房）`,
      )
    } else {
      await goto(`${args.base}/app/chat/${target.room.id}`, '两人群房（@ 候选判据）')
      await page.waitForTimeout(2500)
      const beforeMention = await page.locator('body').innerText()
      const composer = page.getByRole('textbox', { name: /^发送到 / })
      await composer.click()
      await composer.type('@', { delay: 40 })
      await page.waitForTimeout(900)
      const afterMention = await page.locator('body').innerText()
      // 负对照：**数出现次数**而不是"在不在场" —— 那个人的名字可能本来就出现在别处
      // （实测踩过：群里的系统消息「甲 加入了会话」里就有"甲" ⇒ "在不在场"恒真）。
      // 候选行会让它**多出现一次**，这才是"候选里有他"的证据。
      const countOf = (text, needle) => text.split(needle).length - 1
      const beforeCount = countOf(beforeMention, target.human.display_name)
      const afterCount = countOf(afterMention, target.human.display_name)
      check(
        '★ 群房 @ 候选里**含 Human 成员**（不再只认 Agent）',
        afterCount > beforeCount,
        `@前出现 ${beforeCount} 次 · @后 ${afterCount} 次 · ${snippet(afterMention, 160)}`,
      )
      check(
        '候选弹窗不是"没有匹配的参与者"（那说明候选源是空的）',
        !afterMention.includes('没有匹配的参与者'),
        snippet(afterMention, 160),
      )
      await page.keyboard.press('Escape')
    }
  }

  // ---- ★ T3（2026-10-20）：已撤销的 Agent 深链不再是一张"可操作"的页 ----
  //
  // 目标：深链 `/app/agents/<已撤销 id>` 只渲染**页头 ＋ 那句后果 ＋ 返回**，接入 / 凭证 /
  // 撤销 / 接上 / 模型**一件都不许渲染**（它们一件也做不成：世界那一侧早已不接受它）。
  // 造法全程走**公开协议**（`POST /v1/agents` ＋ `DELETE /v1/agents/<id>`）—— 不改库、不塞数据。
  // ⚠ 这一块放在**所有 Agent 主页断言之后**：它会在世界里多一支（已撤销的）Agent，
  //   排在前面的话后面那些"点列表第一个进主页"的断言就会点进那一支。
  {
    const created = await apiJson('/v1/agents', {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'Idempotency-Key': `probe-revoked-${Date.now()}`,
      },
      body: JSON.stringify({ display_name: '探针已撤销' }),
    })
    const revokedId = created.body && created.body.agent ? created.body.agent.id : null
    const revoked = revokedId ? await apiJson(`/v1/agents/${revokedId}`, { method: 'DELETE' }) : null
    const revokedOk =
      revoked !== null && (revoked.status === 200 || revoked.status === 204 || revoked.status === 202)
    if (!revokedId || !revokedOk) {
      check(
        '★ T3：已撤销 Agent 的深链只说后果 ＋ 返回',
        null,
        `⚠ 这一轮没验：造不出一支已撤销的 Agent（建=${created.status} · 撤=${revoked ? revoked.status : '—'}）`,
      )
    } else {
      await goto(`${args.base}/app/agents/${revokedId}`, '已撤销 Agent 的深链')
      await page.waitForTimeout(2500)
      const revokedText = await page.locator('body').innerText()
      // 那五样**管理面**一件都不许在（逐条点名：这几句就是"可操作的页"的形状）。
      const leakedSurface = [
        '签发凭证',
        '轮换凭证',
        '撤销 Agent',
        '接到这台机器上',
        '它用哪一条模型',
      ].filter((phrase) => revokedText.includes(phrase))
      check(
        '★ T3：已撤销 Agent 的深链只说后果 ＋ 返回（接入 / 凭证 / 撤销 / 接上 / 模型一件都不渲染）',
        revokedText.includes('已撤销 —— 不再接收事件，房间成员关系与凭证一并作废') &&
          revokedText.includes('回到我的 Agent') &&
          leakedSurface.length === 0,
        leakedSurface.length
          ? `不该出现的还在：${JSON.stringify(leakedSurface)} · ${snippet(revokedText, 200)}`
          : snippet(revokedText, 200),
      )
      // ★ **列表页不许被那一页顶掉**（2026-10-20 真机撞出来的形状）：列表模式下 `agent` 也会
      // 被算出来（`agentList[0]` 那个兜底）⇒ 少了 `homeMode` 那道门，`/app/agents` 会变成
      // 一页"已撤销"，列表与「添加」一起消失（**世界多一支已撤销的 Agent 就会踩到**）。
      await goto(`${args.base}/app/agents`, '已撤销之后回列表页')
      await page.waitForTimeout(2500)
      const listAfterRevoke = await page.locator('body').innerText()
      const rowsAfterRevoke = await page.getByRole('button', { name: /^打开 .* 的主页$/ }).count()
      check(
        '★ T3：世界里有一支已撤销的 Agent 之后，/app/agents 仍然是**列表**（有那句清单说明 ＋ 行 ＋ 添加）',
        listAfterRevoke.includes('这份清单拼两件事') &&
          listAfterRevoke.includes('添加') &&
          rowsAfterRevoke >= 1 &&
          listAfterRevoke.includes('已撤销 —— 不再接收事件，房间成员关系与凭证一并作废'),
        `行=${rowsAfterRevoke} · ${snippet(listAfterRevoke, 220)}`,
      )
    }
  }

  const readoutSelfInflicted = consoleErrors.length - errorsBeforeReadouts

  // ---- 页面级错误：有就报（不阻断结论，但要说出来）----
  const realErrors = consoleErrors.filter((line) => !line.includes('navigator.vibrate'))
  if (realErrors.length > 0) {
    console.log(
      `注意：页面控制台有 ${realErrors.length} 条错误（未计入断言；其中 ${
        selfInflictedErrors + readoutSelfInflicted
      } 条是上面那些**故意**造的失败请求）`,
    )
    for (const line of realErrors.slice(0, 3)) console.log(`      ${snippet(line, 160)}`)
  }
} finally {
  if (!args.keepOpen) await browser.close()
}

const notApplicable = results.filter((r) => r.ok === null)
const failed = results.filter((r) => r.ok === false)
const passed = results.length - failed.length - notApplicable.length
console.log(
  `\n=== core 通道：${passed}/${results.length - notApplicable.length} 通过` +
    `${notApplicable.length ? ` · ${notApplicable.length} 条不适用（这一轮没验）` : ''} ===`,
)
if (notApplicable.length > 0) {
  console.log('不适用（环境没就绪，不是代码问题 —— 别当成通过）：')
  for (const item of notApplicable) console.log(`  ~ ${item.name}`)
}
if (failed.length > 0) {
  console.log('未通过：')
  for (const item of failed) console.log(`  - ${item.name}`)
}
process.exit(failed.length === 0 ? 0 : 1)
