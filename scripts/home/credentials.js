'use strict'
/**
 * **Agent 的后端凭据**——按「**世界 × Agent 身份**」分条的本地存储（2026-09-25 立，2026-10-01 加世界维）。
 *
 * ## 为什么不再放在"机器上那一个环境变量"里
 *
 * 凭据属于 **Binding**，Binding 属于某个 **Agent**，而 Agent 是**某个世界里的身份**。
 * 也就是说这份事实的作用域是「**Agent × 那个世界**」，而它原来住在 `HKCU\Environment` 的
 * **一个**变量里 —— 那是**机器级**的作用域。同一台机器上两个家（Agent 住两个世界，或两份
 * 不同的 Agent）就会抢同一个格子：谁最后开机谁把格子换成自己的，另一个家的存档值就此丢失
 * （在跑的那支不受影响，它手里那份在自己进程里，但下次开机要重新轮换一次）。
 *
 * 更硬的一条：**非 Windows 上根本没有那个格子**（`HKCU\Environment` 是 Windows 的）
 * ⇒ 首次布置在 Linux 上走不完。按身份分条存成文件，两个问题一起消失。
 *
 * ## 为什么还要加**世界**那一维（2026-10-01，承 PROJECT_SHAPE §8.0 推论 3）
 *
 * 一台机器可以跑多支 Agent（不同的世界、不同的身份）。**同一个 `agent_id` 在不同世界里
 * 是两条互不相干的事实**（各自世界里的一支身份）⇒ 只按 agent id 分条会让它们撞在同一个
 * 文件上。键从 `agent_id` 升到「**世界 × agent_id**」：
 *
 * - **世界那一维 = 世界地址规范化后的稳定摘要**（`worldKey`：小写主机、去尾斜杠、
 *   sha256 前 16 个十六进制位）。选摘要而不是原文：它**稳定**（同一个世界每次一样）、
 *   够短、并且**不把地址原文写进文件名**（少一处能泄漏的东西；凭据本身更不进任何日志）。
 * - 落盘形状：`<目录>/<世界摘要>/<agent_id>.token`（一个世界一个子目录，一支一个文件，0600）。
 *
 * ## 老形状（没有世界维）怎么办
 *
 * 旧文件是 `<目录>/<agent_id>.token`。它**只读**，作为**迁移桥**：新形状那份不在时，
 * `resolve` 会把它当作候选交出来（`legacy: true`），由调用方（`provision.js`）**先问世界
 * "这支代表谁"**，认下了才原文照搬进新形状 —— 认不下就不动它（它可能是别的世界的）。
 * ⇒ 升级后的第一次 `up` 不会白白轮换一次凭据；而"同一台机器两支在不同世界用同一个
 * agent id 不许撞车"这条，从新形状落地那一刻就成立。
 *
 * ## 四个面（读 / 写 / 轮换 / 删除）用**同一个键**
 *
 * `read` / `write` / `remove` 与 `credentialFile` 全走同一处路径裁决；**轮换**是"用同一把
 * 键写一份新的"（`provision.js` 的 `obtainBackendCredential` 拿到新原文后照样调 `write`）。
 * 一个身份一个文件（而不是一份大 JSON）：两个家同时布置时不会互相覆盖整份文件。
 *
 * ## 为什么**不**放进家目录
 *
 * 手册承诺过「家里不放任何凭据」，而**升级/回退的正规动作就是整份复制家目录**
 * （见手册「升级与回退」）—— 凭据一旦进家，每一份备份都会携带一份活凭据。
 * 所以：按身份分条，但住在**家之外**的机器级目录里。
 *
 * ## 环境变量那条路还在（老路 / 手工路）
 *
 * `CELIRA_IM_BACKEND_AUTH` 仍然被**读**（`resolve` 的回落）：老家的迁移期、CI、以及
 * 主人手工指定的场合都靠它。**但不再写它** —— 一份事实一个权威位置，两个写者就是抢格子。
 */

const crypto = require('node:crypto')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

/** 一个身份一个文件（文件名就是身份 id + 这个后缀）。 */
const FILE_SUFFIX = '.token'

/** 世界摘要的长度（十六进制位）。够区分（2^64），又短得能当目录名读。 */
const WORLD_KEY_LENGTH = 16

/** 环境变量名（与 `provision.js` 的 `BACKEND_ENV` 同一份事实；这里只读不写）。 */
const BACKEND_ENV = 'CELIRA_IM_BACKEND_AUTH'

/** 覆盖目录（测试与 CI）。不给就按平台算。 */
const DIR_ENV = 'CELIRA_CREDENTIALS_DIR'

