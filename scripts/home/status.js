'use strict'
/**
 * `node home.js status` —— 家的**状态读数**（移植自已退役的 bash 版 `scripts/home_status.sh`）。
 *
 * 四节：im / **住户（一支一行）** / client / 装配面（MCP 对账）+ 最后一节直接把
 * `agent_host_status` 的读数搬过来——**那是 Agent 处境与"下一步该敲哪条命令"的唯一权威**，
 * 本文件不另写一份判断。
 *
 * ## 住户化之后（2026-10-01 多支刀，承 PROJECT_SHAPE §8.0）
 *
 * **起停单位是 Agent** ⇒ 读数也按支：一个家住 N 支时，每支一行——
 * 身份（`agent_id`）、房（值班室）、宿主在不在、**正忙否**、凭据在不在、模型绑定在不在。
 * 常驻件（im / client / 两个门）是**家级**的（一个家一份），照旧一节。
 * 单支的家与从前逐字相同（同一批 `[agent]` / `[state]` 行，没有多余的抬头）。
 *
 * 退出码见 `scripts/HOME_MANUAL.md`。（实测登记：bash 版在"少了东西"时实际返回的是 **1**，
 * 而手册那张表写的是 3 —— 本文件**照实现移植**（返回 1），差异登记在报告里等人裁。）
 */

const fs = require('node:fs')
const path = require('node:path')

const P = require('./paths')
const { confGet, mcpNames, portOf } = require('./conf')
const { readUserEnv } = require('./userenv')
const credentials = require('./credentials')
const { BACKEND_ENV } = require('./provision')
const modelSources = require('./model-sources')
const proc = require('./proc')
const residents = require('./residents')
const homeKey = require('./home-key')

function tailLines(file, count) {
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch {
    return []
  }
  const lines = text.split('\n').map((line) => line.replace(/\r$/, ''))
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines.slice(-count)
}

function linesOf(file) {
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch {
    return []
  }
  return text.split('\n').map((line) => line.replace(/\r$/, ''))
}

/**
 * 这一段开机以来跑过**几段在场**（宿主每跑完一段打一行 `[run]`）。
 *
 * ⚠ **2026-10-10 在场那一刀改**：从前这一格数的是 `[wake]`——一次唤醒一行。而今天
 * **一段在场会吸收后来的每一声音铃**（同一段被反复驱动），所以"唤醒了多少次"与"主体醒过几段"
 * 不再是同一个数。读数该说的是后者：`[run]` 一行 = 一段。
 */
function countPresences(file) {
  return linesOf(file).filter((line) => line.startsWith('[run]')).length
}

/**
 * **一支住户**的处境读数（不打印，只收集）——`status` 的"一支一行"与下面的 `[agent]` 细读
 * 共用同一份事实（一个命令取一次快照）。
 *
 * `stateText` 是 `agent_host_status` 的原样输出（**唯一权威**：准入三节与"下一步敲哪条命令"
 * 都在它里面）；`alreadyOpen` = 它这一轮正跑着一次活动（环境同时只有一个写者）。
 */
