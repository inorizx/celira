'use strict'
/**
 * **布置一支 Agent 的安家**（一支一次）—— bash 版是一段 ~140 行的 python agentedoc，这里原样搬成 Node。
 *
 * ## 一支 Agent 的六步（幂等：每一步都先看"有没有"）
 *   1. Human：没有就 `bootstrap-human`（token 只在这里打印一次）；有就补一支 Session 用于布置；
 *   2. 身份（Agent）：住户已有身份就用它；`--agent` 认领世界里已有的一个；都没有就 `POST /v1/agents` 自建；
 *   3. 这支的**后端凭据**：按「**世界 × agent_id**」分条的本地存储 → 用
 *      `GET /v1/backend/me` 问它代表谁 → 不是本家就换一份
 *      （凭据属于**Agent × 那个世界**，代表谁**由服务端说了算**，不靠猜；
 *       `CELIRA_IM_BACKEND_AUTH` 是老路/手工路的回落，仍读、不再写）；
 *   4. 一间房（值班室）：`POST /v1/direct-rooms`（请求体只有 `agent_id`——**direct 房没有
 *      标题**，2026-09-27 批次二；`ROOM_NAME` 只是日志里的人话，不是房名）；
 *   5. **支级配置**（`<环境根>/agent.conf`：身份 / 值班室 / 住户序号 / 显示名）—— 住户化的落点；
 *   6. 这支的**环境**（`agent_run init`）：已有 `identity.json` 就不动。
 *
 * ## 幂等键：按「**家 × 住户**」取（2026-10-01 多支刀）
 *
 * 形状：**住户 1** 用 `home-agent:<家 id>`（**沿用第三刀的旧形状，无后缀**）；
 * **住户 2 起**用 `home-agent:<家 id>:<住户序号>`。
 *
 * ⚠ **这个不对称是有意的，别"顺手统一"**：第三刀之后的家，世界里那支身份就是用
 * `home-agent:<家 id>` 建的 —— 给住户 1 加上后缀会让它**在世界里另建一支 Agent**
 * （旧那支留着不动，静默重复）。而住户 2 起本来就没有旧记录可沿用，带上序号才能让
 * **同一台机器上两支不同的 Agent 各建各的**（不带序号时，第二支的键会撞上第一支的
 * 幂等记录：同 Human 同键、目标却不同 ⇒ 409 idempotency_conflict）。
 *
 * ## 凭据的键：`<世界> × <agent_id>`（承 PROJECT_SHAPE §8.0 推论 3）
 *
 * 同一台机器上两支 Agent 可以住同一个世界（不同的身份），也可能住不同的世界 ——
 * 凭据文件按「世界摘要 / 身份」分条（`credentials.js`），**读 / 写 / 轮换 / 删除同一个键**。
 *
 * ## 凭据纪律
 *
 * 后端凭据**只经环境**进子进程、**只落按身份分条的本地存储**（家之外）；
 * **不打印、不进 argv、不进家目录**。
 *
 * ## 远端跑法（2026-09-30）：`ctx.remote = { base, token }`
 *
 * 给了它 ⇒ **对着那个世界做同一套布置**（`base` 换掉本机的口），并且**跳过铸人那一步**：
 * 主人凭据由外部给（`--token` / 环境变量），本机**不碰任何 core 的 CLI**（runtime 子包没有它）。
 * `token` 可以是 `null`（布置好的家重跑）——走到**任何需要动世界**的步骤时当场 fail-closed，
 * 说清"这一次还需要主人凭据"，绝不静默换个来源。
 *
 * ## 老家的后果（如实登记、不静默；形状迁移在 `residents.js`）
 *
 * `home.conf` 里没有家 id 的家，第一次跑会生成一个 ⇒ 它世界里那支 Agent 是旧键建的，
 * 键换了 ⇒ **会在世界里另建一支 Agent**（旧那支留着不动；旧环境整份让位到
 * `<家>/env.old-<旧 Agent id>`，不删；**模型绑定跟着带到新环境** —— 不带过去的话，
 * 绑定在场却读不到 ⇒ 宿主会拒绝启动）。预发布破坏式收敛，可接受。
 */

const fs = require('node:fs')
const path = require('node:path')
const crypto = require('node:crypto')
const { spawnSync } = require('node:child_process')

const { confGet, confSet } = require('./conf')
const { imRequest } = require('./imapi')
const { readUserEnv } = require('./userenv')
const credentials = require('./credentials')
const P = require('./paths')
const { planAdoption, verifyOwnedAgent, findExistingDirectRoom } = require('./adopt')
const { writeAgentConf } = require('./residents')
const { ledgerBody } = require('./ledger_line')
// 环境变量名**只有一处权威**（`remote.js` 的 `HUMAN_ENV`）：读数里那句也用它拼，别抄第二份。
const { HUMAN_ENV: HUMAN_TOKEN_ENV } = require('./remote')

const BACKEND_ENV = 'CELIRA_IM_BACKEND_AUTH'
/**
 * 首位 Human 的显示名：**由人给**（`node home.js up --human-name <名字>`，2026-10-20 主人裁决
 * "必须要填"）。没给 ⇒ 占位 `未命名`（明确是占位，不是称呼），而且**读数要说出来**。
 *
 * ## 为什么不再取这台机器的用户名
 *
 * 判据（主人 2026-10-03 原话的意思）：**显示名是公共呈现，不是关系**。用户名是**机器事实**
 * —— 把它写进公共世界就是拿机器事实冒充公共事实；而且它常常不是你（`Administrator` /
 * `user` / `DESKTOP-8K2LQ`），同一台机器上两个人也分不开。
 *
 * 这与 Agent 那条（2026-10-12）是**同一条**：系统补能力缺口，不替谁认定身份（判据 9）。
 *
 * ⚠ 壳那一侧同样的规矩在 `app/src-tauri/src/local_world.rs` 的 `normalize_human_name`
 * （页面那条路）—— 两处**规则逐条同名**，但壳不再从环境取名字。
 * ⚠ 改名的路**已经有了**（2026-10-18 切片：只有本人能写自己的 `display_name`，
 * `CORE_V1_PARTICIPANT_PRESENTATION.md` §4）⇒ 占位与起错都换得掉。
 */
const HUMAN_DISPLAY_NAME_PLACEHOLDER = '未命名'

