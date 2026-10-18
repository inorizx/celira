import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import { installShellTransport } from './api/shell.ts'
import { installShellStorage } from './api/shellStorage.ts'
import { hydrateLocalState } from './api/store.ts'
import { hydrateThemePrefs } from './lib/themePrefs.ts'
import { hydrateUserThemes } from './lib/userTheme.ts'
import { hydrateWallpaper } from './lib/wallpaper.ts'
import { hydrateDrafts } from './lib/messageDrafts.ts'
import { officialPackageCss } from './lib/themeCatalog.ts'
import App from './App.tsx'

/**
 * **官方主题包在挂载之前就位**（刀 3）。
 *
 * 包是**数据**（`?raw` 随包同步可得）⇒ 不闪、不发请求、不碰壳。放在 `createRoot` 之前
 * 是刻意的：挂载之后再注入，会先出一帧基底配色再跳成用户选的那套。
 *
 * 三个 `<style>` 各有唯一写者，这里是其中之一：
 *   `#celira-theme-packages`（本处，全量、一次性、静态）
 *   `#celira-user-themes`（`hooks/use-theme.tsx`，**本机装的全部**用户主题，持久；
 *     生效哪一套由 `<html data-theme>` 说，不由这个元素说）
 *   `#celira-theme-preview`（设置页「我的主题」，小样，瞬时）
 */
function installOfficialThemePackages() {
  const style = document.createElement('style')
  style.id = 'celira-theme-packages'
  style.textContent = officialPackageCss()
  document.head.appendChild(style)
}
installOfficialThemePackages()

// **在任何网络请求之前**接好出网（`api/shell.ts`）：
//   - 浏览器里这是个空操作（判据是"有没有壳的内部注入"）；
//   - 壳里把出网换成 Rust 侧 ⇒ 连任意地址的世界不再受浏览器的同源策略限制。
// 不 await：浏览器里它同步返回；壳里装好之前不会有请求（React 还没挂载）。
void installShellTransport()

// **存储也换到壳侧**（`api/shellStorage.ts`，2026-10-05）：壳里状态层的持久化落
// `<app_config_dir>/local-store.json`，不再依赖 webview 的 `localStorage`；浏览器里同样是空操作。
void installShellStorage()

/**
 * **两段式启动**（2026-10-08 立，同日第二刀铺满全部五族）：① 装配（把窄口换到壳侧）
 * → ② **补水**（把本机持久值经那条窄口读进来）→ ③ 才挂载。
 *
 * 为什么必须分两段：窄口**从第一天起就是异步的**（`localStorage` 是同步的，文件不是），
 * 而这些值原先是在**模块求值期**同步读的 —— 于是壳里那份状态一直住在 **webview 的
 * `localStorage`**（不在家里、不跟着家走、清一次站点数据就没了）。现在模块只给缺省值，
 * 真值在挂载**之前**由下面这几步读进来：**用户看到的第一帧就已经是他自己的东西**
 * （连接、置顶、主题、壁纸、上次没发完的草稿都不闪、不先出默认再跳）。
 *
 * ⚠ 顺序不能反：补水要**经过**装配好的那条窄口。各族之间的顺序只有一处讲究 ——
 *   `hydrateLocalState` 里那两族（连接偏好 → 房间用户侧偏好）有先后（后者按世界分键）；
 *   其余三族与世界无关，谁先谁后都行。
 * ⚠ **每一族都必须在这里点名**（漏了 = 那一族的第一帧退回缺省值，而且没有任何东西会提醒你）。
 *   机械守卫在 `tests/local-state.test.ts`：搬家读的调用点必须与这份清单对得上。
 * ⚠ 失败也要挂载（缺省值 ⇒ 落登录页）：**白屏比"没读到偏好"糟得多** —— 每一步自己吞错、
 *   自己留痕，外面仍各兜一层 try（意外抛出同样不许拦住挂载）。
 */
void (async () => {
  try {
    await installShellStorage()
  } catch (error) {
    console.error('[startup] 存储没换到壳侧（这次仍走浏览器那份）', error)
  }
  try {
    await hydrateLocalState()
  } catch (error) {
    console.error('[startup] 本地状态没补上水（按缺省值启动）', error)
  }
  // 外观与草稿：与世界无关，各自幂等、各自降级（见各自模块的文件头）。
  // ⚠ 仍然兜一层 try：四步里任何一步**意外**抛出都不许拦住挂载 —— 白屏是最坏的结果。
  try {
    await Promise.all([hydrateThemePrefs(), hydrateUserThemes(), hydrateWallpaper(), hydrateDrafts()])
  } catch (error) {
    console.error('[startup] 外观 / 草稿没补上水（按缺省值启动）', error)
  }
  // ⚠ 主题的 `data-theme` / 字号 / 配色变体由 `ThemeProvider` 的 layout effect 写 ——
  //   补水在挂载之前完成，所以那一笔落在**首帧绘制之前**（顺序不能反，反了就是闪一下默认配色）。
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
})()
