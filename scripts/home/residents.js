'use strict'
/**
 * **住户**与**家的形状迁移**（2026-10-01 多支刀，承 PROJECT_SHAPE §8.0）。
 *
 * ## 一句话原理
 *
 * **隔离的事实单位是 Agent，不是机器**：一支 Agent = 一个环境根（`<家>/agents/<支>/`）
 * ＋ 一份支级配置（`<环境根>/agent.conf`：身份 / 值班室 / 住户序号 / 显示名）
 * ＋ 一个可分别起停的宿主进程。家（`home.conf`）只留机器级事实。
 *
 * ## 这个文件管两件事
 *
 * 1. **住户的读取**（`residents` / `runtimeResidents` / `allocateSlot` / 账本路径）——
 *    布局的唯一装配点仍是 `paths.js`；这里只负责"读出来"。
 * 2. **老家迁移**（`migrateHomeShape`）——一个家一支（环境根 `<家>/env`）的旧形状
 *    ⇒ 住户化。它**必须幂等**（重跑不重复搬、不丢文件、不改 mtime 无谓变动的文件），
 *    且**排在读身份之前**（第三刀 `provision.js` 里已有这个顺序的先例）。
 *
 * ## 老家迁移的两种身份走向（别混）
 *
 * | 老家形状 | 身份走向 | 环境怎么处理 |
 * |---|---|---|
 * | **有 `home_id`**（第三刀之后的家） | 不变：世界里那支还是它 | `<家>/env` **整份迁到** `<家>/agents/<旧 agent_id>/`（住户 1） |
 * | **没有 `home_id`**（更早的家） | **会变**：布置键按家取 ⇒ 世界里**另建一支** Agent | 旧环境**整份让位**（`env.old-<旧 agent_id>`，不删）；新那支得到干净的环境，模型绑定跟着带过去 |
 *
 * 为什么"没有 home_id"那一种必须让位：旧环境里的身份记录属于**旧那支** ——
 * 存储层打开它会以 `identity-mismatch` 拒绝（fail-closed、不泄漏经历），新那支根本跑不起来。
 * 两条纪律（照第三刀，停手不半途改）：**旧那支还在跑 ⇒ 先 down 再 up**；
 * **远端跑法没有主人凭据 ⇒ 当场停手**（本地一个字都不改）。
 *
 * ## 幂等的落点
 *
 * · 迁过一次 ⇒ `<家>/env` 不在 ⇒ 直接 `none`（不碰任何文件）；
 * · `agent.conf` **按内容比对**再写（一样就不写 ⇒ mtime 不动）；
 * · 运行账本（`logs/host.pid` / `logs/host.out`）**只在目标缺失时**搬（rename，不复制、不覆盖）；
 * · `home.conf` 那两行（`agent_id` / `room_id`）**只在真的迁成之后**才删（删不存在的键不写盘）。
 */

const fs = require('node:fs')
const path = require('node:path')

const P = require('./paths')
const { confGet, confDelete, mcpDeclarations } = require('./conf')
const proc = require('./proc')

/** 支级配置里那几行的键名（读取与渲染共用一处）。 */
const AGENT_CONF_KEYS = {
  agent: 'agent_id',
  room: 'room_id',
  slot: 'slot',
  name: 'display_name',
}

/**
 * 支级**手**声明的键名（2026-10-01 第五刀）。
 *
 * - `mcp`：这一支的外部 server 模式。唯一认的值是 `none`（"一只都不要"）；
 *   真正的声明走 `mcp_<名字>=<transport>:<命令>` 那几行（与家级同格式）。
 * - `terminal`：终端档（`full` / `none`）。**它是"这一支不要终端"的唯一说法** ——
 *   不许靠"旗标缺席"去表达（那正是"用户靠猜"的来源）。
 */
const HAND_KEYS = {
  mcp: 'mcp',
  terminal: 'terminal',
  // **技能引用**（2026-10-25）：这一支能看到技能库里的哪些一级目录。
  // 空 = 不声明 ⇒ 宿主缺省 all；不跟家级走（技能可见性是支级事实）。
  skills: 'skills',
}