/**
 * 名字这一格合不合格（与 Core 的 `DisplayName::parse` 同一套三条，见契约 4.1）：
 * 禁 U+0000、至多 80 个 Unicode scalar value、trim 后非空。**权威是世界那一侧** ——
 * 这一道只为了"失败时说得出为什么"，不是第二个裁决者。
 *
 * @returns {string|null} 不合格 ⇒ 一句人话；合格 ⇒ `null`
 */
function humanNameProblem(raw) {
  if ([...String(raw ?? '')].includes('\u0000')) return '名字里不能有控制字符'
  const trimmed = String(raw ?? '').trim()
  if (trimmed === '') return '名字不能是空的'
  if ([...trimmed].length > 80) return '名字最多 80 个字（按字符数，不按字节）'
  return null
}
/**
 * 自建那支 Agent 的**默认显示名**（2026-10-12 改）。
 *
 * 判据与上面首位 Human 那条**是同一条**：**显示名是公共呈现，不是关系** —— 系统补能力缺口，
 * 不替谁认定身份（判据 9）。从前这里写死 `小满`：一个**具体的名字**被当成"默认"，等于
 * **系统借一个昵称替那支 Agent 认定了身份**；而且它是一个人的名字躺在**开源仓库**里 ——
 * 既不该有，也不可能对所有语言都对。
 *
 * 现在的规矩：
 *   · **人给了名字就用人的**（`node home.js up --agent-name <名字>`）—— 名字归它自己或它的
 *     Owner（表达权见 `CORE_V1_PARTICIPANT_PRESENTATION.md` §4）；
 *   · **没给就是占位**：`未命名`（明确是占位，不是称呼），而且读数要说出来。
 *
 * ⚠ 与 Human 那条的差别（2026-10-20 更正）：Human 的显示名**也有改的路**了 ——
 * 2026-10-18 的切片只允许**本人**写自己的 `display_name`（`CORE_V1_PARTICIPANT_PRESENTATION.md`
 * §4）⇒ 占位与起错都换得掉，只是从"我的资料"里改。
 */
const AGENT_DISPLAY_NAME_PLACEHOLDER = '未命名'
/** 「值班室」是**叫法**（日志与手册里的人话），不是房名：direct 房没有标题（批次二）。 */
const ROOM_NAME = '值班室'

/**
 * 四个幂等键的**基础名**；世界里按 `<基础名>:<家 id>[:<住户序号>]` 的提交原文比较
 * （`provisioningKeys`）。从前的形状是裸的 `home-agent` / `home-binding` / … ——
 * 那是**世界级**的常量（见文件头）。
 */
const KEY_NAMES = {
  agent: 'home-agent',
  binding: 'home-binding',
  credential: 'home-credential',
  direct: 'home-direct',
}

/**
 * 这个家**这支住户**的布置键（2026-10-01）：`<基础名>:<家 id>`（住户 1，**无后缀**）
 * 或 `<基础名>:<家 id>:<住户序号>`（住户 2 起）。
 *
 * 形状纪律：冒号是可见 ASCII（世界的幂等键允许 1–128 个可见 ASCII、不含逗号），
 * 一眼能读出这把键属于哪个家的哪一支。**住户 1 的不对称是有意的** —— 见文件头。
 */
function provisioningKeys(homeId, slot = 1) {
  const suffix = slot === 1 ? '' : `:${slot}`
  return {
    agent: `${KEY_NAMES.agent}:${homeId}${suffix}`,
    binding: `${KEY_NAMES.binding}:${homeId}${suffix}`,
    credential: `${KEY_NAMES.credential}:${homeId}${suffix}`,
    direct: `${KEY_NAMES.direct}:${homeId}${suffix}`,
  }
}

/**
 * 家 id 的**读取**（读得到就逐字用；没有 ⇒ `null`，等第一次真的要用键时再生成 —— 见下）。
 *
 * ⚠ 形状迁移（旧环境让位、`agent_id` / `room_id` 两行让位）**不在这一步**：它是
 * `residents.js` 的 `migrateHomeShape`，由 `up` 排在**读住户之前**调用（第三刀的顺序先例）。
 * 这里只做"读"，因为布置本身不搬环境。
 *
 * @returns {{homeId: string|null, lines: string[], refuse: string|null}}
 */
function ensureHomeId({ confFile }) {
  const existing = (confGet(confFile, 'home_id') || '').trim()
  if (existing !== '') return { homeId: existing, lines: [], refuse: null }
  return { homeId: null, lines: [], refuse: null }
}

function fail(message) {
  return { ok: false, message }
}

/**
 * **凭据那一步的裁决与动作**（从流程里提出来 ⇒ 可机械判错；注入点与 `obtainBackendCredential` 同款）。
 *
 * 三条路，按优先级：
 *  1. **存储里那份可用** ⇒ 不动（`keep`）：它是权威位置，且没有轮换的理由；
 *  2. **只有环境变量那份（或老形状那份）、而它确实是本家的** ⇒ **迁移**：原文照搬进
 *     按「世界 × 身份」分条的存储（**不轮换** —— 轮换会撤销 Agent 手里那支，而这一步
 *     没有任何需要轮换的理由）；
 *  3. **都没有 / 那支不属于本家** ⇒ `backend = null`，交给调用方走 `issue-or-rotate`。
 *
 * ⚠ 凭据读不出来（`unreadable`）**不许回落到环境变量**：那会把"读不出来"伪装成
 * "用环境里那支"，掩盖真问题（`credentials.resolve` 也是这么定的）。
 *
 * 返回 `{ backend, backendUsable, lines }`：`lines` 是给主人看的读数（调用方原样打印）。
 */
async function resolveBackendCredential({
  base,
  agentId,
  resolved,
  agentRunning = false,
  imRequest: request = imRequest,
  write = credentials.write,
}) {
  const lines = []
  if (resolved.state === 'unreadable') {
    lines.push(`[布置] ⚠ 凭据文件读不出来（${resolved.reason}）—— 按"没有"处理：这一次会重新签发一支`)
  }
  let backend = resolved.token
  let backendUsable = false
  if (backend) {
    const me = await request(base, 'GET', '/v1/backend/me', { bearer: backend })
    const represented = me.status === 200 ? me.body?.agent?.id ?? null : null
    if (represented === agentId) {
      backendUsable = true
      if (resolved.source === 'env' || resolved.legacy) {
        try {
          const file = write({ world: base, agentId }, backend)
          lines.push(
            resolved.legacy
              ? `[凭据] 老形状那份（没有世界维）已收进「世界 × 身份」的新形状（**没有轮换**）：${file}`
              : `[凭据] 已把 ${credentials.BACKEND_ENV} 里那支收进按「世界 × 身份」分条的本地存储（**没有轮换**）：${file}`,
          )
        } catch (error) {
          lines.push(`[凭据] ⚠ 收进本地存储没成功（${error.message}）—— 这一次照旧用拿到的这份`)
        }
      }
    } else {
      const where = resolved.legacy
        ? '本地存储（老形状那份）'
        : resolved.source === 'store'
          ? '本地存储'
          : '用户环境变量'
      lines.push(
        `[布置] ${where}里的后端凭据不属于本家（它代表 ${represented ?? '认不出来'}，本家是 ${agentId}）——` +
          (agentRunning ? '但 Agent 正在跑，先不动它' : '换一份'),
      )
      backend = null
    }
  }
  return { backend, backendUsable, lines }
}

