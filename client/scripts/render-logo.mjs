/**
 * **把 Celira 的标记渲染成一张大 PNG**（给应用图标当源图用）。
 *
 * 为什么要有这一步：应用图标那几档（`.ico` / `Square*Logo.png` / `.icns`）都得从**像素**出，
 * 而仓库里的标记是 SVG（`client/public/logo/logo-mark.svg`）⇒ 需要一次光栅化。
 * 用 Playwright 渲（它已经是这个仓库的 devDependency，探针一直在用）——
 * **不引新的图形依赖**，也不需要系统里装 ImageMagick/Inkscape。
 *
 * 产物：`app/src-tauri/icons/source-mark.png`（1024×1024，透明底）。
 * 下一步：`python app/src-tauri/icons/make-icons.py`（那一支负责写全套尺寸）。
 *
 * 运行：`cd client && node scripts/render-logo.mjs`
 */
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const SIZE = 1024
const SVG = fileURLToPath(new URL('../public/logo/logo-mark.svg', import.meta.url))
const OUT = fileURLToPath(new URL('../../app/src-tauri/icons/source-mark.png', import.meta.url))

const svg = readFileSync(SVG, 'utf8')
// 标记自己只有 `viewBox`、没有宽高 ⇒ 这里给一个正好 SIZE×SIZE 的画布（不缩放、不裁切）。
const html = `<!doctype html><html><head><meta charset="utf-8"><style>
  html,body{margin:0;padding:0;background:transparent}
  svg{display:block;width:${SIZE}px;height:${SIZE}px}
</style></head><body>${svg}</body></html>`

const browser = await chromium.launch()
try {
  const page = await browser.newPage({
    viewport: { width: SIZE, height: SIZE },
    deviceScaleFactor: 1,
  })
  await page.setContent(html, { waitUntil: 'load' })
  // `omitBackground` ⇒ 透明底（图标不能带白底，深浅任务栏/磁贴都要能用）。
  const png = await page.screenshot({ omitBackground: true })
  writeFileSync(OUT, png)
  console.log(`[render-logo] 写出 ${OUT}（${SIZE}×${SIZE}，透明底，${png.length} 字节）`)
} finally {
  await browser.close()
}