/** 读 `<环境根>/identity.json` 的身份（`{ agent }`）；读不出来 ⇒ `null`（不编）。 */
function readEnvIdentity(envDir) {
  let text
  try {
    text = fs.readFileSync(path.join(envDir, 'identity.json'), 'utf8')
  } catch {
    return null
  }
  try {
    const parsed = JSON.parse(text)
    const agent = parsed && typeof parsed.agent === 'string' ? parsed.agent.trim() : ''
    return agent || null
  } catch {
    return null
  }
}

/** 住户序号（`slot`）：正整数；读不懂 ⇒ `null`（不猜一个数出来）。 */
function parseSlot(raw) {
  const value = Number.parseInt(String(raw ?? '').trim(), 10)
  return Number.isInteger(value) && value >= 1 ? value : null
}

/** 读一份 `agent.conf`（缺文件/缺键都给稳定缺省，不抛）。 */
function readAgentConf(file) {
  return {
    agentId: (confGet(file, AGENT_CONF_KEYS.agent) || '').trim(),
    roomId: (confGet(file, AGENT_CONF_KEYS.room) || '').trim(),
    slot: parseSlot(confGet(file, AGENT_CONF_KEYS.slot)),
    displayName: (confGet(file, AGENT_CONF_KEYS.name) || '').trim(),
  }
}

/**
 * 支级资源上界的键（**与命令行旗标同名**：一个概念一个名称）。
 *
 * 2026-10-05：这两个值原先由宿主**写死**（`up.js` 与壳的 `host_process.rs` 各写一个 `8`）⇒
 * 用户改不了，而且同一事实两个写者。现在**唯一权威是 `agent.conf` 这一份文件**，两个宿主只是读者。
 */
const AGENT_BUDGET_KEYS = {
  maxTurns: 'max_turns',
}

/**
 * 读一支住户的资源上界（`agent.conf` 的可选键）。**缺省 = 不在场**（`null`），调用方给内置缺省。
 *
 * ⚠ **值原样返回，不在这一层校验**：校验的权威是 `agent_host` 自己的旗标解析（非整数、0 都被拒，
 * 且有明确失败文本）。在这一层"顺手兜底成缺省"会让**配置的值**与**运行的值**悄悄不一致 ——
 * 那正是假绿；宁可让宿主带着原文去拒绝，也不在这里改写成另一个值。
 */
function readAgentBudget(file) {
  const raw = (confGet(file, AGENT_BUDGET_KEYS.maxTurns) || '').trim()
  return { maxTurns: raw === '' ? null : raw }
}

/**
 * 把一段**文本**切成行（与 `conf.readLines` 同款口径，只是手里没有路径）。
 *
 * ⚠ 为什么不能直接把文本交给 `conf.readLines`：它收的是**路径**（内部 `readFileSync`）——
 * 传文本进去等于拿"文件内容"当文件名读，必然抛，于是**别人的行被静默丢掉**
 * （真机/单测撞到过：`agent.conf` 里的 `note=` 那类行写一次就没了）。
 */
function linesOfText(text) {
  if (typeof text !== 'string') return []
  const lines = text.split('\n')
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines.map((line) => (line.endsWith('\r') ? line.slice(0, -1) : line))
}

/**
 * 渲染一份 `agent.conf`（**已知那几行按规范顺序**，别的行原样保留 —— 与 `home.conf`
 * 同一条"不吞别人的行"纪律）。
 */
function renderAgentConf(current, { agentId, roomId = '', slot, displayName = '' }) {
  const known = Object.values(AGENT_CONF_KEYS).map((key) => `${key}=`)
  const kept = linesOfText(current).filter((line) => !known.some((prefix) => line.startsWith(prefix)))
  const out = [
    `${AGENT_CONF_KEYS.agent}=${agentId}`,
    `${AGENT_CONF_KEYS.room}=${roomId}`,
    `${AGENT_CONF_KEYS.slot}=${slot}`,
  ]
  if (displayName) out.push(`${AGENT_CONF_KEYS.name}=${displayName}`)
  return [...out, ...kept, ''].join('\n')
}

