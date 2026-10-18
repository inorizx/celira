#!/usr/bin/env node
/**
 * **临时世界 → 喂饱它 → 跑 `verify:ui`**（2026-10-08 刀）。
 *
 * ## 为什么需要它
 *
 * `verify:ui`（web 形态真机断言）从 2026-09-26 起**没人跑得起来**：它要一支 Human token，
 * 而"从凭据库里掏 token"是禁止的（会进 argv/日志/输出）。这条路绕开那个两难：
 * **起一个临时家 → 铸一支只属于这个临时世界的 token → 喂饱它 → 跑门禁 → 删掉整个临时家**。
 *
 * 装配全在 `./temp-world.js`（喂料清单、安全判据、收尾只有那一份；`probe-forward.js` 也用它）。
 *
 * ## 用法
 *
 *   node scripts/home/probe-ui.js [--keep]
 *
 * 退出码：**0** = 门禁全绿 ／ **1** = 门禁有断言失败 ／ **2** = 装配不起来（含"8080 上已经有
 * 一个世界在跑"——那可能是**真家**，绝不在它上面做实验）。
 *
 * ⚠ 凭据纪律：token 只经 **argv** 交给子进程（那是 `verify-ui.mjs` 的既定接口），
 * **不打印、不写文件、不进日志**。
 */
const path = require('node:path')
const fs = require('node:fs')
const { spawnSync } = require('node:child_process')

const { startTempWorld, REPO_DIR } = require('./temp-world')

const VERIFY_UI = path.join(REPO_DIR, 'client', 'scripts', 'verify-ui.mjs')

async function main() {
  const keep = process.argv.includes('--keep')
  let world = null
  try {
    world = await startTempWorld()
    console.log('[probe-ui] 开始跑门禁（token 只走 argv）')

    const gate = spawnSync(
      process.execPath,
      [VERIFY_UI, '--core', world.uiBase, '--token', world.token, '--home', world.homeDir],
      {
        cwd: REPO_DIR,
        stdio: 'inherit',
        env: {
          ...process.env,
          no_proxy: '127.0.0.1,localhost',
          NO_PROXY: '127.0.0.1,localhost',
          // 那支 Agent 的 BackendAuth 凭据（判「对方已读」那一支要推进它的读游标）。
          // ⚠ 只经 env 交子进程、**不打印、不写文件、不进日志**（与 Human token 同规）。
          ...(world.ids && world.ids.agentCredential
            ? { CELIRA_PROBE_AGENT_CREDENTIAL: world.ids.agentCredential }
            : {}),
          // 夹具那间 direct 房（**那一支 Agent 真的在里面**）——判「对方已读」那一支要
          // 明确进这间房去推进它的读游标（"打开的第一行"可能是别的房，实测踩过：404）。
          ...(world.ids && world.ids.directRoomId
            ? { CELIRA_PROBE_DIRECT_ROOM_ID: world.ids.directRoomId }
            : {}),
        },
      },
    )
    console.log(`[probe-ui] 门禁退出码 = ${gate.status}`)
    return gate.status === 0 ? 0 : 1
  } catch (error) {
    if (error && error.assembly) {
      console.error(`[probe-ui] ${error.message}`)
      return 2
    }
    console.error(`[probe-ui] 探针自己抛了：${error && error.stack ? error.stack : error}`)
    return 2
  } finally {
    if (world) {
      world.stop()
      if (keep) console.log(`[probe-ui] --keep ⇒ 临时家留着：${world.homeDir}`)
      else {
        fs.rmSync(world.homeDir, { recursive: true, force: true })
        console.log('[probe-ui] 临时家已删')
      }
    }
  }
}

main().then(
  (code) => process.exit(code),
  (error) => {
    console.error(`[probe-ui] 探针自己抛了：${error && error.stack ? error.stack : error}`)
    process.exit(2)
  },
)
