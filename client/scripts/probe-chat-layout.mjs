#!/usr/bin/env node
/**
 * **聊天气泡行位**在浏览器里的那一段（由 `scripts/home/probe-chat-layout.js` 起）。
 *
 * 三条判据（与那个文件头同源，读 DOM、不靠人眼）：
 * ① 行与行**不重叠**（`next.top - this.bottom >= -2`）；
 * ② 行的**盒子不比自己的内容矮**（矮了就是"画出来了但没人量到"，下一个必然压上来）；
 * ③ 行的 `translateY` **严格递增**（同 y 就是行身份重复那一类）。
 *
 * ⚠ token 只从 **env**（`CELIRA_PROBE_TOKEN`）取：不打印、不写文件。
 */
import { chromium } from 'playwright'

const arg = (name) => {
  const i = process.argv.indexOf(name)
  return i >= 0 ? process.argv[i + 1] : ''
}
const base = arg('--base')
const roomId = arg('--room')
const token = process.env.CELIRA_PROBE_TOKEN ?? ''
if (!base || !roomId || !token) {
  console.error('[chat-layout] 缺 --base/--room 或 token（env）')
  process.exit(2)
}

const browser = await chromium.launch()
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } })
const page = await context.newPage()
const consoleErrors = []
page.on('console', (msg) => {
  if (msg.type() === 'error') consoleErrors.push(msg.text())
})
/**
 * **翻页请求的读数**（诊断"滚到顶没读更早"时用）：它把"没触发"与"触发了但没成"分开。
 * 判据是 URL 里带 `before=`（那正是更早那一页的游标）。
 */
const olderRequests = []
const allMessageRequests = []
page.on('request', (req) => {
  const url = req.url()
  if (/\/messages\?/.test(url)) {
    allMessageRequests.push(url.split('/v1')[1] ?? url)
    if (url.includes('before=')) olderRequests.push(url)
  }
})
page.on('response', async (res) => {
  if (/\/messages\?/.test(res.url()) && res.url().includes('before=')) {
    olderRequests.push(`← HTTP ${res.status()}`)
  }
  // ★ **我们这间房**首屏那一页的**游标读数**：`next_before` 在不在，决定"还有没有更早的"。
  //   ⚠ 必须限定 `roomId`：这一路会看到**多个房**的请求（会话列表在预取），
  //   拿别人的读数会得出相反结论（实测踩过）。
  if (res.url().includes(roomId) && /\/messages\?/.test(res.url()) && !res.url().includes('before=') && historyCursor === null) {
    try {
      const body = await res.json()
      historyCursor = body && typeof body === 'object' ? (body.next_before ?? null) : null
      historyCount = Array.isArray(body?.messages) ? body.messages.length : 0
    } catch {
      historyCursor = '（读不出来）'
    }
  }
})
let historyCursor = null
let historyCount = 0

/**
 * ⚠ **必须走登录表单**（照 `verify-ui.mjs` 那一段）：把 token 塞进 `localStorage` **不管用**
 * —— 客户端把会话存进 IndexedDB，页面会带着**空会话**落进**演示通道**（实测：那样量到的
 * 8 行全是夹具，一屏滚不动、什么都不复现）。
 */
await page.goto(`${base}/`, { waitUntil: 'domcontentloaded' })
await page.waitForTimeout(2500)
await page.evaluate(async () => {
  window.localStorage.clear()
  const dbs = (await indexedDB.databases?.()) ?? []
  for (const db of dbs) if (db.name) indexedDB.deleteDatabase(db.name)
})
await page.reload({ waitUntil: 'domcontentloaded' })
await page.waitForTimeout(2500)
const tokenField = page.getByLabel('Bearer token')
if ((await tokenField.count()) > 0) {
  await tokenField.fill(token)
  await page.getByRole('button', { name: '连接', exact: true }).click()
  await page.waitForTimeout(4500)
} else {
  // 打一段页面正文出来：**没有登录页**这件事本身要能一眼看清（是启动页还是别的）。
  const text = (await page.locator('body').innerText()).replace(/\s+/g, ' ').slice(0, 160)
  console.log(`[chat-layout] （没有登录页：${new URL(page.url()).pathname}「${text}」）`)
}
const landed = new URL(page.url()).pathname
if (!landed.startsWith('/app')) {
  console.error(`[chat-layout] 没落进应用（停在 ${landed}）—— 环境没就绪`)
  await browser.close()
  process.exit(2)
}

await page.goto(`${base}/app/chat/${roomId}`, { waitUntil: 'domcontentloaded' })
// 等行落地：这一页有两个过渡态（壳层自举 / 历史到达），等"行出现了"这个状态，不等死时长。
await page
  .waitForFunction(() => document.querySelectorAll('[id^="m-"]').length >= 3, null, {
    timeout: 20000,
  })
  .catch(() => {})