/**
 * 写一份 `agent.conf`：**内容没变就一个字都不动**（mtime 是读数的一部分；重跑不重复写）。
 * 返回 `{ changed, file }`。
 */
function writeAgentConf(file, values) {
  let current = null
  try {
    current = fs.readFileSync(file, 'utf8')
  } catch {
    current = null
  }
  const text = renderAgentConf(current, values)
  if (current === text) return { changed: false, file }
  fs.mkdirSync(path.dirname(file), { recursive: true })
  fs.writeFileSync(file, text, 'utf8')
  return { changed: true, file }
}

/**
 * 这个家住着哪几支（**只认新布局**：`<家>/agents/<支>/`）。
 *
 * 一支 = 一个子目录，且（有 `agent.conf` **或**有 `identity.json`）——两者都没有的子目录
 * 不是住户（半成品/杂物，不列）。身份以**目录名**为准（壳的布局就是这样：`agents/<id>/`）；
 * `agent.conf` 里那行与目录名不一致时挂一个 `mismatch` 读数（**如实说，不静默选边**）。
 * 按 `slot` 排序（没写 slot 的排最后），同 slot 按 id 排 —— 顺序稳定。
 */
function residents(homeDir) {
  const root = P.agentsRoot(homeDir)
  let entries
  try {
    entries = fs.readdirSync(root, { withFileTypes: true })
  } catch {
    return []
  }
  const list = []
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    if (entry.name.startsWith('.')) continue
    const envDir = path.join(root, entry.name)
    const confFile = path.join(envDir, P.AGENT_CONF)
    const hasConf = fs.existsSync(confFile)
    const hasIdentity = fs.existsSync(path.join(envDir, 'identity.json'))
    if (!hasConf && !hasIdentity) continue
    const conf = readAgentConf(confFile)
    list.push({
      agentId: entry.name,
      envDir,
      confFile,
      roomId: conf.roomId,
      slot: conf.slot,
      displayName: conf.displayName,
      legacy: false,
      mismatch: conf.agentId && conf.agentId !== entry.name ? conf.agentId : null,
    })
  }
  list.sort(
    (left, right) =>
      (left.slot ?? Number.MAX_SAFE_INTEGER) - (right.slot ?? Number.MAX_SAFE_INTEGER) ||
      (left.agentId < right.agentId ? -1 : left.agentId > right.agentId ? 1 : 0),
  )
  return list
}

/**
 * **老家那一支**（还没迁的单支形状）：`home.conf` 的 `agent_id` ＋ `<家>/env`。
 *
 * 返回 `null`（没有老家）或一个 `legacy: true` 的住户行 —— 它的"支级配置"还住在 `home.conf`
 * 里，所以 `confFile` 指向家级那份。`up` 会在读它**之前**先做迁移（`migrateHomeShape`）；
 * `status` / `down` 是只读/收场路径，直接按这个形状如实读数与收场。
 */
function legacyResident(homeDir, confFile) {
  const envDir = P.legacyEnvRoot(homeDir)
  const confAgentId = (confGet(confFile, 'agent_id') || '').trim()
  const identityAgentId = readEnvIdentity(envDir)
  // ⚠ 两处都可能**没有**（`confAgentId` 是 `''`、`readEnvIdentity` 给 `null`）⇒ 归一成 `''`：
  // `null === ''` 是 false，会让"没有老家"被读成"有一支 id 叫 null 的住户"（真机撞到过：
  // 全新的家里凭空多出一支 `null`）。
  const agentId = confAgentId || identityAgentId || ''
  if (agentId === '') return null
  return {
    agentId,
    envDir,
    confFile,
    roomId: (confGet(confFile, 'room_id') || '').trim(),
    slot: 1,
    displayName: '',
    legacy: true,
    mismatch: confAgentId && identityAgentId && confAgentId !== identityAgentId ? identityAgentId : null,
  }
}

