#!/usr/bin/env node
/**
 * **壳里"麦克风那一格"真机探针**（2026-10-09 立）——语音消息与将来的通话都压在它上面。
 *
 * ## 为什么必须有它
 *
 * 壳里那份 WebView2 的权限处理器由 wry 装，**只放行剪贴板读**
 * （`wry-0.57.0/src/webview2/mod.rs` 的 `PermissionRequested`）⇒ `getUserMedia` 在壳里
 * **永远不答**。2026-10-09 实测到的症状是"**按了麦克风按钮界面什么都不发生**"（不是被拒，
 * 是没反应）——而**只改浏览器参数**（自动放行 ＋ 假设备）立刻 `granted` ⇒ 管道没问题，堵点在权限。
 * 修法两半：客户端一条有界等待 ＋ 稳定分类（`client/src/lib/voice.ts`），壳侧
 * `app/src-tauri/src/lib.rs` 的 `on_permission_request` **只放行麦克风**（2026-10-09 主人裁）。
 *
 * ⇒ 这条探针量的就是**那一处放行还在不在**：它跑两次，**只改浏览器参数**（同一颗壳、同一个家、
 * 同一段代码）：
 *
 *   A. **默认参数**：真实用户走的那条路 ⇒ `granted` = 放行在位；`no-answer` = **那一处被摘掉了**
 *   B. `--use-fake-ui-for-media-stream --use-fake-device-for-media-stream`：辅助读数 ——
 *      A 若落成 `no_device`（这台机器没麦克风），B 仍该 `granted`，用来分清"没设备"与"没放行"
 *
 * ## 退出码
 *
 * - **0** = 两档都**落定**（A 档 `granted`，或 A 档是一个明确分类而 B 档 `granted`）
 * - **1** = A 档**没落定**（`no-answer`）⇒ 壳里的放行被摘掉了，或者换了一颗没带它的壳
 * - **2** = 环境没就绪（没有打包形态的壳）
 *
 * ## 收尾
 *
 * 与 `verify:ui:app` 同规：**只收自己起的**（世界按端口差集＋身份，住户按基线差集）。
 * ⚠ 它**会把这个家的世界起起来**（打开壳本来就会这样）。
 *
 * **触发条件**：改壳的权限处理器、改 `client/src/lib/voice.ts` 那条采集路、或接通话（Call P0）时跑它。
 */

import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

import { chromium } from 'playwright'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const REPO = path.resolve(HERE, '..', '..')
const APP = path.join(REPO, 'app')
const EXE = path.join(APP, 'src-tauri', 'target', 'release', 'celira-app.exe')
const homePaths = createRequire(import.meta.url)('../../scripts/home/paths.js')
const homeConf = createRequire(import.meta.url)('../../scripts/home/conf.js')
const homeProc = createRequire(import.meta.url)('../../scripts/home/proc.js')
const homeResidents = createRequire(import.meta.url)('../../scripts/home/residents.js')

const home = (process.env.CELIRA_HOME_DIR || '').trim() || homePaths.DEFAULT_HOME
const shellLayout = {
  ...homePaths.detectLayout(),
  hostExe: path.join(path.dirname(EXE), 'bin', 'agent_host.exe'),
}
const homeFacts = () => {
  const list = homeResidents.runtimeResidents(home, path.join(home, 'home.conf'))
  return { homeDir: home, envDirs: list.map((r) => r.envDir) }
}
const residentsNow = () => {
  try {
    return homeProc.describeHostProcesses(shellLayout, homeFacts()).map((r) => r.pid)
  } catch {
    return null
  }
}
const confFile = path.join(home, 'home.conf')
const imPort = homeConf.portOf(homeConf.confGet(confFile, 'listen_addr', homePaths.DEFAULT_LISTEN))

const worldPidsBefore = homeProc.listenersOnPort(imPort)
const hostPidsBefore = residentsNow()

if (!fs.existsSync(EXE)) {
  console.error(`[mic] 没有打包形态的壳（${EXE}）—— 先 cd app && npm run build -- --no-bundle`)
  process.exit(2)
}
console.log(`[mic] 壳 = ${EXE}`)
console.log(
  `[mic] 家 = ${home} · 世界口 ${imPort} · 进来时住户 ${hostPidsBefore === null ? '读不到' : `${hostPidsBefore.length} 支`}`,
)

