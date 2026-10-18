#!/usr/bin/env node
'use strict'
/**
 * **跑一次执行端 ↔ im 的真机边界探针**（`im_probe`）。
 *
 * ## 为什么要有这一层壳
 *
 * 探针本体（`agent/src/bin/im_probe.rs`）只认两样东西：`--im-endpoint` 与环境变量
 * `CELIRA_IM_BACKEND_AUTH`。**凭据不进命令行**（命令行会进进程表、也可能进日志），所以
 * 取凭据这件事必须发生在进程外面 —— 这一层就是干这个的，与 `home.js up` 起宿主时同一条纪律：
 *
 * - 凭据按**身份**去本地存储里取（`credentials.resolve`，与宿主同一支、同一个来源）；
 * - 只经**白名单环境变量**注入子进程，**绝不打印、绝不落盘、绝不进 URL**；
 * - 探针自己的输出原样透传（它只回动词名、稳定状态与稳定失败分类，不含凭据与正文）。
 *
 * ## 用法
 *
 * ```text
 * node scripts/home/probe-agent-im.js [--root <家目录>] [--agent <世界里的 Agent id>] [--room <房间标识>] [--no-room]
 * ```
 *
 * - 缺省读 `CELIRA_HOME_DIR` / `DEFAULT_HOME` 那个家（与 `home.js` 同一处取值）；
 * - **探哪一支**问权威那一支（`runtimeResidents` ＝ `<家>/agents/<支>/` 的新布局 ＋ 还没迁的
 *   老家 `<家>/env`）：家里正好一支就用它；一支都没有 / 不止一支 ⇒ 明确拒绝并说清，
 *   要探多支的家就用 `--agent <id>` 点名（**不猜** —— 猜错就是拿着别人的凭据去探）；
 * - 房间缺省取**这一支自己的** `agent.conf` 的 `room=`（住户化之后"值班室"住在那儿），
 *   再退到 `home.conf` 的 `room_id`（老形状）：房间相关的两个动词只读、要一个真实存在的房
 *   才能探。`--no-room` 明确**不探**房间相关动词（例如刚建的家还没有房）；
 * - 退出码就是探针的退出码（0 全通 / 2 装配不起来 / 3 有确定的失败 / 4 有 `unknown`）。
 *
 * ⚠ 它是**真机门禁**（要一个跑着的 im），进不了 `cargo test`。与 `npm run verify:ui` 同族：
 * **改动执行端的端点面（路径、方法、响应解析、失败分类）时必须跑它** —— 这条写在 `AGENTS.md`。
 */

const path = require('node:path')
const { spawn } = require('node:child_process')

const P = require('./paths.js')
const credentials = require('./credentials.js')
const { confGet } = require('./conf.js')
const { runtimeResidents } = require('./residents.js')
const { readUserEnv } = require('./userenv')

/** 注入子进程的环境变量**白名单**（与状态口 spawn `agent_host_status` 同一条纪律）。 */
const ALLOWED_ENV_KEYS = ['PATH', 'SystemRoot', 'windir', 'TEMP', 'TMP', 'NO_PROXY', 'no_proxy']

function parseArguments(argv) {
  const options = { root: null, room: null, agent: null, noRoom: false }
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index]
    if (flag === '--no-room') {
      options.noRoom = true
    } else if (flag === '--root' || flag === '--room' || flag === '--agent') {
      const value = argv[index + 1]
      if (typeof value !== 'string' || value.trim() === '') {
        throw new Error(`${flag} 后面要给一个非空取值`)
      }
      // 同一个选项给两次**明确失败**：静默取最后一个会让"我以为探的是 A 房"与
      // "它探的是 B 房"看起来一样（与探针本体的解析同一条纪律）。
      if (flag === '--root') {
        if (options.root !== null) throw new Error('--root 给了不止一次')
        options.root = value.trim()
      } else if (flag === '--room') {
        if (options.room !== null) throw new Error('--room 给了不止一次')
        options.room = value.trim()
      } else {
        if (options.agent !== null) throw new Error('--agent 给了不止一次')
        options.agent = value.trim()
      }
      index += 1
    } else {
      throw new Error(`不认识的参数：${flag}`)
    }
  }
  return options
}

/**
 * **探哪一支**（2026-10-05 修：从前这条自己读 `home.conf` 的 `agent_id`）。
 *
 * ⚠ 为什么要改：住户化（2026-10-01）之后 `home.conf` **不再有** `agent_id`（身份搬到
 * `<家>/agents/<支>/identity.json`），于是这个**注册在案的真机门禁**在任何一个新形状的家上
 * 都装配不起来（exit 2）—— 探针一次都没真跑过，而它自己看不出这一点。
 * 判据一句话：**门禁读"这个家住着谁"要问权威那一支**（`runtimeResidents` ＝ 新布局 ＋ 老家），
 * 不许自己拼一份"我以为的家"。
 *
 * 三态：点名了就用它（**不在这个家里 ⇒ 拒绝**，不许探别人）；没点名而家里正好一支 ⇒ 就用它；
 * 一支都没有 / 不止一支 ⇒ 拒绝并说清（**不猜** —— 猜错就是拿着别人的凭据去探）。
 */
function resolveAgent({ explicit, ids }) {
  const list = Array.isArray(ids) ? ids : []
  if (explicit) {
    if (!list.includes(explicit)) {
      return {
        refuse: `这个家里没有 ${explicit} 这一支（家里：${list.length ? list.join(' / ') : '一支都没有'}）`,
      }
    }
    return { agentId: explicit }
  }
  if (list.length === 1) return { agentId: list[0] }
  if (list.length === 0) return { refuse: '这个家里还没有住户（先 node home.js up）' }
  return {
    refuse: `这个家住着 ${list.length} 支（${list.join(' / ')}）—— 用 --agent <id> 点名探哪一支`,
  }
}

