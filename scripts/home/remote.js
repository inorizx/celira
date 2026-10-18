'use strict'
/**
 * **远端跑法**（`node home.js up --im-endpoint <世界地址> [--token <主人凭据>]`，2026-09-30）——
 * 本机**不起 core**，对着**在别处跑着的世界**做同一套布置，再让本机 Agent 接上它。
 *
 * ## 一句话原理
 *
 * 「布置」从前是对着**本机**的库做引导（`bootstrap-human` / `issue-human-session` 直接摸本机的
 * SQLite）；本模块把同一套布置改成对着**显式声明的世界地址**走 HTTP，其中"人"那一步用外部给的
 * 主人凭据替代 ⇒ 编排链的每一段都指向那个地址，本机因此可以只跑执行端那一半（runtime 子包）。
 *
 * ## 三条纪律（与 `world.js` 的三态、`imapi.js` 的"HTTP 错误是读数不是异常"同一条）
 *
 * 1. **世界是显式声明的**：地址只从 `--im-endpoint` 来，绝不猜、绝不回落 —— 没给这个旗标就是
 *    本机全量（既有行为一字不动）；给了而世界侧不可用 ⇒ **fail-closed**，不静默换一个世界。
 * 2. **凭据只按来源说**：`--token` 的值只经内存与请求头（`Bearer`），不打印、不落盘、不进日志；
 *    读数的世界里只有"给了 / 没给 / 来自旗标还是环境"。
 * 3. **三种卡点分开说**：世界不可达 / 凭据被拒 / 世界里没有可认领的身份 —— 三种事实，三种读数，
 *    绝不混成一句"失败了"；每一种都**停在动手之前**（布置一个字都没写）。
 *
 * 本模块是**纯逻辑 ＋ 注入的 imRequest**：`classifyWorldAnswer` / `findClaimableIdentity` /
 * `parseImEndpoint` / `remoteTokenFrom` 是纯函数，两个 `preflight*` 只经注入的请求函数说话
 * ⇒ 全部可机械判错（照 `world.js` / `adopt.js` 的先例）。
 */

const { imRequest } = require('./imapi')
const { readUserEnv } = require('./userenv')

/** 主人凭据的**环境变量**来源（与 `CELIRA_IM_BACKEND_AUTH` 同一族；只在布置那一次用）。 */
const HUMAN_ENV = 'CELIRA_IM_HUMAN_AUTH'

/** 预检的耐心：这是开机路上的一句话（世界多半就在隔壁或同网），等不到就如实说"不可达"。 */
const PREFLIGHT_TIMEOUT_MS = 10000

/**
 * 解析 `--im-endpoint` 的取值（**纯函数** ⇒ 可判错）。
 *
 * 只收"协议头 ＋ 主机[:端口]"这一种形状（世界地址就是根）：带路径 / 空格 / 查询 / 片段一律
 * 当场拒 —— 那些形状到了 Agent 那里只会变成看不懂的 404，晚说不如早说。**不修复、不猜**：
 * 缺 `http://` 就说不缺什么，绝不替用户补一个前缀（照"绝不静默修复"的既有纪律）。
 *
 * @returns {{ok: true, base: string} | {ok: false, error: string}}
 */