/**
 * **运行时住户表**：新布局的住户 ＋（还没迁的）老家那一支。
 *
 * 老家的那一支只在"新布局里没有同一个身份"时补进来（同一个身份在两边都出现 = 迁到一半，
 * 以新布局为准，不重复列）。
 */
function runtimeResidents(homeDir, confFile) {
  const list = residents(homeDir)
  const legacy = legacyResident(homeDir, confFile)
  if (legacy && !list.some((resident) => resident.agentId === legacy.agentId)) {
    list.unshift(legacy)
  }
  return list
}

/** 下一个住户序号：`max(已有 slot) + 1`（单调；**绝不回收** —— 回收会让布置键撞上旧记录）。 */
function allocateSlot(list) {
  let max = 0
  for (const resident of list) {
    if (Number.isInteger(resident.slot) && resident.slot > max) max = resident.slot
  }
  return max + 1
}

/** 一支住户的宿主 pid 文件（老家那一支仍用 `logs/host.pid`）。 */
function residentPidFile(homeDir, resident) {
  return resident.legacy ? path.join(homeDir, 'logs', 'host.pid') : P.agentPidFile(homeDir, resident.agentId)
}

/** 一支住户的宿主日志（老家那一支仍用 `logs/host.out`）。 */
function residentHostLog(homeDir, resident) {
  return resident.legacy ? path.join(homeDir, 'logs', 'host.out') : P.agentHostLog(homeDir, resident.agentId)
}

/**
 * **不属于本家任何一支**的住户账本目录（`logs/agents/<支>/`）—— 搬家后留在老家的那些。
 *
 * 账本按设计**不进环境根**（见 `paths.js` 的 `agentLogDir`）：一支 Agent 换机器时环境根
 * 整份搬走，而 pid 文件与宿主日志**留在老家**（它们记的是**另一台机器上的进程**，搬过去
 * 没有意义）。`up` / `down` / `status` 都不碰这些目录；本函数只把它们**读出来**，由 `status`
 * 如实提示一行（"本家没有这一支（可能搬走了）—— 可以删"）。
 *
 * 判据只认**目录名**：`logs/agents/` 下只有 up 建的住户账本目录；不在住户表里 ⇒ 孤儿。
 * 没有孤儿（含目录根本不存在）⇒ 空数组 —— 读数因此**零噪音**。
 */
function orphanLedgerDirs(homeDir, residentList) {
  let entries
  try {
    entries = fs.readdirSync(path.join(homeDir, 'logs', 'agents'), { withFileTypes: true })
  } catch {
    return []
  }
  const known = new Set(
    (Array.isArray(residentList) ? residentList : []).map((resident) => resident.agentId),
  )
  return entries
    .filter((entry) => entry.isDirectory() && !entry.name.startsWith('.'))
    .map((entry) => entry.name)
    .filter((name) => !known.has(name))
    .sort()
}

/** 让位目标：本名被占就往后编号（不覆盖任何既有目录）。 */
function freeAsidePath(base) {
  if (!fs.existsSync(base)) return base
  for (let n = 2; n < 100; n += 1) {
    const candidate = `${base}-${n}`
    if (!fs.existsSync(candidate)) return candidate
  }
  return `${base}-${Date.now()}`
}

/**
 * **老家那支宿主还在跑吗**（迁移的前置判据）。
 *
 * 复用既有的两条（不新造）：账本（`logs/host.pid`，四态里 `ours` / `unverifiable` 都算在跑）
 * ＋ 进程表里的**本家**孤儿（`--root` 落在 `<家>/env` 上）。
 */
