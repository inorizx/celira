'use strict'
/**
 * **这个家与那个世界的关系** —— 只回答一个问题：某个地址上的 im 服务端，是不是**本家**的世界？
 *
 * ## 为什么需要它
 *
 * 家事实里 im 那一维只能是**端口**（`celira-im.exe` 不带参数，命令行里没有家的痕迹），
 * 而端口是**机器级**命名空间 ⇒ 两个家用同一组默认端口时，判据分不出彼此。
 * 真机实测（2026-09-25，同包 + 同端口）：本家把**别家的世界**认成自己的、把它的 pid 写进
 * 本家账本，`down` 于是可能停掉别家的世界 —— 那是这套编排里最大的跨家爆炸半径。
 *
 * ## 证明只用本家自己的东西，且不新增任何事实
 *
 * 用**本家的后端凭据**问一句 `GET /v1/backend/me`：那个世界认下它、并且说是**本家的 Agent**
 * ⇒ 是本家的世界（后端凭据只在本家的世界里有效）。不读数据库、不问 `/v1/home`（那是认证
 * Human 的端点），也不新增 home.conf 键。
 *
 * ## 三态（"判不了"必须与"不是"分开 —— 与 `proc.ledgerPid` 的四态同一条纪律）
 *
 * - `ours`    认下凭据且代表本家的 Agent ⇒ **自己人**（可以认领、可以收）；
 * - `foreign` 认下凭据却代表**别的** Agent ⇒ 端口上跑的是**别家**的世界
 *             （本家的凭据被别家的世界认下，只可能是"本机凭据格被那个家占着、而端口是它的"）；
 * - `unknown` 没凭据 / 它不认这支凭据 / 连不上 ⇒ **判不了**，不是"不是"。
 *             ⚠ 本家凭据格被别家占了时，**本家的世界**也会给 `unknown` ⇒
 *             这一档**不许**用来否认本家，只许用来"不认领、不动手"。
 */

const { imRequest } = require('./imapi')

/** 证明用的超时：这是开机路上的一句话，等不到就如实说"判不了"（默认 20 秒太长）。 */
const PROVE_TIMEOUT_MS = 5000

/**
 * 三态裁决（**纯函数**：喂一份 `GET /v1/backend/me` 的回执就够 ⇒ 可机械判错）。
 *
 * @param {{credential: string|null, agentId: string|null, me: {status:number, body:any}|null}} input
 * @returns {'ours'|'foreign'|'unknown'}
 */
function classifyOwnership({ credential, agentId, me }) {
  if (!credential || !agentId) return 'unknown'
  if (!me || me.status !== 200) return 'unknown'
  const represented = me.body?.agent?.id
  if (typeof represented !== 'string' || represented === '') return 'unknown'
  return represented === agentId ? 'ours' : 'foreign'
}

/**
 * 真的去问一句。**永不抛**（HTTP 层的失败是读数，不是异常 —— 与 `imapi.js` 同一条纪律）。
 *
 * @returns {Promise<{state:'ours'|'foreign'|'unknown', reason:string|null, represented:string|null}>}
 */
async function proveWorldOwnership({
  base,
  credential,
  agentId,
  imRequest: request = imRequest,
  timeoutMs = PROVE_TIMEOUT_MS,
}) {
  if (!credential || !agentId) return { state: 'unknown', reason: 'no-credential', represented: null }
  const me = await request(base, 'GET', '/v1/backend/me', { bearer: credential, timeoutMs })
  const state = classifyOwnership({ credential, agentId, me })
  const represented = typeof me?.body?.agent?.id === 'string' ? me.body.agent.id : null
  if (state !== 'unknown') return { state, reason: null, represented }
  return {
    state,
    reason: me && me.status === 0 ? 'unreachable' : `status-${me ? me.status : 0}`,
    represented,
  }
}

module.exports = {
  PROVE_TIMEOUT_MS,
  classifyOwnership,
  proveWorldOwnership,
}