function readResidentFacts({ layout, homeDir, worldBase, resident }) {
  const scoped = { homeDir, envDirs: [resident.envDir] }
  const hostPidFile = residents.residentPidFile(homeDir, resident)
  const hostLog = residents.residentHostLog(homeDir, resident)
  const facts = {
    resident,
    hostLog,
    hostLogLabel: path.relative(homeDir, hostLog) || hostLog,
    hostPid: null,
    orphan: null,
    presences: countPresences(hostLog),
    exited: linesOf(hostLog).some((line) => line.startsWith('[exit]')),
    credential: 'missing',
    binding: 'absent',
    bindingSource: null,
    // **这一支的手**（2026-10-01 第五刀）：外部 server ＋ 终端档（来源也一起收着，
    // 读数要说清"跟家走还是它自己的"）。
    hands: residents.residentHands({ homeDir, resident, homeConfFile: path.join(homeDir, 'home.conf') }),
    stateText: '',
    alreadyOpen: false,
  }
  facts.hostPid = proc.trustedLedgerPid('agent', hostPidFile, layout, scoped)
  if (facts.hostPid === null) facts.orphan = proc.hostOrphanPids(layout, scoped)[0] ?? null
  const resolved = credentials.resolve(
    { world: worldBase, agentId: resident.agentId },
    { fallback: readUserEnv(BACKEND_ENV) },
  )
  facts.credential = resolved.state === 'present' ? 'present' : resolved.state
  const binding = modelSources.readBinding(resident.envDir)
  facts.binding = binding.state
  facts.bindingSource = binding.source
  if (P.isFile(layout.statusExe)) {
    const done = require('node:child_process').spawnSync(
      layout.statusExe,
      [resident.envDir, '--agent', resident.agentId],
      { encoding: 'utf8', windowsHide: true },
    )
    facts.stateText = `${done.stdout ?? ''}${done.stderr ?? ''}`
    facts.alreadyOpen = facts.stateText.includes('already-open')
    facts.stateCode = done.status ?? 0
  }
  return facts
}

/** 凭据那一格的读数（**只给形状，不给值**）。 */
function credentialLabel(state) {
  if (state === 'present') return '在场'
  if (state === 'unreadable') return '读不出来'
  return '没有'
}

/**
 * **外部 server 的逐支对账**（纯函数 ⇒ 可机械判错；2026-10-01 第五刀）。
 *
 * 判据：手是**支级事实** ⇒ "配置声明了什么"与"在跑那支装了什么"必须**按支**摆在一起读。
 * 拿家级 `home.conf` 的声明去和某一支 argv 对账，会把**有意的覆盖**（支级 `mcp_*` / `mcp=none`）
 * 报成"不一致"——那正是真机验收当场抓到的假警报。
 *
 * 只报**名字与数量**（命令是 argv 的事）；**渐进默认不可见**（这一支该有 0 台、在跑那支也 0 台 ⇒
 * 这一支整行不出现）。多支的家**点名是哪一支**。
 *
 * @param {{residentList: Array, confNames: string[], rowOfResident: Function, namesOfRow: Function, hostCount: number, show: Function}} ctx
 * @returns {string[]} 要打的读数行
 */
function mcpReconcileLines({ residentList, confNames = [], rowOfResident, namesOfRow, hostCount = 0, show = (v) => v || '无' }) {
  const lines = []
  for (const resident of residentList) {
    const expected = resident.hands?.mcp?.map((declaration) => declaration.split('=', 1)[0]) ?? []
    const row = rowOfResident(resident)
    const actual = namesOfRow(row)
    const who = residentList.length > 1 ? `住户 ${resident.agentId}：` : ''
    if (!row) {
      if (expected.length > 0) {
        lines.push(`[mcp] ${who}这一支该装 ${expected.length} 台：${show(expected.join(','))}（没在跑，无从对账）`)
      }
      continue
    }
    const onlyExpected = expected.filter((name) => !actual.includes(name))
    const onlyActual = actual.filter((name) => !expected.includes(name))
    if (onlyExpected.length === 0 && onlyActual.length === 0) {
      if (expected.length > 0) {
        lines.push(`[mcp] ${who}装了 ${expected.length} 台：${show(expected.join(','))}（在跑的那支与这一支该有的手一致）`)
      }
      continue
    }
    lines.push(
      `[mcp] ${who}这一支该装 ${expected.length} 台：${show(expected.join(','))}；` +
        `在跑那支装的是 ${actual.length} 台：${show(actual.join(','))}——**不一致**`,
    )
    lines.push(`      只在"这一支该有的手"里：${show(onlyExpected.join(','))}；只在在跑那支里：${show(onlyActual.join(','))}`)
    lines.push('      （装配面是起 Agent 那一刻的事实：换面要先 node home.js down + up）')
  }
  // 一支住户都没有的家（还没安家）：只剩"家级默认"这一格可读 —— 如实说，不假装有支可对账。
  if (residentList.length === 0 && confNames.length > 0) {
    lines.push(`[mcp] 家级默认 ${confNames.length} 台：${show(confNames.join(','))}（还没有住户，无从对账）`)
  }
  if (hostCount > residentList.length) {
    lines.push(
      `      ⚠ 读到 ${hostCount} 支宿主，而家里只有 ${residentList.length} 个住户——异常读数（TASK_40 的接管只该每支一支）`,
    )
  }
  return lines
}