/**
 * **可重入的接受判据**：`201` = 这一次创建的；`200` = 同一幂等键的**重放**（契约如此）。
 *
 * 为什么必须两个都收：本文件四个创建步骤**都带幂等键**，而幂等键存在的**唯一理由**就是
 * "再跑一次不会造出第二个"。只认 201 等于把"重入"这条设计目的**判成失败**。
 *
 * ⚠ **但不是每个步骤都能"重放即接受"**：见凭据那一步——**幂等键与幂等响应是两件事**，
 * 带 key 只保证"不重复创建"，**不保证重放能还给你一次性秘密**（token 原文只给一次）。
 */
function acceptedCreate(status) {
  return status === 201 || status === 200
}

/**
 * **「世界里这支身份已有一条后端连接」时的那段读数**（`409 backend_binding_exists`；纯函数 ⇒ 可判错）。
 *
 * 场景：**同一支 Agent ＋ 没带凭据 ＋ 换机器** —— 世界里那条 binding 还在（它的凭据在另一台机器上），
 * 这一步建不出第二条 ⇒ 停手。从前这里把裸 JSON 抛给用户（看不懂、也不知道下一步干吗）；
 * 本读数只做一件事：**把两条出路说出口**（本刀只指路、不代跑；是否给 CLI 一条显式出路归主人裁）。
 *
 *  - 出路一：把那份凭据带到**本机同一位置**（`＜凭据目录＞/＜世界摘要＞/＜agent id＞.token`）再重跑；
 *  - 出路二：按手册「最后手段」那一节重签一支 —— ⚠ 旧的那支会**立即失效**（另一台机器上正在跑的她
 *    会被踢下线；纪律：先在那台机器 `down`，再重签）。
 *
 * `credentialFile` 是**本机的落点**（`credentials.credentialFile` 的真值，可能为 `null`）：它只是
 * **路径**，不是凭据 —— 读数里绝不打印凭据原文。fail-closed 一切照旧：退出码、动过的文件、退出时机不变。
 */
function backendBindingExistsReadout({ credentialFile = null } = {}) {
  const at = typeof credentialFile === 'string' && credentialFile !== '' ? credentialFile : null
  const where = at === null ? '' : `；本机是 ${at}`
  return [
    '[布置] 建 Backend Binding 失败：409 backend_binding_exists —— 世界里这支身份**已有一条后端连接**（可能来自另一台机器）',
    `[布置] 出路一：把那份凭据带到本机同一位置再跑一次（＜凭据目录＞/＜世界摘要＞/＜agent id＞.token${where}）`,
    '[布置] 出路二（最后手段）：按手册「最后手段」那一节重签一支 —— 旧的那支会**立即失效**（另一台机器上正在跑的她会被踢下线；纪律：先在那台机器 down，再重签）',
  ].join('\n')
}

function runCli(exe, args, env) {
  const done = spawnSync(exe, args, { env, encoding: 'utf8', windowsHide: true })
  return {
    code: done.status,
    stdout: done.stdout ?? '',
    stderr: done.stderr ?? '',
    error: done.error ? String(done.error.message) : null,
  }
}

/**
 * 取一支**能用的**后端凭据原文（可重入版，TASK_59）。
 *
 * 三条路：
 *  - `201`：这一次签发的 ⇒ 直接拿 `token`；
 *  - `200`（同 key 重放）：服务端**只存摘要** ⇒ 响应里 `token: null` ⇒ **拿不回原文**。
 *    只能**轮换**：换一支幂等键、并声明替换掉那条重放回来的凭据。
 *    幂等键**钉在"被替换的那条凭据 id"上**：同一状态重试会重放（无害），一旦轮换成功状态
 *    就变、键也跟着变 ⇒ 既不会连转两次，也不会卡死。
 *  - 其余：失败。
 *
 * `keys` 是**这支住户的布置键**（`provisioningKeys`）：签发与轮换都带上它 ——
 * 同一台机器上两支不同的 Agent 时，各自的凭据记录互不相干（不带上住户 ⇒ 第二支的签发会
 * 撞上第一支的幂等记录：同 Human 同键、目标却不同 ⇒ 409 idempotency_conflict）。
 *
 * ⚠ **绝不在没有原文的情况下返回成功** —— 那会把 `null` 写进用户环境变量，
 * 把一个"响亮失败"变成"静默坏掉"（Agent 拿着空凭据连不上，而 `up` 说它成了）。
 *
 * `imRequest` 从参数进（默认用真的）：这一段是本文件唯一"会拿走一次性秘密"的地方，
 * 必须能被单测直接喂假的响应（照 `status-server.js` 里 `readAgentStatus({ spawn })` 的先例）。
 */