/**
 * **探哪个房间**（2026-10-05 同一次修：从前缺省只读 `home.conf` 的 `room_id`）。
 *
 * ⚠ 住户化之后那个"值班室"落在**住户自己**的 `<家>/agents/<支>/agent.conf`（`room=`），
 * `home.conf` 那份是老形状的（新家上通常没有）—— 只读后者会让房间那两个动词被**静默跳过**
 * （读数上看着像"全通"，其实少探了两个）。判据同 `resolveAgent`：**问住户那一支**。
 *
 * 次序是语义：点名 > 这一支自己的房 > 家级那份（老形状）；`--no-room` 是"明确不探"。
 */
function resolveRoom({ explicit, noRoom, residentRoomId, confRoomId }) {
  if (noRoom) return null
  if (explicit) return explicit
  const own = typeof residentRoomId === 'string' ? residentRoomId.trim() : ''
  if (own !== '') return own
  const home = typeof confRoomId === 'string' ? confRoomId.trim() : ''
  return home !== '' ? home : null
}

function childEnvironment(credential) {
  const env = {}
  for (const key of ALLOWED_ENV_KEYS) {
    if (process.env[key] !== undefined) env[key] = process.env[key]
  }
  // 唯一的秘密：只进子进程的环境，不进 argv、不进任何输出。
  env[credentials.BACKEND_ENV] = credential
  return env
}

function main() {
  let options
  try {
    options = parseArguments(process.argv.slice(2))
  } catch (error) {
    console.error(`[probe] ${error.message}`)
    console.error('用法: node scripts/home/probe-agent-im.js [--root <家目录>] [--agent <世界里的 Agent id>] [--room <房间标识>] [--no-room]')
    return 1
  }

  const homeDir = P.homeDir(options.root)
  const layout = P.detectLayout()
  const confFile = path.join(homeDir, 'home.conf')

  // 身份：**问权威那一支**（`runtimeResidents` ＝ 新布局 ＋ 老家），不自己读 `home.conf`
  // ——住户化之后那份里没有 `agent_id`（见 `resolveAgent` 的文档：这个门禁曾因此一次没跑过）。
  const inHome = runtimeResidents(homeDir, confFile)
  const picked = resolveAgent({
    explicit: options.agent,
    ids: inHome.map((resident) => resident.agentId).filter(Boolean),
  })
  if (picked.refuse) {
    console.error(`[probe] ${picked.refuse}`)
    return 2
  }
  const agentId = picked.agentId
  const resident = inHome.find((each) => each.agentId === agentId) ?? null

  // 端点：与宿主同一条取值（`home.conf` 的 listen_addr），拼成宿主用的那个形状。
  const listen = confGet(confFile, 'listen_addr', P.DEFAULT_LISTEN)
  const endpoint = /^https?:\/\//.test(listen) ? listen : `http://${listen}`

  // 凭据：**与 `up.js` 起宿主时同一份口径**（`{ world, agentId }` 那条新形状 → 老路环境变量）。
  // ⚠ 2026-10-05 同一次修：从前这里传的是**字符串** `agentId` —— 那只会去看**老形状**那份文件
  //   （`<目录>/<id>.token`），于是住户化的家上必然 `missing`（凭据实际按「世界 × 身份」分条）。
  //   世界那一维取 `listen_addr` 拼出来的那个端点，与宿主用的是同一个值。
  const resolved = credentials.resolve(
    { world: endpoint, agentId },
    { fallback: readUserEnv(credentials.BACKEND_ENV) },
  )
  if (resolved.state !== 'present' || !resolved.token) {
    console.error(
      `[probe] 取不到 Agent 的 im 凭据（${resolved.state}${resolved.reason ? `：${resolved.reason}` : ''}）` +
        ` —— 这个家还没认领这一支（先 node home.js up），或它还没进这台机器的凭据存储`,
    )
    return 2
  }

  const room = resolveRoom({
    explicit: options.room,
    noRoom: options.noRoom,
    residentRoomId: resident ? resident.roomId : '',
    confRoomId: confGet(confFile, 'room_id'),
  })
  const exe = P.exeIn(layout.agentDir, 'im_probe')

  console.log(`[probe] ${exe}`)
  console.log(`[probe] 家=${homeDir} · 端点=${endpoint} · 房间=${room ?? '（不探）'}`)
  console.log(`[probe] 凭据来源=${resolved.source}（**不打印取值**）`)
  console.log('')

  const args = ['--im-endpoint', endpoint]
  if (room) args.push('--room', room)

  const child = spawn(exe, args, {
    env: childEnvironment(resolved.token),
    stdio: ['ignore', 'inherit', 'inherit'],
    windowsHide: true,
  })

  child.on('error', (error) => {
    console.error(`[probe] 起不来：${error.message}（先 cargo build --release --bin im_probe）`)
    process.exitCode = 2
  })
  child.on('close', (code) => {
    const verdict = code === 0 ? '全通' : code === 3 ? '有确定的失败' : code === 4 ? '★ 有 unknown —— 边界对不上' : `异常退出（${code}）`
    console.log('')
    console.log(`[probe] 结论：${verdict}`)
    process.exitCode = typeof code === 'number' ? code : 2
  })
}

// 直接被跑时才执行；被 `require`（测试）时只拿到那几个纯函数。
if (require.main === module) {
  main()
}

// 只有这五个是给测试的：参数解析、身份选择、房间选择、环境白名单、以及白名单本身。
// 其余（取凭据、起子进程、透传输出）都是副作用，测试里不碰。
module.exports = { parseArguments, resolveAgent, resolveRoom, childEnvironment, ALLOWED_ENV_KEYS }
