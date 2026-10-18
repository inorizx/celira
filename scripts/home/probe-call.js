#!/usr/bin/env node
/**
 * **通话那条真机探针**（2026-10-09 刀 ③）。
 *
 * ## 为什么必须"驱动页面"而不是"调协议"
 *
 * 通话这条链上真正会坏的地方**不在协议里**：协议本来就允许两个 participant 互呼（`CORE_V1_CALL.md`），
 * 服务端那一半有自己的套件。会坏的是**客户端这一半**：
 *   · 来电有没有被收到（第二条流 `/v1/call/stream` 有没有接上、帧有没有被 fail-closed 掉）；
 *   · 接听之后**媒体到底连上没有**（SDP/ICE 有没有往返、`callee 先建对象后采集` 的缓冲对不对）；
 *   · 挂断之后对端看不看得到、**说法对不对**（同一个 `hangup_caller`，两侧说法相反）；
 *   · 通话有没有**在消息那条链上留下事实**（契约 §8：不产生 Message/Human Event/…）。
 * ⇒ 这些只有"两个真人、两个浏览器上下文、真点"才量得到。
 *
 * ## 判据（黑盒优先；界面读数只用来"知道该点哪里"）
 *
 * 1. A 点语音通话 ⇒ A 侧出现「正在呼叫」；
 * 2. **B 侧收到来电** —— 且 B **故意停在会话列表页**（不打开那间房）：来电那一层挂在外壳上，
 *    收不到就是"只有打开那间房才收得到电话"的形状；
 * 3. B 点接听 ⇒ 双方都进「通话中」；
 * 4. ★ **两端媒体都真的连上**（界面读数「已接通」= `RTCPeerConnection.connectionState === 'connected'`）
 *    —— 信令往返 ＋ WebRTC P2P 都成立（假麦克风设备，判据确定）；
 * 5. A 点挂断 ⇒ **B 侧**看到「对方挂断了」（从 B 的角度说，不是把 `hangup_caller` 原样端上来）；
 * 6. ★ **公开协议读**：`GET /v1/calls/{id}` 说这一通是 `ended(hangup_caller)`（`call_id` 从
 *    `POST /v1/calls` 的**响应**里取 —— 不从界面里抠）；
 * 7. ★ **通话在消息那条链上什么都没留下**：那间房的消息列表**逐字节没变**（契约 §8）；
 * 8. 门禁不许空跑（判据条数 > 0）。
 *
 * ⚠ **没验的**（如实登记，不假装）：响铃 45 秒超时（Core 固定值，等它太慢）、
 * Core 重启后 fail-closed 404 收敛、NAT/防火墙下的打洞（契约 §9 明写那属 WebRTC 传输面、
 * 不在验收里）、多方/群呼（P0 明确不做）、视频（壳只放行麦克风）。
 *
 * 世界是**临时家**（`temp-world.js`）：跑完就删，全程不碰真家。两个 Human 都走**公开协议**
 * 造出来（第二个经 Pairing Grant 兑换），token 只经 argv/env 交给页面、**不打印**。
 *
 * 用法： node scripts/home/probe-call.js [--keep] [--headed]
 * 退出码：0 = 判据全过；1 = 有判据不成立；2 = 装配不起来。
 */

const path = require('node:path')

const { startTempWorld, api } = require('./temp-world')

/** 等一段文字出现（**等状态，不等时长**；判据是"界面上真的写着它"）。 */
async function waitForText(page, text, timeoutMs) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const body = await page.locator('body').innerText().catch(() => '')
    if (body.includes(text)) return true
    await page.waitForTimeout(250)
  }
  return false
}

/** 通话那一层（`role="dialog"`）的正文 —— 红的时候用它说清"界面到底写着什么"。 */
async function overlayText(page) {
  const dialog = page.getByRole('dialog', { name: '通话' })
  if ((await dialog.count()) > 0) return (await dialog.innerText()).replace(/\s+/g, ' ').slice(0, 200)
  const body = await page.locator('body').innerText().catch(() => '')
  return `（没有通话层）${body.replace(/\s+/g, ' ').slice(-200)}`
}