async function obtainBackendCredential({
  base,
  token,
  agentId,
  bindingId,
  keys,
  imRequest: request = imRequest,
}) {
  const url = `/v1/agents/${agentId}/backend-bindings/${bindingId}/credentials`
  const issued = await request(base, 'POST', url, {
    bearer: token,
    key: keys.credential,
    body: { replaces_credential_id: null },
  })
  const tokenOf = (response) => {
    const value = response.body?.token
    return typeof value === 'string' && value !== '' ? value : null
  }
  if (issued.status === 201) {
    const fresh = tokenOf(issued)
    if (fresh !== null) return { ok: true, token: fresh }
    return fail(`[布置] 签发后端凭据的响应里没有原文：${JSON.stringify(issued.body)}`)
  }

  // 走到这里只有一条路：**这条凭据已经存在**，而"原文只给一次" ⇒ 必须**轮换**一支。
  // 触发它的状态有**两种**（真机都见过，别只认一种）：
  //   · `200` 重放 —— 同一幂等键的同一请求还在记录里（响应带 credential id，但 `token: null`）；
  //   · `409 credential_state_conflict` —— **状态已经变过**（例如上一支被别的路径轮换过）
  //     ⇒ 服务端连 id 都不给，只回"issue or rotation does not match current state"。
  // ⇒ 统一去**读一次现有凭据**，拿它的 id 当 `replaces_credential_id` 重发一次（换新键）。
  if (issued.status !== 200 && issued.status !== 409) {
    return fail(`[布置] 签发后端凭据失败：${issued.status} ${JSON.stringify(issued.body)}`)
  }
  const current = await request(base, 'GET', url, { bearer: token })
  if (current.status !== 200) {
    return fail(`[布置] 读现有后端凭据失败：${current.status} ${JSON.stringify(current.body)}`)
  }
  const list = Array.isArray(current.body?.credentials) ? current.body.credentials : []
  const replacedId = (list.find((item) => item && item.status === 'active') ?? list[0])?.id
  if (typeof replacedId !== 'string' || replacedId === '') {
    // 既不给签发（201）又说没有可替换的 —— 这是我们读不懂的状态，如实失败。
    return fail(`[布置] 没有可替换的后端凭据，却也不给签发新的：${JSON.stringify(issued.body)}`)
  }
  const rotated = await request(base, 'POST', url, {
    bearer: token,
    key: `${keys.credential}:${replacedId}`,
    body: { replaces_credential_id: replacedId },
  })
  const fresh = tokenOf(rotated)
  if (rotated.status !== 201 || fresh === null) {
    // **绝不在没有原文的情况下返回成功** —— 那会把 null 写进用户环境变量，
    // 把一个"响亮失败"变成"静默坏掉"。
    return fail(`[布置] 轮换后端凭据失败：${rotated.status} ${JSON.stringify(rotated.body)}`)
  }
  return { ok: true, token: fresh }
}

/**
 * 凭据这一步**该不该动**（TASK_62 的判据本体；纯函数 ⇒ 可判错、可负对照）。
 *
 * 三条，按优先级：
 *  1. **Agent 正在跑 ⇒ 绝不动**。换钥匙（`replaces_credential_id`）会**撤销 Agent 手里那支**，
 *     而门铃 SSE **每次发送前重认证、撤销即断流 fail-closed**，且通道坏了不自动重连
 *     ⇒ Agent 会**变聋**（读不了也写不了 im）。up 的幂等**不该有副作用地动在跑的东西**。
 *  2. 存储里那份**可用且属于本家** ⇒ 不动。这一支自 TASK_59 起就是"不动"，本刀只是把它
 *     写成**显式**的（原来靠 `if (!backend)` 隐式表达）。
 *  3. 其余（Agent 不在跑 ＋ 凭据缺失/认不出/不属本家）⇒ 走 TASK_59 的重入语义
 *     （读现有凭据 → 换新键轮换）。**那是布置中断后的恢复路径**，一字不变。
 *
 * ⚠ 判据落在**Agent 在不在跑**上，不是"这次 up 是第几次"——判"次数"会把
 * "Agent 在跑但我必须恢复"这种真正该动的情况一起挡掉。
 */
function credentialAction({ agentRunning, backendUsable }) {
  if (agentRunning) return 'keep-agent-running'
  if (backendUsable) return 'keep-existing'
  return 'issue-or-rotate'
}

/**
 * 跳过凭据那一步时**该打的读数**（TASK_62 补刀；纯函数 ⇒ 可判错）。
 *
 * 为什么 `backendUsable === false` 时**必须多一句**：那份凭据认不出来时，从外面**判不出**
 * Agent 手里那支还好不好 —— Agent 的钥匙是**起 Agent 那一刻**给的，之后谁换过都不影响
 * Agent 进程里那一份；而**自己不会换钥匙**（真机实测：被撤销后宿主日志写
 * `reason=credential-rejected`，`[next]` 指的就是"重启本进程"）。
 * ⇒ 只打"凭据未动"，用户会读成"没事"；**而此刻 Agent 可能就是聋的**。
 * 读数要把"怎么确认、怎么修"说出口，这是它唯一能替用户做的事。
 */
function credentialSkipLines({ agentPid, backendUsable }) {
  const lines = [`[agent] 已在跑（pid ${agentPid ?? '?'}）——凭据未动`]
  if (!backendUsable) {
    lines.push(
      '[warn] 那支后端凭据认不出来 ⇒ Agent 手里那支**未必**还好（自己不会换钥匙）；' +
        '要确认 Agent 还听得见、并把钥匙补好：先 node home.js down，再 node home.js up',
    )
  }
  return lines
}

/**
 * 这一支的**经历账本里有几段活动**（账本里不同的 `run` 值）。
 *
 * 与 `agent_host_status` 的 `activities-page=… of=N` **同一个口径**，而那个口径是
 * **总次数**：agent 侧 `store.rs` 的 `summaries_at` 读的是「**封口段（`trace.parts/`）＋
 * 活动段（`trace.ndjson`）**」，再按 `run` 合并去重（`merge_summaries`：同一个活动横跨两卷
 * 时只算一次）。⇒ 本函数也必须读**两处**。
 *
 * ⚠ **两处真机换来的更正**（2026-10-14）：
 *   ① 2026-10-06 起每行行尾多了一列校验和 ⇒ 从前对整行 `JSON.parse` 时，段数对**每一支**
 *      都读不出来（`null` ⇒ 整段省略），而用例一直绿（fixture 写的是旧形状）；
 *   ② 从前只读 `trace.ndjson` ⇒ **分卷之后少报一大截**（真机：小欣 4 段 vs 真实的 41 段）。
 *      它自称"与 `of=` 同一个口径"，那句当时**不成立**。
 *
 * 读不出来 / 读不懂 ⇒ `null`：读数里那一段**整个省略**（拿不到就不硬凑）。
 */
function countTraceRuns(envDir) {
  const runs = new Set()
  for (const file of traceLedgerFiles(envDir)) {
    let text
    try {
      text = fs.readFileSync(file, 'utf8')
    } catch {
      return null
    }
    for (const line of text.split('\n')) {
      if (line.trim() === '') continue
      let row
      try {
        // ⚠ 行尾那一列不是正文（见 `ledger_line.js`）。
        row = JSON.parse(ledgerBody(line))
      } catch {
        return null
      }
      if (row && typeof row.run === 'string' && row.run.trim() !== '') runs.add(row.run)
    }
  }
  return runs.size
}