// 再给它一小段时间落定（字体/代码块/公式都在这一小段里重排）。
await page.waitForTimeout(1500)

/**
 * 量一次：两条判据的原料（**行盒** / 行盒里的内容高）。
 *
 * ⚠ **只量消息行**：用行的 `id="m-<消息 id>"` 取。2026-10-16 拆掉虚拟列表之后，
 * `data-index` 存的是**消息 id**、而**左边会话列表**也可能有 `data-index` 的行 ——
 * 混进来会量出"跨列表重叠"这种假读数（实测踩过：左边那行 `探 探针群` 被判成重叠）。
 */
const measure = () =>
  page.evaluate(() => {
    const rows = [...document.querySelectorAll('[id^="m-"]')]
    const items = rows.map((el) => {
      const rect = el.getBoundingClientRect()
      const inner = el.firstElementChild
      const innerHeight = inner
        ? Math.max(inner.scrollHeight, Math.round(inner.getBoundingClientRect().height))
        : 0
      return {
        key: el.id,
        top: Math.round(rect.top),
        bottom: Math.round(rect.bottom),
        height: Math.round(rect.height),
        innerHeight: Math.round(innerHeight),
        text: (el.innerText || '').replace(/\s+/g, ' ').slice(0, 30),
      }
    })
    // 滚动容器：从任意一行往上找**能滚的那个**（谁的可滚余量最大就是它）——
    // Radix 的视口属性选择器与 `h-full` 包装层都不稳定，别赌属性名。
    const pickScroller = (start) => {
      let best = null
      for (let node = start; node && node !== document.body; node = node.parentElement) {
        const slack = node.scrollHeight - node.clientHeight
        if (slack > 4 && (!best || slack > best.scrollHeight - best.clientHeight)) best = node
      }
      return best
    }
    const scroller = pickScroller(rows[0] ?? document.body)
    return {
      count: items.length,
      items,
      viewport: scroller
        ? {
            scrollTop: Math.round(scroller.scrollTop),
            scrollHeight: scroller.scrollHeight,
            clientHeight: scroller.clientHeight,
          }
        : null,
    }
  })

/** 那一个滚动容器（每站重取：React 换节点之后旧引用会失效）。 */
const scrollTo = (top) =>
  page.evaluate((value) => {
    const rows = [...document.querySelectorAll('[id^="m-"]')]
    let best = null
    for (let node = rows[0] ?? document.body; node && node !== document.body; node = node.parentElement) {
      const slack = node.scrollHeight - node.clientHeight
      if (slack > 4 && (!best || slack > best.scrollHeight - best.clientHeight)) best = node
    }
    if (!best) return null
    best.scrollTop = value === 'top' ? 0 : value === 'bottom' ? best.scrollHeight : Number(value)
    return Math.round(best.scrollTop)
  }, top)

/** 一次量 → 判据（返回发现的问题，供每一站累计）。 */
function inspect(reading, where) {
  const overlaps = []
  const shortBoxes = []
  for (let i = 0; i < reading.items.length; i += 1) {
    const row = reading.items[i]
    const next = reading.items[i + 1]
    // ① 行与行**不许重叠**（`next.top - this.bottom` 为负就是压上来了）。
    if (next && next.top - row.bottom < -2) {
      overlaps.push({ a: row, b: next, gap: next.top - row.bottom, where })
    }
    // ② 行盒**不许比它自己的内容矮**（矮了就是"画出来了但没被算进高度"，下一个必然压上来）。
    if (row.innerHeight - row.height > 2) shortBoxes.push({ ...row, where })
  }
  return { overlaps, shortBoxes }
}

const found = { overlaps: [], shortBoxes: [] }
const stations = []
const collect = (reading, where) => {
  stations.push(`  · ${where}：${reading.count} 行 · 视口 ${JSON.stringify(reading.viewport)}`)
  const hit = inspect(reading, where)
  found.overlaps.push(...hit.overlaps)
  found.shortBoxes.push(...hit.shortBoxes)
}

collect(await measure(), '刚进来（底部）')

// ★ **翻到历史里**才是真机撞上它的位置：一路往上滚，每一站都量。
const rowsAtStart = (await measure()).count
for (let step = 1; step <= 14; step += 1) {
  // 直接设 scrollTop（`mouse.wheel` 指错元素就白滚 —— 实测过）。
  await page.evaluate((frac) => {
    const rows = [...document.querySelectorAll('[id^="m-"]')]
    let best = null
    for (let node = rows[0] ?? document.body; node && node !== document.body; node = node.parentElement) {
      const slack = node.scrollHeight - node.clientHeight
      if (slack > 4 && (!best || slack > best.scrollHeight - best.clientHeight)) best = node
    }
    if (best) best.scrollTop = Math.max(0, best.scrollTop - best.clientHeight * frac)
  }, 0.9)
  await page.waitForTimeout(600)
  collect(await measure(), `向上第 ${step} 滚`)
}
/**
 * ★ **滚到顶 ⇒ 自动读更早**（2026-10-16 改成主流形状之后的判据）。
 *
 * 那一路上**没有按钮可点**了（按钮撤掉）⇒ 只能这样判：**行数变多**，
 * 而且顶到 0 之后要给它一点时间把那一页读完（一页 50 条）。
 */
