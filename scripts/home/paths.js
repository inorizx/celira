'use strict'
/**
 * 家的路径与形态判据（`home.js` 全家共用的唯一一份）。
 *
 * 两种形态、同一套命令（与三个 .sh 逐条对齐）：
 *   - **源码形态**（clone）：`target/release` + `agent/target/release` + `client/dist`
 *   - **发布形态**（解压 zip）：`bin/` + `client-dist/`（由 scripts/make_release.ps1 组装）
 *
 * ⚠ 迁移红利之一：Node 里**没有** MSYS / Windows 两套路径世界——
 *   bash 版里的 `cygpath`、`win_path`、「环境变量里的路径不会被转换」那一整类事故
 *   （`\d\celira-home\…` 落到当前盘根下）在这里**结构性地不存在**：全流程只用 `path`
 *   模块算出来的原生路径。所以本文件里一个转换函数都没有，这是**故意**的。
 */

const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

/** 仓库根：本文件在 `<repo>/scripts/home/`。 */
const REPO = path.resolve(__dirname, '..', '..')

/**
 * **家的默认位置按平台**（2026-09-30）：
 * - Windows：`D:/celira-home`（**一字未动** —— 那边今天是这么跑的，明天还这么跑）；
 * - 别的平台：`~/celira-home`（Linux 上 `D:/…` 会被 resolve 成 `<cwd>/D:/celira-home`
 *   —— 一个跟着当前目录跑的"家"，那是事故不是默认值）。
 *
 * 换位置的老路不变：环境变量 `CELIRA_HOME_DIR`（或常驻件命令行里的 `--home`）。
 */
const DEFAULT_HOME =
  process.platform === 'win32' ? 'D:/celira-home' : path.join(os.homedir(), 'celira-home')
const DEFAULT_LISTEN = '127.0.0.1:8080'
const DEFAULT_CLIENT_PORT = '5173'
/**
 * 只读状态口的默认端口（`home.conf` 的 `status_port` 可覆盖）。
 * 放在这里与另两个端口默认值同一个家：它们是**同一类事实**（家的端口）。
 */
const DEFAULT_STATUS_PORT = '5190'
/**
 * **模型配置门**的默认端口（`home.conf` 的 `config_port` 可覆盖）。
 *
 * 与另三个端口同一个家：它们是**同一类事实**（家的端口）。老 `home.conf` 里没有这一键也必须
 * 能起来——新键绝不能变成"必须手改配置才能跑"的门。
 */
const DEFAULT_CONFIG_PORT = '5191'

/** 家目录（`CELIRA_HOME_DIR` 可换）。
 *
 *  `override` 是**常驻件自己在命令行里声明的那一份**（`--home <家目录>`）：给了就以它为准，
 *  没给才看环境变量。为什么要有这条：进程表里读得到命令行、读不到环境变量，而"这支进程
 *  属于哪个家"正是认领判据要的那一格（见 `proc.js` 的家事实）。
 */
function homeDir(override) {
  const declared = typeof override === 'string' && override.trim() !== '' ? override.trim() : null
  return path.resolve(declared || process.env.CELIRA_HOME_DIR || DEFAULT_HOME)
}

/**
 * **建新家时避开已被占用的端口**（纯函数：占用与否由调用方注入 ⇒ 可判错）。
 *
 * 为什么需要它：默认端口是**机器级**命名空间，而"两家共用同一份发布包 + 同一组默认端口"
 * 会让家事实退化（可执行件路径相同、端口也相同 ⇒ 分不出彼此）。真机实测的后果是
 * 本家认领别家的世界、`down` 收尾时把别家的世界停掉。⇒ 从源头让**每个家有自己的端口**，
 * 比事后辨认"这支是不是我的"便宜得多。
 *
 * `taken(port)` 返回 true ⇒ 这个默认口用不了，往后找第一个空闲的（最多找 `span` 个）。
 * 返回 `{ ports, moved, exhausted }`：
 *   · `moved`     被顶开的那些（调用方要把它们说出来）；
 *   · `exhausted` 窗口里**一个都没找到**的那些 —— 调用方应当**拒绝建家**并说清
 *                 （写一份注定起不来的 home.conf 不是诚实降级，是把问题推到下一次开机）。
 *
 * ⚠ **整组一起算，两个口不许落到同一格**（2026-09-30）：从前每个口各自独立往后找，
 * 于是两个门（只读状态口 / 模型配置门）在默认口都被占时会**各自挑到同一个空闲口**
 * （真机实测：两个都挑到 5192）—— 后起的门绑定失败退出（它的日志里是诚实的 EADDRINUSE），
 * 而 `up` 的就绪探活会把**另一个门**应下的 404 当成本门起来了（读数与事实不符）。
 * 现在的两条纪律：
 *   ① 已经被本组定下来的口，别的口不许再挑（**互不相同**）；
 *   ② 挪位时**不占别人家的默认格**（哪怕它此刻空着 —— 那一格可能正是它的主人要留下的）。
 */