/**
 * 经历账本的**全部段**：封口段（`trace.parts/`，按序号＝时间序）在前，活动段在后。
 *
 * 目录不在 / 读不动 ⇒ 只给活动段那一条（与"这个家还没封过卷"同形；
 * 读不动会在调用方那里变成 `null`，那才是该说的话）。
 */
function traceLedgerFiles(envDir) {
  const files = []
  try {
    const partsDir = path.join(envDir, 'trace.parts')
    for (const name of fs.readdirSync(partsDir).sort()) {
      if (name.endsWith('.ndjson')) files.push(path.join(partsDir, name))
    }
  } catch {
    // 没有封口段：照常只读活动段。
  }
  files.push(path.join(envDir, 'trace.ndjson'))
  return files
}

/**
 * **「这就是她、认出来了」那一行**（2026-10-01 第七刀补读数；`up` 重跑**已安家**的住户时打）。
 *
 * 形状（紧凑一行；手边没有的东西**不硬凑**）：
 * `[布置] 住户 <agent_id>（<显示名>）已安家：房 <room_id> —— 不新建、不认领（经历 N 段）`
 *
 * - 显示名取**支级 `agent.conf`** 那行；没有 ⇒ 整个括号省略（不拿别处顶替）；
 * - 经历段数＝她的经历账本里不同的 run（`countTraceRuns`，与 `of=` 同一口径）；读不到 ⇒ 省略；
 * - **不为了读数去起宿主、也不多问世界** —— 只用开机前手边就有的事实。
 */
function settledNotice(resident) {
  // 显示名取支级 `agent.conf` 那行；没有 ⇒ 整个括号省略，但**留一个空格**
  // —— 不然读数会长成 `住户 a-2已安家`（中文与 id 粘在一起，读起来是坏的）。
  const name = resident.displayName ? `（${resident.displayName}）` : ' '
  const count = countTraceRuns(resident.envDir)
  const carried = count === null ? '' : `（经历 ${count} 段）`
  return `[布置] 住户 ${resident.agentId}${name}已安家：房 ${resident.roomId || '无'} —— 不新建、不认领${carried}`
}

/** 需要主人凭据时那句共用的读数（远端跑法的 fail-closed 之一，三处共用一处说）。 */
const NEED_HUMAN_TOKEN =
  '[布置] 远端布置还需要主人凭据： --token <那个世界的主人凭据>（clr_hs_ 开头；' +
  `或用环境变量 ${HUMAN_TOKEN_ENV}）`

/**
 * **给一支 Agent 安家**（一支一次；`up` 对每一支各调一次）。
 *
 * @param {object} ctx
 * @param {string} ctx.homeDir
 * @param {string} ctx.confFile `home.conf`（机器级事实；支级事实写在 `<环境根>/agent.conf`）
 * @param {string} ctx.listen 本机世界地址的口（本机全量时 `base = http://<listen>`）
 * @param {object} ctx.imEnv 起 core CLI 子命令用的环境（bootstrap-human 那一族）
 * @param {string} ctx.imExe / `ctx.runExe` 可执行件
 * @param {string|null} [ctx.humanName] 首位 Human 的显示名（`--human-name`）。
 *   **null = 没给 ⇒ 占位 `未命名`**（明确是占位，不是称呼；系统不替谁起名）。
 *   ⚠ 从前这里取这台机器的用户名（2026-10-20 撤掉）：那是机器事实，写进公共世界
 *   就是拿它冒充公共事实 —— 判据与 Agent 那条同一条（显示名是公共呈现，系统不替谁认定身份）。
 * @param {object|null} [ctx.resident] **已有的住户**（`{ agentId, envDir, slot, roomId }`）；
 *   `null` = 这一支还没安家（要认领或自建）
 * @param {string|null} [ctx.adoptAgentId] `--agent <世界里的 id>`（认领）
 * @param {number|null} [ctx.slot] 新住户的**住户序号**（调用方分配：单调、不回收）
 * @param {boolean} [ctx.agentRunning] 这一支的宿主在不在跑（凭据那一步按它决定动不动）
 * @param {number|null} [ctx.agentPid]
 * @param {object|null} [ctx.remote] `{ base, token }` 远端跑法
 * @param {string|null} [ctx.envAside] 老家让位目录（模型绑定从这里带进新环境）
 * @param {string|null} [ctx.carryBindingFrom] 家里**别的住户**的环境根（新住户跟着用同一条模型绑定）
 * @param {Function} [ctx.runCli] / `ctx.imRequest` / `ctx.newHomeId` 注入点（用例用）
 * @returns {Promise<{ok: boolean, message?: string, agentId?: string, roomId?: string, slot?: number}>}
 */
