'use strict'
/**
 * `node home.js up` —— 家的**开机键**（移植自已退役的 bash 版 `scripts/home_up.sh`）。
 *
 * 一条命令按顺序把三件事点亮：**im 服务端 → Agent（agent_host）→ client 入口**。
 * 幂等：已经活着的那段会如实报"已经在跑"并跳过（所以重复敲、敲两遍都安全）。
 *
 * 退出码见 `scripts/HOME_MANUAL.md`（权威表；`codes.js` 只是给代码一个可读名字）：
 *   0 = 家亮着 ／ 1 = 用法错 ／ 2 = 环境不可用 ／ 3 = 需要你动手 ／ 4 = 只起来了一部分
 *
 * 凭据纪律：`DEEPSEEK_API_KEY` 与 `CELIRA_IM_BACKEND_AUTH` 只按**变量名**读，
 * 只注入子进程的环境——不落文件、不进 argv、不进日志。
 */

const fs = require('node:fs')
const path = require('node:path')

const { EXIT } = require('./codes')
const P = require('./paths')
const { confGet, confSet, portOf } = require('./conf')
const { readUserEnv } = require('./userenv')
const { imRequest } = require('./imapi')
const { proveWorldOwnership } = require('./world')
const remote = require('./remote')
const credentials = require('./credentials')
const proc = require('./proc')
const residents = require('./residents')
const worldWitness = require('./world_witness')
const statusServer = require('./status-server')
const configServer = require('./config-server')
const homeKey = require('./home-key')
const modelSources = require('./model-sources')
const { provision, BACKEND_ENV } = require('./provision')

const USAGE = [
  '用法: node home.js up [--token [<主人凭据>] | --status | --agent <世界里的 Agent id>] [--im-endpoint <世界地址>]',
  '  （无参数）= 开机（本机全量：世界 + 每个住户各一支 Agent + 页面 + 两个门）；',
  '  --token   不带值 = 补发 Human token；跟一个值 = 远端跑法的主人凭据（要配 --im-endpoint）；',
  '  --status  看状态；',
  '  --agent   = 给**这一支** Agent 安家（世界里已有它 ⇒ 认领；家里已有别的住户不挡路，',
  '             它会是新的一支住户）—— 只动这一支，别的住户照旧',
  '  --attach-only = **只接上，不要求模型**（2026-10-15）：没有模型来源时**接上但先不起它**',
  '             （缺省口径是"没模型就早失败" —— 那是部署者的口径，不是"接上"的口径）',
  '  --im-endpoint <世界地址> = **远端跑法**（2026-09-30）：本机不起 core，对着那个世界做布置，',
  '             再让本机 Agent 接上它（形如 http://主机:端口；配合 --token 或环境变量 ' +
    remote.HUMAN_ENV + '）',
  '  --agent-name <名字> = **自建那一支**的名字（只在"这个家还没有住户"那一次有用）。',
  '             不给 ⇒ 显示名是占位 `未命名`：**名字归它自己或它的 Owner**，系统不替它起名',
  '  --human-name <名字> = **这个世界里你自己的名字**（只在"这个家还没铸首位 Human"那一次有用）。',
  '             不给 ⇒ 占位 `未命名` 并在读数里说出来：**名字归你自己**，系统不替你起名；',
  '             进 app 首屏那张卡问的也是它（本地模式第一次）',
]

// ---------------------------------------------------------------------------
// 参数与命令行构造（纯函数：这几条形状是"参数顺序就是语义"的那类契约，必须可单测）
// ---------------------------------------------------------------------------

function parseUpArgs(argv) {
  const out = {
    wantToken: false,
    wantStatus: false,
    adoptAgent: null,
    // `--agent-name`（2026-10-12）：**自建那一支**的显示名。`null` = 没给 ⇒ 占位 `未命名`
    // （系统不替它起名：显示名是公共呈现，归它自己或它的 Owner）。
    agentName: null,
    // `--human-name`（2026-10-20）：**这个世界里你自己**的显示名。`null` = 没给 ⇒ 占位
    // `未命名`（系统不替你起名：机器用户名→公共世界那条路已撤掉）。
    humanName: null,
    // **远端跑法**（2026-09-30）：`imEndpoint` 是规范化过的世界地址（`null` = 没给 ⇒ 本机全量）；
    // `remoteToken` 是 `--token` **跟的那个值**（主人凭据；`null` = 没跟值）。
    imEndpoint: null,
    remoteToken: null,
    // ★ `--attach-only`（2026-10-15）：**只接上，不要求模型** —— 没模型就接上但先不起它。
    // 缺省 `false` ⇒ 既有口径（没模型就早失败）一字不变。
    attachOnly: false,
    error: null,
  }
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (arg === '--token') {
      // **带值与不带值是两件事**（2026-09-30）：
      //   · 不带值 = 老路"补发 Human token"（本机全量专用）；
      //   · 跟一个非旗标的值 = 远端跑法的主人凭据（要配 `--im-endpoint`）。
      // 缺值/像旗标的下一项一律按"没跟值"读 ⇒ 由组合纪律去说"远端模式里 --token 要跟值"。
      const value = argv[index + 1]
      if (typeof value === 'string' && value.trim() !== '' && !value.trim().startsWith('--')) {
        out.remoteToken = value.trim()
        index += 1
      } else {
        out.wantToken = true
      }
    } else if (arg === '--im-endpoint') {
      // **缺值一律是用法错**（与 `--agent` 同一条纪律）：绝不回落成"没给这个旗标"
      // —— 那会让一次远端跑法悄悄变成"本机全量"。
      const value = argv[index + 1] ?? ''
      if (!value.trim() || value.trim().startsWith('--')) {
        out.error = '[home_up] --im-endpoint 要一个世界地址（形如 http://主机:端口）'
        return out
      }
      const parsed = remote.parseImEndpoint(value.trim())
      if (!parsed.ok) {
        out.error = `[home_up] --im-endpoint 读不懂：${parsed.error}`
        return out
      }
      out.imEndpoint = parsed.base
      index += 1
    } else if (arg === '--status') out.wantStatus = true
    else if (arg === '--resume') {
      // ⚠ **这条路已退役**（2026-09-26）：跨进程「接着跑完同一段活动」不再存在 ——
      // 一段清醒被一段不清醒隔开就是两段。**不许静默忽略**（用户会以为它跑了），
      // 也不许回落成"没给这个参数"：当场说出真相与出路（照旧是用法错，退出码 1）。
      out.error =
        '[home_up] --resume 已退役（跨进程恢复退役于 2026-09-26：一段清醒被一段不清醒隔开就是两段）' +
        ' —— 未收尾的活动**不挡开机**：直接 node home.js up 就开新的一段；要看它停在哪： node home.js status'
      return out
    } else if (arg === '--attach-only') {
      // ★ **2026-10-15：接上 ≠ 起起来**（主人裁的边界；远程接上那条路用的就是它）。
      //
      // 没有它时，这一条命令的既有口径是"没模型就**早失败**"（`EXIT.NEEDS_YOU`，
      // 见下面 0c 那一段）—— 那是**部署者**的口径：他要的是"这台机器能跑起来"。
      // 而"**接上**"是另一件事（那台机器上有个位置），它不该因为还没选模型而整件事被拒：
      // 没模型 ⇒ **接上，但先不起它**（如实说；页面/终端随时可以再去配）。
      out.attachOnly = true
    } else if (arg === '--agent') {
      // **缺值一律是用法错**（绝不回落成"没给这个参数"——
      // 那会让一次"认领"悄悄变成"自建一个新身份"）。
      const value = argv[index + 1] ?? ''
      if (!value.trim() || value.trim().startsWith('--')) {
        out.error = '[home_up] --agent 要一个世界里的 Agent id（客户端「我的 Agent」里那个 id）'
        return out
      }
      out.adoptAgent = value.trim()
      index += 1
    } else if (arg === '--agent-name') {
      // **缺值一律是用法错**（与 `--agent` 同一条纪律）：绝不回落成"没给这个参数"
      // —— 那会让"人给的名字"悄悄变成占位，而占位与"人起的名"是两件不同的事。
      const value = argv[index + 1] ?? ''
      if (!value.trim() || value.trim().startsWith('--')) {
        out.error =
          '[home_up] --agent-name 要一个显示名（名字归那支 Agent 自己或它的 Owner；' +
          '不给这个旗标就是占位「未命名」）'
        return out
      }
      out.agentName = value.trim()
      index += 1
    } else if (arg === '--human-name') {
      // 与 `--agent-name` 同一条纪律：**缺值一律是用法错**，绝不回落成"没给"——
      // 那会让"人给的名字"悄悄变成占位，而占位与"人起的名"是两件不同的事。
      const value = argv[index + 1] ?? ''
      if (!value.trim() || value.trim().startsWith('--')) {
        out.error =
          '[home_up] --human-name 要一个显示名（名字归你自己；不给这个旗标就是占位「未命名」）'
        return out
      }
      out.humanName = value.trim()
      index += 1
    } else {
      out.error = `[home_up] 不认识的参数：${arg}`
      return out
    }
  }
  // **组合纪律**（与逐项纪律同一条：说清哪两个放一起读不通，绝不猜用户想干什么）。
  if (out.remoteToken !== null && out.imEndpoint === null) {
    out.error =
      '[home_up] --token 跟了值 ⇒ 那是**远端跑法**的主人凭据，要配 --im-endpoint <世界地址> 一起用' +
      '（不带值的 --token 才是"补发 Human token"）'
    return out
  }
  if (out.wantToken && out.imEndpoint !== null) {
    out.error =
      '[home_up] 远端跑法里 --token 要跟一个值（那个世界的主人凭据，clr_hs_ 开头）——' +
      '补发 token 是**世界那台机器**上的事（在那边敲 node home.js up --token）'
    return out
  }
  if (out.wantStatus && out.imEndpoint !== null) {
    out.error = '[home_up] --status 与 --im-endpoint 是两件事，不能一起用（--status 看的是本机的家）'
    return out
  }
  return out
}

/**
 * 起 Agent 时的参数（**顺序是语义**，与 bash 版逐字节对齐）：
 * `--root … --agent … --materials … --max-turns 300 [--terminal full] [--mcp …]* --im-endpoint …
 *  [--model-sources …] [--identity-agent … --identity-name …]`
 * 声明 0 台外部 server 时**一个参数都不多**（回归判据就是这条"零个"）。
 *
 * ⚠ **`--im-endpoint` 的取值是宿主显式声明的**（远端跑法刀）：缺省仍是本机
 * `http://${listen}`（现状一字不动）；远端跑法把**远端世界地址**交给 `imEndpoint` ⇒ 位置不变、
 * 值换成那个地址（Agent 的门铃与 im 动词都指向它）。
 *
 * ⚠ **两处平台缝**（都按"本构建里有没有能用的臂"给）：
 * - 终端档位（第五刀）：`--terminal full` 给**有终端臂的平台**——Windows 与 Linux
 *   （`agent/src/terminal_app.rs`：臂今天两支；别的平台上这一档是**有意的 fail-closed**，传它
 *   等于让 Agent 每次开机都被拒绝启动）。没有臂的平台上**不给这个旗标**：这台机器上 Agent
 *   没有"终端"这只手 —— 如实、不静默换一个近似行为（读数由 `terminalNotice` 说出来）。
 *   **2026-10-01 第五刀**：档位由 `terminal` 入参决定（一支一个身体，手是**支级事实**）。
 *   ⚠ **2026-10-14 主人裁**：**缺省改成"不给手"** —— 这个入参的默认值因此是 `'none'`
 *   （`agent.conf` 里只有显式 `terminal=full` 才给手；老住户由 `migrateTerminalDefault`
 *   把现行的 full 显式写进它们自己的 `agent.conf`，所以"缺省变了、老住户不掉东西"）。
 *   ⇒ **入参缺省不许再写成 `'full'`**：那等于把"装完即整机全权"重新变成没人声明过的默认。
 * - MCP（本刀）：`--mcp` 给**有 stdio 通道臂的平台**——同样是 Windows 与 Linux
 *   （`agent/src/mcp`：臂今天两支；别的平台上入口会**拒绝开启**这一档）。没有臂的平台上
 *   **不传**：`home.conf` 里的声明照旧在场（那是配置），但这台机器上装不进来 ——
 *   一行 `mcp_*` 不许把整个家弄停摆（读数由 `mcpNotice` 说出来）。
 *   **2026-10-01 第五刀**：`declarations` 由调用方按**这一支**解析好（`residents.residentHands`）。
 * `isWindows` / `isLinux` 可注入 ⇒ 各分支都能被机械判错（本机是 Linux，Windows 分支靠显式传参守住）。
 */
function buildHostArgs({
  envDir,
  agentId,
  materialsDir,
  listen,
  imEndpoint = null,
  declarations = [],
  identity = null,
  modelSourcesFile = null,
  // **web 源清单在哪**（2026-10-20）：与模型源同款——给清单，用哪一条在环境根的绑定里。
  webSourcesFile = null,
  // ⚠ **缺省是"不给手"**（2026-10-14 主人裁）：只有 `terminal === 'full'` 才给这只手。
  //   入参的默认值也是 `'none'` —— "没说"与"说了不要"在这一档上是同一个结果，而"要"必须被说出来。
  terminal = 'none',
  // **支级资源上界**（2026-10-05）：`agent.conf` 的 `max_turns`（缺省 = 内置 300）。
  // 值**原样**交给宿主，校验归宿主自己的旗标解析（非整数、0 都被拒）。
  maxTurns = null,
  isWindows = proc.IS_WINDOWS,
  isLinux = proc.IS_LINUX,
}) {
  const args = [
    '--root', envDir,
    '--agent', agentId,
    '--materials', materialsDir,
    '--max-turns', maxTurns ?? '300',
  ]
  if (terminal === 'full' && (isWindows || isLinux)) args.push('--terminal', 'full')
  if (isWindows || isLinux) {
    for (const declaration of declarations) args.push('--mcp', declaration)
  }
  args.push('--im-endpoint', imEndpoint ?? `http://${listen}`)
  // **模型源清单在哪**（刀2b）：与 `--im-endpoint` 同款"宿主显式声明"——Agent 本机不去猜路径。
  // 给 Agent 的是**清单**（机器级那份）；"这个 Agent 用哪一条"在环境根的绑定文件里（自己去读）。
  if (modelSourcesFile) args.push('--model-sources', modelSourcesFile)
  if (webSourcesFile) args.push('--web-sources', webSourcesFile)
  // **是谁**（TASK_70）：两个值**成对**给（缺一半会被入口拒绝 ⇒ 要么都给、要么都不给）。
  // 问不到（没声明 identity）⇒ **一个参数都不多** ⇒ 开场照旧跳过那一行。
  if (identity) {
    args.push('--identity-agent', identity.id)
    args.push('--identity-name', identity.displayName)
  }
  return args
}

