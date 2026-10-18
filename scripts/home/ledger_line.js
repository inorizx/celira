'use strict'
/**
 * **账本一行的读法**（家侧唯一一处）。
 *
 * 一行的形状是 `<JSON 正文>` 或 `<JSON 正文>\t<8 位十六进制>`：**2026-10-06 起**，经历与事实的
 * 每一行**行尾都带一个 CRC-32 链值**。权威在 Agent 侧——`agent/src/store/format.rs` 的
 * `signed_line`（怎么写）与 `line_sum`（怎么切）：那一段校验的是**盘上真正的那串字节**，
 * 而正文里不可能出现裸制表符（JSON 会把它转义成 `\t` 两个字符），所以按**最后一个**制表符切
 * 是无歧义的；制表符之后**不是**十六进制 ⇒ 权威读法在这一档**明确失败**（fail-closed）。
 *
 * ⚠ **为什么要有这个模块**：这条规则变过一次，而家侧几个读者各自 `JSON.parse(整行)`
 * ⇒ 它们对那之后写下的**每一行都解不出来**，却**一声不吭**（真机读数，2026-10-14）：
 *
 *   · `status-server.js` 的「经历」——小欣那本 3353 条记录，读数成 `{state:'loaded', rows:[]}`
 *     （一条都没有）；而那段代码自己的注释写着「**"没有经历"与"读不到经历"是两件事**」。
 *   · `provision.js` 的「经历 N 段」——整段省略（`null`）。
 *
 * 两处的用例都没抓住，因为 fixture 里写的是**加校验和之前**的行。⇒ 现在两处的 fixture 都按
 * **今天的形状**写，并且这一条规则只在这里写一遍。
 *
 * 三态（与 `LineSum` 一一对应，**不许压成两态**）：
 *   · `present`    带校验列（今天）：正文是制表符之前那一段；
 *   · `absent`     旧记录（没有那一列）：正文是整行；
 *   · `unreadable` 制表符之后不是十六进制 ⇒ 尾巴被截断、或这一行被改过。
 *     **调用方不许把它当 `absent`**（"没有校验和"与"校验和读不出来"是两件事）。
 *
 * 本模块**不核链**（`chain_i = crc32(chain_{i-1}, body_i)` 那条链的核对归经历存储与
 * `environment_diagnose`）：它只把那一列切开。`tools/trace_view.py`（Python）跨不了语言，
 * 按同一条规则自己切，并由一条**跨语言对照**用例钉住。`probe-recall.js` 那条读法也照同一条
 * 规则，但它有自己的取证口径与真机门禁，未并进来。
 */

/** 切一行：返回 `{ body, state }`，`state` 是 `present` / `absent` / `unreadable` 三态之一。 */
function splitLedgerLine(line) {
  const index = line.lastIndexOf('\t')
  if (index < 0) return { body: line, state: 'absent' }
  const tail = line.slice(index + 1)
  // 与 `u32::from_str_radix(tail, 16)` 对齐：1–8 位十六进制；多了就是溢出（那也读不出来）。
  if (/^[0-9a-fA-F]{1,8}$/.test(tail)) return { body: line.slice(0, index), state: 'present' }
  return { body: line, state: 'unreadable' }
}

/** 一行的正文。读不出来时**原样交回**——怎么说由调用方定，本模块不替它吞。 */
function ledgerBody(line) {
  return splitLedgerLine(line).body
}

module.exports = { splitLedgerLine, ledgerBody }