async function provision(ctx) {
  const { confFile, homeDir, imExe, runExe, imEnv } = ctx
  // **远端跑法**（2026-09-30）：`remote` 非空 ⇒ 世界在别处。`base` 是唯一的"对谁说话"；
  // `token` 是外部给的主人凭据（可以是 `null` —— 只有"不需要动世界"的重跑才走得完）。
  const remote = ctx.remote ?? null
  const base = remote ? remote.base : `http://${ctx.listen}`
  // 起 CLI 子进程这一步可注入（测试用它把"agent_run init"换成替身；线上默认就是真 spawn）。
  const runCliFn = ctx.runCli ?? runCli
  // `imRequest` 从参数进（默认用真的）：测试用它把"对谁说话"换成替身（真机默认不变）。
  const request = ctx.imRequest ?? imRequest

  // ── 1) Human ──────────────────────────────────────────────────────────────
  //
  // 本机全量：没有就 `bootstrap-human`（token 只在这里打印一次）；有就补一支 Session 用于布置。
  // **远端跑法：这一步整段跳过** —— 人不在本机，凭据由外部给；本机**不碰 core 的 CLI**。
  let token
  if (remote) {
    token = typeof remote.token === 'string' && remote.token !== '' ? remote.token : null
    console.log(
      token
        ? '[布置] 远端跑法：不铸人（主人凭据由外部给；本机不起 core）'
        : '[布置] 远端跑法：这次没有主人凭据 —— 只有"家里已经布置好"的重跑才走得完',
    )
  } else {
    let humanId = confGet(confFile, 'human_id')
    if (!humanId) {
      // ★ 名字**由人给**（`--human-name`）；没给就是占位 `未命名`，而且读数要**说出来**
      //   它是占位。从前这里取这台机器的用户名（2026-10-20 撤掉）—— 那是机器事实。
      const given =
        typeof ctx.humanName === 'string' && ctx.humanName.trim() !== ''
          ? ctx.humanName.trim()
          : null
      if (given) {
        const problem = humanNameProblem(given)
        if (problem) {
          return fail(`[布置] 首位 Human 的名字不能用：${problem}（--human-name）`)
        }
      }
      const humanName = given ?? HUMAN_DISPLAY_NAME_PLACEHOLDER
      if (!given) {
        console.log(
          `[布置] 首位 Human 没有给名字（--human-name）⇒ 显示名先用占位「${HUMAN_DISPLAY_NAME_PLACEHOLDER}」` +
            ' —— 占位不是称呼；名字归你自己，进 app 首屏或「我的资料」里改',
        )
      }
      const done = runCliFn(
        imExe,
        ['bootstrap-human', '--display-name', humanName],
        imEnv,
      )
      if (done.code !== 0) {
        return fail(`[布置] bootstrap-human 失败：${(done.stderr || done.error || '').trim()}`)
      }
      let opened
      try {
        opened = JSON.parse(done.stdout.trim())
      } catch {
        return fail('[布置] bootstrap-human 的输出读不懂（预期一行 JSON）')
      }
      humanId = opened.participant_id
      confSet(confFile, 'human_id', humanId)
      console.log(`[布置] 主人（Human）已开：${humanId}`)
      // 这一支是**唯一的打印点**（手册里那句"只在这里打印一次"就是它）。
      console.log(`[token] ${opened.token}`)
      console.log('[token] ↑ 这一支只在这里打印一次（client 用它进值班室）。丢了就补发： node home.js up --token')
      token = opened.token
    } else {
      const done = runCliFn(imExe, ['issue-human-session', '--human-id', humanId], imEnv)
      if (done.code !== 0) {
        return fail(`[布置] issue-human-session 失败：${(done.stderr || done.error || '').trim()}`)
      }
      try {
        // 布置用的一支：**只活在这一次调用里**（不进日志、不进文件、不打印）。
        token = JSON.parse(done.stdout.trim()).token
      } catch {
        return fail('[布置] issue-human-session 的输出读不懂（预期一行 JSON）')
      }
    }
  }

  // ── 1b) 家 id（幂等键的作用域，2026-10-01）─────────────────────────────────
  // 读得到就逐字用；没有就等第一次真的要用键时再生成（`needKeys`）。形状迁移（旧环境让位、
  // home.conf 那两行让位）在 `residents.js` 的 `migrateHomeShape`，由 `up` 排在读住户之前调。
  const homeIdState = ensureHomeId({ confFile })
  // 家 id 的**生成**推迟到第一次真的要用键时：这样"停在动手之前"（缺主人凭据等）连本地也
  // 一个字都不写；一旦生成就写进 home.conf，之后只读。
  let keys = null
  const needKeys = () => {
    if (keys !== null) return keys
    if (slot === null) {
      return fail(
        '[布置] 这一支还没有住户序号（slot）—— 布置键（幂等键）无从取。' +
          '先修好它的 agent.conf（加一行 slot=<正整数>），或删掉这个住户目录重新安家',
      )
    }
    let homeId = homeIdState.homeId
    if (homeId === null) {
      const mint = ctx.newHomeId ?? (() => crypto.randomUUID())
      homeId = mint()
      confSet(confFile, 'home_id', homeId)
      console.log(`[布置] 这个家还没有家 id：生成 ${homeId}（布置键的作用域，之后只读）`)
    }
    keys = provisioningKeys(homeId, slot)
    return keys
  }

  // ── 2) 身份（这一支的 Agent）───────────────────────────────────────────────
  //
  // 三条路：**已有的住户**（家里那支就是它）／**认领**世界里已有的身份（`--agent <id>`）／**自建**。
  // 认领的裁决全在 `adopt.js`（纯函数 + 注入的 imRequest）；这里只做接线与读数。
  let resident = ctx.resident ?? null
  let agentId = resident ? resident.agentId : null
  let slot = resident
    ? // 住户自己的序号优先；**半成品住户**（还没安家、读不到序号）用调用方分给它的那个
      // （分配只在 `up` 的目标裁决那一处发生 —— 这里**绝不猜**，见 2026-10-01 第五刀）。
      resident.slot ?? (Number.isInteger(ctx.slot) ? ctx.slot : null)
    : Number.isInteger(ctx.slot)
      ? ctx.slot
      : null
  let roomId = resident ? resident.roomId : ''
  let displayName = resident ? resident.displayName ?? '' : ''
  let adopted = false
  // ⚠ **半成品**（目录在、`agent.conf` 写了手/显示名，但**还没安家**：没有身份 / 没有住户序号）
  // 不算"已经安过家"——它要走**认领**那条路（问世界这一支是不是你的、还 active 吗），
  // 然后写身份、补住户序号，已写的手与显示名原样留着（2026-10-01 第五刀真机发现的坑：
  // 从前这里按"目录在"判成 already ⇒ 接着因为读不到 slot 而停手）。
  const settled = Boolean(resident) && Boolean(resident.slot) && fs.existsSync(path.join(resident.envDir, 'identity.json'))
  const adoption = planAdoption({
    residentAgentIds: settled && agentId ? [agentId] : [],
    requestedId: ctx.adoptAgentId ?? null,
  })
  if (adoption.kind === 'refuse-bad-id') {
    return fail('[布置] `--agent` 后面要跟一个世界里的 Agent id')
  }
  // **已安家**（`settled`：有身份 ＋ 有住户序号）⇒ 重跑她时把"这就是她、认出来了"**显式**说一句
  // （2026-10-01 第七刀补读数）。从前这一条路**一句话不说** —— 用户只能从"没有新建读数 ＋
  // `[state]` 带着旧经历"反推；`already`（`--agent` 指的就是家里这一支）与 `as-is`（裸 `up`
  // 遍历到她）是同一个事实，走同一行读数。新建（`Agent（…）已建`）与认领（`认领了世界里…`）
  // 那两种读数**一字不动**（见下面两条路）。
  if (settled && (adoption.kind === 'already' || adoption.kind === 'as-is')) {
    console.log(settledNotice(resident))
  }
  if (adoption.kind === 'adopt') {
    if (remote && !token) return fail(NEED_HUMAN_TOKEN)
    const verdict = await verifyOwnedAgent({ base, token, agentId: adoption.agentId, imRequest: request })
    for (const line of verdict.lines) console.log(line)
    if (!verdict.ok) return fail(`[布置] 认领不成（${verdict.reason}）—— 什么都没改`)
    agentId = adoption.agentId
    adopted = true
    resident = null
    roomId = ''
    displayName = verdict.name
    console.log(`[布置] 认领了世界里已有的 Agent：${verdict.name}（${agentId}）—— 不新建身份`)
  }
  // 人有没有给这支新 Agent 起名（`up --agent-name <名字>`）。只影响"自建"那一条路。
  const askedName = String(ctx.agentName ?? '').trim()
  if (!agentId) {
    // **要动世界了**：远端跑法到这里还没有主人凭据 ⇒ 当场 fail-closed（说清"这一次还需要什么"）。
    if (remote && !token) return fail(NEED_HUMAN_TOKEN)
    const currentKeys = needKeys()
    if (currentKeys.ok === false) return currentKeys
    // 名字：人给了就用人的，没给就是**占位**（系统不替它起名 —— 见上面那条判据）。
    const builtName = askedName !== '' ? askedName : AGENT_DISPLAY_NAME_PLACEHOLDER
    const answer = await request(base, 'POST', '/v1/agents', {
      bearer: token,
      key: currentKeys.agent,
      body: { display_name: builtName },
    })
    if (!acceptedCreate(answer.status)) {
      return fail(`[布置] 建 Agent 失败：${answer.status} ${JSON.stringify(answer.body)}`)
    }
    agentId = answer.body.agent.id
    // 家里也记下世界那一支的名字（`agent.conf` 的 `display_name`），重跑时读数说得出来。
    displayName = builtName
    const nameNote =
      askedName === ''
        ? ' —— 占位，不是称呼；名字归它自己或你，随时可在「我的 Agent → 资料」里改'
        : ''
    console.log(
      `[布置] Agent 已建：${agentId}（显示名 ${builtName}${nameNote}）` +
        (answer.status === 200 ? '（沿用上一次创建的——幂等重放）' : ''),
    )
  } else if (askedName !== '') {
    // **不许静默忽略**：给了名字却没派上用场（这个家已经有住户了）⇒ 说清它没用上、以及改名的正路。
    console.log(
      `[布置] --agent-name 没派上用场：这个家已经有住户了（${agentId}）—— 改名走「我的 Agent → 资料」`,
    )
  }
  // 这一支的环境根：已有住户用它自己的；新住户用 `<家>/agents/<id>/`（壳的布局）。
  const envDir = resident ? resident.envDir : P.agentEnvRoot(homeDir, agentId)

  // ── 3) 这一支的后端凭据（必须是"本家这支 Agent"的）──────────────────────────
  //
  // **凭据的权威来源是按「世界 × agent_id」分条的本地存储**（`credentials.js`）；
  // `CELIRA_IM_BACKEND_AUTH` 是**老路 / 手工路**的回落（仍然读，但不再写）。
  const resolved = credentials.resolve(
    { world: base, agentId },
    { fallback: readUserEnv(BACKEND_ENV) },
  )
  const decided = await resolveBackendCredential({
    base,
    agentId,
    resolved,
    agentRunning: Boolean(ctx.agentRunning),
    imRequest: request,
  })
  for (const line of decided.lines) console.log(line)
  let backend = decided.backend
  const backendUsable = decided.backendUsable
  // TASK_62：Agent 正在跑 ⇒ 这一步整块跳过（含建 binding）——见 `credentialAction` 的三条。
  const action = credentialAction({ agentRunning: Boolean(ctx.agentRunning), backendUsable })
  if (action === 'keep-agent-running') {
    for (const line of credentialSkipLines({ agentPid: ctx.agentPid, backendUsable })) {
      console.log(line)
    }
  }
  if (action === 'issue-or-rotate') {
    // **要动世界了**（建 binding / 签凭据都是写）：远端跑法没有主人凭据 ⇒ fail-closed。
    if (remote && !token) return fail(NEED_HUMAN_TOKEN)
    const currentKeys = needKeys()
    if (currentKeys.ok === false) return currentKeys
    const binding = await request(base, 'POST', `/v1/agents/${agentId}/backend-bindings`, {
      bearer: token,
      key: currentKeys.binding,
      body: { label: 'home' },
    })
    if (!acceptedCreate(binding.status)) {
      // 「同一支、没带凭据、换机器」那条死路：世界里这支身份**已有一条后端连接**（409
      // backend_binding_exists）⇒ 读数说清两条出路（只指路、不代跑）；仍然 fail-closed
      // ——退出码、动过的文件、退出时机与从前一字不差（`up` 侧照旧映射成退出码 4）。
      if (binding.status === 409 && binding.body?.error?.code === 'backend_binding_exists') {
        return fail(
          backendBindingExistsReadout({
            credentialFile: credentials.credentialFile({ world: base, agentId }),
          }),
        )
      }
      return fail(`[布置] 建 Backend Binding 失败：${binding.status} ${JSON.stringify(binding.body)}`)
    }
    // 重放也带回 binding id（`CreateBindingOutcome::Replayed { binding }`）⇒ 沿用那条即可。
    const bindingId = binding.body?.binding?.id
    if (typeof bindingId !== 'string' || bindingId === '') {
      return fail(`[布置] 建 Backend Binding 的响应里没有 binding id：${JSON.stringify(binding.body)}`)
    }

    // ── 凭据：**这里绝不能"重放即接受"**（见 `obtainBackendCredential` 的注释）──
    const obtained = await obtainBackendCredential({
      base,
      token,
      agentId,
      bindingId,
      keys: currentKeys,
      imRequest: request,
    })
    if (!obtained.ok) return obtained
    try {
      const file = credentials.write({ world: base, agentId }, obtained.token)
      console.log(`[布置] Agent 的后端凭据已写进本地存储（按「世界 × 身份」分条）：${file}`)
    } catch (error) {
      return fail(`[布置] 后端凭据写不进本地存储：${error.message}`)
    }
  }

  // ── 4) 一间房（Agent 与主人说话的地方）──────────────────────────────────────
  // 一间房 = 一段关系（2026-09-27 批次二）：请求体**只有 `agent_id`**——direct 房没有标题
  // （`name` 已是未知字段 ⇒ 400），而且要"再开一间"也开不出来：同一对的私人房至多一间，
  // 服务端会把已有那间回放回来（`home-direct:<家 id>[:<slot>]` 那把 key 是第二道保险）。
  if (!roomId) {
    // **优先用已有的那间** —— ⚠ **2026-10-13 改成无条件**（从前只在「认领」那条路上做）。
    //
    // 为什么（真机两例，症状都是**家起不来**）：这间房是**世界的事实**（`GET /v1/rooms` 读得到），
    // 而「这一次布置是不是认领来的」只是**来路** —— 两件事无关。从前首装／重跑那条路**不看世界**
    // 就直接去写，而写要过幂等键；键一旦被**上一次半途而废的尝试**占住（世界对失败的请求也记键），
    // 就是 409 ⇒ `up` 退出 4 ⇒ **家永久起不来** —— 而世界里那间房其实早就在（真家与 dev 家各撞一次）。
    //
    // 判据：**世界的现状是权威，键是附注**。读要主人凭据（`GET /v1/rooms` 是 Human 面）；
    // 没有它（远端跑法）⇒ 照旧走下面那条 fail-closed。
    if (token) {
      const existing = await findExistingDirectRoom({ base, token, agentId, imRequest: request })
      if (existing.roomId) {
        roomId = existing.roomId
        console.log(`[布置] 沿用已有的值班室（这一支与主人的那间 direct room）：${roomId}`)
      } else if (!existing.looked) {
        console.log('[布置] 房间列表读不到 ⇒ 照旧开一间（幂等键 home-direct:<家 id>[:<住户序号>]）')
      }
    }
  }
  if (!roomId) {
    // **要动世界了**（开房是写）：远端跑法没有主人凭据 ⇒ fail-closed。
    if (remote && !token) return fail(NEED_HUMAN_TOKEN)
    const currentKeys = needKeys()
    if (currentKeys.ok === false) return currentKeys
    const room = await request(base, 'POST', '/v1/direct-rooms', {
      bearer: token,
      key: currentKeys.direct,
      body: { agent_id: agentId },
    })
    if (!acceptedCreate(room.status)) {
      // **写被拒 ≠ 事实不存在**（2026-10-13）：409 说的是「这把键配过另一个请求」——那是**键**的事；
      // 「这一对有没有房」要问**世界**（同一对的私人房，世界里至多一间）。再读一次：读到了就用它，
      // 不再重写、也不判死；读不到 ⇒ 照旧把世界那句话原样交出去（fail-closed）。
      if (room.status === 409 && token) {
        const again = await findExistingDirectRoom({ base, token, agentId, imRequest: request })
        if (again.roomId) {
          roomId = again.roomId
          console.log(`[布置] 那把幂等键被占过（409），而世界里已经有这间房：${roomId} —— 用它`)
        }
      }
      if (!roomId) {
        return fail(`[布置] 开房间失败：${room.status} ${JSON.stringify(room.body)}`)
      }
    }
    if (!roomId) roomId = room.body?.room?.id
    if (typeof roomId !== 'string' || roomId === '') {
      return fail(`[布置] 开房间的响应里没有 room id：${JSON.stringify(room.body)}`)
    }
    console.log(
      `[布置] ${ROOM_NAME}已开：${roomId}` + (room.status === 200 ? '（沿用上一次开的——幂等重放）' : ''),
    )
  }

  // ── 5) 支级配置（`<环境根>/agent.conf`）──────────────────────────────────────
  //
  // 支级事实住**这一支自己的环境根**里：身份 / 值班室 / 住户序号 / 显示名。
  // 家级 `home.conf` 只留机器级事实（承 PROJECT_SHAPE §8.0 裁决）。
  // ⚠ 住户序号不猜：读不到又没分配到 ⇒ 当场说清（写一个猜出来的 slot 会让下一次的布置键错位）。
  if (slot === null) {
    return fail(
      `[布置] 住户 ${agentId} 的 agent.conf 里没有住户序号（slot）—— 不猜：` +
        '先修好它（加一行 slot=<正整数>），或删掉这个住户目录重新安家',
    )
  }
  const confFileForAgent = path.join(envDir, P.AGENT_CONF)
  writeAgentConf(confFileForAgent, { agentId, roomId, slot, displayName })

  // ── 6) 这一支的环境（幂等：已有就不动）──────────────────────────────────────
  if (!fs.existsSync(path.join(envDir, 'identity.json'))) {
    const done = runCliFn(runExe, ['init', '--root', envDir, '--agent', agentId], imEnv)
    if (done.code !== 0) {
      return fail(`[布置] 建 Agent 的环境失败：${(done.stderr || done.error || '').trim()}`)
    }
    console.log(`[布置] Agent 的环境已建：${envDir}`)
    // 老家的**模型绑定**跟着带到新环境：它是"这台机器这条家"的配置，不是旧那支的私有物。
    // 不带过去的话，绑定在场却读不到 ⇒ 宿主会拒绝启动（"两个来源不许同时说话"那条纪律的另一面）。
    // ⚠ **新住户**同理（2026-10-01 多支刀）：家里别的支已经绑好了 ⇒ 这一支跟着用同一条
    // （"这台机器上这个家用哪条模型"是家级选择；一支一份只是它的落盘形状）——
    // 不然"再安一支"会当场卡在"缺 DEEPSEEK_API_KEY"上，而家里明明已经配过了。
    const carried = [ctx.carryBindingFrom, ctx.envAside]
      .filter((dir) => typeof dir === 'string' && dir !== '')
      .map((dir) => path.join(dir, 'model_binding.json'))
      .find((file) => fs.existsSync(file))
    if (carried && !fs.existsSync(path.join(envDir, 'model_binding.json'))) {
      fs.copyFileSync(carried, path.join(envDir, 'model_binding.json'))
      console.log(
        ctx.carryBindingFrom && carried.startsWith(ctx.carryBindingFrom)
          ? '[布置] 家里别的住户已经绑好模型了 —— 这一支跟着用同一条（model_binding.json）'
          : '[布置] 旧环境的模型绑定已带到新环境（model_binding.json）',
      )
    }
  }

  return { ok: true, agentId, roomId, slot: slot ?? 1 }
}

module.exports = {
  provision,
  credentialAction,
  credentialSkipLines,
  resolveBackendCredential,
  acceptedCreate,
  backendBindingExistsReadout,
  obtainBackendCredential,
  provisioningKeys,
  ensureHomeId,
  countTraceRuns,
  settledNotice,
  KEY_NAMES,
  NEED_HUMAN_TOKEN,
  BACKEND_ENV,
  humanNameProblem,
  HUMAN_DISPLAY_NAME_PLACEHOLDER,
  AGENT_DISPLAY_NAME_PLACEHOLDER,
  ROOM_NAME,
}