function legacyHostRunning(homeDir, layout) {
  const home = { homeDir, envDirs: [P.legacyEnvRoot(homeDir)] }
  const pidFile = path.join(homeDir, 'logs', 'host.pid')
  const ledger = proc.ledgerPid('agent', pidFile, layout, home)
  if (ledger.state === 'ours' || ledger.state === 'unverifiable') return true
  return proc.hostOrphanPids(layout, home).length > 0
}

/**
 * 老家运行账本（pid 文件 ＋ 宿主日志）跟着搬到一支住户的账本位（`logs/agents/<支>/`）。
 * **只在目标缺失时搬**（rename，保留 mtime；目标在场 ⇒ 一个字节都不动）。
 */
function migrateLegacyRuntimeFiles(homeDir, agentId) {
  const moves = [
    [path.join(homeDir, 'logs', 'host.pid'), P.agentPidFile(homeDir, agentId)],
    [path.join(homeDir, 'logs', 'host.out'), P.agentHostLog(homeDir, agentId)],
  ]
  const moved = []
  for (const [from, to] of moves) {
    if (!fs.existsSync(from)) continue
    if (fs.existsSync(to)) continue
    fs.mkdirSync(path.dirname(to), { recursive: true })
    fs.renameSync(from, to)
    moved.push(path.basename(from))
  }
  return moved
}

/**
 * **家的形状迁移**（幂等；`up` 在读住户之前调它）。
 *
 * @param {object} ctx
 * @param {string} ctx.homeDir
 * @param {string} ctx.confFile `home.conf`
 * @param {object} ctx.layout `detectLayout()` 的结果（进程表判据要用）
 * @param {boolean} [ctx.canTouchWorld] 这一次能不能动世界（远端跑法没有主人凭据 ⇒ false）
 * @param {() => boolean} [ctx.hostRunning] 老家那支在不在跑（用例注入；缺省走进程表）
 * @returns {{ok: boolean, action: string, agentId: string|null, envAside: string|null,
 *            lines: string[], refuse: string|null}}
 *   `action ∈ none | migrated | aside | finish | refuse | failed`；
 *   `refuse` 非空 ⇒ **这一次什么都不改**（调用方当场停手、原样打印 `lines`）。
 */
