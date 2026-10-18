#!/usr/bin/env node
/**
 * **转发那条真机驱动**（2026-10-08 刀）。
 *
 * ## 为什么是"驱动页面"而不是"调协议"
 *
 * 转发带附件这件事**全部发生在客户端**（取原字节 → 在目标房重新上传一份 → 绑进新消息，
 * 见 `client/src/lib/forwardMessage.ts`）：协议层验它等于**没验**（协议本来就能收发附件）。
 * 所以这条探针**真的去点**：登录 → 进房 → 选文件 → 发送 → 右键 → 转发 → 选目标房。
 *
 * ## 判据（黑盒：只看世界里的公共事实，不看界面自述）
 *
 * 1. 源房里那条消息**带上了附件**（上传 + 发送那段路真的成了）；
 * 2. 目标房里出现了一条**新消息**，它的附件**文件名与原件一致**；
 * 3. **目标房那份附件的字节与原件逐字节相同** —— 这是"转发真的把内容带过去了"的硬证据
 *    （只看 id 或名字都能被"占了个位"骗过去）。
 *
 * ⚠ 世界是**临时家**（`temp-world.js`）：跑完就删，全程不碰真家。
 *
 * 用法：node scripts/home/probe-forward.js [--keep] [--headed]
 * 退出码：0 = 三条判据都成立；1 = 判据不成立；2 = 装配不起来。
 */
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const { startTempWorld, api } = require('./temp-world')