/** 模型绑定那一格的读数。 */
function bindingLabel(facts) {
  if (facts.binding === 'bound') return `在场（${facts.bindingSource}）`
  if (facts.binding === 'unreadable') return '读不出来'
  return '没有'
}

/** 手（外部 server）那一格的读数：**数量 ＋ 来源**（跟家走 / 它自己的 / 明确不要）。 */
function handsLabel(facts) {
  const hands = facts.hands
  if (!hands) return '读不出来'
  if (hands.mcpSource === 'none') return '0 台（这一支不要外部 server）'
  if (hands.mcp.length === 0) return '0 台'
  const from = hands.mcpSource === 'resident' ? '它自己的 agent.conf' : '跟家走'
  return `${hands.mcp.length} 台（${from}）`
}

/** 终端那一格的读数：有 / 没有 ＋ 来源（平台默认 / 它自己的 / 家级）。 */
function terminalLabel(facts) {
  const hands = facts.hands
  if (!hands) return '读不出来'
  const resolved = residents.resolveTerminal(hands, { isWindows: proc.IS_WINDOWS, isLinux: proc.IS_LINUX })
  if (resolved.unavailable) return '声明了但**这台机器上没有终端臂**（这次不带）'
  if (resolved.mode === 'none') {
    if (resolved.source === 'resident') return '没有（它自己的声明）'
    if (resolved.source === 'home') return '没有（家级声明）'
    // ⚠ **2026-10-14 起平台默认就是"不给手"**：这一格从前写的是"在场（平台默认）"——
    // 现在没人声明过的时候**没有**这只手，要手得显式声明（读数不许让用户以为它是默认给的）。
    return '没有（平台默认；要手就在它自己的 agent.conf 里写 terminal=full）'
  }
  // ⚠ `source === 'platform'` 这一档**到不了这里**了：默认不给手 ⇒ `mode === 'full'` 只可能是
  // 显式声明出来的（支级或家级）。这里因此不再写"在场（平台默认）"——那会是一句已经没有来路的话。
  return '在场'
}

/** 宿主那一格的读数。 */
function hostLabel(facts) {
  if (facts.hostPid !== null) return `在等唤醒（pid ${facts.hostPid}）`
  if (facts.orphan !== null) return `在线（pid ${facts.orphan}；账本没指向它）`
  return '没在跑'
}