function migrateHomeShape({ homeDir, confFile, layout, canTouchWorld = true, hostRunning = null }) {
  const lines = []
  const legacyEnv = P.legacyEnvRoot(homeDir)
  const legacyExists = fs.existsSync(legacyEnv)
  const homeId = (confGet(confFile, 'home_id') || '').trim()
  const confAgentId = (confGet(confFile, 'agent_id') || '').trim()
  const confRoomId = (confGet(confFile, 'room_id') || '').trim()
  const identityAgentId = readEnvIdentity(legacyEnv)
  // ⚠ 两处都可能**没有**（`confAgentId` 是 `''`、`readEnvIdentity` 给 `null`）⇒ 归一成 `''`：
  // `null !== ''` 会让下面那些"老家还在不在"的判据**误判成"在"**（真机撞到过：全新家被当成老家）。
  const staleAgent = confAgentId || identityAgentId || ''
  const running = () => (typeof hostRunning === 'function' ? hostRunning() : legacyHostRunning(homeDir, layout))

  const nothing = (action = 'none') => ({ ok: true, action, agentId: null, envAside: null, lines, refuse: null })

  // ── 老家（没有家 id）＋ 旧身份还在 ⇒ 键换形状会在世界里另建一支 ⇒ 旧环境让位 ──
  if (homeId === '' && staleAgent !== '') {
    if (running()) {
      return {
        ok: false,
        action: 'refuse',
        agentId: null,
        envAside: null,
        lines: [],
        refuse:
          '[布置] 这个家还没有家 id（home.conf 里没有 home_id），而它世界里那支 Agent 是**旧的「世界级键」**建的：' +
          '按新键重做会在世界里**另建一支 Agent**（旧那支留着不动）。\n' +
          '[布置] 但家里那支 Agent 还在跑 —— 不许在跑着的时候换身份：先 node home.js down，再 node home.js up。',
      }
    }
    if (!canTouchWorld) {
      return {
        ok: false,
        action: 'refuse',
        agentId: null,
        envAside: null,
        lines: [],
        refuse:
          '[布置] 这个家还没有家 id（home.conf 里没有 home_id），而它世界里那支 Agent 是**旧的「世界级键」**建的：' +
          '按新键重做会在世界里**另建一支 Agent**（旧那支留着不动）。\n' +
          '[布置] 这一次没有主人凭据 ⇒ 停在动手之前（本地那几行一个字都没改）：带 --token <那个世界的主人凭据> 重来。',
      }
    }
    // 旧环境整份让位（改名，不删）—— 排在改 home.conf 之前：这一步失败 ⇒ 什么都没改。
    let envAside = null
    if (legacyExists && fs.existsSync(path.join(legacyEnv, 'identity.json'))) {
      envAside = freeAsidePath(`${legacyEnv}.old-${staleAgent}`)
      try {
        fs.renameSync(legacyEnv, envAside)
      } catch (error) {
        return {
          ok: false,
          action: 'failed',
          agentId: null,
          envAside: null,
          lines: [],
          refuse: `[布置] 旧环境整份让位失败（${error.message}）—— 停在这里，什么都没改（重跑一次即可）`,
        }
      }
    }
    confDelete(confFile, 'agent_id')
    confDelete(confFile, 'room_id')
    lines.push(
      `[布置] ⚠ 它世界里那支身份（${staleAgent}）与那间房（${confRoomId || '无'}）是**旧的「世界级键」**建的：` +
        '键从此按家取 ⇒ 这个家会在世界里**另建一支 Agent**（旧那支留着不动，本地这两行跟着让位）',
    )
    if (envAside) lines.push(`[布置] ⚠ 旧环境整份让位（不删）：${envAside}`)
    return { ok: true, action: 'aside', agentId: null, envAside, lines, refuse: null }
  }

  // ── 没有旧环境：也许 home.conf 里还留着那两行（上次迁到一半 / 环境已经不在）──
  if (!legacyExists) {
    if (staleAgent === '' || homeId === '') return nothing()
    const target = P.agentEnvRoot(homeDir, staleAgent)
    if (fs.existsSync(target)) {
      const confFileForAgent = P.agentConfPath(homeDir, staleAgent)
      const existing = readAgentConf(confFileForAgent)
      writeAgentConf(confFileForAgent, {
        agentId: staleAgent,
        roomId: confRoomId || existing.roomId,
        slot: existing.slot ?? 1,
        displayName: existing.displayName,
      })
      migrateLegacyRuntimeFiles(homeDir, staleAgent)
      confDelete(confFile, 'agent_id')
      confDelete(confFile, 'room_id')
      lines.push(
        `[布置] 住户化的收尾：${staleAgent} 的环境已在 <家>/agents/ 里 ⇒ home.conf 那两行（agent_id / room_id）让位给支级 agent.conf`,
      )
      return { ok: true, action: 'finish', agentId: staleAgent, envAside: null, lines, refuse: null }
    }
    // 环境与住户目录都不在：那两行只是旧账 —— 删掉（世界侧身份由布置键重放找回）。
    confDelete(confFile, 'agent_id')
    confDelete(confFile, 'room_id')
    lines.push(
      '[布置] 住户化的收尾：home.conf 里还留着旧的 agent_id / room_id，而环境已经不在 —— 让位（世界里那支由布置键重放找回）',
    )
    return { ok: true, action: 'finish', agentId: staleAgent, envAside: null, lines, refuse: null }
  }

  // ── 旧环境在场，而它没有身份可归属（没布置完的空 env）⇒ 什么都不动 ──
  if (staleAgent === '') return nothing()

  // ── 有家 id（身份不变）：`<家>/env` 整份迁成住户 1 的环境根 ──
  if (running()) {
    return {
      ok: false,
      action: 'refuse',
      agentId: null,
      envAside: null,
      lines: [],
      refuse:
        '[布置] 这个家还是**一支一个环境根**的旧形状（环境根 `<家>/env`），要把它迁成 `<家>/agents/<支>/`。\n' +
        '[布置] 但家里那支 Agent 还在跑 —— 搬环境要在它不在跑的时候做：先 node home.js down，再 node home.js up。',
    }
  }
  const target = P.agentEnvRoot(homeDir, staleAgent)
  let action = 'migrated'
  let envAside = null
  if (!fs.existsSync(target)) {
    try {
      fs.mkdirSync(path.dirname(target), { recursive: true })
      fs.renameSync(legacyEnv, target)
    } catch (error) {
      return {
        ok: false,
        action: 'failed',
        agentId: null,
        envAside: null,
        lines: [],
        refuse: `[布置] 旧环境迁到住户位失败（${error.message}）—— 停在这里，什么都没改（重跑一次即可）`,
      }
    }
  } else {
    // 目标被占（同一个身份不该出现两次）：旧的那份**整份让位**（不覆盖、不合并）。
    envAside = freeAsidePath(`${legacyEnv}.old-${staleAgent}`)
    try {
      fs.renameSync(legacyEnv, envAside)
    } catch (error) {
      return {
        ok: false,
        action: 'failed',
        agentId: null,
        envAside: null,
        lines: [],
        refuse: `[布置] 旧环境让位失败（${error.message}）—— 停在这里，什么都没改（重跑一次即可）`,
      }
    }
    action = 'aside'
  }
  const confFileForAgent = P.agentConfPath(homeDir, staleAgent)
  const existing = readAgentConf(confFileForAgent)
  writeAgentConf(confFileForAgent, {
    agentId: staleAgent,
    roomId: confRoomId || existing.roomId,
    slot: existing.slot ?? 1,
    displayName: existing.displayName,
  })
  const moved = migrateLegacyRuntimeFiles(homeDir, staleAgent)
  confDelete(confFile, 'agent_id')
  confDelete(confFile, 'room_id')
  lines.push(
    `[布置] 住户化：环境根 ${legacyEnv} ⇒ ${target}（住户 1：${staleAgent}；身份不变，世界里的记录照旧）`,
  )
  if (action === 'aside') lines.push(`[布置] ⚠ 目标住户位已存在 ⇒ 旧的那份整份让位（不删）：${envAside}`)
  if (moved.length > 0) lines.push(`[布置] 运行账本跟着搬（${moved.join('、')}）⇒ logs/agents/${staleAgent}/`)
  return { ok: true, action, agentId: staleAgent, envAside, lines, refuse: null }
}

