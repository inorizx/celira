'use strict'
/**
 * `node home.js down` —— 家的**关机**（移植自已退役的 bash 版 `scripts/home_down.sh`）。
 *
 * 停的顺序：**Agent → client → 服务端**，最后再按端口收一遍孤儿（真正的收尾判据是
 * "端口放没放"，比"谁是谁的孩子"硬）。只停**自己人**——不做任何"按名字杀一遍"的清扫。
 *
 * 比 bash 版多一道**身份验证**（本刀明写的要求）：pid 文件里的数先问一遍进程表
 * "这个 pid 是不是我们要停的那个"（种类 ＋ **家事实**：可执行件/命令行 ＋ `--root`/`--port`/端口）。
 * 不是 ⇒ **不杀**，只把账本清掉并如实说出来。理由：陈旧账本 + 两个 pid 世界的历史遗留，
 * 盲杀会误伤无关进程；⚠ 2026-09-24 起还多一档：**别家的进程**（同一份发布包服务多个家，
 * 路径一模一样）—— 那正是本刀要修的跨家误杀。
 */

const fs = require('node:fs')
const path = require('node:path')

const { EXIT } = require('./codes')
const P = require('./paths')
const { confGet, portOf } = require('./conf')
const { readUserEnv } = require('./userenv')
const { proveWorldOwnership } = require('./world')
const credentials = require('./credentials')
const { BACKEND_ENV } = require('./provision')
const proc = require('./proc')
const residents = require('./residents')
const worldWitness = require('./world_witness')

const USAGE = [
  '用法: node home.js down [--agent <世界里的 Agent id>]',
  '  关机（数据不动）。不带 --agent = 停**整个家**（每个住户的宿主 ＋ 页面 ＋ 两个门 ＋ 世界）；',
  '  --agent <id> = **只停这一支住户**（它的宿主与孤儿），世界、页面、别的住户一律不动。',
  '  要指一个临时的家，用环境变量 CELIRA_HOME_DIR。',
]

/**
 * 解析 `node home.js down [参数]` —— 与 `up.js` 的 `parseUpArgs` **同一份参数纪律**：
 * 不认识的参数一律用法错（退出码 1），**绝不静默忽略**。
 *
 * ## 该接受什么：只有 `--agent <id>`（2026-10-01 多支刀）
 *
 * `down` 原来**一个旗标都没有**，依据是仓库里已经写着的两处事实（不是某一刀的口味）：
 *   · `home.js` 的 `USAGE` 里 `down` 那一行只有「关机（数据不动）」，一个 `--旗标` 都没有
 *     （对照 `up` 那一行：它把自己的旗标逐个列了出来）；
 *   · `scripts/HOME_MANUAL.md` 的退出码表**已经**给 `home_down` 那一列写了 `1 = 用法错`
 *     —— 这条契约早就发布了，只是代码里从来没有过这条路径（那一刀把它补上）。
 *
 * 住户化之后，起停单位是 **Agent** ⇒ `down` 必须能只停一支（PROJECT_SHAPE §8.0 的裁决 3）：
 * `--agent <世界里的 Agent id>` = 只停那一支的宿主（连带它的孤儿），
 * 世界 / 页面 / 两个门 / 别的住户**一律不动**。别的旗标照旧一律用法错。
 *
 * ## 为什么 `--home` 也不收
 *
 * 与 `up` 一致（`up` 真机实测：`[home_up] 不认识的参数：--home`，退出码 1）。
 * 指一个临时的家**只有一条路**：环境变量 `CELIRA_HOME_DIR`（`paths.homeDir()`）。
 * `--home` 是**常驻件自己在命令行里声明"我属于哪个家"**用的（`home.js` 的 `homeFlag`：
 * 进程表读得到命令行、读不到环境变量，所以 `status --serve` / `config --serve` 才要带它）；
 * 一次性的 `up` / `down` 从环境变量读家 —— 多开一条入口就是多一份不一致。
 *
 * ## 为什么这一问排在 `P.homeDir()` **之前**
 *
 * 这正是要修的那个 bug：从前 `down` 不解析 argv，`node home.js down --help` 会**静默按
 * 默认家跑**（真机撞到过：家当时没跑才侥幸是空操作，家要是在跑那一下就把主人的家关了）。
 * 所以判参数排在**一切之前**：参数错的时候，连"哪个家"都不该被解析出来 ——
 * `P.homeDir()` 与它后面所有动作（端口快照、进程表、杀进程）**一个都不发生**。
 *
 * @returns {{error: string|null, agentId: string|null}} 与 `parseUpArgs` 同一个形状
 */