function parseImEndpoint(value) {
  const raw = typeof value === 'string' ? value.trim() : ''
  if (raw === '') return { ok: false, error: '要一个世界地址（形如 http://主机:端口）' }
  const scheme = /^https?:\/\//i.exec(raw)
  if (!scheme) {
    return { ok: false, error: `要带 http:// 或 https:// 开头（收到的是「${raw}」）` }
  }
  // ⚠ 尾斜杠只从**协议头之后**那一段去（`http://` 本身以斜杠结尾 —— 先整体去尾斜杠会把
  // 协议头啃掉一半，真机判据就是这么写的）。
  const rest = raw.slice(scheme[0].length).replace(/\/+$/, '')
  if (rest === '') return { ok: false, error: '只有协议头，没有主机（形如 http://主机:端口）' }
  if (/[\s?#]/.test(rest)) {
    return { ok: false, error: '不要带空格 / 查询 / 片段（只要 主机:端口）' }
  }
  if (rest.includes('/')) {
    return { ok: false, error: '只要 主机:端口，不要带路径（世界地址就是根）' }
  }
  return { ok: true, base: `${scheme[0]}${rest}` }
}

/**
 * 主人凭据从哪来（**纯函数** ＋ 注入的环境读取 ⇒ 可判错）。
 *
 * 顺序：`--token` 的值 → 环境变量 `CELIRA_IM_HUMAN_AUTH` → 没有。
 * 没有它**不一定是错**：一个已经布置好的远端家，重跑不需要主人凭据（Agent 手里那支后端凭据
 * 才是它之后要用的）；只有"这一次还需要动世界"的那些步骤才要它（`provision` 会当场说）。
 */
function remoteTokenFrom({ flagToken = null, readEnv = readUserEnv } = {}) {
  const flag = typeof flagToken === 'string' ? flagToken.trim() : ''
  if (flag !== '') return { token: flag, source: 'flag' }
  let fromEnv = null
  try {
    fromEnv = readEnv(HUMAN_ENV)
  } catch {
    fromEnv = null
  }
  const value = typeof fromEnv === 'string' ? fromEnv.trim() : ''
  if (value !== '') return { token: value, source: 'env' }
  return { token: null, source: null }
}

/**
 * `GET /v1/agents` 的回执该怎么读（**纯函数** ⇒ 四态可判错）。
 *
 * - `unreachable`：连不上 / 超时（`status === 0`，与 `imapi.js` 的约定一致）；
 * - `refused`：世界**明确拒绝**了这支凭据（401 / 403）；
 * - `ok`：200 且带着 agents 列表（**只有这个形状才配往下走**）；
 * - `unreadable`：其余一切（5xx、200 但形状不对）—— 读不懂的状态不硬猜。
 */
function classifyWorldAnswer(answer) {
  if (!answer || answer.status === 0) return 'unreachable'
  if (answer.status === 401 || answer.status === 403) return 'refused'
  if (answer.status === 200 && Array.isArray(answer.body?.agents)) return 'ok'
  return 'unreadable'
}

/**
 * 在 `GET /v1/agents` 的列表里找**可认领**的那个身份（**纯函数** ⇒ 三态可判错）。
 *
 * 判据与 `adopt.js` 同款（世界只列"你拥有的"；撤销过的身份不许安家），但读的是**已经拿到手**
 * 的那一份列表 —— 预检因此只花一次请求。
 *
 * @returns {{ok: true, name: string, status: string}
 *          |{ok: false, reason: 'missing'|'not-active', count?: number, status?: string, name?: string}}
 */
function findClaimableIdentity(agents, agentId) {
  const list = Array.isArray(agents) ? agents : []
  const found = list.find((entry) => entry?.agent?.id === agentId)
  if (!found) return { ok: false, reason: 'missing', count: list.length }
  const status = typeof found.agent.status === 'string' ? found.agent.status : ''
  const name =
    typeof found.agent.display_name === 'string' && found.agent.display_name
      ? found.agent.display_name
      : agentId
  if (status !== 'active') return { ok: false, reason: 'not-active', status, name }
  return { ok: true, name, status }
}

/** 停手那两行是**所有卡点共用**的（"什么都没改"必须每次都说得出口）。 */
const STOP_LINES = ['[远端] 停手：这一次没有对任何世界做任何改动（本机也没起 core）']

/**
 * 带**主人凭据**的预检：世界可达吗？凭据被认下吗？要认领的身份在吗？
 *
 * 一次 `GET /v1/agents` 回答全部三问。**只读**：预检一个字都不写（建身份、签凭据都在布置那一步，
 * 排在它后面）。
 *
 * @returns {Promise<{ok: boolean, reason: string, lines: string[]}>} `lines` 是给主人看的读数
 */
async function preflightWorld({
  base,
  token,
  adoptAgentId = null,
  confAgentId = null,
  imRequest: request = imRequest,
}) {
  const answer = await request(base, 'GET', '/v1/agents', {
    bearer: token,
    timeoutMs: PREFLIGHT_TIMEOUT_MS,
  })
  const verdict = classifyWorldAnswer(answer)
  if (verdict === 'unreachable') {
    return {
      ok: false,
      reason: 'unreachable',
      lines: [
        `[远端] 世界**不可达**：${base}（${answer && answer.error ? answer.error : '连不上'}）`,
        ...STOP_LINES,
        '[远端] 查一下地址与网络；那台机器上的世界在不在跑：在那边敲 node home.js status',
      ],
    }
  }
  if (verdict === 'refused') {
    return {
      ok: false,
      reason: 'refused',
      lines: [
        `[远端] 世界**拒绝了这支主人凭据**（GET /v1/agents → HTTP ${answer.status}）：${base}`,
        ...STOP_LINES,
        '[远端] 凭据用世界那台机器第一次开机打印的 clr_hs_ 那一串（丢了：在那边 node home.js up --token 补发一支）',
      ],
    }
  }
  if (verdict === 'unreadable') {
    return {
      ok: false,
      reason: 'unreadable',
      lines: [
        `[远端] 世界的回执**读不懂**（GET /v1/agents → HTTP ${answer ? answer.status : '?'}）：` +
          `${JSON.stringify(answer ? answer.body : null).slice(0, 200)}`,
        ...STOP_LINES,
        '[远端] 那个地址上真的是 Celira 的 im 服务端吗？（世界地址是根，不带路径）',
      ],
    }
  }
  const okLines = [`[远端] 世界可达、主人凭据被认下：${base}`]
  const wanted = adoptAgentId ?? confAgentId
  if (!wanted) return { ok: true, reason: 'ok', lines: okLines }
  const found = findClaimableIdentity(answer.body.agents, wanted)
  if (found.ok) {
    okLines.push(
      adoptAgentId
        ? `[远端] 认领目标在场：${found.name}（${wanted}）status=active`
        : `[远端] 这个家记着的身份在这个世界里：${found.name}（${wanted}）status=active`,
    )
    return { ok: true, reason: 'ok', lines: okLines }
  }
  if (found.reason === 'not-active') {
    return {
      ok: false,
      reason: 'not-active',
      lines: [
        `[远端] 这个 Agent 不是 active（status=${found.status || '未知'}）—— 撤销过的身份不许安家：${wanted}`,
        ...STOP_LINES,
      ],
    }
  }
  if (adoptAgentId) {
    return {
      ok: false,
      reason: 'not-claimable',
      lines: [
        `[远端] 这个世界里**没有这个 Agent，或者它不是你的**：${adoptAgentId}`,
        `[远端] （GET /v1/agents 只列你拥有的，共 ${found.count} 个 —— 换一个 id，或先在客户端把它建出来）`,
        ...STOP_LINES,
      ],
    }
  }
  // conf 里记着的身份不在这个世界里：那是"换了世界"的形状，给出两条出路（不是含糊的 404）。
  return {
    ok: false,
    reason: 'stale-identity',
    lines: [
      `[远端] 这个家记着的 Agent **不在这个世界里**：${wanted}`,
      '[远端] （home.conf 里的身份是**某个世界**的事实 —— 换世界时它不通用）',
      `[远端] 两条出路：① 认领这个世界里已有的一个： --agent <那个世界里的 id>；` +
        '② 清掉 home.conf 的 agent_id 再开机（会在远端新建一个身份）',
      ...STOP_LINES,
    ],
  }
}

/**
 * 带**已存后端凭据**的预检（没有主人凭据时的重跑路）：世界可达吗？那支凭据还认吗？代表的还是本家吗？
 *
 * 这条路存在的理由：一个布置好的远端家，日常重跑**不该**要求主人凭据（Agent 之后用的是它自己的
 * 后端凭据）——但"世界还在不在、凭据还认不认"必须先问清楚，否则一开机就是聋的。
 */
async function preflightStoredCredential({
  base,
  agentId,
  credential,
  imRequest: request = imRequest,
}) {
  const me = await request(base, 'GET', '/v1/backend/me', {
    bearer: credential,
    timeoutMs: PREFLIGHT_TIMEOUT_MS,
  })
  if (!me || me.status === 0) {
    return {
      ok: false,
      reason: 'unreachable',
      lines: [
        `[远端] 世界**不可达**：${base}（${me && me.error ? me.error : '连不上'}）`,
        ...STOP_LINES,
      ],
    }
  }
  if (me.status !== 200) {
    return {
      ok: false,
      reason: 'credential-rejected',
      lines: [
        `[远端] 世界**不认这支后端凭据**（GET /v1/backend/me → HTTP ${me.status}）—— 可能被撤销 / 轮换过`,
        '[远端] 带主人凭据重来一次会按本家重签一支： --token <那个世界的主人凭据>',
        ...STOP_LINES,
      ],
    }
  }
  const represented = me.body?.agent?.id ?? null
  if (represented !== agentId) {
    return {
      ok: false,
      reason: 'wrong-identity',
      lines: [
        `[远端] 这支凭据代表 ${represented ?? '认不出来'}，本家是 ${agentId} —— 对不上，停手`,
        '[远端] 带主人凭据重来一次会按本家重签一支： --token <那个世界的主人凭据>',
        ...STOP_LINES,
      ],
    }
  }
  return { ok: true, reason: 'ok', lines: [`[远端] 世界可达、后端凭据被认下（代表 ${agentId}）：${base}`] }
}

/**
 * 不带 `--im-endpoint` 时，这个家能不能走**本机全量**（**纯函数** ⇒ 可判错）。
 *
 * 为什么要这一问：远端家一旦裸跑 `up` 而静默回落成"本机全量"，就会对着本机的库去办**另一个
 * 世界**的身份（agent_id / room_id 都不是本机的）——那是"静默回落"的教科书形状。所以：
 * 记着世界的家，裸跑 `up` 一律**停手并说清两条出路**；`up --token`（补发）也同款 —— 补发是
 * **世界那台机器**上的事。
 */
function localStartVerdict({ confImEndpoint, wantToken = false }) {
  if (!confImEndpoint) return { ok: true, lines: [] }
  const lines = [`[home_up] 这个家是**远端跑法**：世界在 ${confImEndpoint}（本机不起 core）`]
  if (wantToken) {
    lines.push('[home_up] 补发 Human token 是**世界那台机器**上的事：在那边敲 node home.js up --token')
    lines.push('[home_up] 本机这半边不需要 token（Agent 用的是它自己的后端凭据）')
  } else {
    lines.push(`[home_up] 开机要带上那个世界： node home.js up --im-endpoint ${confImEndpoint}`)
    lines.push(
      '[home_up] 要改回本机全量：清掉 home.conf 的 im_endpoint ＋ human_id / agent_id / room_id 三行' +
        '（后三行是那个世界的事实）再开机 —— 见手册「世界在别处」与「重新奠基」',
    )
  }
  return { ok: false, lines }
}

module.exports = {
  HUMAN_ENV,
  PREFLIGHT_TIMEOUT_MS,
  STOP_LINES,
  parseImEndpoint,
  remoteTokenFrom,
  classifyWorldAnswer,
  findClaimableIdentity,
  preflightWorld,
  preflightStoredCredential,
  localStartVerdict,
}
