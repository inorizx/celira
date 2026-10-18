/**
 * 一次性真机探针：设置→通用页上那两格邀请面**真的能用**吗。
 *
 * 凭据只从 `node home.js up --token` 的内部输出里取，**不打印、不写文件**
 * （与 `verify-ui.mjs` 同一条纪律）。判据：两个块在场 → 点「生成邀请码」
 * 真的出现一串 `clr_pg_` → 撤销掉刚才那张（不往真世界里留活码）。
 */
import { chromium } from 'playwright'
import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

// 仓库根从**本文件的位置**折出来，不写死本机路径（与 `probe-shell-mic.mjs` 同一条纪律）。
const HERE = path.dirname(fileURLToPath(import.meta.url))
const REPO = path.resolve(HERE, '..', '..')
const BASE = 'http://localhost:5173'

const out = execFileSync('node', ['home.js', 'up', '--token'], { cwd: REPO, encoding: 'utf-8' })
const token = (out.match(/clr_hs_[A-Za-z0-9_-]+/) || [])[0]
if (!token) {
  console.error('拿不到 token')
  process.exit(1)
}

const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
const fails = []
const check = (name, ok, detail = '') => {
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`)
  if (!ok) fails.push(name)
}

await page.goto(`${BASE}/`, { waitUntil: 'domcontentloaded' })
await page.waitForTimeout(2500)
await page.getByLabel('Bearer token').fill(token)
await page.getByRole('button', { name: '连接', exact: true }).click()
await page.waitForTimeout(4500)

await page.goto(`${BASE}/settings/general`, { waitUntil: 'domcontentloaded' })
await page.waitForTimeout(2500)
let text = await page.locator('body').innerText()

check('进门页有「用邀请码进来」那一格', text.includes('用邀请码进来'))
check('进门页有「邀请一个人」那一格', text.includes('邀请一个人'))
check('邀请码那一格说了"没有注册"这件事', text.includes('没有注册') || text.includes('要么你自己部署'))
check('邀请那一格说清了一次性/有效期/可撤销', text.includes('一次性') && text.includes('72 小时') && text.includes('撤销'))

// 真的点一次「生成邀请码」：必须当场出现一串 clr_pg_
await page.getByRole('button', { name: '生成邀请码' }).click()
await page.waitForTimeout(2500)
text = await page.locator('body').innerText()
const m = text.match(/clr_pg_[A-Za-z0-9_-]{43}/)
check('点一下就拿到一串邀请码（形状 clr_pg_ + 43）', Boolean(m))
check('复制按钮跟着出现', text.includes('复制邀请码'))

// 列表里应当有这张（未使用），并且带「撤销」
check('已发出的邀请出现在列表里且可撤销', text.includes('还没人用') && text.includes('撤销'))

// 收尾：把刚才那张撤销掉，不在真世界里留活码
const revoke = page.getByRole('button', { name: '撤销', exact: true }).first()
if (await revoke.count()) {
  await revoke.click()
  await page.waitForTimeout(2000)
  const after = await page.locator('body').innerText()
  check('撤销后状态变成「已撤销」', after.includes('已撤销') || !after.includes('还没人用'))
}

// ⚠ **别假设证据库目录在**（2026-10-18 修）：`outputs/` 是维护者那一侧的证据库，
// **公开仓里整目录不发表** ⇒ 这里以前直接往它里面写截图，在公开仓里会 ENOENT 崩掉。
// 自己建目录，两个仓都能跑（判据与 `scripts/check-docs.mjs` 那条 N/A 同源）。
const shot = path.join(REPO, 'outputs', 'invite-ui-settings-general.png')
mkdirSync(path.dirname(shot), { recursive: true })
await page.screenshot({ path: shot, fullPage: true })
console.log(`\n=== 进门页真机探针：${fails.length === 0 ? '全部通过' : fails.length + ' 条未过'} ===`)
await browser.close()
process.exit(fails.length === 0 ? 0 : 1)