function parseDownArgs(argv) {
  const args = Array.isArray(argv) ? argv : []
  let agentId = null
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (arg === '--agent') {
      const value = args[index + 1]
      if (typeof value !== 'string' || value.trim() === '' || value.startsWith('--')) {
        return { error: '[home_down] `--agent` 后面要跟一个世界里的 Agent id', agentId: null }
      }
      agentId = value.trim()
      index += 1
      continue
    }
    return { error: `[home_down] 不认识的参数：${arg}`, agentId: null }
  }
  return { error: null, agentId }
}

async function down(argv = []) {
  const options = parseDownArgs(argv)
  if (options.error) {
    console.log(options.error)
    for (const line of USAGE) console.log(line)
    return EXIT.USAGE
  }
  const homeDir = P.homeDir()
  if (!fs.existsSync(homeDir)) {
    console.log(`[home_down] 没有这个家: ${homeDir}`)
    return EXIT.ENV_UNAVAILABLE
  }
  const layout = P.detectLayout()
  const logDir = path.join(homeDir, 'logs')
  const confFile = path.join(homeDir, 'home.conf')
  const listen = confGet(confFile, 'listen_addr', P.DEFAULT_LISTEN)
  const imPort = portOf(listen)
  const clientPort = confGet(confFile, 'client_port', P.DEFAULT_CLIENT_PORT)
  // **远端跑法**（2026-09-30）：`home.conf` 记着 `im_endpoint` ⇒ 本机不起 core，
  // 本机 listen 口不是"世界的家"（下面收端口那一节据此跳过 im 那一支 —— 那个口上的
  // 东西与这个家无关，去收它正是跨家误杀的形状）。
  const imEndpoint = (confGet(confFile, 'im_endpoint', '') || '').trim()
  // **家的门**（2026-10-01 第六刀）：读端面 ＋ 写端面 = **一个进程**，一本账（`logs/doors.pid`）。
  const doorsPidFile = path.join(logDir, 'doors.pid')
  // 旧形状（两个门各一个进程）的两本账：**只作迁移桥** —— 老家还跑着旧门时也要收得掉，
  // 但认不出就不杀（只清账本），与 `stopOne` 同一条纪律。
  const legacyDoorPidFiles = [
    { target: 'status', label: 'status', file: path.join(logDir, 'status.pid') },
    { target: 'config', label: 'config', file: path.join(logDir, 'config.pid') },
  ]
  // **家事实**（本刀）：认领判据的第二维——只停**本家**的进程（同一份发布包可以服务多个家，
  // 光看可执行件路径会把别家的杀掉）。一个命令取一次快照（netstat）。
  //
  // ⚠ **住户化之后分两段**：常驻件（client / im / status / config）用这一份；
  // 宿主（agent）那一支要的是**一支一个环境根**那一组（`envDirs`），逐支现算（`scopedHome`）。
  const homeBase = {
    homeDir,
    clientPort,
    imPort,
    statusPort: confGet(confFile, 'status_port', P.DEFAULT_STATUS_PORT),
    configPort: confGet(confFile, 'config_port', P.DEFAULT_CONFIG_PORT),
    // **家的门**（2026-10-01 第六刀）：读口 ＋ 写口 = 一个进程的两个端面（见 proc.js 的 doors 指纹）。
    doorsPorts: [
      confGet(confFile, 'status_port', P.DEFAULT_STATUS_PORT),
      confGet(confFile, 'config_port', P.DEFAULT_CONFIG_PORT),
    ],
    listening: proc.listenerSnapshot(),
  }

  // **住户表**（只读路径：`down` 不做形状迁移 —— 那是 `up` 的事；还没迁的老家那一支照旧收，
  // 它的账本在 `logs/host.pid`，见 `residents.residentPidFile`）。
  const residentList = residents.runtimeResidents(homeDir, confFile)
  let targets = residentList
  if (options.agentId) {
    targets = residentList.filter((resident) => resident.agentId === options.agentId)
    if (targets.length === 0) {
      console.log(`[home_down] 这个家里没有这支住户：${options.agentId}`)
      if (residentList.length === 0) {
        console.log('[home_down] 这个家一个住户都没有（先 node home.js up 安一支）')
      } else {
        console.log(`[home_down] 家里住着：${residentList.map((resident) => resident.agentId).join('、')}`)
      }
      return EXIT.USAGE
    }
  }
  /** 一支住户的家事实（`--root` 落点就是它的环境根）。 */
  const scopedHome = (resident) => ({ ...homeBase, envDirs: [resident.envDir] })
  /** 一支住户在读数里的称呼（多支时带上 id —— 一行读数不许让主人猜是哪一支）。 */
  const labelOf = (resident) => (residentList.length > 1 ? `${resident.agentId}` : '')

  console.log(`[home_down] 家 = ${homeDir}`)
  if (options.agentId) {
    console.log(
      `[home_down] 只停这一支住户：${options.agentId}（世界、页面、两个门、别的住户一律不动）`,
    )
  }
  let status = EXIT.OK

  /** 停一个：先请 → 等 → 强停；返回 0 = 停了。 */
  const stopOne = async (name, target, pidFile, facts = homeBase) => {
    const ledger = proc.ledgerPid(target, pidFile, layout, facts)
    if (ledger.state === 'gone') {
      console.log(`[${name}] 已经在停着（本来就没跑）`)
      proc.clearPid(pidFile)
      return true
    }
    const pid = ledger.pid
    if (ledger.state === 'not-ours') {
      // 三档都落在这里：陈旧账本/撞号/**别家的进程**（家事实对不上）。裁决（2026-09-24）：
      // 一律**只清账本、不杀** —— 被清的那支若在跑，它属于别家，不该由本家杀。
      const info = proc.describeProcess(pid)
      const who = info ? `${info.name || '?'}${info.exePath ? `（${info.exePath}）` : ''}` : '认不出来'
      console.log(`[${name}] pid 文件里的 ${pid} 不是我们的进程（它属于 ${who}）——不杀，只清账本`)
      proc.clearPid(pidFile)
      return true
    }
    if (ledger.state === 'unverifiable') {
      // 进程表读不到（脚本宿主不可用）⇒ 退回 .sh 的语义：按账本停，但如实说没验成身份。
      console.log(`[${name}] 进程表读不到，没法验身份——按账本停（pid ${pid}）`)
    }
    await proc.killPid(pid, { force: true })
    if (proc.isAlive(pid)) {
      console.log(`[${name}] 停不下来（pid ${pid}）`)
      return false
    }
    console.log(`[${name}] 已停（pid ${pid}）`)
    proc.clearPid(pidFile)
    return true
  }

  /**
   * 端口上还有**本家**的谁在听 ⇒ 收掉它（那是我们这一支留下的孤儿）。
   *
   * ⚠ 2026-09-24 加门（裁决）：收之前**先验身份**（种类 ＋ 家事实）。终点站这一道原来不看
   * 判据、端口上站着谁就杀谁 —— 本家端口要是被**别的程序**占着（用户自己的别的服务、
   * 或别家的进程），那一下就是误杀无关进程。认不出的**不杀**，如实报"端口仍被占着"。
   * 本家自己的孤儿（账本丢了、人还在）照旧收掉：端口就是它，家事实必然对上。
   *
   * ⚠ 2026-09-25 再加一门（**im 那一支**）：端口是**机器级**命名空间，两家共用同一份发布包
   * ＋同一组默认端口时，"同包 + 同端口"两条判据**同时**成立 ⇒ 本家的 down 会停掉**别家的
   * 世界**（真机实测）。所以 im 这一支要 `prove`：**证明得了是本家的才收**（证明在
   * `world.js`：本家的后端凭据被那个世界认下、且代表本家的 Agent）。证明不了 ⇒ 不杀，如实报。
   */
  let imProofCache
  const imProof = async () => {
    if (imProofCache === undefined) {
      const agentId = residentList.length > 0 ? residentList[0].agentId : null
      const proved = await proveWorldOwnership({
        base: `http://${listen}`,
        // 与 `up` / 布置**同一份取值口径**（按「世界 × 身份」分条的本地存储 → 老路的环境变量）。
        credential: agentId
          ? credentials.resolve({ world: `http://${listen}`, agentId }, { fallback: readUserEnv(BACKEND_ENV) })
              .token
          : null,
        agentId,
      })
      imProofCache = proved.state
    }
    return imProofCache
  }

  const sweepPort = async (port, name, { prove = null } = {}) => {
    for (const pid of proc.listenersOnPort(port)) {
      const info = proc.describeProcess(pid)
      if (!proc.ownsPid(name, info, layout, homeBase)) {
        const who = info ? `${info.name || '?'}${info.exePath ? `（${info.exePath}）` : ''}` : '认不出来'
        console.log(`[${name}] 端口 ${port} 上那支不是我们的（pid ${pid}，属于 ${who}）——不杀`)
        continue
      }
      if (prove !== null) {
        const state = await prove()
        if (state !== 'ours') {
          const why =
            state === 'foreign'
              ? '它认下本家的凭据、却说是别人的 Agent'
              : '本家的凭据它不认（或连不上）'
          console.log(`[${name}] 端口 ${port} 上还有一支（pid ${pid}），但**证明不了是本家的**：${why}——不杀`)
          console.log(`[${name}] 别家的世界占着同一个端口时就是这样。要停它请到那个家去停（确认属于本家后也可以手动结束它）`)
          continue
        }
      }
      console.log(`[${name}] 端口 ${port} 上还有孤儿（pid ${pid}）——收掉它`)
      await proc.killPid(pid, { force: true })
      await proc.sleep(300)
    }
    if (proc.portInUse(port)) {
      console.log(`[${name}] ⚠ 端口 ${port} 仍被占着（不是我们这一支在听？）`)
      return false
    }
    console.log(`[${name}] 端口 ${port} 已放`)
    return true
  }

  // 关机顺序：**每一支宿主 → client → 服务端**（Agent 的收场最要紧：先停 Agent，
  // Agent 的活动才不会被打断在写一半）。一支一个宿主 ⇒ 一支一支停（各自的账本 + 各自的
  // 家事实：`--root` 落在它自己的环境根上）。
  //
  // 注：.sh 版在这里还有一段"账本对账"——因为那时候 pid 文件里可能是 MSYS pid，直接交给
  // `kill` 有误杀风险。Node 里没有第二个 pid 世界，而 `stopOne` 里的**身份验证**已经覆盖了
  // 同一件事（认不出就不杀），所以那一段被**合并**进 `stopOne`，不再单独存在。
  for (const resident of targets) {
    const who = labelOf(resident)
    if (who) console.log(`[agent] 住户 ${who}：`)
    const scoped = scopedHome(resident)
    if (!(await stopOne('agent', 'agent', residents.residentPidFile(homeDir, resident), scoped))) {
      status = EXIT.PARTIAL
    }
    // 宿主对账：**pid 文件说没跑 ≠ 真没跑**（它不占端口，端口扫碰不到它）。
    // ⚠ 只收**本家这一支**的（种类 ＋ `--root` 家事实）：同一份发布包服务多个家/多支时，
    // 别家/别支的宿主路径一模一样 —— 按路径收就是误杀（本刀要修的就是这一条）。
    for (const orphan of proc.hostOrphanPids(layout, scoped)) {
      console.log(`[agent] 宿主孤儿在场（pid ${orphan}，pid 文件没指向它——托管/重起过的痕迹）——收掉它`)
      await proc.killPid(orphan, { force: true })
      await proc.sleep(300)
    }
    if (proc.hostOrphanPids(layout, scoped).length > 0) {
      console.log('[agent] ⚠ 还有一支宿主没收掉（它可能刚起、或不肯退）')
      status = EXIT.PARTIAL
    }
  }

  if (!options.agentId) {
    if (!(await stopOne('client', 'client', path.join(logDir, 'client.pid')))) status = EXIT.PARTIAL
    if (!(await stopOne('im', 'im', path.join(logDir, 'im.pid')))) status = EXIT.PARTIAL

    // **世界停了 ⇒ 记一份见证**（`world_witness.js`）：世界不在场的那段时间里有没有人动过它，
    // 靠的就是这一条基线。顺序是承重的：**必须在世界真的停了之后**记 —— 世界一跑起来，
    // 任何一句话都会改它，那个窗口就没了。
    // ⚠ 它**不拒绝任何东西**：记不下就如实说一声，绝不假装记了。
    if (!worldWitness.isCleanlyStopped(homeDir)) {
      console.log('[im] 见证：库旁边还有日志兄弟（上一次不是干净停的）—— 这一次**不记基线**，免得造一个假基准')
    } else {
      const recorded = worldWitness.recordCleanStop(homeDir, {
        homeId: confGet(path.join(homeDir, 'home.conf'), 'home_id', null),
      })
      console.log(
        recorded
          ? `[im] 见证：记下这一份（core.db ${recorded.record['core-size']} B）—— 下次起世界之前核它`
          : '[im] 见证：记不下（此刻的事实读不出来）—— 如实说一声，不假装记了'
      )
    }

    // **家的门**（读端面 ＋ 写端面，一个进程）：它们是**读者/写口**（Agent 不依赖），
    // 所以最后收 —— 收不掉也算"只起来一部分"。顺序理由与上面一样：重要的先停。
    if (!(await stopOne('doors', 'doors', doorsPidFile))) status = EXIT.PARTIAL

    // 旧形状的门（迁移桥）：只在它还指向一支**活着的本家旧门**时才走 `stopOne`；
    // 账本早已过期的（常态）就静默清掉，不必为一句"已经在停着"吵人。
    for (const legacy of legacyDoorPidFiles) {
      const ledger = proc.ledgerPid(legacy.target, legacy.file, layout, homeBase)
      if (ledger.state === 'gone') {
        proc.clearPid(legacy.file)
        continue
      }
      if (ledger.state === 'not-ours') {
        console.log(
          `[${legacy.label}] 旧账本里的 pid ${ledger.pid} 不是我们的进程——不杀，只清账本（旧形状迁移）`,
        )
        proc.clearPid(legacy.file)
        continue
      }
      console.log(`[${legacy.label}] 旧形状的门还在跑（pid ${ledger.pid}）—— 收掉它`)
      if (!(await stopOne(legacy.label, legacy.target, legacy.file))) status = EXIT.PARTIAL
    }

    // 最后一道：端口放没放。
    // **远端跑法**：本机没有 core 要收 —— 不碰本机 listen 口（那个口上的东西与这个家无关；
    // 世界那台机器上的 core 由它自己管，`down` 的作用域**只在这台机器**）。
    if (imEndpoint) {
      console.log(`[im] 远端跑法：本机没有 core 要收（世界在 ${imEndpoint}；那边由它自己管）`)
    } else if (!(await sweepPort(imPort, 'im', { prove: imProof }))) status = EXIT.PARTIAL
    if (!(await sweepPort(clientPort, 'client'))) status = EXIT.PARTIAL
    // ★ **门的两个口也要扫**（2026-10-11 补，缺口是真撞上的）：上面 `stopOne('doors', …)` 认的是
    //   **账本**，而账本是**更弱的来源** —— 它可能被清掉、可能是陈旧号。今天真撞上：一个临时家的门
    //   活着占着 5190/5191，而它的账本没了 ⇒ `stopOne` 说"已经在停着（本来就没跑）"、孤儿留在盘上，
    //   后面**每一次产品形态门禁量的都是它**（看着像代码坏了）。
    //   ⇒ 与 im / client 同一道：**端口 ＋ 家事实**（`sweepPort` 先验身份，认不出的**不杀**）。
    //   ⚠ 目标名用 `doors`（它认"听没听这两个口中的任一个"）—— 与 `stopOne` 那一支同源。
    for (const doorPort of [homeBase.statusPort, homeBase.configPort]) {
      if (!(await sweepPort(doorPort, 'doors'))) status = EXIT.PARTIAL
    }
  }

  // Agent 是不是**被硬停的**：正跑着一次活动时关机，环境里会留下一个未收尾的活动。
  // ⚠ **2026-09-26 改**：那个活动**不再挡住下次开机**（一段清醒被一段不清醒隔开就是两段，
  // 重启开新的一段）——所以这是一条**读数**，不是要用户动手的闸。如实说出来。**按住户**读。
  if (P.isFile(layout.statusExe)) {
    for (const resident of targets) {
      const done = require('node:child_process').spawnSync(
        layout.statusExe,
        [resident.envDir, '--agent', resident.agentId],
        { encoding: 'utf8', windowsHide: true },
      )
      const text = `${done.stdout ?? ''}${done.stderr ?? ''}`
      const admission = text
        .split('\n')
        .map((line) => line.replace(/\r$/, ''))
        .find((line) => line.startsWith('admission='))
      if (!admission) continue
      const who = labelOf(resident)
      if (admission.includes('unfinished')) {
        console.log(`[home_down] ⚠ ${who ? `${who}：` : ''}${admission} —— Agent 当时正跑着一次活动，被硬停了。`)
        console.log('[home_down]   它**不挡开机**：下次 up 就开新的一段，那一段留在历史里（看它停在哪： node home.js status）')
        status = EXIT.NEEDS_YOU
      } else {
        console.log(`[home_down] ${who ? `${who} 的` : 'Agent 的'}准入：${admission}`)
      }
    }
  }

  if (options.agentId) {
    console.log(
      `[home_down] 只停了这一支（${options.agentId}）；世界、页面、两个门、别的住户照旧。它的环境、笔记都还在 ${homeDir}（家是持久的，关机不动它们）`,
    )
  } else {
    console.log(`[home_down] Agent 的环境、im 的历史、笔记都还在 ${homeDir}（家是持久的，关机不动它们）`)
  }
  return status
}

module.exports = { down, parseDownArgs }