/**
 * 一支的**手**（2026-10-01 第五刀）：外部 MCP server 声明 ＋ 终端档。
 *
 * **判据**：手长在身体上 ⇒ 手是**支级事实**；家级那两处只是**默认**（没写支级时的回落）。
 *
 * 三条规则（读数里逐条说清**来源**，用户不许靠"旗标少了一个"去猜）：
 *   · 支级的 `agent.conf` 里有 `mcp_*` 行 ⇒ **用这些**（覆盖家级 —— 是覆盖不是合并：
 *     合并会让"我要一支完全没有外部 server"变成做不到）；
 *   · 支级写了 `mcp=none` ⇒ 这一支**一只外部 server 都不要**（哪怕家级声明了）；
 *   · 都没有 ⇒ **跟家走**（`home.conf` 的 `mcp_*` —— 2026-10-01 之前的行为一字不变）。
 *
 * 终端档同款：支级 `terminal=full|none` ⇒ 家级 `terminal=` ⇒ **平台默认（2026-10-14 起是"不给手"）**。
 * 支级 `terminal=none` 是"这一支不要终端这只手"的**唯一**说法——它不靠旗标缺席去表达；
 * 而"要手"也只有一个说法：**显式写 `full`**。
 *
 * ⚠ **老家那一支**（还没迁的住户）：它的支级事实还在 `home.conf` 里，所以"支级/家级"两层
 * 在它身上是同一份文件 —— 来源如实报 `home`（不假装它有自己的 agent.conf）。
 *
 * @returns {{mcp: string[], mcpSource: 'resident'|'home'|'none', terminal: string, terminalSource: 'resident'|'home'|'platform'}}
 */