/** 登录（地址填本页同源 ⇒ 走 serve_client 的代转，仍是那个临时世界）。 */
async function login(page, uiBase, token) {  await page.goto(`${uiBase}/`, { waitUntil: 'domcontentloaded', timeout: 30000 })
  await page.locator('input[placeholder="http://127.0.0.1:8080"]').fill(uiBase)
  await page.locator('input[placeholder="服务端签发的那一支"]').fill(token)
  await page.getByRole('button', { name: '连接', exact: true }).click()
  await page.waitForURL(/\/app\//, { timeout: 30000 })
  await page.waitForTimeout(2500)
}

/** 那间房的消息列表（判据 7 要比对"有没有多出东西"）。 */
async function messagesOf(base, token, roomId) {
  const listed = await api(base, token, 'GET', `/v1/rooms/${roomId}/messages`)
  const list = (listed.json && (listed.json.messages || listed.json.items)) || []
  return list.map((each) => `${each.id}|${each.body}|${(each.attachments || []).length}`)
}

async function main() {
  const keep = process.argv.includes('--keep')
  const headed = process.argv.includes('--headed')

  let chromium
  try {
    // ⚠ playwright 装在 **client** 里（本脚本在 scripts/home/ 下，从仓根 require 不到）。
    const { createRequire } = require('node:module')
    const clientRequire = createRequire(path.join(__dirname, '..', '..', 'client', 'package.json'))
    ;({ chromium } = clientRequire('playwright'))
  } catch {
    console.error('[probe-call] 没装 playwright（client 的 devDependencies 里有）—— 装配不起来')
    return 2
  }

  let world = null
  let browser = null
  const verdicts = []
  const judge = (name, ok, detail = '') => {
    verdicts.push({ name, ok })
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? `  — ${detail}` : ''}`)
  }

  try {
    world = await startTempWorld()
    const { token, ids, base, uiBase } = world

    // ---- 1) 造"人与人"的那间房（公开协议；两位 Human 在 temp-world 里已是 accepted 联系人）----
    const created = await api(base, token, 'POST', '/v1/human-direct-rooms', { human_id: ids.secondId })
    const roomId = created.json && created.json.room ? created.json.room.id : null
    if (!roomId) throw Object.assign(new Error('建人与人的私人房失败'), { assembly: true, detail: created })
    const meView = await api(base, token, 'GET', '/v1/me')
    const myName = meView.json && meView.json.participant ? meView.json.participant.display_name : ''
    if (!myName) throw Object.assign(new Error('读不到我自己的显示名'), { assembly: true })

    // ---- 2) 两个上下文 = 两个人（各自独立存储与 WS）--------------------------------
    browser = await chromium.launch({
      headless: !headed,
      // 假麦克风：让"媒体能不能连上"这一格**确定**（不弹权限框、不依赖这台机器有没有麦克风）。
      args: [
        '--no-proxy-server',
        '--use-fake-ui-for-media-stream',
        '--use-fake-device-for-media-stream',
      ],
    })
    const ctxA = await browser.newContext()
    const ctxB = await browser.newContext()
    const pageA = await ctxA.newPage()
    const pageB = await ctxB.newPage()

    // `call_id` 从 **`POST /v1/calls` 的响应**里取（公开协议；不从界面里抠）。
    let initiated = null
    pageA.on('response', async (response) => {
      if (response.request().method() !== 'POST') return
      if (!response.url().endsWith('/v1/calls')) return
      try {
        initiated = await response.json()
      } catch {
        /* 不是 JSON 就算了（判据 6 会红并说清） */
      }
    })

    await login(pageA, uiBase, token)
    await login(pageB, uiBase, ids.secondToken)

    // A 进那间房（只有"对端是人"的 1:1 房才给通话入口）；**B 故意留在会话列表页**。
    await pageA.getByText('探针第二位', { exact: false }).first().click()
    await pageA.waitForTimeout(1500)
    const audioButton = pageA.getByRole('button', { name: '语音通话' })
    if ((await audioButton.count()) === 0) {
      throw Object.assign(new Error('A 侧没有语音通话入口（对端是人的 1:1 房应该给）'), { assembly: true })
    }

    // ---- 3) 公共事实基线（通话前）-------------------------------------------------
    const beforeMessages = await messagesOf(base, token, roomId)

    // ---- 4) A 发起 ----------------------------------------------------------------
    await audioButton.click()
    const aCalling = await waitForText(pageA, '正在呼叫', 10000)
    judge(
      'A 侧：发起后出现「正在呼叫」',
      aCalling,
      // 红了要说得出**它在读什么**：通话那一层（`role="dialog"`）的正文，或整页最后一段。
      aCalling ? '' : `A 侧通话层/正文：${await overlayText(pageA)}`,
    )

    // ---- 5) B 收到来电（且 B 停在会话列表页 —— 外壳那一层是对的）----------------------
    const bRang = await waitForText(pageB, '来电', 10000)
    judge(
      '★ B 侧收到来电（停在会话列表页也收得到 —— 来电那一层挂在外壳上）',
      bRang,
      bRang ? '' : `B 侧正文：${(await pageB.locator('body').innerText()).slice(0, 120)}`,
    )

    // ---- 6) B 接听 ⇒ 双方进「通话中」-----------------------------------------------
    if (bRang) await pageB.getByRole('button', { name: '接听' }).click()
    const aTalking = await waitForText(pageA, '通话中', 10000)
    const bTalking = await waitForText(pageB, '通话中', 10000)
    judge('A 侧：进入「通话中」', aTalking)
    judge('B 侧：进入「通话中」', bTalking)

    // ---- 7) ★ 两端媒体都真的连上（信令往返 ＋ WebRTC P2P）----------------------------
    const aConnected = await waitForText(pageA, '已接通', 25000)
    const bConnected = await waitForText(pageB, '已接通', 25000)
    judge(
      '★ 两端媒体都真的连上（`connectionState === connected`）',
      aConnected && bConnected,
      `A=${aConnected} B=${bConnected}`,
    )

    // ---- 8) A 挂断 ⇒ B 侧的说法是「对方挂断了」（从 B 的角度）-------------------------
    await pageA.getByRole('button', { name: '挂断' }).click()
    judge('B 侧：看到「对方挂断了」（不是把 hangup_caller 原样端上来）', await waitForText(pageB, '对方挂断了', 10000))

    // ---- 9) ★ 公开协议读：这一通是 ended(hangup_caller) ------------------------------
    const callId = initiated && initiated.call ? initiated.call.call_id : null
    const readBack = callId ? await api(base, token, 'GET', `/v1/calls/${callId}`) : null
    const callView = readBack && readBack.json ? readBack.json.call : null
    judge(
      '★ 公开协议读：这一通是 ended(hangup_caller)',
      Boolean(callView) && callView.state === 'ended' && callView.ended_reason === 'hangup_caller',
      callView ? `state=${callView.state} reason=${callView.ended_reason}` : `call_id=${callId ? '有' : '没取到'}`,
    )

    // ---- 10) ★ 通话在消息那条链上什么都没留下（契约 §8）------------------------------
    const afterMessages = await messagesOf(base, token, roomId)
    judge(
      '★ 世界里没有留下任何通话痕迹（那间房的消息列表逐条没变）',
      JSON.stringify(beforeMessages) === JSON.stringify(afterMessages),
      `前 ${beforeMessages.length} 条 / 后 ${afterMessages.length} 条`,
    )

    judge('门禁不许空跑：这一轮真的判了 8 条以上', verdicts.length >= 8, `实际 ${verdicts.length} 条`)
  } catch (error) {
    console.error(`[probe-call] 装配/驱动失败：${error && error.message ? error.message : error}`)
    if (error && error.detail) console.error(JSON.stringify(error.detail).slice(0, 400))
    if (world) {
      try {
        world.stop()
      } catch {
        /* 收尾失败不掩盖原因 */
      }
    }
    return 2
  } finally {
    if (browser) await browser.close().catch(() => {})
  }

  // ---- 收尾：临时家整份删掉（`--keep` 留着看现场）--------------------------------
  if (world) {
    try {
      world.stop()
    } catch {
      /* 同上 */
    }
    if (!keep) world.homeDirCleanup()
  }

  const failed = verdicts.filter((each) => !each.ok).length
  console.log(`=== 通话真机：${verdicts.length - failed}/${verdicts.length} 通过${keep ? '（现场留着）' : ' · 临时家已删'} ===`)
  return failed === 0 ? 0 : 1
}

main().then((code) => process.exit(code))