/** 起壳 → 连 CDP → 在页面里问一次麦克风 → 关掉。 */
async function probe(label, port, extraArgs) {
  const shell = spawn(EXE, [], {
    cwd: APP,
    stdio: 'ignore',
    env: {
      ...process.env,
      CELIRA_HOME_DIR: home,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        `--remote-debugging-port=${port}` + (extraArgs ? ` ${extraArgs}` : ''),
    },
  })
  const cdpUrl = `http://127.0.0.1:${port}`
  const deadline = Date.now() + 60000
  let alive = false
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${cdpUrl}/json/version`, { signal: AbortSignal.timeout(1000) })
      if (response.ok) {
        alive = true
        break
      }
    } catch {
      /* 还没起来 */
    }
    await new Promise((r) => setTimeout(r, 500))
  }
  if (!alive) {
    console.log(`[mic] ${label}：壳起来了但 CDP 没开 —— 这一档没测到`)
    shell.kill()
    return null
  }
  const browser = await chromium.connectOverCDP(cdpUrl)
  const page = browser.contexts()[0].pages()[0]
  await page.setDefaultTimeout(30000)
  for (let i = 0; i < 40; i += 1) {
    if (/^https?:/.test(page.url()) && page.url().includes('tauri.localhost')) break
    await page.waitForTimeout(500)
  }
  // ⚠ 界与产品同一条（`client/src/lib/voice.ts` 的 `VOICE_PERMISSION_TIMEOUT_MS`）：
  //   探针自己另定一个更长的界，就量不出"产品那条路会不会挂住"。
  const verdict = await page.evaluate(async () => {
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== 'function') {
      return { kind: 'no-api' }
    }
    try {
      const stream = await Promise.race([
        navigator.mediaDevices.getUserMedia({ audio: true }),
        new Promise((_, reject) => setTimeout(() => reject(new Error('no-answer-8s')), 8000)),
      ])
      const tracks = stream.getAudioTracks().length
      for (const track of stream.getTracks()) track.stop()
      return { kind: 'granted', tracks }
    } catch (error) {
      return {
        kind: String(error && error.message).startsWith('no-answer') ? 'no-answer' : 'rejected',
        name: String(error && error.name),
      }
    }
  })
  console.log(`[mic] ${label} ⇒ ${JSON.stringify(verdict)}  （页面 ${page.url()}）`)
  await browser.close()
  shell.kill()
  await new Promise((r) => setTimeout(r, 1500))
  return verdict
}

const a = await probe('A 默认参数（真实用户那条路）', 9333, '')
const b = await probe('B 自动放行＋假设备（辅助读数：只改浏览器参数）', 9334, '--use-fake-ui-for-media-stream --use-fake-device-for-media-stream')

// ── 收尾：只收自己起的（与 verify:ui:app 同一条纪律）────────────────────────
spawnSync('node', ['home.js', 'down'], { cwd: REPO, encoding: 'utf8' })
const shellTarget = path.join(APP, 'src-tauri', 'target')
const isUnder = (child, dir) => {
  const rel = path.relative(path.resolve(dir), path.resolve(child || ''))
  return rel !== '' && !rel.startsWith('..') && !path.isAbsolute(rel)
}
for (const pid of homeProc.listenersOnPort(imPort)) {
  if (worldPidsBefore.includes(pid)) continue
  const info = homeProc.describeProcess(pid)
  if (/^celira-im/i.test(String(info?.name ?? '')) && isUnder(String(info?.exePath ?? ''), shellTarget)) {
    await homeProc.killPid(pid, { force: true })
    console.log(`[mic] 收掉孤儿世界 pid ${pid}`)
  }
}
if (hostPidsBefore !== null) {
  let rows = []
  try {
    rows = homeProc.describeHostProcesses(shellLayout, homeFacts())
  } catch {
    /* 读不到就不动手（认不出就不杀） */
  }
  for (const row of rows.filter((r) => !hostPidsBefore.includes(r.pid))) {
    await homeProc.killPid(row.pid, { force: true })
    console.log(`[mic] 收掉孤儿住户 pid ${row.pid}`)
  }
}

if (!a) {
  console.log('[mic] A 档没测到（CDP 没开）⇒ 环境没就绪')
  process.exit(2)
}
if (a.kind === 'granted') {
  console.log('[mic] ✅ 壳里麦克风那一格通着（默认参数就能拿到）')
  process.exit(0)
}
if (a.kind === 'no-answer') {
  console.log('[mic] ❌ A 档**没落定**（8 秒无应答）⇒ 壳里的放行被摘掉了，或换了一颗没带它的壳')
  process.exit(1)
}
if (b && b.kind === 'granted') {
  console.log(`[mic] ⚠ A 档落成 ${a.kind}（${a.name ?? ''}）而 B 档 granted ⇒ 权限那一格通着，是设备/环境这一档`)
  process.exit(0)
}
console.log(`[mic] ⚠ A 档落成 ${a.kind}（${a.name ?? ''}），B 档也没拿到 ⇒ 管道这一档有问题`)
process.exit(1)
