// 陈列室的**渲染健全性检查**（legacy 通道）——不比对像素。
//
// ## 2026-09-25 裁决：拆掉像素对齐
//
// 主人原话：**「不要走这种截图对齐了，这种没啥意义」**。理由（我认同，而且有实证）：
//
// - **像素 diff 只能证明"变了"，永远证明不了"变好还是变坏"**。于是它拦下的东西里，
//   真退化与预期内改动混在一起，每次都要人肉裁决 —— 成本落在人身上，收益却只是
//   "有人看过这一眼"；
// - 而"有人看过这一眼"**本来就不可靠**：同一轮里我自己拿一张 1440×900、压到 63KB 的
//   PNG 去分辨 40px 头像上的青（`#6ec6ff`）与紫（`#c5b8f5`）——**看错了，还把结论
//   当事实说了出来**。连人都会读错的图，做成机械基线，拦住的到底是退化还是误读，分不清；
// - 真正防"AI 乱写样式"的**不是**这个：是**固定组件 + 语义令牌**（颜色由
//   `scripts/check-tokens.mjs` 机械守住）。视觉回归从来不是那条防线。
//
// ## 现在这条通道守什么（唯一职责）
//
// **只守灾难**：陈列室必须真的渲染出来，而不是静默变成门禁页或空白。
// 三条 fail-loud（都**不依赖像素**，所以不会因改样式而误报）：
//
// 1. `dist` 必须比 `src` 新 —— 否则看的是旧界面，绿色是骗人的；
// 2. 不许出现 fixture 守卫那句诚实门文案（那是"进错通道"的机械证据）；
// 3. `SECTIONS` 里每个展示节的标题必须真的在页面上找得到。
//
// 用法：node scripts/screenshot-catalog.mjs
// 前置：先 `npx vite build`（本脚本服务 dist）。
//
// ⚠ **不要再把像素比对加回来**。要"看这一眼"就直接开 dev server
// 用眼睛看，或写一次性脚本截图（看完即删）——那比维护一份二进制基线便宜得多。
import { spawn } from 'node:child_process'
import { existsSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const DIST_INDEX = join(ROOT, 'dist', 'index.html')
const SRC_DIR = join(ROOT, 'src')
const PORT = Number(process.env.CELIRA_CATALOG_PORT ?? 5199)
const BASE = `http://localhost:${PORT}`
const DESKTOP = { width: 1280, height: 800 }

/** 必须真的渲染出来的展示节：`title` 是 `Section` 的标题（GalleryPage 里 `<Section title=…>`）。 */
const SECTIONS = [
  { key: 'connection', title: 'Connection & Pill' },
  // 视频那一节摆的是**真字节**（`public/mock/video-1.webm`，由浏览器自己录出来的）：
  // 它证明的是"这一节真的渲染出来了、有面积"，**不是**像素对齐（见文件头）。
  { key: 'video', title: 'Video Attachment' },
]

/** 视频那一节里坏源的降级提示（与 `components/celira/VideoAttachment.tsx` 对齐）。 */
const VIDEO_FALLBACK_TEXT = '这个视频在这里放不出来，下载后可以看。'
const THEMES = ['dark', 'light']

/** dist 必须比 src 新——否则看的是旧界面，绿色会骗人。 */
function assertDistFresh() {
  if (!existsSync(DIST_INDEX)) {
    throw new Error('缺少 dist/index.html：先跑 `npx vite build`')
  }
  const newestSrc = (dir) =>
    readdirSync(dir, { withFileTypes: true }).reduce((acc, entry) => {
      const full = join(dir, entry.name)
      if (entry.isDirectory()) return Math.max(acc, newestSrc(full))
      return entry.isFile() ? Math.max(acc, statSync(full).mtimeMs) : acc
    }, 0)
  if (statSync(DIST_INDEX).mtimeMs < newestSrc(SRC_DIR)) {
    throw new Error('dist 比 src 旧：先跑 `npx vite build` 再检查（否则看的是旧界面）')
  }
}

function startPreview() {
  return spawn(
    process.execPath,
    [
      join(ROOT, 'node_modules', 'vite', 'bin', 'vite.js'),
      'preview',
      '--port',
      String(PORT),
      '--strictPort',
    ],
    { cwd: ROOT, stdio: 'ignore' },
  )
}

async function waitForServer(timeoutMs = 30000) {
  const start = Date.now()
  for (;;) {
    try {
      const res = await fetch(BASE)
      if (res.ok) return
    } catch {
      // 未起，继续等
    }
    if (Date.now() - start > timeoutMs) throw new Error(`预览服务启动超时（${BASE}）`)
    await new Promise((r) => setTimeout(r, 300))
  }
}

async function main() {
  assertDistFresh()

  const preview = startPreview()
  const results = []
  let browser
  try {
    await waitForServer()
    browser = await chromium.launch({
      // 与 `verify-ui.mjs` / `probe.mjs` 同一入口：优先用本机既有的引擎（不下载）。
      executablePath: process.env.CELIRA_BROWSER_EXECUTABLE?.trim() || undefined,
    })
    for (const theme of THEMES) {
      const page = await browser.newPage({ viewport: DESKTOP })
      page.setDefaultTimeout(15000)
      // 确定性硬化：钉 colorScheme + 钉主题偏好。
      await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' })
      await page.addInitScript((t) => {
        localStorage.setItem('celira-theme', t)
        // 这条偏好**就是「演示通道」**：`mode='core'` + 空 token = 未连接 ⇒
        // `isRealWorldSession` 为假 ⇒ fixtures 驱动页（含陈列室）放行。
        localStorage.setItem('celira.connection.prefs', JSON.stringify({ mode: 'core', token: '' }))
      }, theme)
      await page.goto(`${BASE}/gallery`, { waitUntil: 'load' })
      await page.waitForTimeout(2000)

      // ① 防「静默截到门禁页」——必须 fail loudly。
      // ⚠ 文案与 `components/celira/FixtureGuard.tsx` 的 `FixturePageUnavailable`
      // 对齐：**改了那边的诚实门文案要同步这里**，否则这道 fail-loud 会静默失效。
      if ((await page.getByText('本页面尚未提供真实数据').count()) > 0) {
        throw new Error('陈列室未渲染（拿到 fixture 守卫的诚实门）：确认进的是演示通道（core + 空 token）')
      }

      // ② 每个展示节必须真的在页面上（标题改了 / 页没渲染完 ⇒ 红）。
      for (const section of SECTIONS) {
        const target = page.locator('section').filter({ hasText: section.title }).first()
        const count = await target.count()
        if (count === 0) {
          throw new Error(`陈列室缺少展示节「${section.title}」：标题改了或页未渲染完`)
        }
        // 顺带确认它**有面积**：一个 0 高度的节等于没渲染（只查 count 会放过它）。
        const box = await target.boundingBox()
        if (!box || box.width < 100 || box.height < 20) {
          throw new Error(
            `展示节「${section.title}」渲染出来了但没有面积（${box ? `${box.width}x${box.height}` : 'null'}）`,
          )
        }
        // 视频那一节还要**真的降级**（渲染出来 ≠ 做对了）：那一节里第二格是坏源，
        // 它必须说出口、不许留一块点不动的黑框。这条判据是拿真机换来的 ——
        // 首屏那次 `error` 早于 passive effect 落下来，失败被清零，黑框就留在那儿了
        // （2026-10-08，见 `components/celira/VideoAttachment.tsx` 里那段注释）。
        // ⚠ 文案与那个组件对齐：改了那边的降级提示要同步这里，否则这道 fail-loud 会静默失效。
        if (section.key === 'video') {
          if ((await target.getByText(VIDEO_FALLBACK_TEXT).count()) === 0) {
            throw new Error(
              `展示节「${section.title}」里的坏源没有降级：界面上留了一块点不动的黑框（缺「${VIDEO_FALLBACK_TEXT}」）`,
            )
          }
        }
        results.push(`✓ ${section.key}-${theme} 渲染正常（${Math.round(box.width)}x${Math.round(box.height)}）`)
      }
      await page.close()
    }
  } finally {
    await browser?.close()
    preview.kill()
  }

  for (const line of results) console.log(line)
  console.log('\n陈列室渲染检查通过（只查"渲染出来了"，不比对像素）。')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
