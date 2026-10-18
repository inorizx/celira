/**
 * 官方主题**包**的读取层：**唯一碰打包器特性的模块**。
 *
 * ## 包是什么
 *
 * **一个包 = 一个文件，文件名就是作用域名**（`glass.css` ⇄ `[data-theme='glass']`）。
 * 包里是**所有带该作用域的规则**：日/夜两套令牌表、质感层/形态层、以及它自己的配色变体。
 * 判据一句话（可机械判）：**一条规则属于主题 X ⟺ 它的每一个选择器分支都带 `[data-theme='X']`**。
 * 于是"一套主题的所有事实在一处"，而**用户主题与官方主题共用同一个格式**——
 * 官方这 9 个包就是这个格式的**官方实现版本**。
 *
 * 默认主题（`celira`）**没有包**：它是**基底**（`:root` / `.dark`），连它的配色变体也锚在
 * `:root` 上（那些规则带一个无作用域的分支，按上面的判据不属于任何包）。它留在 `tokens.css`。
 *
 * ## 为什么是 `?raw` 急加载，而不是运行时取文件
 *
 * 换来三件事：**零首帧风险**（样式在挂载前就位）、**零新网络面**、**零壳命令面**。
 * 代价如实认下：**"加一套主题不用重编"这一格没拿到**（仍要走一次构建）。
 * 升级到"不重编"只需换包的**来源**（`?raw` → 同源取文件）＋ 一个启动闸门
 * （仓库已登记的「两段式启动」），装载器与格式都不用动。
 *
 * ## 为什么单独一个模块
 *
 * `?raw` 与 `import.meta.glob` 都是**打包器写法**，Node 不认（`node --test` 解析到
 * `…/tokens.css?raw` 直接报 `Unknown file extension ".css"`）。所以这条依赖只许出现在
 * 本模块，纯逻辑（`themeFile` / `themeAudit` / `userTheme`）保持零打包器特性。
 */

import tokensRaw from '../styles/tokens.css?raw'
import { buildThemeFile, parseThemeCss, type ParsedThemeFile, type ScopedTokens } from './themeFile.ts'
import type { AuditBase } from './themeAudit.ts'

/** 默认主题（`celira`）没有包：它是基底（`:root` / `.dark`）。 */
export const DEFAULT_PRESET_SCOPE = ''

/**
 * 官方主题包目录。`eager: true` ⇒ **构建期就收进来、同步可用**（启动时一次注入，不闪）。
 */
const PACKAGE_SOURCES = import.meta.glob('../styles/themes/*.css', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>

/** `{ 作用域名 → 包文本 }`：**作用域名就是文件名**（这条不变式由测试钉住）。 */
function packages(): [string, string][] {
  return Object.entries(PACKAGE_SOURCES)
    .map(([path, text]) => [path.split('/').pop()!.replace(/\.css$/, ''), text] as [string, string])
    .sort((a, b) => a[0].localeCompare(b[0]))
}

/**
 * 全部官方包的样式文本（启动时**一次**注入 `#celira-theme-packages`）。
 *
 * 注入全部而不是只注入当前那一个：**陈列室与设置页的预览卡要同时渲染十套**
 * （嵌套 `data-theme` 作用域），只注入当前那套会让它们空掉。
 * 代价与今天一样（这些样式本来就在产物里），换来的是"预览任意一套"不必再取东西。
 */
export function officialPackageCss(): string {
  return packages()
    .map(([, text]) => text.trim())
    .join('\n\n')
}

let catalogCache: Record<string, ScopedTokens> | null = null

/**
 * 所有作用域的令牌表：基底（`''`）来自 `tokens.css`，其余来自包目录。
 *
 * 同步（包是急加载的）—— 于是体检的基底、导出、启动注入都不必 await，
 * 也就没有"样式晚到一帧"这种事。
 */
export function loadThemeCatalog(): Record<string, ScopedTokens> {
  if (!catalogCache) {
    const catalog: Record<string, ScopedTokens> = { ...parseThemeCss(tokensRaw) }
    for (const [id, text] of packages()) {
      const scoped = parseThemeCss(text)[id]
      if (scoped) catalog[id] = scoped
    }
    catalogCache = catalog
  }
  return catalogCache
}

/** 某个官方预设的令牌表；认不出 id 时回落默认主题的基底（不猜别的主题）。 */
export function officialTokens(presetId: string): ScopedTokens {
  const catalog = loadThemeCatalog()
  return catalog[presetId] ?? catalog[DEFAULT_PRESET_SCOPE] ?? { light: {}, dark: {} }
}

/** 体检用的基底：当前生效的官方预设那一份。 */
export function auditBaseFor(presetId: string): AuditBase {
  const tokens = officialTokens(presetId)
  return { light: tokens.light, dark: tokens.dark }
}

/**
 * 把一套官方预设组装成一份**用户主题文件**（逐档抄进 `user-<preset>` 作用域）。
 * 读不到令牌表时回 `null` —— **不编一份空的**（空主题装上也看不出问题，最难查）。
 *
 * ⚠ 如实认下的边界：导出的是**配色那一份**（令牌表）。包里的**质感层与形态层**
 * （切角、纹理、辉光、字族、关键帧）**不跟着走** —— 那需要把任意规则也搬进导出物，
 * 而"导出一份用户主题"的语义今天只覆盖令牌。要连形态一起给别人，就从自己写的用户主题导出。
 */
export function officialThemeFile(presetId: string, presetName: string, userId: string): ParsedThemeFile | null {
  const tokens = officialTokens(presetId)
  if (Object.keys(tokens.light).length === 0) return null
  return buildThemeFile(userId, `${presetName}（副本）`, tokens)
}