/**
 * 问服务端「这份凭据代表谁」：`GET /v1/backend/me` ⇒ `{ id, displayName }` 或 `null`。
 *
 * ⚠️ 两个值**同源**（同一次回执）——**不是**本机 `home.conf` 里记的那个身份（那是另一套命名空间）。
 * 读不回来就返回 `null`：起 Agent 时不带那两个旗标 ⇒ 开场如实跳过那一行（不编、也不拿本机身份顶替）。
 *
 * `base` 是**世界地址**（本机全量 = `http://${listen}`；远端跑法 = 那个世界）——问的永远是
 * "Agent 要连的那个世界"。
 */
async function readIdentity(base, backend) {
  const me = await imRequest(base, 'GET', '/v1/backend/me', { bearer: backend })
  if (me.status !== 200) return null
  const id = me.body?.agent?.id
  const displayName = me.body?.agent?.display_name
  if (!id || !displayName) return null
  return { id, displayName }
}

/**
 * **平台默认那一行的读数**（2026-10-14 主人裁：默认不给手）。
 *
 * 它回答"没有谁声明过的时候，这台机器给不给这只手"——答案是**不给**：要手就在那一支自己的
 * `agent.conf` 里写 `terminal=full`。这行因此与平台无关（Windows / Linux / 别的平台上默认
 * 都是不给）；差别只在"声明了 full 却装不进来"那一档——那是 `unavailable`，另说一句。
 *
 * 为什么要有这条读数：旗标少传一个字节，行为上"Agent 起得来"，但用户看到的开机读数里
 * 一个字的差别都没有 —— 那是"静默"；而"这台机器上 Agent 有什么手"属于用户有权知道的事。
 */
function terminalNotice() {
  return '[terminal] 平台默认不给「终端」这只手（2026-10-14 起）；要手就在它自己的 agent.conf 里写 terminal=full'
}

/**
 * **MCP 那一行的读数**（本刀）：按平台**如实说**这些声明会不会被带上。
 *
 * - Windows / Linux：stdio 通道在场（`--mcp` 照给）——不宣告（返回 `null`，两边行为一字不变）；
 * - 别的平台：宿主这一档是 **fail-closed**（`agent/src/mcp` 入口会拒绝开启）⇒ 家**不传** `--mcp`
 *   （一行 `mcp_*` 不许把整个家弄停摆），并把这行如实读数打出来：声明还在配置里，
 *   但这台机器上装不进来——不静默降级。
 *
 * 为什么要有这条读数：与终端那行同一个理由——旗标少传一个字节，行为上"Agent 起得来"，
 * 但用户看到的开机读数里一个字的差别都没有，那是"静默"。
 */
function mcpNotice(isWindows = proc.IS_WINDOWS, isLinux = proc.IS_LINUX, count = 0, names = '') {
  if (isWindows || isLinux) return null
  const declared = count > 0 ? `声明了 ${count} 台（${names}）` : '配置里有声明'
  return `[mcp] 这台机器上没有 stdio MCP 通道（有实现的平台：Windows、Linux）：${declared}这次不带——不静默降级`
}

/**
 * **一支的手**那一组读数（2026-10-01 第五刀）：外部 server ＋ 终端档，**按支说、说清来源**。
 *
 * 三条纪律（与 status 那一行同一个口径）：
 *   · 多支的家**点名是哪一支**（`who` 由调用方给，一行读数不许让主人猜）；
 *   · **说清来源**：它自己的 `agent.conf` ／ 跟家走（`home.conf`）／ 这一支明确不要；
 *   · 平台没臂时**如实说"这次不带"**，不静默降级（沿用 `mcpNotice` / `terminalNotice` 的原话）。
 *
 * 为什么要有它：手从家级变成支级之后，"这支为什么没有终端/为什么多了三台 server"如果只能靠
 * 对比两个文件才看得出来，那就是**静默**——读数必须自己说清。
 */
function handNotices({ who = '', hands, terminal, platform = null }) {
  // ⚠ **平台只有一处来源**（2026-10-05 修）：`resolveTerminal` 由调用方按**传进来的**平台算，
  //   而这两条 notice 从前各自去问**真实**平台 ⇒ 同一个事实两个来源。后果实测：用例给
  //   "假装的 Linux"算出了终端档，notice 却按真机器（Windows）不宣告 ⇒ 读数 0 行，
  //   用例红得看不懂。这里统一：不给就按真实平台（生产行为一字不变）。
  const plat = platform ?? { isWindows: proc.IS_WINDOWS, isLinux: proc.IS_LINUX }
  const out = []
  const names = hands.mcp.map((declaration) => declaration.split('=', 1)[0]).join(',')
  if (hands.mcpSource === 'none') {
    out.push(`[mcp] ${who}这一支不要外部 server（agent.conf 里写了 mcp=none）—— 家级那几台不带给它`)
  } else if (hands.mcp.length > 0) {
    const from = hands.mcpSource === 'resident' ? '它自己的 agent.conf' : '跟家走（home.conf）'
    out.push(`[mcp] ${who}装了 ${hands.mcp.length} 台外部 server：${names}（来自${from}）`)
    const notice = mcpNotice(plat.isWindows, plat.isLinux, hands.mcp.length, names)
    if (notice) out.push(notice)
  }
  // ⚠ 一台都没有、也没写 `mcp=none` ⇒ **一个字都不打**（那是默认状态，不是新闻；
  //   今天的行为就是这样，本刀不动它）。

  if (terminal.unavailable) {
    out.push(
      `[terminal] ${who}声明了终端档 full，但这台机器上没有终端臂（有实现的平台：Windows、Linux）—— 这次不带；不静默降级`,
    )
  } else if (terminal.mode === 'none') {
    // 三条来路分开说：它自己写的 / 家级写的 / **没人写过**（平台默认，2026-10-14 起是"不给"）。
    const why =
      terminal.source === 'resident'
        ? 'agent.conf 里写了 terminal=none'
        : terminal.source === 'home'
          ? '家级写了 terminal=none'
          : null
    if (why) {
      out.push(`[terminal] ${who}这一支没有「终端」这只手（${why}）`)
    } else {
      const notice = terminalNotice()
      out.push(who ? notice.replace('[terminal] ', `[terminal] ${who}`) : notice)
    }
  } else if (terminal.source === 'resident' || terminal.source === 'home') {
    // 明确声明过 full（支级或家级）⇒ 把"这一档是声明来的"说出来。
    const from = terminal.source === 'resident' ? '它自己的 agent.conf' : '家级 home.conf'
    out.push(`[terminal] ${who}终端档 full（来自${from}）`)
  }
  return out
}

/** 缺省变更那一次迁移的名字（写在 `home.conf` 里；见 [`migrateTerminalDefault`]）。 */
const TERMINAL_DEFAULT_KEY = 'terminal_default'
const TERMINAL_DEFAULT_MIGRATED = 'migrated-2026-10-14'

/**
 * **一次性迁移：缺省变了，但老住户不掉东西**（2026-10-14 主人裁）。
 *
 * 裁的是两件事：① 缺省从"有终端臂就给 full"改成**不给手**（`residents.resolveTerminal`）；
 * ② 迁移取"**把现行的 full 显式写进各支 `agent.conf`**"——老住户保持现状，来源从"平台默认"
 * 变成"它自己的声明"（读数里看得见），要降档就改那一行。
 *
 * ⚠ **判据必须是家级标记，不是"这一支跑过没有"**：拿经历当判据会把**新建的支**也迁到 full
 * （它第二次开机就有经历了）——那是把一次性的历史补偿变成永久的新缺省。
 *
 * ⚠ **只加那一行**：文件里别的字节一个都不动；已经有 `terminal=`（`full` 或 `none`）的支跳过——
 * 那是别人已经表过的态。老家那一支（还没迁的住户）的档写在 `home.conf` 里，**不动它**，
 * 只如实说一句该怎么办。
 *
 * 返回打出来的那几行（调用方负责打印；用例直接看返回值）。
 */
function migrateTerminalDefault({ homeDir, confFile, residents: list, log = null }) {
  if ((confGet(confFile, TERMINAL_DEFAULT_KEY) || '').trim() === TERMINAL_DEFAULT_MIGRATED) {
    return []
  }
  const lines = []
  for (const resident of list) {
    if (resident.legacy) {
      lines.push(
        '[terminal] 迁移：老家那一支的档写在 home.conf 里（它没有自己的 agent.conf）——要手就在那儿写 terminal=full',
      )
      continue
    }
    const file = P.agentConfPath(homeDir, resident.agentId)
    const declared = (confGet(file, residents.HAND_KEYS.terminal) || '').trim().toLowerCase()
    if (declared === 'full' || declared === 'none') continue
    let current = ''
    try {
      current = fs.readFileSync(file, 'utf8')
    } catch {
      current = ''
    }
    const separator = current === '' || current.endsWith('\n') ? '' : '\n'
    fs.writeFileSync(file, `${current}${separator}${residents.HAND_KEYS.terminal}=full\n`, 'utf8')
    lines.push(
      `[terminal] 迁移：住户 ${resident.agentId} 在旧缺省下是「有手」⇒ 已把 ${residents.HAND_KEYS.terminal}=full ` +
        '显式写进它自己的 agent.conf（要降档就改那一行；新缺省是不给手）',
    )
  }
  confSet(confFile, TERMINAL_DEFAULT_KEY, TERMINAL_DEFAULT_MIGRATED)
  lines.push(
    `[terminal] 缺省变更（2026-10-14）：这台机器上此后新建的支**不给手**，要手写 ${residents.HAND_KEYS.terminal}=full；` +
      `已记在 home.conf（${TERMINAL_DEFAULT_KEY}=${TERMINAL_DEFAULT_MIGRATED}）`,
  )
  if (log) for (const line of lines) log(line)
  return lines
}

/**
 * **「这个包里没有 core」的读数**（runtime 子包）。
 *
 * 包里没有 core 是**按设计的形态**（runtime 子包只有 Agent 那一半），不是"坏了"——所以它
 * 不该被说成"缺可执行件"，更不该让用户去 `cargo build` 一个包里本来就没有的东西。
 * 它要接一个**已经存在的世界**；这条路**2026-09-30 起真的有**了（远端跑法），所以这里
 * 如实说清两件事：这台机器只该跑 Agent 那一半；接世界的那条命令长什么样。
 */
function explainRuntimeOnly() {
  console.log('[home_up] 这个包里**没有 core**（runtime 子包：只有 Agent 那一半）')
  console.log('[home_up] 它要接一个**已经存在的世界**：世界在哪台机器上，就在那里跑一体包')
  console.log('[home_up] 接远端世界： node home.js up --im-endpoint <世界地址> --token <那个世界的主人凭据>')
  console.log('[home_up]   （世界地址形如 http://主机:端口；主人凭据是那台机器第一次开机打印的 clr_hs_ 那一串）')
  console.log('[home_up] 想在这台机器上全量起一个家：用一体包（celira-<版本>-<平台>.zip）')
  return EXIT.ENV_UNAVAILABLE
}

/** 发布形态：自带零依赖静态服务 + `/v1` 代理 + `/home-status`、`/home-config` 两段代转。 */
function releaseClientArgs({ serveScript, clientDist, clientPort, listen, statusPort, configPort, homeDir }) {
  return [
    serveScript,
    '--dist',
    clientDist,
    '--port',
    clientPort,
    '--im',
    `http://${listen}`,
    // 状态口开在哪个端口是**家的配置**（`home.conf` 的 `status_port`）⇒ 由这里读出来传进去，
    // 页面因此永远不用知道端口（TASK_61 把那条对偶收成了"运行期事实"）。
    '--status-port',
    statusPort,
    // 模型门同款（TASK_77）：页面对 `config_port` 同样一无所知。
    '--config-port',
    configPort,
    // **家事实**（2026-09-25）：进程表读得到命令行、读不到环境变量 —— 这一句就是
    // "这支常驻件属于哪个家"唯一的可读凭据（同包多家的认领判据靠它）。
    '--home',
    homeDir,
  ]
}

/**
 * **页面那一支的命令行**（2026-10-01 第六刀）：两种形态**同一条路** —— 同一个静态托管
 * （`scripts/serve_client.mjs`）、同一张代转路由表、同一个 `--home` 家事实；只有 `--dist`
 * 随形态走（源码 `client/dist` / 发布 `client-dist`，由 `layout` 决定）。
 *
 * 为什么要收成一处：从前 `up` 在两种形态下走**两套**代转实现（vite 的 proxy 与
 * `serve_client.mjs` 的路由表）——同一件事两份实现，历史上就漏改过一边（TASK_77 的
 * `/home-binding` 只在一边补上）。收成一处之后，"页面能代转到什么"只有一个答案。
 */
function clientServeArgs({ layout, clientPort, listen, statusPort, configPort, homeDir }) {
  return releaseClientArgs({
    serveScript: path.join(layout.repo, 'scripts', 'serve_client.mjs'),
    clientDist: layout.clientDist,
    clientPort,
    listen,
    statusPort,
    configPort,
    homeDir,
  })
}


// ---------------------------------------------------------------------------
// **源码形态的一键前端**（2026-09-30）：装依赖 + 构建，进度与失败如实说
// ---------------------------------------------------------------------------

/** `npm install` 的耐心：要联网、本机实测几分钟量级 —— 给足，超时才报（并停掉它，不假装完成）。 */
const CLIENT_INSTALL_TIMEOUT_MS = 20 * 60 * 1000
/** `npm run build` 的耐心（tsc + vite；慢机器上到分钟量级）。 */
const CLIENT_BUILD_TIMEOUT_MS = 10 * 60 * 1000
/** 进度行的间隔：npm 静默时也要看得出它在动（不能让人以为卡死）。 */
const CLIENT_PROGRESS_INTERVAL_MS = 10 * 1000

/**
 * **前端由谁准备**（本刀的平台缝）：非 Windows 上 `up` 自己装依赖 + 构建；
 * Windows 上保持原样（依赖缺 ⇒ 打印三条手工命令；产物缺 ⇒ 老构建路径）—— 红线
 * 「不改变 Windows 行为」。纯函数 ⇒ 两条分支都能被机械判错。
 */
function clientSetupOwner(isWindows = proc.IS_WINDOWS) {
  return isWindows ? 'user' : 'up'
}

/** 源码形态「缺什么补什么」：依赖缺 ⇒ 装；产物缺 ⇒ 构；两样都在 ⇒ 一步都不跑。 */
function clientSetupSteps({ hasDeps, hasDist }) {
  const steps = []
  if (!hasDeps) steps.push('install')
  if (!hasDist) steps.push('build')
  return steps
}

/** 一步的读数名字（进度行、失败行、日志标记共用一处）。 */
function stepLabel(step) {
  return step === 'install' ? 'npm install' : step === 'build' ? 'npm run build' : String(step)
}

/** 用时的人话（进度行与收尾读数共用）。 */
function formatDuration(ms) {
  const seconds = Math.max(0, Math.round(ms / 1000))
  if (seconds < 60) return `${seconds} 秒`
  const minutes = Math.floor(seconds / 60)
  const rest = seconds % 60
  return rest === 0 ? `${minutes} 分钟` : `${minutes} 分 ${rest} 秒`
}

function messageOf(error) {
  return error && error.message ? error.message : String(error)
}

