'use strict'
/**
 * 本机**用户级环境变量**的**读**（Windows 上是 `HKCU\Environment`）。
 *
 * 用途只有一个：`CELIRA_IM_BACKEND_AUTH` 与 `DEEPSEEK_API_KEY` 这两条**老路 / 手工路**的
 * 回落来源。2026-09-25 起后端凭据的权威位置是 `credentials.js` 那份**按身份分条**的本地
 * 存储（作用域＝「Agent × 那个世界」）；环境变量仍然**读**，但**不再写** —— 一份事实一个权威
 * 位置，两个写者就是抢格子（同一台机器上两个家会互相顶掉对方那一份）。
 *
 * 凭据纪律（红线，一个字节都不许松）：
 * - **只按变量名读**：调用方拿名字进来，值只经 stdout 回内存；
 * - **不落文件、不进 argv、不进日志**：本文件不打印任何值，调用方也不许；
 * - Windows 之外没有 HKCU：读 `process.env` 兜底（写这条路已经不需要了）。
 *
 * 为什么是 PowerShell 而不是 `reg.exe`：`reg query` 会把输出按控制台码页给我们
 * （值里有非 ASCII 就炸），而且它是个可能被安全策略挡住的独立二进制；
 * `[Environment]` 走 .NET，一行拿到值，且**不需要把秘密放进 argv**。
 */

const { spawnSync } = require('node:child_process')

const IS_WINDOWS = process.platform === 'win32'

const READ_SCRIPT =
  '[Console]::OutputEncoding=[Text.Encoding]::UTF8; ' +
  '$v=[Environment]::GetEnvironmentVariable($env:CELIRA_ENV_NAME,"User"); ' +
  'if ($v) { [Console]::Out.Write($v) }'

function powershell(script, env) {
  const done = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', script],
    { env: { ...process.env, ...env }, encoding: 'utf8', windowsHide: true },
  )
  if (done.error) throw new Error(`powershell 起不来：${done.error.message}`)
  return done
}

/** 读一个用户级环境变量；没有/空 ⇒ `null`（空值按"没配"处理，与 bash 版一致）。 */
function readUserEnv(name) {
  if (!IS_WINDOWS) {
    const value = process.env[name]
    return value ? value : null
  }
  let stdout
  try {
    stdout = powershell(READ_SCRIPT, { CELIRA_ENV_NAME: name }).stdout || ''
  } catch {
    // PowerShell 起不来是**环境问题**，不是"凭据不存在"：退到进程环境里再看一眼。
    // 仍然读不到 ⇒ 返回 null，让调用方给出那条诚实的"缺凭据"读数（而不是一个堆栈）。
    const fallback = process.env[name]
    return fallback ? fallback : null
  }
  const value = stdout.trim()
  return value ? value : null
}

module.exports = { IS_WINDOWS, readUserEnv }