/**
 * 凭据目录（纯函数：只看传进来的环境，便于判错）。
 *
 * ⚠ `LOCALAPPDATA` 缺席（罕见的 Windows 环境）时退到用户主目录下的 `.celira/credentials`
 * —— **绝不**退到当前工作目录（那会让凭据落在随机的项目目录里）。
 */
function credentialsDir(env = process.env) {
  const override = typeof env[DIR_ENV] === 'string' ? env[DIR_ENV].trim() : ''
  if (override !== '') return path.resolve(override)
  if (process.platform === 'win32') {
    const local = typeof env.LOCALAPPDATA === 'string' ? env.LOCALAPPDATA.trim() : ''
    if (local !== '') return path.join(local, 'Celira', 'credentials')
  }
  const xdg = typeof env.XDG_CONFIG_HOME === 'string' ? env.XDG_CONFIG_HOME.trim() : ''
  const base = xdg !== '' ? xdg : path.join(os.homedir(), '.config')
  return path.join(base, 'celira', 'credentials')
}

/**
 * 世界地址的**规范化形状**（纯函数 ⇒ 可判错）：`协议//主机[:端口][路径]`，小写主机、去尾斜杠。
 *
 * 读不懂的取值（不是 URL）退回"去尾斜杠的原文"—— 仍然**稳定**（同一个输入每次一样），
 * 只是不享受规范化红利。**绝不**把摘要算在"当时猜出来的地址"上：地址是显式声明的。
 */
function normalizeWorld(base) {
  const raw = typeof base === 'string' ? base.trim() : ''
  if (raw === '') return ''
  try {
    const url = new URL(raw)
    const host = url.hostname.toLowerCase()
    const port = url.port ? `:${url.port}` : ''
    const rest = url.pathname.replace(/\/+$/, '')
    return `${url.protocol}//${host}${port}${rest}`
  } catch {
    return raw.replace(/\/+$/, '')
  }
}

/**
 * 世界那一维的**稳定摘要**（`<目录>/<摘要>/<身份>.token` 的那一段）。
 * 同一个世界每次一样；不泄露凭据（它是地址的摘要，不是凭据）。
 * 世界地址缺席 ⇒ `null`（调用方按"这一维缺了"处理，**不**静默退回只按身份）。
 */
function worldKey(base) {
  const normalized = normalizeWorld(base)
  if (normalized === '') return null
  return crypto.createHash('sha256').update(normalized).digest('hex').slice(0, WORLD_KEY_LENGTH)
}

/** 身份 id 只允许安全字符（它会被拼进文件名）。 */
function safeAgentId(agentId) {
  const id = typeof agentId === 'string' ? agentId.trim() : ''
  if (id === '' || /[^A-Za-z0-9._-]/.test(id)) return null
  return id
}

/**
 * 一份凭据的文件路径。
 *
 * - `ref = { world, agentId }` ⇒ **新形状**：`<目录>/<世界摘要>/<agent_id>.token`；
 *   世界那一维缺了 ⇒ `null`（**不**退回旧形状 —— 静默退回就是让两支撞车）。
 * - `ref = '<agent_id>'`（字符串）⇒ **老形状**：`<目录>/<agent_id>.token`（只读迁移桥，
 *   见文件头；生产读路径由 `resolve` 的 legacy 回落调用，写路径不接受它）。
 */
function credentialFile(ref, env = process.env) {
  if (typeof ref === 'string') return legacyCredentialFile(ref, env)
  const id = safeAgentId(ref ? ref.agentId : null)
  if (id === null) return null
  const key = worldKey(ref ? ref.world : null)
  if (key === null) return null
  return path.join(credentialsDir(env), key, `${id}${FILE_SUFFIX}`)
}

/** 老形状那份（`<目录>/<agent_id>.token`）—— 只读迁移桥。 */
function legacyCredentialFile(agentId, env = process.env) {
  const id = safeAgentId(agentId)
  if (id === null) return null
  return path.join(credentialsDir(env), `${id}${FILE_SUFFIX}`)
}

/** 三态读取（共用实现）：`{ state, file, token?, reason? }`。 */
function readAt(file) {
  if (file === null) return { state: 'missing', file: null, reason: 'invalid-ref' }
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch (error) {
    if (error && error.code === 'ENOENT') return { state: 'missing', file }
    return { state: 'unreadable', file, reason: 'read-failed' }
  }
  const token = text.trim()
  if (token === '') return { state: 'unreadable', file, reason: 'empty' }
  return { state: 'present', file, token }
}

/**
 * 读一个「世界 × 身份」的凭据。三态（"没有"与"读不出来"分开 —— 与 `home-key.js` 的摘要读取同款）：
 *   `{ state: 'present', token }` / `{ state: 'missing' }` / `{ state: 'unreadable', reason }`
 */