/**
 * 跑前端的一步（`npm install` / `npm run build`）—— **边跑边报进度**（本刀）。
 *
 * 为什么不用 spawnSync（老构建路径那种）：`npm install` 要联网、几分钟量级；同步静默调用
 * 看起来与卡死无异（本刀明写：要有进度感、失败要给可复核的原因）。这里用异步 spawn：
 * 输出原样落 `logs/client.out`（起跑线也写进去 —— 出事后一眼看得出哪一段是这一步的），
 * 每 10 秒打一行已用时间；超时（给得很宽）就停掉它，并如实说"超时"，不假装完成。
 * **失败不改 up 的退出码** —— "前端没起来不影响 Agent 在线"是既有判据，调用方按 `ok` 决定读数。
 *
 * 可注入（用例把"退出 0 / 退出非 0 / 超时 / 起不来"四支都变成可判据）：
 * `command` / `spawnFn` / `sleepFn` / `nowMs` / `log`。
 * 返回 `{ ok, pid, code, signal, timedOut, reason, elapsedMs }`（`reason` 失败时非空，给人看）。
 */
async function runClientStep({
  step,
  args,
  command = 'npm',
  cwd,
  logFile,
  timeoutMs,
  spawnFn = require('node:child_process').spawn,
  nowMs = Date.now,
  sleepFn = proc.sleep,
  log = console.log,
}) {
  // 起跑线写进日志（可复核：这一步从哪儿开始、上一步的尾巴到哪儿为止）。
  try {
    fs.appendFileSync(
      logFile,
      `\n=== [client] ${command} ${args.join(' ')} 开始（${new Date(nowMs()).toISOString()}）===\n`,
    )
  } catch {
    // 日志写不进去不拦这一步（下面 spawn 失败会有读数）。
  }
  const fd = fs.openSync(logFile, 'a')
  let child
  try {
    child = spawnFn(command, args, {
      cwd,
      // shell 只在 Windows 需要：`npm` 是个 .cmd 垫片，Node 不许直接 exec .cmd/.bat。
      // 命令行是常量、没有外部输入 ⇒ 没有注入面（与老构建路径同一条）。
      shell: proc.IS_WINDOWS,
      stdio: ['ignore', fd, fd],
      windowsHide: true,
    })
  } catch (error) {
    fs.closeSync(fd)
    return {
      ok: false, pid: null, code: null, signal: null, timedOut: false,
      reason: `起不来（${messageOf(error)}）`, elapsedMs: 0,
    }
  }
  fs.closeSync(fd)

  const startedAt = nowMs()
  let outcome = null
  const finish = (value) => {
    if (outcome === null) outcome = value
  }
  child.once('error', (error) =>
    finish({
      ok: false, pid: child.pid ?? null, code: null, signal: null, timedOut: false,
      reason: `起不来（${messageOf(error)}）`,
    }),
  )
  child.once('close', (code, signal) =>
    finish({
      ok: code === 0,
      pid: child.pid ?? null,
      code,
      signal,
      timedOut: false,
      reason: code === 0 ? null : signal ? `被信号 ${signal} 打断` : `退出码 ${code}`,
    }),
  )

  let lastReportAt = startedAt
  while (outcome === null) {
    await sleepFn(1000)
    if (outcome !== null) break
    const elapsed = nowMs() - startedAt
    if (elapsed >= timeoutMs) {
      // 先把结论记成"超时"（close 事件随后也会来 —— 那一下不许把结论改写成"被信号打断"）。
      finish({
        ok: false, pid: child.pid ?? null, code: null, signal: 'SIGKILL', timedOut: true,
        reason: `超时（超过 ${formatDuration(timeoutMs)} 还没结束）`,
      })
      try {
        child.kill('SIGKILL')
      } catch {
        // 已经死了就死了（结论照实是超时）
      }
      // 等它真的退场（SIGKILL 也要几毫秒）——不等的话紧接着的"还活着吗"会随机
      for (let wait = 0; wait < 30 && child.exitCode === null && child.signalCode === null; wait += 1) {
        await sleepFn(100)
      }
      break
    }
    if (nowMs() - lastReportAt >= CLIENT_PROGRESS_INTERVAL_MS) {
      lastReportAt = nowMs()
      log(`[client] ${stepLabel(step)} 进行中（已 ${formatDuration(elapsed)}）—— 日志 ${logFile}`)
    }
  }
  return { ...outcome, elapsedMs: nowMs() - startedAt }
}

/**
 * 一步失败时的读数（**说清哪一步、什么错、去哪儿看**；纯函数，可判错）：
 * 失败**不影响 Agent 在线**，也**不改 up 的退出码** —— 读数里明说这两件事。
 */
