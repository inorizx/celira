'use strict'
/**
 * **认领一个世界里已有的 Agent**（2026-09-25；2026-10-01 改成**支级判据**）—— 两个平面之间
 * 那条缝的**机器侧那一半**。
 *
 * ## 为什么需要它（这是今天产品上最大的那个洞）
 *
 * 世界侧能建 Agent（客户端「我的 Agent」→「添加」→ `POST /v1/agents`），但那只建了**身份**：
 * 不写配置、不发后端凭据、不起任何东西 —— 所以**她不会回复**。
 * 反过来，`home.js up` 的归属是"家里还没有住户时**自建**一支"，**不是认领**。
 * ⇒ v1 里缺一条把"世界里的她"与"这台机器上的身体"接起来的路。
 *
 * 这一层补的就是那条路：**给一个已存在的世界身份安家**（`node home.js up --agent <id>`）。
 * 它不产生任何新的公共事实 —— 只是让两个已经存在的身份对上号：世界侧的身份 ＋ 机器侧的身体。
 *
 * ## 三条判据（都不许含糊）
 *
 * 1. **支级判据**（承 PROJECT_SHAPE §8.0）：从"这个家有没有主"改成"**这支**安没安家" ——
 *    家里已经有别的住户**不再拒绝**（一台机器可以跑多支）；问的是"**这个 id** 是不是已经
 *    是这个家的住户"；
 * 2. **只能认领自己的**：`GET /v1/agents` 只返回**认证者拥有的** Agent ⇒ 那个 id 必须在里面，
 *    而且 `status` 必须是 `active`（撤销过的身份不许安家）；
 * 3. **问不到就不动**：世界连不上 / 回执读不懂 ⇒ **拒绝**（绝不"先写上，回头再说"）。
 *
 * 本模块的裁决是**纯函数**（`planAdoption`），世界那两问走注入的 `imRequest` ⇒ 可机械判错。
 */

/**
 * 认领的裁决：**这支** Agent 安没安家。
 *
 * @param {{residentAgentIds?: string[], requestedId?: string|null}} input
 *   `residentAgentIds` 是**本家现有的住户身份**（不含"将要以 --agent 认领的那一支"）；
 *   `requestedId` 是 `--agent` 的取值（`undefined`/`null` = 没给这个旗标）。
 */
function planAdoption({ residentAgentIds = [], requestedId }) {
  const residents = Array.isArray(residentAgentIds)
    ? residentAgentIds.map((value) => String(value).trim()).filter(Boolean)
    : []
  // **"没给这个旗标"与"给了个空值"是两件事**（`backup --to` 那一课）：前者照旧（自建那条路），
  // 后者是**用法错** —— 否则一次"认领"会悄悄变成"自建一个新身份"。
  if (requestedId === undefined || requestedId === null) return { kind: 'as-is' }
  const want = typeof requestedId === 'string' ? requestedId.trim() : ''
  if (want === '' || want.includes(' ') || want.length > 200) return { kind: 'refuse-bad-id' }
  if (residents.includes(want)) return { kind: 'already', agentId: want }
  return { kind: 'adopt', agentId: want }
}

/**
 * 问世界：这个 id 是不是**我的** Agent。
 *
 * 返回 `{ ok, reason, name, status, lines }`；`lines` 是给主人看的读数（调用方原样打印）。
 * `reason` 是稳定分类：`unreachable` / `not-owned` / `not-active` / `ok`。
 */
async function verifyOwnedAgent({ base, token, agentId, imRequest }) {
  const lines = []
  const answer = await imRequest(base, 'GET', '/v1/agents', { bearer: token })
  if (answer.status !== 200) {
    return {
      ok: false,
      reason: 'unreachable',
      lines: [`[认领] 问不到世界（GET /v1/agents → ${answer.status}）—— 认领要先能问世界，这一次什么都不改`],
    }
  }
  const agents = Array.isArray(answer.body?.agents) ? answer.body.agents : null
  if (!agents) {
    return {
      ok: false,
      reason: 'unreachable',
      lines: ['[认领] 世界的回执读不懂（没有 agents 列表）—— 什么都不改'],
    }
  }
  const found = agents.find((entry) => entry?.agent?.id === agentId)
  if (!found) {
    return {
      ok: false,
      reason: 'not-owned',
      lines: [
        `[认领] 世界里没有这个 Agent，或者它不是你的：${agentId}`,
        `[认领] （GET /v1/agents 只列**你拥有的**，共 ${agents.length} 个 —— 换一个 id，或先在客户端把它建出来）`,
      ],
    }
  }
  const status = typeof found.agent.status === 'string' ? found.agent.status : ''
  const name = typeof found.agent.display_name === 'string' && found.agent.display_name ? found.agent.display_name : agentId
  if (status !== 'active') {
    return {
      ok: false,
      reason: 'not-active',
      lines: [`[认领] 这个 Agent 不是 active（status=${status || '未知'}）—— 撤销过的身份不许安家`],
    }
  }
  lines.push(`[认领] 问过世界了：${name}（${agentId}）是你的，status=active`)
  return { ok: true, reason: 'ok', name, status, lines }
}

/**
 * 找**已有的值班室**：她与主人的那间 direct room。
 *
 * 为什么要有它：认领的对象多半是"先在客户端建出来、也许已经聊过"的身份 —— 房间是**公共事实**，
 * 不该因为"换了机器"就多一间（**一间房 = 一段关系**，批次二之后服务端也只认那一间）。
 * 读不到房间列表 ⇒ 返回 `null` 并**如实说一句**（照旧开一间，那是幂等的旧行为）。
 */
async function findExistingDirectRoom({ base, token, agentId, imRequest }) {
  const answer = await imRequest(base, 'GET', '/v1/rooms', { bearer: token })
  if (answer.status !== 200 || !Array.isArray(answer.body?.rooms)) {
    return { roomId: null, looked: false }
  }
  const hit = answer.body.rooms.find(
    (entry) => entry?.room?.kind === 'direct' && entry?.peer?.id === agentId,
  )
  return { roomId: typeof hit?.room?.id === 'string' && hit.room.id ? hit.room.id : null, looked: true }
}

module.exports = { planAdoption, verifyOwnedAgent, findExistingDirectRoom }