async function status() {
  const homeDir = P.homeDir()
  const layout = P.detectLayout()
  const confFile = path.join(homeDir, 'home.conf')
  const logDir = path.join(homeDir, 'logs')

  if (!fs.existsSync(homeDir)) {
    console.log(`[home_status] 没有这个家: ${homeDir}（开机： node home.js up）`)
    return 2
  }

  const listen = confGet(confFile, 'listen_addr', P.DEFAULT_LISTEN)
  const clientPort = confGet(confFile, 'client_port', P.DEFAULT_CLIENT_PORT)
  const imPort = portOf(listen)
  // 只读状态口与模型配置门的端口（读数时要用它们的**家事实**：那两支命令行里没有家，
  // 判据落在"它在听哪个端口"上——见 proc.js 的「家事实」段）。
  const statusPort = confGet(confFile, 'status_port', P.DEFAULT_STATUS_PORT)
  const configPort = confGet(confFile, 'config_port', P.DEFAULT_CONFIG_PORT)
  // **住户表**（只读路径：`status` 不做形状迁移 —— 那是 `up` 的事；还没迁的老家那一支
  // 照旧列出来读，它的账本在 `logs/host.pid`）。
  const residentList = residents.runtimeResidents(homeDir, confFile)
  // **Agent 要连的那个世界**：记着 `im_endpoint` ⇒ 就是那个世界；否则本机这个口。
  const imEndpoint = (confGet(confFile, 'im_endpoint', '') || '').trim()
  const worldBase = imEndpoint || `http://${listen}`
  // **家事实**（本刀）：认领判据的第二维；一个命令取一次快照（netstat）。
  // `homeDir` 是 2026-09-25 加的第二维：`up` 起的常驻件命令行里带 `--home <家目录>`，
  // 判据要拿本家的那一份去比（缺了它 `ownsPid` 会**炸**——不许静默退回只看端口）。
  // ⚠ **agent（宿主）那一支**要的是**一支一个环境根**那一组（`envDirs`）——住户化之后
  // 一支一个进程，逐支用各自的环境根判（`readResidentFacts`）。
  const home = {
    homeDir,
    envDirs: residentList.map((resident) => resident.envDir),
    clientPort,
    imPort,
    statusPort,
    configPort,
    // **家的门**（2026-10-01 第六刀）：一个进程两个端面（读口 ＋ 写口）。
    doorsPorts: [statusPort, configPort],
    listening: proc.listenerSnapshot(),
  }

  console.log(`[home] ${homeDir}（家是持久的：关机不动里面的东西）`)
  let missing = 0

  // ⚠ **两件事分开**（2026-10-11）：从前这一支把"没有 `home.conf`"与"一个住户都没有"
  //   合成一句「家里还没有布置过」并**提前 return** ✗ —— 于是**一个已经有世界的家**，
  //   连"世界在不在跑、门在不在听"都读不到（**人不许比系统知道得更少**），
  //   而且给的下一步（`up`）也不对。
  //   现在：没有 `home.conf` ⇒ 真的还没布置过（退出码 2，与 `HOME_MANUAL.md` 那一列一致）；
  //   有 `home.conf`、只是还没有住户 ⇒ **照旧把 im / 门 / client / 装配面读完**，
  //   只在住户那一节如实说"一支都没有"；退出码照常由"该亮的东西亮没亮"算
  //   （手册：`0` = 三个都在 ＋ 每一支都在 —— 一支都没有时后半句空着成立）。
  if (!fs.existsSync(confFile)) {
    console.log('[状态] 家里还没有布置过（没有 home.conf）—— 开机： node home.js up')
    return 2
  }

  // —— im 服务端 ——
  //
  // **远端跑法**（2026-09-30）：`home.conf` 记着 `im_endpoint` ⇒ 这个世界不在这台机器上，
  // 本机不起 core。读数改成"探一下那个世界"，绝不照本机的方式报"im 没在跑"（那是误导：
  // 世界可能好好的，只是不在本机）。探不到 ⇒ 如实说"连不上"，并计入 missing（Agent 会聋）。
  if (imEndpoint) {
    const code = await proc.httpCode(`${imEndpoint}/health`)
    if (code === '200') {
      console.log(`[im] 远端世界 ${imEndpoint}：活着（本机不起 core）`)
    } else {
      console.log(`[im] 远端世界 ${imEndpoint}：连不上（http_code=${code}）—— 本机不起 core；Agent 这时候收不到门铃`)
      missing = 1
    }
  } else {
    const code = await proc.httpCode(`http://${listen}/health`)
    // 账本要**验明正身**（活着 + 身份与**本家**对上）才算"我们的"：见 proc.ledgerPid 的四态。
    const imPid = proc.trustedLedgerPid('im', path.join(logDir, 'im.pid'), layout, home)
    if (imPid !== null && code === '200') {
      console.log(`[im] 活着 http://${listen} （pid ${imPid}）`)
    } else if (code === '200') {
      // 账本对不上但服务端确实在听：问端口上那支是谁（跨实现过渡：.sh 记的是 MSYS pid，
      // Node 读不出来）。status 是**只读**的——这里只如实报，不修账本（修账本是 up 的接管动作）。
      const listener = proc.listenerOf('im', layout, home)
      console.log(
        listener !== null
          ? `[im] 活着 http://${listen} （pid ${listener}；pid 文件记的那支已退场——端口上那支是我们的）`
          : `[im] 活着 http://${listen} （不是本脚本起的）`,
      )
    } else {
      const last = tailLines(path.join(logDir, 'im.out'), 1)[0] ?? ''
      console.log(`[im] 没在跑（http_code=${code}）—— 日志尾： logs/im.out 最后一行是 ${last}`)
      missing = 1
    }
  }

  // —— 住户（**一支一行**：起停单位是 Agent）——
  //
  // 每支一行：身份 / 房 / 宿主在不在 / 正忙否 / 凭据在不在 / 模型绑定在不在。
  // 然后逐支细读（`[agent]` 那几行，与从前同一批读数 —— 多支时带上 id）。
  const factsList = residentList.map((resident) => readResidentFacts({ layout, homeDir, worldBase, resident }))
  const multi = factsList.length > 1
  // ⚠ **一支都没有时也要说一句**（2026-10-11）：从前这一节会**静默跳过** ⇒ 上面刚说
  //   "这个家已经布置过了"，下面却没有一行说"住户呢"（读者会以为漏了）。
  //   ⚠ 那句话**不许**说成"还没有布置过"（那是"没有 home.conf"那一档的事，退出码 2）。
  if (factsList.length === 0) {
    console.log('[住户] 一支都没有 —— 这个家已经布置过了（世界与门的读数在上面）')
    console.log('[住户] 要安一支：node home.js up --agent <AgentId>（那个 Agent 得先在世界里存在）')
  }
  for (const facts of factsList) {
    const { resident } = facts
    console.log(
      `[resident] ${resident.agentId}（住户 ${resident.slot ?? '?'}，房 ${resident.roomId || '无'}）：` +
        `宿主 ${hostLabel(facts)}；正忙 ${facts.alreadyOpen ? '是' : '否'}；` +
        `凭据 ${credentialLabel(facts.credential)}；模型绑定 ${bindingLabel(facts)}；` +
        `手 ${handsLabel(facts)}；终端 ${terminalLabel(facts)}` +
        (resident.legacy ? '；**旧形状**（还没住户化：下次 up 会迁）' : '') +
        // **半成品**（目录在、还没安家）：如实标出来，并给出"怎么安下来"的那一条命令
        // （2026-10-01 第五刀真机发现：先写 agent.conf 再 up 的住户曾在这里读成"正常的住户"）。
        (!resident.legacy && (!resident.slot || !P.isFile(path.join(resident.envDir, 'identity.json')))
          ? `；**还没安家**（要安下来：node home.js up --agent ${resident.agentId}）`
          : ''),
    )
  }
  for (const facts of factsList) {
    const { resident } = facts
    const who = multi ? ` ${resident.agentId}：` : ' '
    if (facts.hostPid !== null) {
      console.log(
        `[agent]${who}在等唤醒（pid ${facts.hostPid}，日志 ${facts.hostLogLabel}；这次开机跑过 ${facts.presences} 段在场）`,
      )
      if (facts.exited) {
        console.log(
          `[agent]${who}⚠ 但 Agent 上一次是**退出**了（${facts.hostLogLabel} 里 [exit] 那一行 + [next] 告诉我们下一步）`,
        )
        missing = 1
      }
    } else if (facts.orphan !== null) {
      // 账本没对上，但 Agent 在。**不置 missing**：Agent 在线就是在线。
      console.log(`[agent]${who}Agent 其实在线（pid ${facts.orphan}，pid 文件没指向它——托管/重起过的痕迹）`)
      console.log(
        `      本次开机以来日志里有 ${facts.presences} 段在场（[run] 行）；要修账本就 node home.js up（会接管）`,
      )
    } else {
      console.log(`[agent]${who}没在跑 —— 日志尾（${facts.hostLogLabel}）：`)
      const all = linesOf(facts.hostLog)
      const at = all.findIndex((line) => line.startsWith('[state]'))
      const tail = (at >= 0 ? all.slice(at) : all).slice(-8)
      for (const line of tail) console.log(`        ${line}`)
      missing = 1
    }
  }
  // —— 旧账本：搬家后留在老家的运行账本（2026-10-01 第七刀补读数）——
  // 账本按设计**不进环境根**（见 `paths.js` 的 `agentLogDir`）：一支 Agent 搬走之后，老家会
  // 留下 `logs/agents/<支>/`（pid 文件与宿主日志 —— 记的是**另一台机器上的进程**）。
  // `up` / `down` 都不碰它；这里**仅当**它不属于本家任何一支时提示一行。
  // 没有孤儿 ⇒ 一个字都不打（零噪音）。
  for (const orphan of residents.orphanLedgerDirs(homeDir, residentList)) {
    console.log(`[旧账本] logs/agents/${orphan}/：本家没有这一支（可能搬走了）—— 可以删`)
  }
  const agentRunning = factsList.some((facts) => facts.hostPid !== null || facts.orphan !== null) ? 1 : 0

  // —— client 入口 ——
  const clientPid = proc.trustedLedgerPid('client', path.join(logDir, 'client.pid'), layout, home)
  if (imEndpoint) {
    // 远端跑法：本机不起页面（页面与世界必须同源，由世界那台机器提供）。
    console.log('[client] 远端跑法：本机不起页面 —— 页面在世界那台机器上（与世界同源）')
  } else if (clientPid !== null) {
    console.log(`[client] 在跑 http://localhost:${clientPort} （pid ${clientPid}）`)
  } else if (proc.portInUse(clientPort)) {
    const listener = proc.listenerOf('client', layout, home)
    console.log(
      listener !== null
        ? `[client] 在跑 http://localhost:${clientPort} （pid ${listener}；pid 文件记的那支已退场——端口上那支是我们的）`
        : `[client] 端口 ${clientPort} 上有人（不是本脚本起的）——浏览器开 http://localhost:${clientPort}`,
    )
  } else {
    console.log(`[client] 没在跑（不影响 Agent 在线）：手动 cd client && npm run preview -- --port ${clientPort}`)
  }

  // —— 只读状态口（TASK_59 起 `up` 会自己起它）——
  // 与 client 同一条纪律：**报而不计入 missing**（Agent 不依赖它，它只影响运行状态页看不看得到读数）。
  // 家口远程访问刀：地址照**绑定**如实写（模式二在 TLS 上、进门要家钥匙；
  // 模式一与从前逐字相同 —— 缺省绑定就是 127.0.0.1）。
  const statusBind = (confGet(confFile, 'status_bind', homeKey.DEFAULT_BIND) || '').trim() || homeKey.DEFAULT_BIND
  const statusUrl = `${homeKey.isLoopbackBind(statusBind) ? 'http' : 'https'}://${statusBind}:${statusPort}/status`
  // **家的门**（2026-10-01 第六刀）：读端面（状态口）与写端面（模型配置门）是**同一个进程** ⇒
  // 一本账（`logs/doors.pid`）认一次，两行读数各报各的地址。
  //
  // ⚠ **逐端面认来源**（真机踩到过）：旧形状可能只跑着其中一个（比如只有状态口在跑），
  // 那时配置门那一行不许照抄"它也在跑" —— 端口是空的就是空的。三档来源：① 新形状的账本；
  // ② 账本过期但端口上那支是我们的（`listenerOf` 按 doors 指纹认：听这两个口中的任一个）；
  // ③ 旧形状对应的那本账（迁移桥）。
  const doorsLedger = proc.trustedLedgerPid('doors', path.join(logDir, 'doors.pid'), layout, home)
  const legacyStatusPid = proc.trustedLedgerPid('status', path.join(logDir, 'status.pid'), layout, home)
  const legacyConfigPid = proc.trustedLedgerPid('config', path.join(logDir, 'config.pid'), layout, home)
  const doorsListener = proc.listenerOf('doors', layout, home)

  const originFor = (legacyPid) => {
    if (doorsLedger !== null) return `pid ${doorsLedger}`
    if (doorsListener !== null) return `pid ${doorsListener}；pid 文件记的那支已退场——端口上那支是我们的`
    if (legacyPid !== null) return `旧形状的门（pid ${legacyPid}）—— 下次 up 会换成"一个进程两个端面"那一支`
    return null
  }

  // 写端面的地址（与读端面同一条口径：**按绑定如实写**，模式二在 TLS 上、进门要家钥匙）。
  const configBind = (confGet(confFile, 'config_bind', homeKey.DEFAULT_BIND) || '').trim() || homeKey.DEFAULT_BIND
  const configUrl = `${homeKey.isLoopbackBind(configBind) ? 'http' : 'https'}://${configBind}:${configPort}/home-config`

  const doors = [
    {
      label: 'status',
      url: statusUrl,
      port: statusPort,
      legacyPid: legacyStatusPid,
      absent: '[status] 没在跑 —— 运行状态页会显示「本机状态口未开」（那不是 Agent 停了）；起它： node home.js doors',
      stranger: `[status] 端口 ${statusPort} 上有人（不是本脚本起的）——运行状态页读不到就别硬看它`,
    },
    {
      label: 'config',
      url: configUrl,
      port: configPort,
      legacyPid: legacyConfigPid,
      absent: '[config] 没在跑 —— 模型配置页会存不下；起它： node home.js doors',
      stranger: `[config] 端口 ${configPort} 上有人（不是本脚本起的）——模型配置页存不下就别硬试`,
    },
  ]

  for (const door of doors) {
    const origin = originFor(door.legacyPid)
    if (origin !== null) {
      console.log(`[${door.label}] 在跑 ${door.url} （${origin}）`)
    } else if (proc.portInUse(Number(door.port))) {
      console.log(door.stranger)
    } else {
      console.log(door.absent)
    }
  }

  // 两个口是不是**同一个进程**：说一次就够（免得读者以为家里有两个常驻件）。
  if (doorsLedger !== null || doorsListener !== null) {
    console.log('[doors] 上面两个口是**同一个进程**（一个进程两个端面：读端面 /status、写端面 /home-config）')
  } else if (legacyStatusPid !== null || legacyConfigPid !== null) {
    console.log('[doors] ⚠ 这两个口现在是**旧形状**（一个门一个进程）—— 下次 `node home.js up` 会换成"一个进程两个端面"那一支')
  }

  // —— 装配面：外部来源的 App（对账，不是劝告）——
  //
  // 配置侧取 `home.conf` 的 `mcp_` 行，在跑侧取**那几支宿主的实际命令行**里的 `--mcp` 名字段。
  // 为什么值得多花一次查询：装配面是**起 Agent 那一刻**的事实，而 home.conf 随时能被编辑——
  // 只报配置就会让读者以为"改了 = 生效了"。
  // **渐进默认不可见**：两边都空时这一行整行不出现。只报名字（命令是 argv 的事）。
  // ⚠ 住户化之后"在跑的那几支"可能是**多支**（各起一个宿主）——这一节说的是**家**（配置）
  // 与**所有在跑的宿主**的对账，不按支拆（换面是家级的 `down` + `up`）。
  // —— 外部 server（MCP）的**逐支对账** ——
  //
  // 判据（2026-10-01 第五刀）：手是**支级事实**，所以"配置声明了什么"与"在跑那支装了什么"必须
  // **按支**摆在一起读 —— 拿家级 `home.conf` 的声明去和某一支 argv 对账，会把**有意的覆盖**
  // （支级 `mcp_*` / `mcp=none`）报成"不一致"。**只报名字**（命令是 argv 的事）。
  // **渐进默认不可见**：这一支该有 0 台、在跑那支也 0 台时，这一支整行不出现。
  const configNames = mcpNames(confFile)
  // ⚠ **一支住户都没有 ⇒ 不问这个问题**（2026-10-11）：认领判据对**空 `envDirs`** 是**必须炸**的
  //   （"不许静默退回只看可执行件"—— 那正是它存在的意义），而"家里一支都没有"时，
  //   "该有几支宿主"这回事根本不存在 ⇒ **空表就是答案**，不必去问一个注定要炸的问题。
  //   ⚠ 这一档以前被上面那句提前 `return` 挡着，**从没走到过** —— 一放开就当场炸出来
  //   （`家事实缺 --root 那一组（home.envDirs）`），这正是"别提前 return"要暴露的东西。
  const hostRows = residentList.length === 0 ? [] : proc.describeHostProcesses(layout, home)
  const hostCount = hostRows.length
  const show = (value) => (value ? value : '无')
  const namesOfRow = (row) => (row ? proc.mcpNamesFromCommandLine(proc.tokenSource(row)) : [])
  const rowOfResident = (resident) => {
    const byRoot = hostRows.find((row) => {
      const root = proc.flagValueFromCommandLine(proc.tokenSource(row), '--root')
      return root && path.resolve(root) === path.resolve(resident.envDir)
    })
    if (byRoot) return byRoot
    return (
      hostRows.find((row) => proc.flagValueFromCommandLine(proc.tokenSource(row), '--agent') === resident.agentId) ?? null
    )
  }

  const mcpLines = mcpReconcileLines({
    // ⚠ 对账要的是**这一支该有的手**，那是 `facts` 里读出来的（`resident.hands`）——
    //   传裸住户表进来会让"该装几台"永远读成 0（真机当场抓到的第二个假警报：把住户 1 的
    //   own_probe 报成"不一致"）。
    residentList: factsList.map((facts) => ({ ...facts.resident, hands: facts.hands })),
    confNames: configNames,
    rowOfResident,
    namesOfRow,
    hostCount,
    show,
  })
  for (const line of mcpLines) console.log(line)

  // —— Agent 的处境（**逐支**）——
  // 环境同时只有一个写者，而 Agent 正在跑一段在场的那一轮读不到（`already-open`）：先试，读到就摆出来；
  // 读不到就如实说清"Agent 这一段正忙着"，并把日常读数换成日志里的最后一条 `[run]` 行。
  console.log('')
  if (!P.isFile(layout.statusExe)) {
    console.log(`[state] 缺 ${layout.statusExe}（先构建： cd agent && cargo build --bins --release）`)
    missing = 2
  } else {
    for (const facts of factsList) {
      const { resident } = facts
      if (multi) console.log(`[state] 住户 ${resident.agentId}（${resident.envDir}）：`)
      if (facts.alreadyOpen) {
        console.log('[agent-state] Agent 这一拍正在跑一段在场（环境同时只有一个写者）⇒ 现在读不到它。')
        console.log(`            此刻看这一行（${facts.hostLogLabel} 最后一条在场读数）：`)
        const run = linesOf(facts.hostLog)
          .filter((line) => line.startsWith('run='))
          .pop()
        if (run) console.log(`              ${run}`)
        console.log('            想要完整的准入读数：等这一拍过去再敲一次本命令。')
      } else {
        const trimmed = facts.stateText.replace(/\s+$/, '')
        if (trimmed) console.log(trimmed)
        if ((facts.stateCode ?? 0) !== 0 && missing === 0) missing = facts.stateCode ?? 0
      }
    }
  }

  return missing
}

module.exports = { status, readResidentFacts, credentialLabel, bindingLabel, hostLabel, handsLabel, terminalLabel, mcpReconcileLines }