function chooseFreshPorts({ defaults, taken, span = 20 }) {
  const keys = Object.keys(defaults)
  // 每个 key 的默认口；读不懂（或不在端口范围）的值单独处理：原样交回，不参与占用判断。
  const baseOf = new Map()
  const defaultPorts = new Set()
  for (const key of keys) {
    const base = Number.parseInt(String(defaults[key]), 10)
    const valid = Number.isInteger(base) && base > 0 && base <= 65535
    baseOf.set(key, valid ? base : null)
    if (valid) defaultPorts.add(base)
  }
  const ports = {}
  const moved = []
  const exhausted = []
  const assigned = new Set()
  for (const key of keys) {
    const raw = defaults[key]
    const base = baseOf.get(key)
    if (base === null) {
      ports[key] = String(raw)
      continue
    }
    // 默认口能用、也没被本组里先说的口占掉 ⇒ 留给自己（现状一字不动）。
    if (!assigned.has(base) && !taken(base)) {
      ports[key] = String(base)
      assigned.add(base)
      continue
    }
    // 要挪：从 base+1 起找第一个「没被本组占、不是别人的默认格、也没被外面占着」的口。
    let found = null
    for (let step = 1; step <= span; step += 1) {
      const candidate = base + step
      if (assigned.has(candidate)) continue
      if (defaultPorts.has(candidate)) continue
      if (taken(candidate)) continue
      found = candidate
      break
    }
    if (found === null) {
      exhausted.push({ key, base })
      ports[key] = String(base)
      continue
    }
    ports[key] = String(found)
    assigned.add(found)
    moved.push({ key, from: base, to: found })
  }
  return { ports, moved, exhausted }
}

function isFile(candidate) {
  try {
    return fs.statSync(candidate).isFile()
  } catch {
    return false
  }
}

// ---------------------------------------------------------------------------
// **家的形状：住户化**（2026-10-01 多支刀，承 PROJECT_SHAPE §8.0）
// ---------------------------------------------------------------------------
//
// 一句话原理：**隔离的事实单位是 Agent，不是机器**。所以：
//   · 一支 Agent 一个**环境根**：`<家>/agents/<支>/`（**与壳同一套布局**，见
//     `app/src-tauri/src/home_facts.rs` 的 `agent_env_root`（2026-10-03 从 `anjia/home.rs` 提升）—— CLI 不许造第三套）；
//   · 支级配置（身份 / 值班室 / 住户序号 / 显示名）住**它自己的环境根**里：`agent.conf`；
//   · 家级配置（`home.conf`）只留机器级事实（端口表、世界地址、`home_id`、`mcp_*`、
//     家钥匙摘要……）。
//
// ⚠ **老家的形状**（2026-09-30 以前，一个家一支）：环境根是 `<家>/env`。它是**迁移的输入**
// （见 `residents.js` 的 `migrateHomeShape`），不是今天的布局 —— 本文件里所有 `agent*`
// 函数都只认新布局。

/** 住户目录名（一支一个子目录）。 */
const AGENTS_DIR = 'agents'
/** 老家的单支环境根目录名（迁移输入；迁完就不再出现）。 */
const LEGACY_ENV_DIR = 'env'
/** 支级配置文件名（住在环境根里，跟着这支 Agent 走）。 */
const AGENT_CONF = 'agent.conf'

/** 住户根：`<家>/agents/`。 */
function agentsRoot(homeDir) {
  return path.join(homeDir, AGENTS_DIR)
}

/** 资料根：`<家>/materials/`（宿主 `--materials` 给的就是它；Agent 读得到的东西都从这里授权出去）。 */
function materialsRoot(homeDir) {
  return path.join(homeDir, 'materials')
}

/**
 * 技能区：`<家>/materials/skills/`。
 *
 * **建家时要把它建出来，哪怕一个技能都没有**（2026-10-20）。判据在 Agent 那一侧：
 * `agent/src/skills_desktop.rs` 把 `None`（没有技能区 ⇒ 整块不出场）与 `Some(0)`
 * （有、但空 ⇒ `skills: 0 项（技能区为空）`）当成两件事。不建它，"这具身体没有技能区"
 * 与"技能区空着"在主体眼里合成一片空白——真机上她据此答成"没有可安装的技能包"。
 * 语义权威在 `skills_desktop.rs`，这里只管家的形状。
 */
function skillsRoot(homeDir) {
  return path.join(materialsRoot(homeDir), 'skills')
}

/**
 * 一支 Agent 的**环境根**：`<家>/agents/<支>/`。
 *
 * 一支一个：身份（`identity.json`）、私有状态（经历 / 记忆 / 笔记 / 活动槽）、
 * 支级配置（`agent.conf`）、模型绑定（`model_binding.json`）都在它里面 ——
 * **绝不与第二支共用**（共用一段记忆是产品错误，不是省事）。
 */
function agentEnvRoot(homeDir, agentId) {
  return path.join(agentsRoot(homeDir), String(agentId).trim())
}