function clientStepFailureLines(step, done, logFile, clientPort) {
  return [
    `[client] ${stepLabel(step)} 没成功 —— ${done.reason ?? '没给原因'}（不影响 Agent 在线）`,
    `[client] 原因在 ${logFile} 的尾巴：`,
    ...tailLines(logFile, 6).map((line) => `       ${line}`),
    '[client] 修好之后重开一次： node home.js up（装好了的会直接跳过）',
    `[client] 手动那条路： cd client && npm install && npm run build && node ../scripts/serve_client.mjs --dist client/dist --port ${clientPort}`,
  ]
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/** 进程侧**模型来源**那两条环境变量（与 Agent 进程环境里的摘除判据同一处权威）。 */
const MODEL_ENV_KEYS = ['DEEPSEEK_API_KEY', 'CELIRA_MODEL_API_KEY']

/**
 * Agent 那一份**模型环境**（TASK_77）。
 *
 * 文件在场 ⇒ **一个模型 env 都不给 Agent**：不只是"不添加那一条"，而是把 `...process.env` 里
 * 手边本来就有的那两条也**摘掉**——否则进程侧那一条会与文件打架，Rust 侧按纪律**拒绝启动**
 * （本机实测：shell 里 `DEEPSEEK_API_KEY` 就在手边，不摘必撞）。这就是"正常家用永远不撞冲突
 * 拒绝"的落地。
 *
 * 文件不在场 ⇒ **原样交回**（现状分毫不动，一个字都不改）。
 */
function modelEnvironment(base, fromFile) {
  if (!fromFile) return base
  const env = { ...base }
  for (const key of MODEL_ENV_KEYS) delete env[key]
  return env
}

/**
 * **代理绕过是"加两条"，不是"换一份"**（2026-10-09 真机实测换来的）。
 *
 * 宿主必须能**不经代理**够到本机的世界与两个口 ⇒ `127.0.0.1` / `localhost` 一定要在
 * `NO_PROXY` 里。但从前这里是**直接覆盖**成一个常量 ⇒ 操作者自己加的绕过（例如给模型那家
 * 加一条）被**静默吞掉**：他改了环境变量也不生效，而症状是"她的模型请求读体中断 / 超时"
 * （实测：这台机器上 `HTTP(S)_PROXY=127.0.0.1:7897` ⇒ 宿主的模型请求经代理 ⇒ 大回信被截断；
 * CLI 探针走 node 的 `fetch`（默认不认代理）⇒ 直连成功 ⇒ 两条路读数不同、看着像宿主坏了）。
 *
 * 所以：**保留原有取值，把本机那两条补进去**（去重、去空）。
 */
function appendNoProxy(existing) {
  const parts = String(existing ?? '')
    .split(',')
    .map((each) => each.trim())
    .filter(Boolean)
  for (const host of ['127.0.0.1', 'localhost']) {
    if (!parts.includes(host)) parts.push(host)
  }
  return parts.join(',')
}

/** 给宿主的进程环境（模型那两条按 [`modelEnvironment`] 的规矩；代理绕过按 [`appendNoProxy`]）。 */
function hostEnvironment(base, { backend, apiKey, modelFromFile }) {
  const merged = {
    ...base,
    [BACKEND_ENV]: backend,
    DEEPSEEK_API_KEY: apiKey,
  }
  const noProxy = appendNoProxy(base.NO_PROXY ?? base.no_proxy)
  merged.NO_PROXY = noProxy
  merged.no_proxy = noProxy
  return modelEnvironment(merged, modelFromFile)
}

function tailLines(file, count) {
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch {
    return []
  }
  const lines = text.split('\n')
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines.slice(-count).map((line) => (line.endsWith('\r') ? line.slice(0, -1) : line))
}

/** 从第一行 `[state]` 到文件尾（自己说了原因，原样交出来）。 */
function fromStateLine(file) {
  const lines = tailLines(file, Number.MAX_SAFE_INTEGER)
  const at = lines.findIndex((line) => line.startsWith('[state]'))
  return at >= 0 ? lines.slice(at) : lines.slice(-5)
}

function indent(lines, prefix = '    ') {
  return lines.map((line) => `${prefix}${line}`)
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

/**
 * **端口上已经有一支 im 在听时，它是不是本家的？**（纯函数 ⇒ 可判错；证明本身在 `world.js`）
 *
 * 为什么要这一问（2026-09-25 真机实测）：im 的家事实只能是**端口**（`celira-im.exe` 不带参数，
 * 命令行里没有家的痕迹），而端口是**机器级**命名空间 ⇒ 两个家用同一组默认端口时判据分不出
 * 彼此：本家会把**别家的世界**认成自己的、把它的 pid 写进本家账本，`down` 于是可能停掉别家的
 * 世界。所以"认领"必须**证明得了才做**。
 *
 * 四种动作，与 `proc.ledgerPid` 的四态同一条纪律（"判不了"不许当成"不是"）：
 *   · `adopt`           认领（账本里那支，或证明 `ours` 的那支）；
 *   · `stop`            **铁证不是本家**（`foreign`）⇒ 不动它，也不认领，当场停手；
 *   · `refuse-stop`     判不了 ＋ 这个家**还没布置过**（没有 `agent_id`）⇒ 那支不可能是本家的
 *                       世界（本家的世界还没建），停手并说清怎么改端口；
 *   · `refuse-continue` 判不了 ＋ 已布置过 ⇒ **不认领、不写账本**，但往下走 ——
 *                       这一档保住手册 §8 那条恢复路径（本家凭据格被别家占了时，
 *                       本家的世界同样"判不了"，而布置那一步正是换回凭据的地方）。
 *
 * ⚠ `proof === null` 表示"这次没问"（账本已经指向端口上那支，不需要问）。
 */
function imListenerVerdict({ ledgerPid, listener, proof, hasAgent, port }) {
  // 端口上什么都没有，或端口上那支就是账本里那支 ⇒ 本家账本自己就是凭据，不需要证明。
  if (listener === null || listener === ledgerPid) {
    return ledgerPid === null
      ? { action: 'none', pid: null, lines: [] }
      : { action: 'adopt', pid: ledgerPid, lines: [`[im] 已经在跑（pid ${ledgerPid}）`] }
  }
  if (proof === 'foreign') {
    return {
      action: 'stop',
      pid: null,
      lines: [
        `[im] 端口 ${port} 上的世界**不是本家的**（它认下本家的后端凭据，却说是别人的 Agent）——不动它、不认领`,
        '[im] 两个家共用同一组端口时就会这样。给这个家换一个口：编辑 home.conf 的 listen_addr，再开机',
      ],
    }
  }
  if (proof === 'ours') {
    if (ledgerPid !== null) {
      // 账本里那支也在、端口上却是另一支：如实两个都说了，仍按账本算（账本是本家自己的事实）。
      return {
        action: 'adopt',
        pid: ledgerPid,
        lines: [
          `[im] 已经在跑（pid ${ledgerPid}；端口 ${port} 上另有一支本家的——账本仍按 pid 文件那一支算）`,
        ],
      }
    }
    return {
      action: 'adopt',
      pid: listener,
      lines: [`[im] 已经在跑（pid ${listener}；pid 文件里记的那支已经退场——账本已对上真实）`],
    }
  }
  // 判不了（没凭据 / 它不认这支凭据 / 连不上）。
  if (ledgerPid !== null) {
    return {
      action: 'adopt',
      pid: ledgerPid,
      lines: [
        `[im] 已经在跑（pid ${ledgerPid}；端口 ${port} 上那支证明不了是本家的——按账本算，不写新账本）`,
      ],
    }
  }
  if (!hasAgent) {
    return {
      action: 'refuse-stop',
      pid: null,
      lines: [
        `[im] 端口 ${port} 上已经有一支在听，可**证明不了是本家的**（本家的后端凭据它不认）`,
        '[im] 这个家还没布置过（home.conf 里没有 agent_id）⇒ 那支不可能是本家的世界 —— 停手',
        '[im] 要让这个家用别的口：编辑 home.conf 的 listen_addr（例如 127.0.0.1:8090），再开机',
      ],
    }
  }
  return {
    action: 'refuse-continue',
    pid: null,
    lines: [
      `[im] 端口 ${port} 上那支证明不了是本家的（本家的凭据它不认）——**不认领、不写账本**`,
      '[im] 若它其实是别家的世界：下一步（布置）会响亮失败 —— 那一步要的是本家的库',
      '[im] 若它确实是本家的：多半是本家的凭据格被别家占了 —— 布置那一步会把凭据换回本家',
    ],
  }
}

/**
 * 家级目录（缺什么补什么）：家根、`im/media`、`agents/`、`materials/`、`materials/skills/`、`logs/`。
 *
 * **技能区空着也要建出来**（2026-10-20）：`skills/` 不在场与"在场但空"在主体那边是两件事
 * （`agent/src/skills_desktop.rs`：`None` 整块不出场，`Some(0)` 说「技能区为空」）。不建它，
 * "这具身体没有技能区"与"技能区空着"在主体眼里合成一片空白——真机上主体据此答成
 * "没有可安装的技能"。幂等：重复调用不报错、不动已有内容。
 */
function ensureHomeDirs(homeDir) {
  for (const dir of [
    homeDir,
    path.join(homeDir, 'im', 'media'),
    P.agentsRoot(homeDir),
    P.materialsRoot(homeDir),
    P.skillsRoot(homeDir),
    path.join(homeDir, 'logs'),
  ]) {
    fs.mkdirSync(dir, { recursive: true })
  }
}

async function up(argv) {
  const options = parseUpArgs(argv)
  if (options.error) {
    console.log(options.error)
    for (const line of USAGE) console.log(line)
    return EXIT.USAGE
  }
  if (options.wantStatus) return require('./status').status([])

  const homeDir = P.homeDir()
  const layout = P.detectLayout()
  const confFile = path.join(homeDir, 'home.conf')

  // —— 家目录（缺什么补什么）——
  // ⚠ 这里**不再建 `<家>/env`**：一支一个环境根（`<家>/agents/<支>/`），住户化之后由布置
  // 按需建。老家的 `<家>/env` 是迁移的输入（`residents.migrateHomeShape`），不在这里动它。
  const materialsDir = P.materialsRoot(homeDir)
  const logDir = path.join(homeDir, 'logs')
  try {
    ensureHomeDirs(homeDir)
  } catch {
    console.log(`[home_up] 家目录写不进去: ${homeDir}`)
    return EXIT.ENV_UNAVAILABLE
  }

  const confExisted = fs.existsSync(confFile)
  let listen = confGet(confFile, 'listen_addr', P.DEFAULT_LISTEN)
  let clientPort = confGet(confFile, 'client_port', P.DEFAULT_CLIENT_PORT)
  // 只读状态口的端口（`status_port` 可覆盖；默认值住在 `paths.js`，与另两个端口同一个家）。
  let statusPort = confGet(confFile, 'status_port', P.DEFAULT_STATUS_PORT)
  // 模型配置门的端口（`config_port` 可覆盖；默认值同样住在 `paths.js`）。
  let configPort = confGet(confFile, 'config_port', P.DEFAULT_CONFIG_PORT)

  // **新家：先避开已经被占用的默认口**（2026-09-25）。
  //
  // 为什么要在建家的这一步做：端口是**机器级**命名空间，而"两家共用同一份发布包 + 同一组
  // 默认端口"会让家事实两维同时退化（可执行件路径相同、端口也相同 ⇒ 判据分不出彼此）。
  // 真机实测的后果是本家认领别家的世界、`down` 可能停掉别家的世界 —— **从源头让每个家有
  // 自己的口**，比事后辨认便宜得多。已有的家**一字不改**（绝不重写主人的 home.conf）。
  if (!confExisted) {
    const chosen = P.chooseFreshPorts({
      defaults: {
        listen_port: portOf(listen),
        client_port: clientPort,
        status_port: statusPort,
        config_port: configPort,
      },
      taken: (port) => proc.portInUse(String(port)),
    })
    const host = listen.slice(0, listen.lastIndexOf(':')) || '127.0.0.1'
    if (chosen.exhausted.length > 0) {
      // 默认口往后 20 个都被占着 ⇒ **不建这个家**（写一份注定起不来的 home.conf 不是诚实降级）。
      const what = chosen.exhausted.map((item) => `${item.key}（默认 ${item.base}）`).join('、')
      console.log(`[布置] ${what} 往后都占着 —— 先腾出一个口，或手工建 ${confFile} 指定端口，再开机`)
      return EXIT.ENV_UNAVAILABLE
    }
    listen = `${host}:${chosen.ports.listen_port}`
    clientPort = chosen.ports.client_port
    statusPort = chosen.ports.status_port
    configPort = chosen.ports.config_port

    fs.writeFileSync(
      confFile,
      [
        `listen_addr=${listen}`,
        `client_port=${clientPort}`,
        `status_port=${statusPort}`,
        `config_port=${configPort}`,
        '',
      ].join('\n'),
      'utf8',
    )
    console.log(`[布置] 家的配置：${confFile}（端口 + 身份 id —— **凭据不在这里**）`)
    for (const move of chosen.moved) {
      console.log(`[布置] ${move.key} 的默认口 ${move.from} 被占着 ⇒ 这个家用 ${move.to}（已写进 home.conf）`)
    }
  }

  // 两个口的**绑定地址**（家口远程访问刀）：缺省 loopback（模式一，一字不变）；
  // 绑非 loopback = 模式二（钥匙 + TLS，fail-closed 在那两个口自己那里核）。
  // `up` 只需要它来知道**去哪儿探活**（模式二探 TCP，不探 HTTP —— 见 `startDoors`）。
  const statusBind = (confGet(confFile, 'status_bind', homeKey.DEFAULT_BIND) || '').trim() || homeKey.DEFAULT_BIND
  const configBind = (confGet(confFile, 'config_bind', homeKey.DEFAULT_BIND) || '').trim() || homeKey.DEFAULT_BIND
  const imPort = portOf(listen)
  const imPidFile = path.join(logDir, 'im.pid')
  const clientPidFile = path.join(logDir, 'client.pid')
  // **家的门**（2026-10-01 第六刀）：读端面 ＋ 写端面 = **一个进程**，一本账。
  // 旧形状（两个门各一个进程）的两本账留在这里，只作**迁移桥**：老家还在跑旧门时
  // `up` 要认得出、收得掉（见 `startDoors` 的 ⓪）。
  const doorsPidFile = path.join(logDir, 'doors.pid')
  const legacyDoorPidFiles = [
    { target: 'status', label: '只读状态口', file: path.join(logDir, 'status.pid') },
    { target: 'config', label: '模型配置门', file: path.join(logDir, 'config.pid') },
  ]

  // im 的三样配置**导给整个流程**：服务端与它的 CLI 子命令（bootstrap-human /
  // issue-human-session）必须看**同一个库**——只加在启动那一条上是不够的，子命令会去
  // 开默认路径下的另一个库（bash 版实测踩到：bootstrap-human 报 error_kind="database"）。
  const imEnv = {
    ...process.env,
    CELIRA_LISTEN_ADDR: listen,
    CELIRA_DB_PATH: path.join(homeDir, 'im', 'core.db'),
    CELIRA_MEDIA_ROOT: path.join(homeDir, 'im', 'media'),
  }

  if (!fs.existsSync(confFile)) {
    fs.writeFileSync(confFile, `listen_addr=${listen}\nclient_port=${clientPort}\n`, 'utf8')
    console.log(`[布置] 家的配置：${confFile}（只有端口与身份 id —— **凭据不在这里**）`)
  }
  // 手册是**我们发的说明书**，不是主人的自持物（他的自持物在 `materials/` 与 `env/`）：
  // 源更新了就跟着更新 —— 否则「代码不要旧做法了，说明书还在教它」（TASK_51 的教训：
  // 换掉某个东西时，要清点"谁在向用户宣告这个依赖"）。
  // 只在**内容不同**时写盘，所以正常情况下这一节一声不响。
  const readmeSource = path.join(layout.repo, 'scripts', 'HOME_MANUAL.md')
  const readmeAtHome = path.join(homeDir, 'README.md')
  if (fs.existsSync(readmeSource)) {
    const source = fs.readFileSync(readmeSource)
    const current = fs.existsSync(readmeAtHome) ? fs.readFileSync(readmeAtHome) : null
    if (current === null || !source.equals(current)) {
      fs.copyFileSync(readmeSource, readmeAtHome)
      console.log(
        current === null
          ? `[布置] 手册：${readmeAtHome}（源在仓库 scripts/HOME_MANUAL.md）`
          : `[布置] 手册与仓库不同，已刷新：${readmeAtHome}`,
      )
    }
  }

  // ---------- 远端跑法（2026-09-30）：先看这个家是哪一种，再决定要不要起 core ----------
  //
  // `--im-endpoint` 给了 ⇒ **远端跑法**：本机不起 core，对着那个世界做布置（预检见下）。
  // 没给 ⇒ 本机全量（现状一字不动）——但**记着世界的家不许静默回落**：裸跑 `up` 一律停手，
  // 说清"要带 --im-endpoint"或"要改回本机全量先清哪几行"（`remote.localStartVerdict`）。
  const remoteRequested = options.imEndpoint !== null
  const confImEndpoint = (confGet(confFile, 'im_endpoint', '') || '').trim()
  if (!remoteRequested && confImEndpoint) {
    for (const line of remote.localStartVerdict({ confImEndpoint, wantToken: options.wantToken }).lines) {
      console.log(line)
    }
    return EXIT.ENV_UNAVAILABLE
  }

  // **包里没有 core**是一种正当形态（runtime 子包按设计只含 Agent 那一半），
  // 不该被说成"缺可执行件"。它要接一个**已经存在的世界**——那条路就是远端跑法
  // （`--im-endpoint`）：给了这个旗标就往下走（不起 core），没给才停在这句读数上。
  if (!remoteRequested && layout.kind === 'release' && !layout.hasCore) {
    return explainRuntimeOnly()
  }

  // ---------- --token：只补发 Human token（不需要 Agent 在线）----------
  if (options.wantToken) {
    if (!P.isFile(layout.imExe)) {
      console.log(`[home_up] 缺可执行件: ${layout.imExe}`)
      console.log(`[home_up] 先构建： (cd "${layout.repo}" && cargo build --release)`)
      return EXIT.ENV_UNAVAILABLE
    }
    const humanId = confGet(confFile, 'human_id')
    if (!humanId) {
      console.log('[home_up] 家里还没有 Human（先跑一次 node home.js up 布置）')
      return EXIT.ENV_UNAVAILABLE
    }
    console.log('[home_up] 补发一支 Human Session（旧的那支仍然有效，只是另给一支）')
    const { spawnSync } = require('node:child_process')
    const done = spawnSync(layout.imExe, ['issue-human-session', '--human-id', humanId], {
      env: imEnv,
      encoding: 'utf8',
      windowsHide: true,
    })
    if (done.status !== 0) {
      console.log(`[home_up] 补发失败（退出码 ${done.status}）：${(done.stderr || '').trim()}`)
      return EXIT.PARTIAL
    }
    try {
      console.log(`[token] ${JSON.parse(done.stdout.trim()).token}`)
    } catch {
      console.log('[home_up] 补发的输出读不懂（预期一行 JSON）')
      return EXIT.PARTIAL
    }
    return EXIT.OK
  }

  // ---------- 0) 组件与端口 ----------
  //
  // **家事实**（本刀）：认领判据的第二维——同一份发布包可以服务多个家，"可执行件路径"认不出
  // 彼此，判据还要「这个进程属于哪个家」（见 proc.js 那段）。一个命令取一次快照（netstat）。
  //
  // ⚠ **住户化之后分两段**：端口那几支（im / client / status / config）只需要这一份；
  // agent（宿主）那一支要的是**一支一个环境根**那一组（`envDirs`）—— 它在读住户之后补上。
  const homeBase = {
    // 家**根**（`--home` 声明的那一份）：常驻件的认领判据第二维要用它
    // （端口是机器级命名空间，同包多家时认不出彼此 —— 见 proc.js 的 homeFlagVerdict）。
    homeDir,
    clientPort,
    imPort,
    statusPort,
    configPort,
    // **家的门**（2026-10-01 第六刀）：一个进程两个端面 ⇒ 认领判据看"听没听这两个口中的任一个"。
    doorsPorts: [statusPort, configPort],
    listening: proc.listenerSnapshot(),
  }
  // **远端跑法只要求 Agent 那一半**（本机不起 core ⇒ 不要求 core 的可执行件）；
  // 本机全量照旧四件都要（含 celira-im）。
  const requiredExecutables = remoteRequested
    ? [layout.hostExe, layout.statusExe, layout.runExe]
    : [layout.imExe, layout.hostExe, layout.statusExe, layout.runExe]
  for (const exe of requiredExecutables) {
    if (P.isFile(exe)) continue
    console.log(`[home_up] 缺可执行件: ${exe}`)
    if (layout.kind === 'release') {
      console.log('[home_up] 这是**发布形态**（zip 解压），可执行件应当就在 bin/ 里 —— 重新解压一份，或重新下载')
    } else {
      console.log(`[home_up] 先构建： (cd "${layout.repo}" && cargo build --release)  以及  (cd "${layout.repo}/agent" && cargo build --bins --release)`)
      // 安装器**只有 Windows 那一份**：在别的平台上说这句话等于让人去找一个不存在的文件。
      console.log(
        proc.IS_WINDOWS
          ? '[home_up] 或者用安装器： .\\scripts\\install.ps1 （它会把 release 构建做齐）'
          : '[home_up] 本平台没有安装器：直接用上面那两条构建命令；或者下载发布包（解压即用）',
      )
    }
    return EXIT.ENV_UNAVAILABLE
  }
  const imListener = proc.listenerOf('im', layout, homeBase)
  // 账本要**验明正身**才算"我们的"：`isAlive(账本里的数)` 单独用会假阳性（陈旧账本里的 MSYS pid
  // 完全可能撞上别的 Windows 进程）——见 proc.ledgerPid 的四态。
  const imLedgerPid = proc.trustedLedgerPid('im', imPidFile, layout, homeBase)
  // ⚠ **远端跑法跳过这一问**：本机不起 core ⇒ 本机 listen 口被谁占着与这个家无关
  // （那个口在本机全量里才是"世界的家"）。
  if (!remoteRequested && imLedgerPid === null && imListener === null && proc.listenersOnPort(imPort).length > 0) {
    console.log(`[home_up] 端口 ${imPort} 被别的进程占着（家起不来）`)
    console.log(`[home_up] 看是谁： ${proc.portLookupHint(imPort)}`)
    return EXIT.ENV_UNAVAILABLE
  }

  // ---------- 0a) 家的形状迁移（住户化；**排在读住户之前**，第三刀的顺序先例）----------
  //
  // 一个家一支（环境根 `<家>/env`）⇒ 一支一个环境根（`<家>/agents/<支>/`）。两种身份走向
  // （有 home_id ⇒ 身份不变、环境整份迁；没有 ⇒ 键换形状、旧环境整份让位）与幂等纪律
  // 都在 `residents.js` 的 `migrateHomeShape` 里（一处权威）。这里只做接线与读数。
  // `canTouchWorld`：远端跑法没有主人凭据 ⇒ 迁移里"会另建一支"的那一步当场停手（本地也不动）。
  const resolvedToken = remoteRequested
    ? remote.remoteTokenFrom({ flagToken: options.remoteToken })
    : { token: null, source: null }
  const shape = residents.migrateHomeShape({
    homeDir,
    confFile,
    layout,
    canTouchWorld: remoteRequested ? resolvedToken.token !== null : true,
  })
  for (const line of shape.lines) console.log(line)
  if (!shape.ok) {
    console.log(shape.refuse)
    return EXIT.PARTIAL
  }

  // ---------- 0b) 住户表 ＋ 这一次开机的目标 ----------
  //
  // `up` 的起停单位是 **Agent**：无参数 = 遍历住户各起一个宿主（家里一支都没有时自建第一支）；
  // `--agent <id>` = 只给这一支安家（已有住户 ⇒ 重跑它；没有 ⇒ 认领成**新住户**，别人照旧）。
  const residentList = residents.residents(homeDir)
  for (const resident of residentList) {
    if (resident.mismatch) {
      console.log(
        `[warn] 住户目录 ${resident.agentId} 的 agent.conf 里写的是 ${resident.mismatch} —— 两边对不上（以目录名为准），先看清再动手`,
      )
    }
  }
  // ---------- 0b-0) 缺省变更的**一次性迁移**（2026-10-14 主人裁） ----------
  //
  // 平台默认从"有终端臂就给 full"改成**不给手**（`residents.resolveTerminal`）；老住户在旧缺省下
  // 本来是**有手**的 ⇒ 迁移把它们现行的 full **显式写进各自的 agent.conf**（来源从"平台默认"变成
  // "它自己的声明"，读数里看得见），**老住户不掉东西**、要手这件事从此看得见。
  // 判据、幂等键与"为什么不能拿经历当判据"都在 `migrateTerminalDefault` 的文档里；
  // 这里只负责在**起宿主之前**跑一次（装配面在起 Agent 那一刻定下，晚了就得再来一次 down/up）。
  migrateTerminalDefault({ homeDir, confFile, residents: residentList, log: console.log })
  const plan = planUpTargets({ residentsList: residentList, adoptAgentId: options.adoptAgent })
  // 半成品住户（目录在、**还没安家**）：裸 `up` 不替用户认领它们，但要**如实说出来** ——
  // 否则用户会看着一个目录问"为什么它没起"（第五刀真机发现：先写 agent.conf 再 up 曾在此停手）。
  if (!options.adoptAgent) {
    const planned = new Set(plan.targets.map((target) => target.resident?.agentId).filter(Boolean))
    for (const resident of residentList) {
      if (planned.has(resident.agentId)) continue
      console.log(
        `[布置] 住户 ${resident.agentId}：目录在，但**还没安家**（没有身份 / 住户序号）—— 这次跳过；` +
          `要把它安下来：node home.js up --agent ${resident.agentId}`,
      )
    }
  }
  if (plan.error) {
    console.log(plan.error)
    for (const line of USAGE) console.log(line)
    return EXIT.USAGE
  }
  const targets = plan.targets
  // agent 那一支的家事实：**一支一个环境根**那一组（现有住户的；新住户在布置里才知道根在哪）。
  const home = { ...homeBase, envDirs: residentList.map((resident) => resident.envDir) }

  // ---------- 0c) 模型那一半（**按住户**：清单归机器、绑定归 Agent）----------
  //
  // 绑定在场 ⇒ 以绑定为源：`DEEPSEEK_API_KEY` **不再要求**，而且**一个模型 env 都不给 Agent 的进程**
  // （连手边本来有的那两条也摘掉，见 `modelEnvironment`）——否则进程侧与绑定会打架，
  // Rust 侧按纪律**拒绝启动**（这正是"两个来源不许同时说话"的落地）。
  // 绑定不在场 ⇒ 现状分毫不动（下面那两条读数与从前逐字相同）。
  //
  // ⚠ **迁移排在它前面**：老的单份 `<环境根>/model_source.json` 要在这一步被迁成
  // "清单第一条 + 绑定"，否则老家的第一次开机还会走 env 那一路（而它其实已经配过了）。
  // 迁移幂等 ⇒ 与 config-server 那次重跑无害（同一份逻辑在 `model-sources.js`，一处权威）。
  const modelSourcesFile = modelSources.listPath(homeDir)
  const residentModel = new Map()
  for (const resident of residentList) {
    const migration = modelSources.migrateIfNeeded(homeDir, resident.envDir)
    const who = residentList.length > 1 ? `住户 ${resident.agentId}：` : ''
    if (migration.action === 'migrated') {
      console.log(`[model] ${who}迁移：${migration.detail}（清单条目 ${migration.entry?.id}）`)
    } else if (migration.action === 'conflict' || migration.action === 'failed') {
      console.log(`[model] ⚠ ${who}迁移没做：${migration.detail}`)
    }
    const binding = modelSources.readBinding(resident.envDir)
    residentModel.set(resident.agentId, { fromFile: binding.state === 'bound', binding })
    if (binding.state === 'bound') {
      console.log(`[model] ${who}模型来自本机清单里的 ${binding.source}：${modelSourcesFile}`)
      console.log('        这次开机**不给 Agent 传任何模型环境变量**；改配置在「模型配置」页，下一次活动生效')
    } else if (binding.state === 'unreadable') {
      console.log(`[model] ⚠ ${who}绑定读不出来（${binding.reason}）——Agent 起不来就是因为这个；去「模型配置」页重绑一次`)
    }
  }
  // 模型钥匙：**这一次要起的那些住户**里，只要有谁没有绑定在场，就要求它。
  //
  // ⚠ **新住户不一定需要钥匙**（2026-10-01 多支刀）：家里别的住户已经绑好模型时，这一支会
  // **跟着用同一条**（`provision` 的 `carryBindingFrom`），或者它的环境根里已经先放好了绑定
  // ——这两种场合都不该再逼用户去配 `DEEPSEEK_API_KEY`（那会让"再安一支"当场卡住，而家里明明
  // 已经配过了）。真正全新的家（连一支都没有）仍然照旧要求它，那才是"一次配好"的时机。
  //
  // ⚠ **半成品住户**（2026-10-01 第五刀真机发现的坑）：它的环境根还没有身份 ⇒ 这次布置会把环境
  // **建起来**，绑定也就跟着带进去了 —— 所以它同样不该被逼着要钥匙。判据统一成一句：
  // **"这一支的环境这次会不会被建起来"**（`envWillBeBuilt`），带绑定的来源从家里**别的**已绑支里挑。
  const envWillBeBuilt = (target) =>
    !target.resident || !fs.existsSync(path.join(target.resident.envDir, 'identity.json'))
  const boundEnvOf = (excludeAgentId = null) =>
    residentList.find((other) => other.agentId !== excludeAgentId && residentModel.get(other.agentId)?.fromFile)?.envDir ?? null
  const carriedFor = (target) => (envWillBeBuilt(target) ? boundEnvOf(target.resident?.agentId ?? null) : null)
  const needsApiKey = targets.some((target) => {
    const state = target.resident ? residentModel.get(target.resident.agentId) : null
    if (state && state.fromFile) return false
    const seeded =
      !target.resident &&
      target.adoptId &&
      fs.existsSync(path.join(P.agentEnvRoot(homeDir, target.adoptId), 'model_binding.json'))
    return !seeded && !carriedFor(target)
  })
  const apiKey = needsApiKey ? readUserEnv('DEEPSEEK_API_KEY') : ''
  if (needsApiKey && !apiKey && !options.attachOnly) {
    console.log('[home_up] 缺凭据：本机用户环境变量里没有 DEEPSEEK_API_KEY（模型那一半）')
    console.log('[home_up] 配一次即可（变量名：DEEPSEEK_API_KEY）——Agent 需要它才能思考')
    console.log('[home_up] 或者开机后在浏览器里配一次：「设置 → 模型配置」（它会写进本机的私有配置）')
    return EXIT.NEEDS_YOU
  }
  // ★ **`--attach-only` 且没有模型**（2026-10-15）：不早失败，也**不起**没模型的那几支
  //   —— 接上是"它在机器上有个位置"，起起来是另一件事（判据与壳那份 `homed-not-started` 同一句话）。
  const modellessTargets = new Set()
  /** 接上了、但因为没模型而**没起**的那几支（收尾时如实说，退出码也跟着它走）。 */
  const skippedModelLess = []
  if (needsApiKey && !apiKey && options.attachOnly) {
    for (const target of targets) {
      const state = target.resident ? residentModel.get(target.resident.agentId) : null
      if (state && state.fromFile) continue
      const seeded =
        !target.resident &&
        target.adoptId &&
        fs.existsSync(path.join(P.agentEnvRoot(homeDir, target.adoptId), 'model_binding.json'))
      if (!seeded && !carriedFor(target)) {
        const id = target.resident?.agentId ?? target.adoptId ?? null
        if (id !== null) modellessTargets.add(id)
      }
    }
    console.log(
      `[home_up] 这一支（${[...modellessTargets].join('、') || '新住户'}）还没有模型来源 ⇒ **接上，但先不起它**`,
    )
    console.log('[home_up] 去「模型配置」页给它选一条（或在它的页面上选），选好之后下一次 up / 打开 app 就会把它带起来')
  }

  // ---------- 0b) 远端跑法：预检（**动手之前**把三件事问清，fail-closed）----------
  //
  // 三问、三种读数（`remote.js`）：世界**可达**吗？主人凭据被**认下**吗？要认领的身份**在场**吗？
  // 任何一问不过 ⇒ **停手**（这一次没有对任何世界做任何改动），退出码 2（环境不可用）。
  // 预检是**只读**的：建身份、签凭据都在布置那一步（排在它后面）。
  //
  // 没有主人凭据时走**另一条预检**（已存后端凭据那支）：布置好的远端家日常重跑不该要求
  // 主人凭据 —— 但"世界还在不在、凭据还认不认"必须先问清楚，否则一开机就是聋的。
  let remoteWorld = null
  if (remoteRequested) {
    if (resolvedToken.source === 'env') {
      console.log(`[远端] 主人凭据来自环境变量 ${remote.HUMAN_ENV}（**不打印取值**）`)
    }
    // 这个家记着的身份：住户表里第一支（住户化之后）；没有住户时看 `--agent` 的认领目标。
    const confAgentId = residentList.length > 0 ? residentList[0].agentId : null
    let preflight
    if (resolvedToken.token) {
      preflight = await remote.preflightWorld({
        base: options.imEndpoint,
        token: resolvedToken.token,
        adoptAgentId: options.adoptAgent,
        confAgentId,
      })
    } else if (options.adoptAgent) {
      preflight = {
        ok: false,
        reason: 'need-token',
        lines: [
          '[远端] 认领世界里已有的一个身份要主人凭据： --token <那个世界的主人凭据>',
          `[远端] （或用环境变量 ${remote.HUMAN_ENV}）`,
          ...remote.STOP_LINES,
        ],
      }
    } else {
      const stored = confAgentId
        ? credentials.resolve({ world: options.imEndpoint, agentId: confAgentId }, { fallback: readUserEnv(BACKEND_ENV) })
        : { token: null }
      if (!confAgentId || !stored.token) {
        preflight = {
          ok: false,
          reason: 'need-token',
          lines: [
            '[远端] 这次布置还需要主人凭据： --token <那个世界的主人凭据>（clr_hs_ 开头）',
            `[远端] （或用环境变量 ${remote.HUMAN_ENV}；家里已经布置好之后的**重跑**不需要它）`,
            ...remote.STOP_LINES,
          ],
        }
      } else {
        preflight = await remote.preflightStoredCredential({
          base: options.imEndpoint,
          agentId: confAgentId,
          credential: stored.token,
        })
      }
    }
    if (!preflight.ok) {
      for (const line of preflight.lines) console.log(line)
      return EXIT.ENV_UNAVAILABLE
    }
    for (const line of preflight.lines) console.log(line)
    remoteWorld = { base: options.imEndpoint, token: resolvedToken.token }
  }

  // ---------- 1) 起 im 服务端 ----------
  if (remoteRequested) {
    // **本机不起 core**（远端跑法的全部意义）：世界在别处，那里才是权威。
    console.log(`[im] 远端跑法：不起本机 core（世界在 ${remoteWorld.base}）`)
  } else {
    const hasAgent = residentList.length > 0
    // 端口上有一支在听、而账本**没指向它** ⇒ 先证明"它是不是本家的"（`world.js`）。
    // 账本指向它时不必问：账本是本家自己的事实（快路径，也不多花一次 HTTP）。
    // ⚠ 凭据**惰性读**：正常开机（账本指向在跑那支）连 PowerShell 都不用起。
    // 来源与布置那一步**同一份口径**（`credentials.resolve`）：按「世界 × 身份」分条的本地存储 →
    // `CELIRA_IM_BACKEND_AUTH`（老路 / 手工路）。证明用**住户表里第一支**的凭据
    // （任何一支都够证明"这个世界是本家的" —— 世界是同一台机器上的同一个）。
    const proofAgentId = residentList.length > 0 ? residentList[0].agentId : null
    let backendForProof
    const proofCredential = () => {
      if (backendForProof === undefined) {
        backendForProof = proofAgentId
          ? credentials.resolve(
              { world: `http://${listen}`, agentId: proofAgentId },
              { fallback: readUserEnv(BACKEND_ENV) },
            ).token
          : null
      }
      return backendForProof
    }
    const proof =
      imLedgerPid === null && imListener !== null
        ? (
            await proveWorldOwnership({
              base: `http://${listen}`,
              credential: proofCredential(),
              agentId: proofAgentId,
            })
          ).state
        : null
    const verdict = imListenerVerdict({
      ledgerPid: imLedgerPid,
      listener: imListener,
      proof,
      hasAgent,
      port: imPort,
    })
    for (const line of verdict.lines) console.log(line)

    if (verdict.action === 'adopt') {
      // 账本陈旧，但端口上那支**就是我们的人**（原因之一是跨实现过渡：.sh 起的那一版
      // 记的是 MSYS pid，Node 读出来对不上任何 Windows 进程）。裁决同 TASK_40 的接管：
      // **把账本对上真实**，不重起——重起会撞端口，而端口上那支本来就是我们要的那个。
      if (verdict.pid !== imLedgerPid) proc.writePid(imPidFile, verdict.pid)
    } else if (verdict.action === 'stop' || verdict.action === 'refuse-stop') {
      // 铁证不是本家（或这个家还没布置过、而那支证明不了是本家的）⇒ 停手，**不动它**。
      return EXIT.ENV_UNAVAILABLE
    } else if (verdict.action === 'refuse-continue') {
      // 判不了 ⇒ 不认领、不写账本，但往下走（保住手册 §8 的凭据恢复路径）。
    } else if (hasAgent && (await proc.httpCode(`http://${listen}/health`)) === '200') {
      // 地址上有东西在答，但 netstat 没给出本家的指纹 ⇒ 也要证明一句（同一个三态）。
      const proved = (
        await proveWorldOwnership({
          base: `http://${listen}`,
          credential: proofCredential(),
          agentId: proofAgentId,
        })
      ).state
      if (proved === 'foreign') {
        console.log(`[im] ${listen} 上的世界**不是本家的**（它认下本家的凭据，却说是别人的 Agent）——不动它`)
        console.log('[im] 给这个家换一个口：编辑 home.conf 的 listen_addr，再开机')
        return EXIT.ENV_UNAVAILABLE
      }
      console.log('[im] 已经在跑（不是本脚本起的；家里的服务端只该有一个）')
    } else {
      // **起世界之前核一次**（`world_witness.js`）：世界不在场的那段时间里，有没有人动过它。
      // 顺序是承重的：**先核，后起** —— 世界一旦跑起来，任何一句话都会改它，那个窗口就没了。
      // ⚠ 它**不拒绝启动**：只如实报一行。"要不要因此拒绝"是另一次取舍，不在这一刀。
      const witness = worldWitness.checkBeforeStart(homeDir, {
        homeId: confGet(path.join(homeDir, 'home.conf'), 'home_id', null),
      })
      console.log(`[im] 见证：${witness.text}（账本 ${witness.records} 条）`)
      if (witness.chain !== null) {
        console.log(`[im] 见证：⚠ 链条在第 ${witness.chain.index} 条断了 —— 有人抽掉了中间的记录`)
      }
      const started = proc.spawnDetached(layout.imExe, [], {
        cwd: homeDir,
        env: imEnv,
        logFile: path.join(logDir, 'im.out'),
      })
      proc.writePid(imPidFile, started)
      const ready = await proc.waitHttp(`http://${listen}/health`, 30)
      if (ready.ok) {
        // 健康检查有可能是**别人**应下的（我们的那个没绑上端口就退了，而端口上另有其人）。
        // 机械判据：我们记下的那个进程必须还活着——否则家会用错数据库。
        if (!proc.isAlive(started)) {
          console.log(`[im] 端口 ${imPort} 上的不是我们起的那个（我们记的 pid 已经退了）——停手，免得用错数据库`)
          console.log(`[im] 看是谁： ${proc.portLookupHint(imPort)}`)
          return EXIT.ENV_UNAVAILABLE
        }
        console.log(`[im] http://${listen} （pid ${started}，日志 logs/im.out）`)
      } else {
        console.log('[im] 起不来（30 秒没回应）—— logs/im.out 的尾巴：')
        for (const line of tailLines(path.join(logDir, 'im.out'), 5)) console.log(line)
        return EXIT.PARTIAL
      }
    }
  }

  // **远端跑法记账**（2026-09-30）：布置成了才把世界记进 `home.conf`（失败不记 —— 那一次
  // 什么都没改，家还是原来那个）。这一行是**裸跑 up 的判据**：记着世界的家不许静默回落成
  // 本机全量（`remote.localStartVerdict`），`status` / `down` 也据此如实说"本机不起 core"。
  if (remoteWorld) {
    confSet(confFile, 'im_endpoint', remoteWorld.base)
    console.log(`[布置] 这个家是**远端跑法**：世界在 ${remoteWorld.base}（已记进 home.conf 的 im_endpoint）`)
  }
  // **Agent 要连的那个世界**（本机全量 = 本机那个口；远端跑法 = 那个世界）——起 Agent、
  // 问身份、`up` 的读数都指向它。
  const worldBase = remoteWorld ? remoteWorld.base : `http://${listen}`

  // ---------- 2) 布置（**一支一支来**：起停单位是 Agent）----------
  //
  // TASK_62：布置**之前**先问这一支"你现在在不在跑" —— 凭据那一步要按这个答案决定动不动。
  // 判据复用既有的（账本 `ledgerPid` 的四态 ＋ 路径与家事实都对得上的孤儿），不新造。
  // ⚠ 新住户（还没有环境根）⇒ 不在跑。
  let partial = false
  const startedResidents = []
  for (const target of targets) {
    const scopedHome = target.resident ? { ...home, envDirs: [target.resident.envDir] } : home
    const agentAtStart = target.resident
      ? agentRunningNow({
          hostPidFile: residents.residentPidFile(homeDir, target.resident),
          layout,
          home: scopedHome,
        })
      : { running: false, pid: null, adopted: false, foreign: null }
    const done = await provision({
      homeDir, materialsDir, listen, confFile, imEnv,
      imExe: layout.imExe, runExe: layout.runExe,
      // 已有住户（重跑它）／认领目标（新住户）／都没有（自建第一支）。
      resident: target.resident,
      adoptAgentId: target.adoptId,
      // 自建那一支的名字（`--agent-name`）：`null` = 没给 ⇒ 布置用占位 `未命名`。
      agentName: options.agentName,
      // 这个世界里你自己的名字（`--human-name`，2026-10-20）：`null` = 没给 ⇒ 占位 `未命名`。
      humanName: options.humanName,
      slot: target.slot,
      agentRunning: agentAtStart.running,
      agentPid: agentAtStart.pid,
      // 老家让位目录（没有 home_id 的那种）：模型绑定从这里带进新环境。
      envAside: shape.envAside ?? null,
      // 家里**别的住户**的环境根（新住户 / 半成品跟着用同一条模型绑定 —— 家级选择，一支一份落盘）。
      carryBindingFrom: carriedFor(target),
      // **远端跑法**（2026-09-30）：给了世界地址 ⇒ 布置对着那个世界走 HTTP（人那一步用
      // `--token` 给的主人凭据替代本机铸人）；没给 ⇒ `null`（本机全量，现状一字不动）。
      remote: remoteWorld ? { base: remoteWorld.base, token: remoteWorld.token } : null,
    })
    if (!done.ok) {
      const messages = String(done.message).split('\n').filter(Boolean)
      for (const line of messages) console.log(line)
      console.log('[布置] 没走完 —— 上面那几行就是原因')
      return EXIT.PARTIAL
    }
    // 布置之后这一支的住户事实（新住户的环境根这时才知道）——起宿主用。
    const resident = {
      agentId: done.agentId,
      envDir: target.resident ? target.resident.envDir : P.agentEnvRoot(homeDir, done.agentId),
      slot: done.slot,
      roomId: done.roomId,
      displayName: target.resident ? target.resident.displayName ?? '' : '',
      legacy: false,
      mismatch: null,
    }
    // **绑定在这一刻才是事实**（新住户的环境根刚建出来，绑定可能是刚搬进来的）——
    // 起宿主之前从**这一支的环境根**重新读一次，而不是沿用布置前的判断（布置前它还没有环境根）。
    // ⚠ 不重读的真机后果：家里/别处已经绑好模型、而用户 shell 里也有 `DEEPSEEK_API_KEY`
    // ⇒ 子进程同时看到**两个来源** ⇒ Rust 侧按纪律**拒绝启动**（"两个来源不许同时说话"）。
    const modelState = modelSources.readBinding(resident.envDir)
    // ★ **没模型的那几支：接上，但先不起它**（2026-10-15；只在 `--attach-only` 下成立）。
    //   判据与壳那份 `homed-not-started` 是同一句话 —— 而且这里**一个字节都不替它绑**
    //   （命令行形态的模型来路是**部署者的进程环境**，不是清单：清单与绑定归页面写）。
    if (modellessTargets.has(resident.agentId)) {
      console.log(
        `[model] 住户 ${resident.agentId}：**接上了，先没起它**（这一支还没有模型来源）—— 配好之后下一次 up 就会把它带起来`,
      )
      skippedModelLess.push(resident.agentId)
      continue
    }
    startedResidents.push({
      resident,
      agentAtStart,
      modelFromFile: modelState.state === 'bound',
    })
  }
  // **远端跑法记账**（2026-09-30）：布置成了才把世界记进 `home.conf`（失败不记 —— 那一次
  // 什么都没改，家还是原来那个）。这一行是**裸跑 up 的判据**：记着世界的家不许静默回落成
  // 本机全量（`remote.localStartVerdict`），`status` / `down` 也据此如实说"本机不起 core"。
  if (remoteWorld) {
    confSet(confFile, 'im_endpoint', remoteWorld.base)
    console.log(`[布置] 这个家是**远端跑法**：世界在 ${remoteWorld.base}（已记进 home.conf 的 im_endpoint）`)
  }

  // ---------- 3) 起宿主（**一支一个进程**）----------
  //
  // ⚠ **"Agent 没在预算内报到"不在这里提前返回**（2026-09-26）：后面那三支
  // （client / 状态口 / 配置门）本来就不依赖 Agent —— 它们自己的注释写着"起不来不算失败"。
  // 而 Agent 迟到现在有一条**合法**来路：宿主的写资格可能正被**只读状态机**握着
  // （`agent_host_status` 一次读要全量解码账本，这个家实测 **11.3 秒**，且随账本增长），
  // 宿主会**等**（`agent/README.md` §3.18 的 `open_when_free`，耐心 90 秒）。
  // 但**自己说了原因就退出**（`[exit]`）是另一回事：那一支当场如实报出来、停在退出码 4。
  for (const entry of startedResidents) {
    const done = await startResidentHost({
      layout,
      homeDir,
      confFile,
      home,
      materialsDir,
      listen,
      worldBase,
      remoteWorld,
      resident: entry.resident,
      agentAtStart: entry.agentAtStart,
      modelFromFile: entry.modelFromFile,
      apiKey,
      // 多支的家：读数要**点名是哪一支**（一行读数不许让主人猜）。
      many: startedResidents.length > 1 || residentList.length > 1,
    })
    if (done.status === 'exited') return EXIT.PARTIAL
    if (done.status === 'needs-you') return EXIT.NEEDS_YOU
    if (done.status === 'partial') partial = true
  }

  // ---------- 4) 起 client 入口（**起不来不算失败**）----------
  // `statusPort` / `configPort` 一起传下去：页面读状态、配模型都走**同源代转**，代转要知道
  // 口子开在哪个端口（TASK_61 / TASK_77）——页面自己永远不知道，这正是那两把刀要的效果。
  //
  // **远端跑法**：本机**不起页面**。页面与世界必须同源（`im/server/README.md` §5.4：
  // core 不发 CORS 头，浏览器从本机页面打远端 core 会在预检那一步被拦）⇒ 远端世界的页面
  // 由**世界那台机器**提供；本机这半边只要 Agent 在线。这是如实省略，不是静默降级。
  if (remoteWorld) {
    console.log('[client] 远端跑法：本机不起页面 —— 页面由世界那台机器提供（页面与世界必须同源）')
  } else {
    await startClient({ layout, homeDir, logDir, clientPort, listen, clientPidFile, statusPort, configPort, home })
  }

  // ---------- 4b) 起**家的门**（**起不来不算失败**）----------
  // 一个进程、两个端面：读端面（只读状态口，运行状态页的活源）＋ 写端面（模型配置门，
  // 模型配置页的写口）。两个端面**各自绑定、各自准入**（见 `doors.js` 与 `proc.js` 的 doors 指纹）。
  await startDoors({
    layout,
    homeDir,
    logDir,
    doorsPidFile,
    legacyPidFiles: legacyDoorPidFiles,
    statusPort,
    statusBind,
    configPort,
    configBind,
    home,
  })

  // ---------- 5) 读数（这就是"家亮着"的证据）----------
  console.log('')
  if (remoteWorld) {
    // 远端跑法：本机没有页面（页面在世界那台机器上，与世界同源）—— 不许打印一个本机地址
    // 让用户去开一个不存在的窗口。
    console.log(`[outside] 本机不起页面 —— 页面在世界那台机器上（世界：${remoteWorld.base}）`)
  } else {
    console.log(`[outside] 浏览器那个窗口（生产页）： http://localhost:${clientPort}`)
  }
  // 值班室**按支**报（一支一间房 = 一段关系）；单支时与从前逐字相同。
  if (startedResidents.length === 1) {
    console.log(`[talk] 值班室 = ${startedResidents[0].resident.roomId}`)
  } else {
    for (const { resident } of startedResidents) {
      console.log(`[talk] 值班室（${resident.agentId}）= ${resident.roomId}`)
    }
  }
  console.log('[stop] 关机： node home.js down       状态： node home.js status')
  if (skippedModelLess.length > 0) {
    console.log(
      `[model] 接上了、但**没起**的有 ${skippedModelLess.length} 支：${skippedModelLess.join('、')} —— 给它们选一条模型之后再跑一次本命令`,
    )
  }
  if (remoteWorld) {
    console.log('[token] 补发 Human token 是**世界那台机器**上的事（那边 node home.js up --token）；本机这半边不需要')
  } else {
    console.log('[token] 补发一支 Human token（client 用）： node home.js up --token')
  }
  // 家亮着，但有一支**这次没读到它的报到**（目前只有 Agent 那一格会置它）：如实给"只起来一部分"。
  // ★ 另有"接上了、但没模型所以没起"那一档（`--attach-only`）：**也是"只起来一部分"** ——
  //   不许给 0（那会让调用方以为它跑起来了）。
  if (skippedModelLess.length > 0) return EXIT.PARTIAL
  return partial ? EXIT.PARTIAL : EXIT.OK
}

/**
 * 起**一支住户**的宿主（一支一个进程；2026-10-01 多支刀从 `up` 主流程里提出来）。
 *
 * 幂等：账本/孤儿对得上 ⇒ "接管，不重起"；已经在跑 ⇒ 只报读数。起不来时**分两档**：
 *   · `[exit]`（自己说了原因）⇒ `exited`，调用方当场停在退出码 4（与单支时代逐字同款）；
 *   · 预算内没等到 `[listening]` ⇒ `partial`（可能只是在等环境写资格），后面几支照常起。
 * 凭据空且不在跑 ⇒ `needs-you`（与从前那句"重开一次会重新布置"同款）。
 *
 * @returns {Promise<{status: 'ok'|'partial'|'exited'|'needs-you'}>}
 */
async function startResidentHost({
  layout,
  homeDir,
  confFile,
  home,
  materialsDir,
  listen,
  worldBase,
  remoteWorld,
  resident,
  agentAtStart,
  modelFromFile,
  apiKey,
  many = false,
}) {
  const agentId = resident.agentId
  const hostPidFile = residents.residentPidFile(homeDir, resident)
  const hostLog = residents.residentHostLog(homeDir, resident)
  fs.mkdirSync(path.dirname(hostPidFile), { recursive: true })
  const logLabel = path.relative(homeDir, hostLog) || hostLog
  // Agent 的处境**在 Agent 起来之前**读：环境同时只有一个写者，而 Agent 的写资格是**一次活动**持有的
  // （不是常驻）：Agent 空闲时读得到；万一读到 `already-open`（Agent 那一轮正在跑一次活动），
  // 就如实说明，并把此刻的读数指到日志里。
  console.log(`[state] 开机前 Agent 的处境（agent_host_status 的三节；住户 ${agentId}）：`)
  const status = require('node:child_process').spawnSync(
    layout.statusExe,
    [resident.envDir, '--agent', agentId],
    { encoding: 'utf8', windowsHide: true },
  )
  const statusText = `${status.stdout ?? ''}${status.stderr ?? ''}`.trimEnd()
  const statusLines = statusText ? statusText.split('\n') : []
  for (const line of indent(statusLines)) console.log(line)
  if (statusText.includes('already-open')) {
    console.log('    ↑ Agent 这一拍正在跑一段在场（环境同时只有一个写者）。要看 Agent 此刻在做什么：')
    console.log(`      看 ${logLabel} 里的 [run] 行；等这一拍过去再敲本命令就能读到完整三节。`)
  }

  // 起宿主**之前**对账：pid 文件不活时，看看进程表里有没有**路径对得上**的孤儿。
  // 裁决（**接管**，不是重起）：孤儿在场 ⇒ 把 pid 文件重写指向它。理由：home_up 的既有
  // 语义就是幂等；起新支反而制造双宿主（两支同时听门铃，靠环境写锁互相挡）。
  // 账本里那支**不是本家**的（家事实对不上：别家的宿主 / 陈旧撞号）⇒ 如实说出来。
  if (agentAtStart.foreign !== null) {
    console.log(
      `[agent] 住户 ${agentId} 的 pid 文件里记的 ${agentAtStart.foreign} 不是本家的宿主——本家那支没找到，起一支新的`,
    )
  }
  if (agentAtStart.adopted) {
    console.log(`[agent] 住户 ${agentId} 已有一支宿主在跑（pid ${agentAtStart.pid}，pid 文件没指向它——接管，不重起）`)
  }

  // **这一支的手**（2026-10-01 第五刀）：外部 server 与终端档都**按支解析**
  // （`residents.residentHands`：支级覆盖家级、家级是默认、平台事实优先）——
  // 读数逐条**说清来源**，用户不许靠"旗标少了一个"去猜。
  const hands = residents.residentHands({ homeDir, resident, homeConfFile: confFile })
  const terminal = residents.resolveTerminal(hands, { isWindows: proc.IS_WINDOWS, isLinux: proc.IS_LINUX })
  const declarations = hands.mcp
  const mcpList = declarations.map((declaration) => declaration.split('=', 1)[0]).join(',')
  const who = many ? `住户 ${agentId}：` : ''

  if (agentAtStart.running) {
    // 接管那一支写进去的是**真实 pid**（对账查出来的），所以这里再读一次账本作为显示值。
    console.log(`[agent] 住户 ${agentId} 已经在等唤醒（pid ${proc.readPid(hostPidFile)}）`)
    if (declarations.length > 0 || hands.mcpSource === 'none') {
      for (const line of handNotices({ who, hands, terminal })) console.log(line)
      console.log('      ⚠ Agent**已经在跑**：装配面是**起 Agent 那一刻**的事实 ⇒ 换面要先 node home.js down 再 up')
    }
    return { status: 'ok' }
  }

  for (const line of handNotices({ who, hands, terminal })) console.log(line)

  // 与布置那一步同一份取值口径（`credentials.resolve`）：按「世界 × 身份」分条的本地存储 →
  // `CELIRA_IM_BACKEND_AUTH`（老路 / 手工路）。起 Agent 时注入给 Agent 的仍是同一个环境变量
  // —— **取值来源换了，接口没换**（Rust 侧一行不改）。
  const backend = credentials.resolve(
    { world: worldBase, agentId },
    { fallback: readUserEnv(BACKEND_ENV) },
  ).token
  if (!backend) {
    console.log(
      `[home_up] 住户 ${agentId} 的后端凭据仍然是空的（本地存储里没有、环境变量里也没有）——重开一次会重新布置`,
    )
    return { status: 'needs-you' }
  }

  // 上一份先留档（`host.out.1`）再清空：**读的那一侧**（下面等 `[listening]` 行）
  // 看到的本轮仍然从空开始，不会被上一次那一行骗出一道假绿；而上一轮的宿主读数
  // （`model-failure` / `drives` / 启动参数）留在盘上，回头还查得到。
  const rotation = proc.rotate(hostLog)
  if (rotation.lostBytes > 0) {
    console.log(`[warn] 上一份宿主日志（${rotation.lostBytes} 字节）没能留档，本次照常开机`)
  }
  // **是谁**（TASK_70）：**起 Agent 之前当场问一次**（两个值同源、都出自这一次回执）。
  // ⚠️ 不落 `home.conf`：此世界的名字会改，转抄一份就是第二个可能过期的真相。
  // 问的是 **Agent 要连的那个世界**（`worldBase`：远端跑法下就是远端地址）。
  const identity = await readIdentity(worldBase, backend)
  // **如实报**：问不到就说"这次开机不带那一行"——它是**合法**状态（开场跳过），
  // 但操作者该知道"Agent 这次没带出生证明"，而不是自己从缺席里猜。
  if (identity) {
    console.log(`[identity] 是谁：${identity.id}（${identity.displayName}）—— 随这次开机交给 Agent`)
  } else {
    console.log('[warn] 问不到是谁（me 没给出 id 与名字）—— 这次开机不带那一行')
  }
  const started = proc.spawnDetached(
    layout.hostExe,
    buildHostArgs({
      envDir: resident.envDir,
      agentId,
      materialsDir,
      listen,
      // **远端跑法**：Agent 的 im 端点指向那个世界（缺省 = 本机 listen，现状一字不动）。
      imEndpoint: remoteWorld ? remoteWorld.base : null,
      declarations,
      identity,
      modelSourcesFile: modelSources.listPath(homeDir),
    webSourcesFile: path.join(homeDir, 'web_sources.json'),
      // **这一支的手**（第五刀）：档位由支级/家级声明解析出来（缺省仍是平台默认）。
      terminal: terminal.mode,
      // **这一支的资源上界**（2026-10-05）：从它自己的 `agent.conf` 读；缺省 = 内置 300。
      maxTurns: residents.readAgentBudget(P.agentConfPath(homeDir, agentId)).maxTurns,
    }),
    {
      cwd: homeDir,
      logFile: hostLog,
      // TASK_77：文件在场时**一个模型 env 都不给 Agent**（连手边本来有的那两条也摘掉）——
      // 见 `modelEnvironment`；文件不在场时这一句与从前逐字相同。
      // ⚠ 代理绕过**保留操作者原有的取值**（见 `appendNoProxy`）：从前这里是直接覆盖成常量，
      // 于是"给模型那家加一条绕过"这种操作者设置会被静默吞掉。
      env: hostEnvironment(
        { ...process.env },
        { backend, apiKey, modelFromFile },
      ),
    },
  )
  proc.writePid(hostPidFile, started)

  let listening = false
  let exited = false
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const text = readText(hostLog)
    if (logHasLine(text, '[exit]')) {
      exited = true
      break
    }
    if (logHasLine(text, '[listening]')) {
      listening = true
      break
    }
    await proc.sleep(500)
  }
  if (exited) {
    console.log(`[agent] 住户 ${agentId} 没起来，自己说了原因（${logLabel}）：`)
    for (const line of fromStateLine(hostLog)) console.log(line)
    return { status: 'exited' }
  }
  if (listening) {
    console.log(
      `[agent] 住户 ${agentId} 在等唤醒（pid ${proc.readPid(hostPidFile)}，日志 ${logLabel}）`,
    )
    return { status: 'ok' }
  }
  // 如实说**两种**可能：宿主可能正等着环境的写资格（家的只读状态口读一次要十几秒），
  // 也可能真的卡住了——预算内分不开，所以不替它下结论，只报"这次没读到那一行"。
  console.log(`[agent] 住户 ${agentId} 20 秒没等到监听那一行（它可能正在等环境的写资格：只读状态口读一次要十几秒）`)
  console.log('       后面几支照常起；等一会儿用 node home.js status 看它有没有报 [listening]')
  console.log(`       ${logLabel} 的尾巴：`)
  for (const line of tailLines(hostLog, 5)) console.log(`       ${line}`)
  return { status: 'partial' }
}

/**
 * **这一次 `up` 要动哪几支**（纯函数 ⇒ 可机械判错；2026-10-01 多支刀）。
 *
 * 三条：
 *   · `--agent <id>`：家里已经有这支住户**且已安家** ⇒ **只重跑它**（`adoptId: null`，不认领）；
 *     **没有**、或者只有一个"半成品"目录（写了 `agent.conf` 但没身份/住户序号）⇒ 走**布置**那条路
 *     （`adoptId` 非空 ＋ 分一个住户序号），别人照旧；
 *   · 无参数、家里一支都没有 ⇒ **自建第一支**（保住"一条命令就能用"）；
 *   · 无参数、家里有住户 ⇒ 遍历**每一支已安家**的各跑一遍（幂等重跑）；半成品不进遍历。
 *
 * @param {{residentsList: Array, adoptAgentId: string|null}} ctx
 * @returns {{targets: Array<{resident: object|null, adoptId: string|null, slot: number|null}>, error: string|null}}
 */
function planUpTargets({ residentsList, adoptAgentId = null }) {
  const list = Array.isArray(residentsList) ? residentsList : []
  if (adoptAgentId) {
    const existing = list.find((resident) => resident.agentId === adoptAgentId)
    if (existing) {
      // ⚠ 「已有住户」有两种，判据是**这一支安没安家**，不是"目录在不在"（2026-10-01 第五刀
      //   真机发现的坑：用户先给第二支写了 `agents/<支>/agent.conf`（手、显示名），再
      //   `up --agent <它>` —— 那时这里把"目录在"读成"已安家"，于是**停手**并要用户手工补
      //   `slot=`；而正确的动作是**把这一支安下来**（认领世界里那支 ⇒ 写身份 ⇒ 补住户序号，
      //   已经写在 `agent.conf` 里的手与显示名原样留着）。
      //   安家的标志：**有自己的环境根身份**（`identity.json`）**且**有住户序号。两者缺一 ⇒ 半成品。
      const settled = Boolean(existing.slot) && fs.existsSync(path.join(existing.envDir, 'identity.json'))
      if (settled) return { targets: [{ resident: existing, adoptId: null, slot: null }], error: null }
      return {
        targets: [
          { resident: existing, adoptId: adoptAgentId, slot: existing.slot ?? residents.allocateSlot(list) },
        ],
        error: null,
      }
    }
    return {
      targets: [{ resident: null, adoptId: adoptAgentId, slot: residents.allocateSlot(list) }],
      error: null,
    }
  }
  if (list.length === 0) {
    return { targets: [{ resident: null, adoptId: null, slot: 1 }], error: null }
  }
  return {
    // 半成品（有目录、没安家）**不进遍历**：裸 `up` 的语义是"点亮这个家"，它不该替用户决定
    // 把一支半成品认领进世界 —— 那是 `up --agent <id>` 的事（`skipped` 由调用方如实打出来）。
    targets: list
      .filter((resident) => Boolean(resident.slot) && fs.existsSync(path.join(resident.envDir, 'identity.json')))
      .map((resident) => ({ resident, adoptId: null, slot: resident.slot ?? null })),
    error: null,
  }
}

function readText(file) {
  try {
    return fs.readFileSync(file, 'utf8')
  } catch {
    return ''
  }
}

/** 日志里有没有以这个标记**起头**的一行（`[listening]` / `[exit]`，容错 CRLF）。 */
function logHasLine(text, tag) {
  return text.split('\n').some((line) => line.replace(/\r$/, '').startsWith(tag))
}

/** **本家**的宿主进程 pid（种类 ＋ 家事实，两手都过：路径对得上、`--root` 也是本家的环境根）。 */
function hostOrphans(layout, home) {
  return proc.hostOrphanPids(layout, home)
}

/**
 * Agent**现在**在不在跑？（TASK_62）
 *
 * 为什么要有这一问、且要**在布置之前**问：布置里的凭据那一步会**换钥匙**
 * （`replaces_credential_id` 撤销旧支），而跑着的 Agent 手里揣的就是旧支 ——
 * 门铃每次发送前重认证、撤销即断流 fail-closed ⇒ Agent 会**变聋**。
 *
 * 判据**复用**既有的两条，不新造：
 *   · 账本里那支（`ledgerPid` 的四态 → 里面走 `ownsPid` 的指纹，认不出就不算；**别家的**也不算）；
 *   · 账本不活但进程表里有**本家**的孤儿（路径 ＋ `--root` 都对）⇒ 视作在跑，并把 pid 文件
 *     重写指向它（这是 `home_up` 既有的"**接管**，不是重起"语义，本函数只是把它挪早）。
 *
 * ⚠ 家事实（本刀）：两条判据都只认**本家**的 —— 同一份发布包服务多个家时，别家的宿主
 * 路径一模一样，光看路径会把"别家在跑"读成"Agent 在跑"（实测：本家反而起不来）。
 *
 * 返回 `{ running, pid, adopted, foreign }`：`running` 为真时 `pid` 非空；`adopted` 只影响读数
 * 措辞；`foreign` 是"账本里那支不是本家的"那个 pid（**没有就是 null**）——它不影响
 * `running`（别家在跑 ≠ Agent 在跑），只让读数把这件事说出来（否则用户看着 pid 文件里有个数，
 * 却读不到"为什么本家还是起了一支新的"）。
 * ⚠ 这是一次**快照**：真正动手（换钥匙）与它相隔几秒。中间 Agent 若正好死了，最坏结果是
 * "少换了一次钥匙"——没有副作用，下一次 up（Agent 不在跑）会照 TASK_59 的语义补上。
 */
function agentRunningNow({
  hostPidFile,
  layout,
  home,
  // 三件可注入的依赖（默认就是线上那三件）—— 照 `status-server.js` 的 `readAgentStatus` 先例，
  // 让"账本活着 / 孤儿接管 / 都不在 / 账本记的是别家的"这几支**能被机械判错**（第三支带写 pid 文件的副作用）。
  ledgerPid = proc.ledgerPid,
  orphansOf = hostOrphans,
  writePid = proc.writePid,
}) {
  const ledger = ledgerPid('agent', hostPidFile, layout, home)
  const foreign = ledger.state === 'not-ours' ? ledger.pid : null
  if (ledger.state === 'ours' || ledger.state === 'unverifiable') {
    return { running: true, pid: ledger.pid, adopted: false, foreign: null }
  }
  const orphan = orphansOf(layout, home)[0]
  if (orphan) {
    writePid(hostPidFile, orphan)
    return { running: true, pid: orphan, adopted: true, foreign }
  }
  return { running: false, pid: null, adopted: false, foreign }
}

/** `npm run build`：日志直接落 client.out（与 bash 版同一条读数）。 */function buildClient(layout, clientLog) {
  const { spawnSync } = require('node:child_process')
  const fd = fs.openSync(clientLog, 'a')
  try {
    return spawnSync('npm', ['run', 'build'], {
      cwd: layout.clientDir,
      // shell 只在**这一处**需要：Windows 上 `npm` 是个 .cmd 垫片，Node 不许直接 exec
      // .cmd/.bat（安全策略）。命令行是**常量**、没有任何外部输入 ⇒ 没有注入面。
      shell: proc.IS_WINDOWS,
      stdio: ['ignore', fd, fd],
      windowsHide: true,
    })
  } finally {
    fs.closeSync(fd)
  }
}

async function startClient({
  layout,
  homeDir,
  logDir,
  clientPort,
  listen,
  clientPidFile,
  statusPort,
  configPort,
  home,
}) {
  const clientLog = path.join(logDir, 'client.out')
  const clientLedgerPid = proc.trustedLedgerPid('client', clientPidFile, layout, home)
  if (clientLedgerPid !== null) {
    console.log(`[client] 已经在跑（pid ${clientLedgerPid}）`)
    return
  }
  if (!/^\d+$/.test(clientPort)) {
    // 端口是从 home.conf 读的（主人可编辑）⇒ 交给子进程之前必须验形状。
    console.log(`[client] home.conf 里的 client_port 读不懂（${clientPort}）—— 不动它，也不算失败`)
    return
  }
  const clientListener = proc.listenerOf('client', layout, home)
  if (clientListener !== null) {
    // 同上：端口上那支就是我们的（可能是 npm/vite 那层壳留下的账本差），对上真实就够。
    proc.writePid(clientPidFile, clientListener)
    console.log(`[client] 已经在跑（pid ${clientListener}；pid 文件里记的那支已经退场——账本已对上真实）`)
    return
  }
  const nodeExe = process.execPath

  if (layout.kind === 'release') {
    const serveScript = path.join(layout.repo, 'scripts', 'serve_client.mjs')
    if (!P.isFile(serveScript)) {
      console.log('[client] 缺 scripts/serve_client.mjs（发布包里应当自带）')
      return
    }
    const started = proc.spawnDetached(
      nodeExe,
      clientServeArgs({ layout, clientPort, listen, statusPort, configPort, homeDir }),
      { cwd: homeDir, env: { ...process.env }, logFile: clientLog },
    )
    proc.writePid(clientPidFile, started)
    const ready = await proc.waitHttp(
      [`http://localhost:${clientPort}/`, `http://127.0.0.1:${clientPort}/`],
      60,
    )
    if (ready.ok) {
      console.log(`[client] 生产页 http://localhost:${clientPort} （pid ${started}，日志 logs/client.out）`)
    } else {
      console.log('[client] 没起来（不影响 Agent 在线）：手动 —— node scripts/serve_client.mjs（在解压根下敲；日志 logs/client.out）')
      for (const line of tailLines(clientLog, 4)) console.log(`       ${line}`)
    }
    return
  }

/** 源码形态：**也用发布形态那条静态托管**，vite 只当**构建器**（见文件尾的迁移说明）。 */
  //
  // 为什么不再用 `vite preview`（2026-10-01 第六刀，两条真机读数 + 一条既有代价）：
  //   · **内存**：同一份 `client/dist`，vite preview 实测 **122.7 MB**，`serve_client.mjs` **60.3 MB**
  //     （差的是一整套 vite 的模块图 —— 它按"开发服务器"装配，即使只做 preview）；
  //   · **代理只有一处实现**：`up` 从前在两种形态下走两套代转（vite 的 proxy 与 `serve_client.mjs`
  //     的路由表）——同一件事两份实现，历史上就漏改过一边（TASK_77 的 `/home-binding` 只在一边补上）；
  //   · **家事实**：vite 是第三方 CLI，收不了我们的 `--home` ⇒ 源码形态那一支的认领判据只能退回
  //     "只看端口"（`proc.js` 的 `homeFlagVerdict`）。换成自己的托管之后，两种形态**逐字同款**。
  //
  // **一键**（本刀）：缺依赖 ⇒ 自己装；缺产物 ⇒ 自己构建 —— 进度与失败如实报
  // （失败**不影响 Agent 在线**，也**不改退出码**：沿用既有判据）。非 Windows 专属：
  // Windows 上「谁准备前端」= 用户（红线：那边行为逐字保留，见 `clientSetupOwner`）。
  const viteScript = path.join(layout.clientDir, 'node_modules', 'vite', 'bin', 'vite.js')
  const serveScript = path.join(layout.repo, 'scripts', 'serve_client.mjs')
  const distIndex = path.join(layout.clientDist, 'index.html')
  const owner = clientSetupOwner()
  if (!P.isFile(serveScript)) {
    // 托管那一件是本仓库自带的（源码形态与发布形态都在 `scripts/` 下）—— 缺它说明这份检出坏了，
    // 如实说一句，别让后面那句 spawn 失败变成"页面没起来"这种含糊读数。
    console.log('[client] 缺 scripts/serve_client.mjs —— 页面起不来（不影响 Agent 在线）')
    return
  }
  if (!P.isFile(viteScript) && owner === 'user') {
    console.log(`[client] 这个 client 还没装依赖（缺 ${path.relative(layout.repo, viteScript)}）`)
    console.log(`[client] 手动： cd client && npm install && npm run build && node ../scripts/serve_client.mjs --dist client/dist --port ${clientPort}`)
    return
  }
  const setupSteps = clientSetupSteps({ hasDeps: P.isFile(viteScript), hasDist: P.isFile(distIndex) })
  for (const step of setupSteps) {
    if (step === 'install') {
      console.log('[client] 这个 client 还没装依赖 —— 这次开机自己装（npm install，要联网，可能几分钟；日志 logs/client.out）')
      const done = await runClientStep({
        step,
        args: ['install'],
        cwd: layout.clientDir,
        logFile: clientLog,
        timeoutMs: CLIENT_INSTALL_TIMEOUT_MS,
      })
      if (!done.ok) {
        for (const line of clientStepFailureLines(step, done, clientLog, clientPort)) console.log(line)
        return
      }
      if (!P.isFile(viteScript)) {
        console.log('[client] 依赖这一步说成了，但 vite 还是缺 —— 看 logs/client.out（这一步没成，不影响 Agent 在线）')
        return
      }
      console.log(`[client] 依赖装好了（用了 ${formatDuration(done.elapsedMs)}）`)
      continue
    }
    // step === 'build'
    if (owner === 'user') {
      // Windows：与从前**逐字相同**的构建路径（spawnSync，静默）。
      console.log(`[client] 没有生产构建（${layout.clientDist}）——先构建一次`)
      if (buildClient(layout, clientLog).status !== 0) {
        console.log('[client] 构建失败（不影响 Agent 在线）—— logs/client.out 的尾巴：')
        for (const line of tailLines(clientLog, 5)) console.log(line)
        console.log(`[client] 手动： cd client && npm install && npm run build && node ../scripts/serve_client.mjs --dist client/dist --port ${clientPort}`)
        return
      }
      continue
    }
    console.log('[client] 还没有生产构建 —— 这次开机自己构建（npm run build；日志 logs/client.out）')
    const done = await runClientStep({
      step,
      args: ['run', 'build'],
      cwd: layout.clientDir,
      logFile: clientLog,
      timeoutMs: CLIENT_BUILD_TIMEOUT_MS,
    })
    if (!done.ok) {
      for (const line of clientStepFailureLines(step, done, clientLog, clientPort)) console.log(line)
      return
    }
    console.log(`[client] 构建好了（用了 ${formatDuration(done.elapsedMs)}）`)
  }
  const started = proc.spawnDetached(
    nodeExe,
    // **两种形态同一条路**（2026-10-01 第六刀）：同一个静态托管、同一张代转路由表、
    // 同一个 `--home` 家事实。`clientDist` 由形态决定（源码 `client/dist` / 发布 `client-dist`）。
    clientServeArgs({ layout, clientPort, listen, statusPort, configPort, homeDir }),
    {
      cwd: homeDir,
      env: { ...process.env },
      logFile: clientLog,
    },
  )
  proc.writePid(clientPidFile, started)
  const ready = await proc.waitHttp(
    [`http://localhost:${clientPort}/`, `http://127.0.0.1:${clientPort}/`],
    60,
  )
  if (ready.ok) {
    console.log(`[client] 生产页 http://localhost:${clientPort} （pid ${started}，日志 logs/client.out）`)
  } else {
    console.log(
      `[client] 没起来（不影响 Agent 在线）：手动 node scripts/serve_client.mjs --dist ${path.relative(layout.repo, layout.clientDist)} --port ${clientPort}（在仓库根下敲；日志 logs/client.out）`,
    )
  }
}

// ---------------------------------------------------------------------------
// **两个门的就绪判据**（2026-09-30）：回执必须是本门的，且起的那支进程还活着
// ---------------------------------------------------------------------------

/**
 * **状态口的就绪判据**：回执必须**是它自己的** —— 200 ＋ `/healthz` 的 `{ok:true}` 标记。
 *
 * 为什么不能只看"端口上有回执"：两个门撞同一个端口时，后起的门绑定失败退出（它的日志里
 * 是诚实的 EADDRINUSE），而另一个门会把 `/healthz` 答成 404 —— 只看"服务器在听"就会把
 * **别的门**当成本门起来了（真机实测：`up` 打印过与事实不符的门读数）。
 */
function statusDoorReady(probe) {
  if (!probe || probe.code !== '200') return false
  try {
    const body = JSON.parse(probe.body)
    return Boolean(body) && body.ok === true
  } catch {
    return false
  }
}

/** **模型配置门的就绪判据**：回执必须是三合一（清单 + 绑定 + 派生）那一份 —— `state` 与 `readAt` 都在。 */
function configDoorReady(probe) {
  if (!probe || probe.code !== '200') return false
  try {
    const body = JSON.parse(probe.body)
    return Boolean(body) && typeof body.state === 'string' && typeof body.readAt === 'string'
  } catch {
    return false
  }
}

/**
 * 门没起来时的**原因读数**（纯函数 ⇒ 三支都可判错）：
 * 「它退了」「没答 / 没连上」「端口上答话的不是它」是三种不同的事实，不许混成一句。
 */
function doorProbeFailureWhy({ alive, remote, code }) {
  if (!alive) return '它退了（多半是端口被占）'
  if (remote) return '20 秒没连上'
  if (code === '000') return '20 秒没答'
  return `端口上答话的不是它（HTTP ${code}）`
}

/**
 * 起**家的门**（2026-10-01 第六刀）：只读状态口（读端面）＋ 模型配置门（写端面），
 * **一个进程、两个端面**。照 `startClient` 的先例：**起不来不算失败**（Agent 在线优先）。
 *
 * 为什么合成一个进程（主人裁："别被以前的文档束缚，要合理"）：两个门服务**同一个读者**
 * （本机页面，走同源代转）、**同一个寿命**（页面在不在跑）、**同一句恢复话**（重开一次 `up`）。
 * 分开只买到"一个崩了另一个还在"，而用户能做的还是同一件事 —— 代价却是每个家多一个 Node
 * 进程（实测 ≈56 MB）＋ **一整套重复的**启动 / 探活 / 账本 / 日志 / 关停 / 归属判据
 * （`proc.js` 里那两条互相提防"认错进程会去杀错人"的跨词短语就是这套重复的化石）。
 *
 * **端面一条不合并**：两个端口、两张逐条列出的路由表、两套准入（读端面可绑远程 + 家钥匙，
 * 写端面默认只本机）。⇒ 合并的是"谁在听"，不是"谁能被谁访问"。
 *
 * 三件它不碰的东西（沿用两个门各自的纪律）：
 *  - 环境：`statusServer.serveEnv`（白名单 ＋ `CELIRA_HOME_DIR`）⇒ 家的凭据一个字节都不进去；
 *  - 端口：读 `home.conf` 的 `status_port` / `config_port`（默认 5190 / 5191），形状不对就不动它、
 *    也不算失败；
 *  - 身份：进程只有一支，**端面各有各的准入**（`home-key.js` 的 `resolveLaunch` 分别判）。
 *
 * **旧形状迁移**（2026-10-01 之前是两个门各一个进程）：老家还在跑旧门时，新的一支会撞端口 ⇒
 * 先认出来、如实说、收掉它们再起。认不出的**不杀**（只清账本），与 `down` 同一条纪律。
 */
async function startDoors({
  layout,
  homeDir,
  logDir,
  doorsPidFile,
  legacyPidFiles = [],
  statusPort,
  statusBind,
  configPort,
  configBind,
  home,
}) {
  const doorsLog = path.join(logDir, 'doors.out')
  const manual = 'node home.js doors'

  // ⓪ 旧形状的门：先收（认得出才收 —— proc.js 那两条旧指纹就是为这一步留的）
  for (const legacy of legacyPidFiles) {
    const ledger = proc.ledgerPid(legacy.target, legacy.file, layout, home)
    if (ledger.state === 'gone') {
      proc.clearPid(legacy.file)
      continue
    }
    if (ledger.state === 'not-ours') {
      console.log(
        `[doors] 旧账本 ${path.basename(legacy.file)} 里的 pid ${ledger.pid} 不是我们的进程——不杀，只清账本`,
      )
      proc.clearPid(legacy.file)
      continue
    }
    console.log(
      `[doors] 旧形状的门还在跑（${legacy.label} pid ${ledger.pid}）—— 收掉它，换成"一个进程两个端面"那一支`,
    )
    await proc.killPid(ledger.pid, { force: true })
    await proc.sleep(300)
    proc.clearPid(legacy.file)
  }

  const ledger = proc.trustedLedgerPid('doors', doorsPidFile, layout, home)
  if (ledger !== null) {
    console.log(`[doors] 已经在跑（pid ${ledger}）`)
    return
  }
  if (!/^\d+$/.test(statusPort) || !/^\d+$/.test(configPort)) {
    // 端口是从 home.conf 读的（主人可编辑）⇒ 交给子进程之前必须验形状（同 client）。
    console.log(
      `[doors] home.conf 里的端口读不懂（status_port=${statusPort} / config_port=${configPort}）—— 不动它，也不算失败`,
    )
    return
  }
  const doorsListener = proc.listenerOf('doors', layout, home)
  if (doorsListener !== null) {
    proc.writePid(doorsPidFile, doorsListener)
    console.log(`[doors] 已经在跑（pid ${doorsListener}；pid 文件里记的那支已经退场——账本已对上真实）`)
    return
  }
  const homeScript = path.join(layout.repo, 'home.js')
  if (!P.isFile(homeScript)) {
    console.log(`[doors] 缺 ${path.relative(layout.repo, homeScript)}——家的门起不来（不影响 Agent 在线）`)
    return
  }
  const started = proc.spawnDetached(process.execPath, [homeScript, 'doors', '--home', homeDir], {
    cwd: homeDir,
    env: statusServer.serveEnv(process.env, homeDir),
    logFile: doorsLog,
  })
  proc.writePid(doorsPidFile, started)

  // **两个端面各探各的**（回执要对得上）：模式一认**本端面自己的标记** —— 另一个端面在应门
  // （404 / 别的 JSON）不算它起来了；模式二在 TLS 上、还要钥匙 ⇒ TCP 连一下。
  const statusRemote = !homeKey.isLoopbackBind(statusBind)
  const configRemote = !homeKey.isLoopbackBind(configBind)
  const statusReady = statusRemote
    ? await proc.waitTcp(statusBind, Number(statusPort), 20)
    : await proc.waitHttp([`http://127.0.0.1:${statusPort}/healthz`], 20, 500, { match: statusDoorReady })
  const configReady = configRemote
    ? await proc.waitTcp(configBind, Number(configPort), 20)
    : await proc.waitHttp([`http://127.0.0.1:${configPort}/home-config`], 20, 500, { match: configDoorReady })

  // **记下的那个进程必须还活着**：端口上有人答话、而我们起的那支已经退了 ⇒ 那是**别人的**
  // 回执，读数不许照抄成"这个门起来了"（与 im 那一支同一条纪律）。
  const alive = proc.isAlive(started)
  const statusWhere = `${statusRemote ? 'https' : 'http'}://${statusRemote ? statusBind : '127.0.0.1'}:${statusPort}/status`
  const configWhere = `${configRemote ? 'https' : 'http'}://${configRemote ? configBind : '127.0.0.1'}:${configPort}/home-config`

  if (statusReady.ok && alive) {
    console.log(
      `[status] 只读状态口 ${statusWhere} （与模型配置门**同一个进程**：pid ${started}，日志 logs/doors.out${statusRemote ? '；远程模式：要家钥匙' : ''}）`,
    )
  } else {
    const why = doorProbeFailureWhy({ alive, remote: statusRemote, code: statusReady.code })
    console.log(`[status] 没起来（不影响 Agent 在线）：${why} —— 手动 —— ${manual}（日志 logs/doors.out）`)
  }
  if (configReady.ok && alive) {
    console.log(`[config] 模型配置门 ${configWhere} （同一个进程：pid ${started}）`)
  } else {
    const why = doorProbeFailureWhy({ alive, remote: configRemote, code: configReady.code })
    console.log(`[config] 没起来（不影响 Agent 在线）：${why} —— 手动 —— ${manual}（日志 logs/doors.out）`)
  }
  if (!alive || (!statusReady.ok && !configReady.ok)) {
    for (const line of tailLines(doorsLog, 4)) console.log(`       ${line}`)
  }
}

module.exports = {
  up,
  parseUpArgs,
  planUpTargets,
  ensureHomeDirs,
  buildHostArgs,
  terminalNotice,
  migrateTerminalDefault,
  TERMINAL_DEFAULT_KEY,
  TERMINAL_DEFAULT_MIGRATED,
  mcpNotice,
  handNotices,
  explainRuntimeOnly,
  releaseClientArgs,
  clientServeArgs,
  hostOrphans,
  agentRunningNow,
  imListenerVerdict,
  modelEnvironment,
  appendNoProxy,
  hostEnvironment,
  MODEL_ENV_KEYS,
  // **源码形态的一键前端**（2026-09-30）：平台缝、缺什么补什么、进度与失败读数。
  clientSetupOwner,
  clientSetupSteps,
  runClientStep,
  clientStepFailureLines,
  formatDuration,
  CLIENT_INSTALL_TIMEOUT_MS,
  CLIENT_BUILD_TIMEOUT_MS,
  CLIENT_PROGRESS_INTERVAL_MS,
  // **两个门的就绪判据**（2026-09-30）：回执必须是本门的；失败三态分开说。
  statusDoorReady,
  configDoorReady,
  doorProbeFailureWhy,
}