function read(ref, env = process.env) {
  return readAt(credentialFile(ref, env))
}

/**
 * 写一份凭据（**原子**：同目录临时文件 + rename；0600 是 POSIX 上的意思，
 * Windows 上权限随用户目录 ACL —— 与它取代的那个注册表值同级）。
 *
 * ⚠ 已经存在但**读不出来**时**拒绝覆盖**：那多半是别人的文件/坏文件，
 * 覆盖它等于替主人做决定（而这里存的是凭据，不是缓存）。
 * ⚠ 写只认**新形状**（世界那一维必填）：老形状是只读桥，写它等于退回"两支撞车"。
 */
function write(ref, token, env = process.env) {
  const file = credentialFile(ref, env)
  if (file === null) {
    throw new Error(
      `凭据的键要「世界 × 身份」：世界那一维缺了或身份 id 不能当文件名（${JSON.stringify(ref)}）`,
    )
  }
  const value = typeof token === 'string' ? token.trim() : ''
  if (value === '') throw new Error('凭据是空的 —— 不写（空值会让"缺凭据"变成"静默坏掉"）')

  const existing = readAt(file)
  if (existing.state === 'unreadable') {
    throw new Error(`凭据文件读不出来（${file}：${existing.reason}）—— 先看清它再重试`)
  }
  fs.mkdirSync(path.dirname(file), { recursive: true })
  const temp = path.join(path.dirname(file), `.${path.basename(file)}.${process.pid}.tmp`)
  fs.writeFileSync(temp, `${value}\n`, { encoding: 'utf8', mode: 0o600 })
  try {
    fs.renameSync(temp, file)
  } catch (error) {
    try {
      fs.rmSync(temp, { force: true })
    } catch {
      // 清不掉临时文件不改变结论（要报的是 rename 失败的因）
    }
    throw error
  }
  return file
}

/**
 * 删一份凭据（**撤销式**：查无也成功）。删的是**新形状**那一份（同一个键）。
 *
 * 老形状那份**不删**（它是只读迁移桥，且可能属于另一个世界 —— 没有证据就不替主人清账）。
 * 返回 `{ removed, file }`。
 */
function remove(ref, env = process.env) {
  const file = credentialFile(ref, env)
  if (file === null) return { removed: false, file: null }
  const existed = fs.existsSync(file)
  try {
    fs.rmSync(file, { force: true })
  } catch {
    return { removed: false, file }
  }
  return { removed: existed, file }
}

/**
 * **取一份能用的凭据**（唯一的取值口径，各调用点共用：布置 / 起 Agent / 关机时的归属证明）。
 *
 * 顺序：**按「世界 × 身份」分条的那份** → **老形状那份**（只读迁移桥，`legacy: true`，
 * 由调用方去问世界"它代表谁"）→ `CELIRA_IM_BACKEND_AUTH`（老路 / 手工路）→ 没有。
 *
 * 返回 `{ token, source, state, legacy? }`，`source` 是 `'store' | 'env' | null`：
 * 调用方据此说清这份凭据是从哪儿来的（迁移读数、诊断都靠它），也据此决定要不要把它收进存储。
 * `legacy: true` = 这份来自老形状文件（**还没进新形状** —— 认下之后要照搬进新形状）。
 */
function resolve(ref, { fallback = null, env = process.env } = {}) {
  const stored = read(ref, env)
  if (stored.state === 'present') return { token: stored.token, source: 'store', state: 'present' }
  if (stored.state === 'unreadable') {
    return { token: null, source: null, state: 'unreadable', reason: stored.reason }
  }
  const legacyFile = legacyCredentialFile(ref ? ref.agentId : null, env)
  if (legacyFile !== null && legacyFile !== stored.file) {
    const legacy = readAt(legacyFile)
    if (legacy.state === 'present') {
      return { token: legacy.token, source: 'store', state: 'present', legacy: true, legacyFile }
    }
    if (legacy.state === 'unreadable') {
      return { token: null, source: null, state: 'unreadable', reason: legacy.reason, legacy: true }
    }
  }
  const fromEnv = typeof fallback === 'string' ? fallback.trim() : ''
  if (fromEnv !== '') return { token: fromEnv, source: 'env', state: 'present' }
  return { token: null, source: null, state: 'missing' }
}

module.exports = {
  BACKEND_ENV,
  DIR_ENV,
  FILE_SUFFIX,
  WORLD_KEY_LENGTH,
  credentialsDir,
  normalizeWorld,
  worldKey,
  credentialFile,
  legacyCredentialFile,
  read,
  write,
  remove,
  resolve,
}