/** 支级配置：`<环境根>/agent.conf`（身份 / 值班室 / 住户序号 / 显示名）。 */
function agentConfPath(homeDir, agentId) {
  return path.join(agentEnvRoot(homeDir, agentId), AGENT_CONF)
}

/**
 * 一支 Agent 的**运行账本目录**：`<家>/logs/agents/<支>/`。
 *
 * 为什么账本不进环境根：pid 文件与日志是**机器侧编排事实**，而环境根会跟着 Agent 换机器
 * （「一支 Agent 换机器 = 把它的环境根搬过去」）—— 搬走的档案里不该夹着一个别的机器上的 pid。
 */
function agentLogDir(homeDir, agentId) {
  return path.join(homeDir, 'logs', 'agents', String(agentId).trim())
}

/** 一支 Agent 的宿主 pid 文件（`up` 写、`down` 读）。 */
function agentPidFile(homeDir, agentId) {
  return path.join(agentLogDir(homeDir, agentId), 'host.pid')
}

/** 一支 Agent 的宿主日志（`[listening]` / `[run]` / `[exit]` 都落在它里面）。 */
function agentHostLog(homeDir, agentId) {
  return path.join(agentLogDir(homeDir, agentId), 'host.out')
}

/** 老家的单支环境根：`<家>/env`（迁移输入）。 */
function legacyEnvRoot(homeDir) {
  return path.join(homeDir, LEGACY_ENV_DIR)
}

/**
 * 可执行件名：Windows 上是 `<base>.exe`，别的平台是 `<base>`。
 *
 * 两样都不在时返回**本平台**该有的那个名字——"缺可执行件"这条读数要说的是"**这台机器上**
 * 它该叫什么"。从前一律返回 `.exe`（Windows 优先），于是在 Linux 上报出 `celira-im.exe`：
 * 一个这台机器上永远不会有的名字，把"包里没有 core"和"少了一个文件"混成一件事（真机撞到过）。
 */
function exeIn(dir, base) {
  const win = path.join(dir, `${base}.exe`)
  if (isFile(win)) return win
  const plain = path.join(dir, base)
  if (isFile(plain)) return plain
  return process.platform === 'win32' ? win : plain
}

/**
 * 形态判据：**两问，不再是一问**（2026-09-30）。
 *
 * ① `packaged`：我在一个**发布包**里吗？判据＝`bin/` 里有 **Agent 的可执行件**（`agent_host`）
 *    —— 这是"可执行件从哪儿来"这个问题的**直接**答案。
 * ② `hasCore`：这台机器上**有没有 core 的可执行件**？它只决定**编排**（本机全量 vs 接远端世界），
 *    **不是**形态判据。
 *
 * 从前把两问合成一问（"`bin/celira-im` 在不在" ⇒ 是不是发布形态），于是 **runtime 子包**
 * （按设计**不含 core**）把自己判成源码形态，反过来要用户 `cargo build`，还让他用
 * `.\scripts\install.ps1`（Linux 上根本没有那个文件）。**形态判据不该锚在"某个成员在不在"上**：
 * 包就是包，缺的是编排（见 PROJECT_SHAPE 缺口 1 / 4）。
 *
 * 纯函数（接受 repo 根，便于测试）：返回形态 + 三组路径 + 四个可执行件 + `hasCore`。
 */
function detectLayout(repo = REPO) {
  const bin = path.join(repo, 'bin')
  const packaged =
    isFile(path.join(bin, 'agent_host.exe')) || isFile(path.join(bin, 'agent_host'))
  const agentDir = packaged ? bin : path.join(repo, 'agent', 'target', 'release')
  const imExe = packaged
    ? exeIn(bin, 'celira-im')
    : exeIn(path.join(repo, 'target', 'release'), 'celira-im')
  return {
    kind: packaged ? 'release' : 'source',
    hasCore: isFile(imExe),
    repo,
    imExe,
    agentDir,
    hostExe: exeIn(agentDir, 'agent_host'),
    statusExe: exeIn(agentDir, 'agent_host_status'),
    runExe: exeIn(agentDir, 'agent_run'),
    clientDir: path.join(repo, 'client'),
    clientDist: packaged ? path.join(repo, 'client-dist') : path.join(repo, 'client', 'dist'),
  }
}

module.exports = {
  REPO,
  DEFAULT_HOME,
  DEFAULT_LISTEN,
  DEFAULT_CLIENT_PORT,
  DEFAULT_STATUS_PORT,
  DEFAULT_CONFIG_PORT,
  homeDir,
  chooseFreshPorts,
  isFile,
  exeIn,
  detectLayout,
  // **家的形状：住户化**（2026-10-01 多支刀）
  AGENTS_DIR,
  LEGACY_ENV_DIR,
  AGENT_CONF,
  agentsRoot,
  materialsRoot,
  skillsRoot,
  agentEnvRoot,
  agentConfPath,
  agentLogDir,
  agentPidFile,
  agentHostLog,
  legacyEnvRoot,
}