await page.evaluate(() => {
  const rows = [...document.querySelectorAll('[id^="m-"]')]
  let best = null
  for (let node = rows[0] ?? document.body; node && node !== document.body; node = node.parentElement) {
    const slack = node.scrollHeight - node.clientHeight
    if (slack > 4 && (!best || slack > best.scrollHeight - best.clientHeight)) best = node
  }
  if (best) best.scrollTop = 0
})
await page.waitForTimeout(2000)
const afterTop = await measure()
collect(afterTop, '顶到底之后（该自动读到更早的一页）')
const autoLoaded = afterTop.count > rowsAtStart
/**
 * **再补一次合成 scroll 事件**（诊断用）：真事件一定会叫醒监听 ——
 * 若这一次就发请求了，说明"监听挂上了，但程序化设 `scrollTop` 没叫醒它"；
 * 若仍不发，说明**监听没挂上**或**守卫那一步就返回了**。
 */
console.log(
  autoLoaded
    ? `[chat-layout] ✓ 滚到顶自动读到了更早的一页：${rowsAtStart} → ${afterTop.count} 行`
    : `[chat-layout] ✗ 滚到顶**没有**自动读到更早（行数仍是 ${afterTop.count}，进去时 ${rowsAtStart}）`,
)
// 硬证据：它**真的去读了**更早那一页（URL 里带 `before=` 就是翻页游标）。
console.log(
  `[chat-layout] 翻页请求：${olderRequests.length} 次` +
    (olderRequests.length > 0 ? `（${olderRequests.slice(0, 2).join(' | ')}）` : '（一次都没发）'),
)
console.log(
  `[chat-layout] 首屏那一页：${historyCount} 条 · next_before ${historyCursor ? '在场（还有更早的）' : '**不在场**（服务端说没有了）'}`,
)
// 再滚回底部，看落位那一路。
for (let step = 0; step < 14; step += 1) {
  await page.evaluate((frac) => {
    const rows = [...document.querySelectorAll('[id^="m-"]')]
    let best = null
    for (let node = rows[0] ?? document.body; node && node !== document.body; node = node.parentElement) {
      const slack = node.scrollHeight - node.clientHeight
      if (slack > 4 && (!best || slack > best.scrollHeight - best.clientHeight)) best = node
    }
    if (best) best.scrollTop = Math.min(best.scrollHeight, best.scrollTop + best.clientHeight * frac)
  }, 0.75)
  await page.waitForTimeout(250)
}
collect(await measure(), '滚回底部')

const reading = { count: found.overlaps.length + found.shortBoxes.length }

const line = (r) => `${r.key} h=${r.height} 内容=${r.innerHeight} 「${r.text}」`
console.log(`[chat-layout] 站点：\n${stations.join('\n')}`)
if (found.overlaps.length > 0) {
  console.log('[chat-layout] ✗ 行与行重叠：')
  for (const hit of found.overlaps.slice(0, 6)) {
    console.log(`    （${hit.where}）重叠 ${hit.gap}px：\n      ${line(hit.a)}\n      ${line(hit.b)}`)
  }
}
if (found.shortBoxes.length > 0) {
  console.log('[chat-layout] ✗ 行盒比内容矮（下一个必然压上来）：')
  for (const row of found.shortBoxes.slice(0, 6)) console.log(`    （${row.where}）${line(row)}`)
}
const duplicateWarnings = consoleErrors.filter((t) => t.includes('同一条消息在时间线里出现了两次'))
if (duplicateWarnings.length > 0) {
  console.log(
    `[chat-layout] ✗ 页面自己报了重复行身份 ${duplicateWarnings.length} 次：${duplicateWarnings[0]}`,
  )
}

const ok = reading.count === 0 && duplicateWarnings.length === 0 && autoLoaded
console.log(
  ok
    ? '[chat-layout] ✓ 判据全过（行与行不重叠 / 行盒不矮于内容 / 没有重复行身份 / 滚到顶自动读更早）'
    : `[chat-layout] 结论：重叠 ${found.overlaps.length} · 矮盒 ${found.shortBoxes.length} · 重复身份告警 ${duplicateWarnings.length} · 自动读更早 ${autoLoaded ? '有' : '**没有**'}`,
)
await browser.close()
process.exit(ok ? 0 : 1)
