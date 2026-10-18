'use strict'
/**
 * `home.conf` 的读与写 —— **唯一权威**（三个 .sh 各写了一份 `conf_get`，那是三份口径）。
 *
 * 文件长这样（键值对，一行一条）：
 *
 * ```text
 * listen_addr=127.0.0.1:8080
 * client_port=5173
 * human_id=…
 * agent_id=…
 * room_id=…
 * mcp_browser=stdio:npx @playwright/mcp@latest --browser chrome
 * ```
 *
 * 三条纪律：
 * 1. **读一律去行尾 CR**：家在 Windows 上，而记事本（以及 python 的文本模式写回）
 *    都会写 CRLF。bash 版只在 `mcp_` 那条路上去了 CR，其余键没去 —— 那是**潜伏的
 *    不一致**（`listen_addr` 的值会带一个不可见字符，于是端口判据永远不命中）。
 *    这里对所有值统一去 CR：与 python 侧 `conf_get` 的 `.strip()` 是同一种意图。
 * 2. **`mcp_` 只是配置前缀**：`mcp_<名>=<transport>:<命令>` 里的 `<名>` 才是 server 名
 *    （动词前缀）。进 `--mcp` 的是 `<名>=<transport>:<命令>`，等号右边**原样**交出——
 *    解析只有一处权威（宿主入口），本文件不解析、不校验。
 * 3. 值里可以含 `=`：**只按第一个等号切**。
 */

const fs = require('node:fs')
const path = require('node:path')

/** 按行读，去掉 `\r`（容错 CRLF/混合行尾），空文件返回 `[]`。 */
function readLines(file) {
  let raw
  try {
    raw = fs.readFileSync(file, 'utf8')
  } catch {
    return []
  }
  const lines = raw.split('\n')
  // 结尾换行会多出一个空串：去掉它，避免写回时越写越多空行。
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines.map((line) => (line.endsWith('\r') ? line.slice(0, -1) : line))
}

/** 取一个键的值；读不到返回 `fallback`。**只认行首的 `键=`**（与 bash 的 `^key=` 同义）。 */
function confGet(file, key, fallback = '') {
  const prefix = `${key}=`
  for (const line of readLines(file)) {
    if (line.startsWith(prefix)) return line.slice(prefix.length)
  }
  return fallback
}

/**
 * `home.conf` 里声明了几台外部来源的 App（TASK_43）。
 * 返回**剥掉 `mcp_` 前缀**的声明（`<名>=<transport>:<命令>`），按文件里的顺序；
 * 只有裸的 `mcp_`（前缀后面什么都没有）那一种行走掉——`mcp_=` 这类畸形行会原样
 * 交出 `=`，由宿主入口去报（本层不校验，见文件头第 2 条）。
 */
function mcpDeclarations(file) {
  const out = []
  for (const line of readLines(file)) {
    if (!line.startsWith('mcp_')) continue
    const declaration = line.slice('mcp_'.length)
    if (declaration) out.push(declaration)
  }
  return out
}

/** 只要名字（读数用：命令不进读数，那是 argv 的事）。 */
function mcpNames(file) {
  return mcpDeclarations(file).map((declaration) => declaration.split('=', 1)[0])
}

/**
 * 写一个键：**保留其它行**（含全部 `mcp_` 行），同键老行删掉，新行追加在末尾。
 * 统一写 LF（读的一侧对 CRLF 容错，两边都不会看错）。
 */
function confSet(file, key, value) {
  const prefix = `${key}=`
  const lines = readLines(file).filter((line) => !line.startsWith(prefix))
  lines.push(`${key}=${value}`)
  fs.writeFileSync(file, `${lines.join('\n')}\n`, 'utf8')
}

/**
 * 删一个键：整行删掉，其它行保留；返回是否真的删掉了（没有这一行 ⇒ **一个字都不动**）。
 *
 * 为什么需要它（2026-10-01）：一个家第一次按「家 id」取布置键时，它原来那几行世界侧身份
 * （`agent_id` / `room_id`）属于旧的「世界级键」那一套 —— 那个家会在世界里**另建一支 Agent**
 * （旧那支留着不动），本地这两行也必须跟着让位（否则新 Agent 会与旧房对不上）。
 * 没有这一行时**不写盘**：删一个不存在的键不该动文件的任何字节（mtime 也是读数的一部分）。
 */
function confDelete(file, key) {
  const prefix = `${key}=`
  const lines = readLines(file)
  const kept = lines.filter((line) => !line.startsWith(prefix))
  if (kept.length === lines.length) return false
  fs.writeFileSync(file, `${kept.join('\n')}\n`, 'utf8')
  return true
}

/** 从 `listen_addr` 里取出端口号（`127.0.0.1:8080` ⇒ `8080`）。 */
function portOf(listen) {
  const at = listen.lastIndexOf(':')
  return at >= 0 ? listen.slice(at + 1) : listen
}

/**
 * **这个家自己的世界地址**（`http://127.0.0.1:<它挑到的口>`），读不到就是 `null`。
 *
 * 判据一句话：**世界地址由那个家自己说了算，不由调用方写死。**
 *
 * 为什么需要它（2026-10-11，真机门禁换来的）：`up` 建**新**家时会避开已被占用的默认口
 * （`chooseFreshPorts`），并把挑到的那一个写进**那个家自己的** `home.conf`（`up.js` 的
 * `listen_addr=`）。而临时世界那几条门禁从前把 `127.0.0.1:8080` **写死在代码里** ⇒
 * 主人正开着自己的家时：① 门禁根本跑不了（或更糟——临时世界起在 8081，探针却去跟 8080 上的
 * **别人的世界**说话）；② "别在别人的世界上做实验"这条安全判据只能靠"8080 忙就拒绝跑"来兜。
 *
 * ⚠ **读不到就是 `null`，绝不回落到默认口**：回落正是上面第 ① 条那个事故的形状。
 * 调用方拿到 `null` 应当**拒绝跑**（而不是拿一个猜的口去说话）。
 */
function worldBaseOf(homeDir) {
  const listen = String(confGet(path.join(homeDir, 'home.conf'), 'listen_addr', '') || '').trim()
  if (!listen) return null
  return /^https?:\/\//.test(listen) ? listen : `http://${listen}`
}

module.exports = {
  readLines,
  confGet,
  confSet,
  confDelete,
  mcpDeclarations,
  mcpNames,
  portOf,
  worldBaseOf,
}