function residentHands({ homeDir, resident, homeConfFile }) {
  const legacy = Boolean(resident && resident.legacy)
  const residentFile = legacy ? homeConfFile : P.agentConfPath(homeDir, resident.agentId)
  const homeDecls = mcpDeclarations(homeConfFile)
  const ownDecls = legacy ? [] : mcpDeclarations(residentFile)
  const wantsNone = !legacy && (confGet(residentFile, HAND_KEYS.mcp) || '').trim().toLowerCase() === 'none'

  let mcp = homeDecls
  let mcpSource = 'home'
  if (wantsNone) {
    mcp = []
    mcpSource = 'none'
  } else if (ownDecls.length > 0) {
    mcp = ownDecls
    mcpSource = 'resident'
  }

  const ownTerminal = (!legacy ? (confGet(residentFile, HAND_KEYS.terminal) || '').trim().toLowerCase() : '')
  const homeTerminal = (confGet(homeConfFile, HAND_KEYS.terminal) || '').trim().toLowerCase()
  const skills = (!legacy ? (confGet(residentFile, HAND_KEYS.skills) || '').trim() : '')
  const known = (value) => value === 'full' || value === 'none'
  let terminal = ''
  let terminalSource = 'platform'
  if (known(ownTerminal)) {
    terminal = ownTerminal
    terminalSource = 'resident'
  } else if (known(homeTerminal)) {
    terminal = homeTerminal
    terminalSource = 'home'
  }

  return { mcp, mcpSource, terminal, terminalSource, skills }
}

/**
 * 一支的终端档最终是什么（**平台事实优先**：没有终端臂的平台上，声明 full 也装不进来）。
 *
 * ⚠ **2026-10-14 主人裁**：**平台默认不给手**（要手就显式写 `terminal=full`）。从前这一格是
 * "有臂就给 full"——那让"装完即整机全权"成了一个**没人声明过**的默认，而缺省那一档该是
 * "自己区可写、出不去"（她自己的东西不需要授权；见 `agent/README.md` §3.105 与施工单 §12.12）。
 * 与它配套的那一步（老住户把**现行的** full 显式写进自己的 `agent.conf`）见 `up.js` 的
 * `migrateTerminalDefault`：**缺省变了、老住户不掉东西**，但要手这件事从此看得见。
 *
 * 与 `up.js` 的 `terminalNotice` 同一条纪律：**如实说、不静默降级** —— 返回值里带
 * `unavailable`，调用方据此把"声明了但这台机器上没有这只手"打在读数里。
 */
function resolveTerminal(hands, { isWindows, isLinux }) {
  const arm = Boolean(isWindows || isLinux)
  // 只有**显式声明** `terminal=full` 才给手；`none` 与"没写"（平台默认）都是不给。
  const wanted = hands.terminal === 'full' ? 'full' : 'none'
  const mode = arm ? wanted : 'none'
  return { mode, source: hands.terminalSource, unavailable: !arm && wanted === 'full' }
}

module.exports = {
  AGENT_CONF_KEYS,
  AGENT_BUDGET_KEYS,
  HAND_KEYS,
  readEnvIdentity,
  parseSlot,
  readAgentConf,
  readAgentBudget,
  renderAgentConf,
  writeAgentConf,
  residents,
  legacyResident,
  runtimeResidents,
  allocateSlot,
  residentPidFile,
  residentHostLog,
  orphanLedgerDirs,
  residentHands,
  resolveTerminal,
  freeAsidePath,
  legacyHostRunning,
  migrateLegacyRuntimeFiles,
  migrateHomeShape,
}