const FILE_NAME = '探针附件.txt'
const FILE_BODY = `celira-forward-probe ${Date.now()}`
const MESSAGE_BODY = '这条带附件，转发给群里那份'

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
    console.error('[probe-forward] 没装 playwright（client 的 devDependencies 里有）—— 装配不起来')
    return 2
  }

  let world = null
  let browser = null
  let page = null
  const verdicts = []
  const judge = (name, ok, detail = '') => {
    verdicts.push({ name, ok })
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? `  — ${detail}` : ''}`)
  }

  try {
    world = await startTempWorld()
    const { token, ids, base, uiBase } = world

    // 本地造一份"要转发的文件"：**名字定死**（判据要按它核对），目录另起（免得撞名）。
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-forward-'))
    const localFile = path.join(dir, FILE_NAME)
    fs.writeFileSync(localFile, FILE_BODY, 'utf8')

    browser = await chromium.launch({ headless: !headed, args: ['--no-proxy-server'] })
    page = await browser.newPage()

    // ---- 1) 登录（地址填本页同源 ⇒ 走 serve_client 的代转，仍是那个临时世界）----
    await page.goto(`${uiBase}/`, { waitUntil: 'domcontentloaded', timeout: 30000 })
    await page.locator('input[placeholder="http://127.0.0.1:8080"]').fill(uiBase)
    await page.locator('input[placeholder="服务端签发的那一支"]').fill(token)
    await page.getByRole('button', { name: '连接', exact: true }).click()
    await page.waitForURL(/\/app\//, { timeout: 30000 })
    await page.waitForTimeout(3000)
    console.log('[probe-forward] 进到应用里了')

    // ---- 2) 进"和 Agent 的那间私人房"，选文件 + 发送 --------------------------
    await page.getByText('探针小测', { exact: false }).first().click()
    await page.waitForTimeout(1500)
    await page.locator('input[type="file"]').setInputFiles(localFile)
    // 等待发条里出现那个文件（上传是异步的）。
    await page.getByText(FILE_NAME, { exact: false }).first().waitFor({ timeout: 20000 })
    await page.locator('textarea[placeholder=""]').first().fill(MESSAGE_BODY)
    // ⚠ 用角色定位：`getByLabel('发送')` 会同时命中输入框（aria-label="发送到 <房名>"）与按钮。
    await page.getByRole('button', { name: '发送', exact: true }).click()
    // 等那条消息真的落地（气泡里出现正文）。
    await page.getByText(MESSAGE_BODY, { exact: false }).first().waitFor({ timeout: 20000 })
    await page.waitForTimeout(1500)

    // 判据 1：源房里那条消息带上了附件。
    const sent = await api(base, token, 'GET', `/v1/rooms/${ids.directRoomId}/messages`)
    const list = (sent.json && (sent.json.messages || sent.json.items)) || []
    const sourceMessage = list.find((each) => each.body === MESSAGE_BODY)
    const sourceAttachment = sourceMessage && sourceMessage.attachments && sourceMessage.attachments[0]
    judge(
      '判据 1：源房里那条消息带着附件（上传 + 发送那段路真的成了）',
      Boolean(sourceAttachment),
      sourceAttachment ? `${sourceAttachment.file_name} · ${sourceAttachment.size_bytes} B` : '消息或附件没找到',
    )
    if (!sourceAttachment) return 1

    // ---- 3) 右键 → 转发 → 选群房 → 转发 --------------------------------------
    const bubble = page.locator(`[data-message-id="${sourceMessage.id}"]`).first()
    await bubble.click({ button: 'right' })
    await page.getByText('转发', { exact: true }).first().click()
    // ⚠ 定位一律**收进面板里**：房名在面板背后的会话列表里也有一个，点它会撞上遮罩
    //   （Playwright 报 "sheet-overlay intercepts pointer events"）。
    const sheet = page.getByRole('dialog')
    await sheet.getByText('探针群', { exact: false }).first().click()
    await sheet.getByRole('button', { name: /^转发$/ }).click()
    await page.waitForTimeout(5000)

    // ---- 4) 判据 2/3：目标房里那条新消息，附件名字对、**字节也逐字节相同** -----
    const after = await api(base, token, 'GET', `/v1/rooms/${ids.groupId}/messages`)
    const groupList = (after.json && (after.json.messages || after.json.items)) || []
    const forwarded = groupList.filter((each) => each.attachments && each.attachments.length > 0).pop()
    const forwardedAttachment = forwarded && forwarded.attachments[0]
    judge(
      '判据 2：目标房里出现了一条带附件的新消息，文件名与原件一致',
      Boolean(forwardedAttachment) && forwardedAttachment.file_name === FILE_NAME,
      forwardedAttachment ? `${forwardedAttachment.file_name} · ${forwardedAttachment.size_bytes} B` : '目标房里没有带附件的消息',
    )
    if (!forwardedAttachment) return 1

    const downloaded = await fetch(`${base}/v1/rooms/${ids.groupId}/attachments/${forwardedAttachment.id}`, {
      headers: { Authorization: `Bearer ${token}` },
    })
    const bytes = Buffer.from(await downloaded.arrayBuffer())
    judge(
      '判据 3：目标房那份附件的**字节与原件逐字节相同**',
      downloaded.status === 200 && bytes.toString('utf8') === FILE_BODY,
      `HTTP ${downloaded.status} · ${bytes.length} B`,
    )

    // ---- 5) 批量：多选两条（**故意先点后一条**），断言出去的是**时间线顺序** ----------
    //
    // ⚠ 这一段验的是"顺序"这条判据 —— 它是那种**错了也没人当场发现**的错（对面读到的会是
    //   一段被打乱的对话），所以必须在真机上按"反着点"来验。
    const SECOND_BODY = '第二条（批量用）'
    await page.locator('textarea[placeholder=""]').first().fill(SECOND_BODY)
    await page.getByRole('button', { name: '发送', exact: true }).click()
    await page.getByText(SECOND_BODY, { exact: false }).first().waitFor({ timeout: 20000 })
    await page.waitForTimeout(1500)

    const beforeBatch = await api(base, token, 'GET', `/v1/rooms/${ids.groupId}/messages`)
    const groupBefore = ((beforeBatch.json && (beforeBatch.json.messages || beforeBatch.json.items)) || []).length

    // 先右键**后一条**进多选，再点**前一条** ⇒ 选中顺序是反的，时间线顺序是正的。
    await page.locator(`[data-message-id]`).last().click({ button: 'right' })
    await page.getByText('选择', { exact: true }).first().click()
    await page.waitForTimeout(400)
    await page.locator(`[data-message-id]`).first().click()
    await page.waitForTimeout(400)

    const toolbar = page.getByRole('toolbar', { name: '多选动作' })
    const selectedLabel = await toolbar.innerText()
    judge(
      '判据 4：多选条说得出选了几条',
      /已选\s*2\s*条/.test(selectedLabel),
      selectedLabel.replace(/\s+/g, ' ').slice(0, 60),
    )
    await toolbar.getByRole('button', { name: '转发', exact: true }).click()
    const batchSheet = page.getByRole('dialog')
    const sheetText = await batchSheet.innerText()
    judge(
      '判据 5：面板说清是**一批**、而且按时间顺序',
      /2 条消息/.test(sheetText) && /按时间顺序/.test(sheetText),
      sheetText.replace(/\s+/g, ' ').slice(0, 80),
    )
    await batchSheet.getByText('探针群', { exact: false }).first().click()
    await batchSheet.getByRole('button', { name: '转发 2 条' }).click()
    await page.waitForTimeout(6000)

    const afterBatch = await api(base, token, 'GET', `/v1/rooms/${ids.groupId}/messages`)
    const groupAfter = (afterBatch.json && (afterBatch.json.messages || afterBatch.json.items)) || []
    const newOnes = groupAfter.slice(groupBefore)
    judge(
      '判据 6：★ 目标房多出**两条**，且顺序是**时间线顺序**（先发的在前）',
      newOnes.length === 2 &&
        newOnes[0].body === MESSAGE_BODY &&
        newOnes[1].body === SECOND_BODY,
      newOnes.map((each) => `「${String(each.body).slice(0, 14)}」`).join(' → ') || '目标房没多出消息',
    )
    const batchAttachment = newOnes[0] && newOnes[0].attachments && newOnes[0].attachments[0]
    const batchBytes = batchAttachment
      ? Buffer.from(
          await (
            await fetch(`${base}/v1/rooms/${ids.groupId}/attachments/${batchAttachment.id}`, {
              headers: { Authorization: `Bearer ${token}` },
            })
          ).arrayBuffer(),
        ).toString('utf8')
      : ''
    judge(
      '判据 7：批量里带附件的那一条，字节同样逐字节相同',
      Boolean(batchAttachment) && batchBytes === FILE_BODY,
      batchAttachment ? `${batchAttachment.file_name} · ${batchBytes.length} B` : '没带附件',
    )

    // ---- 6) 整窗拖放：往**窗口任意处**丢一个文件（不是丢在输入框上） ------------------
    //
    // 注意这里**不经 <input type=file>**：直接构造一次真实拖放（DataTransfer ＋ DragEvent）
    // 丢给 window —— 判据里的"整窗"就是这件事，丢在输入框上验不出来。
    const DROPPED_NAME = '拖放进来的.txt'
    const DROPPED_BODY = `celira-drop-probe ${Date.now()}`
    await page.evaluate(
      ({ name, body }) => {
        const dt = new DataTransfer()
        dt.items.add(new File([new TextEncoder().encode(body)], name, { type: 'text/plain' }))
        for (const type of ['dragover', 'drop']) {
          window.dispatchEvent(
            new DragEvent(type, { dataTransfer: dt, bubbles: true, cancelable: true }),
          )
        }
      },
      { name: DROPPED_NAME, body: DROPPED_BODY },
    )
    const chipShown = await page
      .getByText(DROPPED_NAME, { exact: false })
      .first()
      .waitFor({ timeout: 20000 })
      .then(() => true)
      .catch(() => false)
    judge('判据 8：拖到窗口任意处，文件真的进了待发条', chipShown, chipShown ? DROPPED_NAME : '待发条里没出现它')

    await page.getByRole('button', { name: '发送', exact: true }).click()
    await page.waitForTimeout(6000)
    const afterDrop = await api(base, token, 'GET', `/v1/rooms/${ids.directRoomId}/messages`)
    const directList = (afterDrop.json && (afterDrop.json.messages || afterDrop.json.items)) || []
    const dropped = directList.filter((each) => each.attachments && each.attachments.length > 0).pop()
    const droppedAttachment = dropped && dropped.attachments[0]
    const droppedBytes = droppedAttachment
      ? Buffer.from(
          await (
            await fetch(`${base}/v1/rooms/${ids.directRoomId}/attachments/${droppedAttachment.id}`, {
              headers: { Authorization: `Bearer ${token}` },
            })
          ).arrayBuffer(),
        ).toString('utf8')
      : ''
    judge(
      '判据 9：★ 拖放那条真的发出去了，且字节逐字节相同',
      Boolean(droppedAttachment) &&
        droppedAttachment.file_name === DROPPED_NAME &&
        droppedBytes === DROPPED_BODY,
      droppedAttachment ? `${droppedAttachment.file_name} · ${droppedBytes.length} B` : '源房里没有带附件的新消息',
    )

    fs.rmSync(localFile, { force: true })
    const failed = verdicts.filter((each) => !each.ok)
    console.log(`\n=== 转发真机驱动：${verdicts.length - failed.length}/${verdicts.length} 通过 ===`)
    return failed.length === 0 ? 0 : 1
  } catch (error) {
    if (error && error.assembly) {
      console.error(`[probe-forward] ${error.message}`)
      return 2
    }
    console.error(`[probe-forward] 探针自己抛了：${error && error.message ? error.message : error}`)
    // 出岔子时把**页面当时的样子**打出来（省得靠猜）：URL、文件格数量、正文前 900 字。
    if (page) {
      try {
        console.error(`[probe-forward] 当时 URL：${page.url()}`)
        console.error(`[probe-forward] 文件格数量：${await page.locator('input[type="file"]').count()}`)
        const text = await page.locator('body').innerText()
        console.error(`[probe-forward] 当时正文：${text.replace(/\s+/g, ' ').slice(0, 900)}`)
      } catch (dumpError) {
        console.error(`[probe-forward] （连现场都读不出来：${dumpError && dumpError.message}）`)
      }
    }
    return 2
  } finally {
    await browser?.close()
    if (world) {
      world.stop()
      if (keep) console.log(`[probe-forward] --keep ⇒ 临时家留着：${world.homeDir}`)
      else {
        world.homeDirCleanup()
        console.log('[probe-forward] 临时家已删')
      }
    }
  }
}

main().then(
  (code) => process.exit(code),
  (error) => {
    console.error(`[probe-forward] 探针自己抛了：${error && error.stack ? error.stack : error}`)
    process.exit(2)
  },
)
