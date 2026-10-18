/**
 * 用户主题：**本机存储 + 导入/导出**。与 `wallpaper.ts` 同形。
 *
 * 外观是纯本机事实（`DESIGN_SYSTEM.md`「外观个性化边界」）：只落本机持久面，
 * **不进 Core、不跨任何协议、不新增第二个表征源**。
 *
 * ## 这一层的边界：只收数据，不读东西
 *
 * 本模块的东西只有三类：存储读写（**经 `api/storage.ts` 的窄口**）、格式解析、体检编排。
 * 官方令牌表与基底**由调用方传进来** —— 因为"从哪儿读"是打包器的事
 * （`themeCatalog.ts` 才碰 `?raw`），而"是什么形状、怎么算"是这一层的事。
 * 这条分界让整套判据能在 `node --test` 里直接跑。
 *
 * ## 先有值再渲染（2026-10-08 存储收尾第二刀）
 *
 * 存储读写从"模块求值期同步读 `localStorage`"改成了**窄口 + 首帧快照**，形状照
 * `api/store.ts` 的 `hydrateLocalState`：
 *
 * 1. 模块求值期只有缺省值（空列表 / 无生效主题）；
 * 2. `hydrateUserThemes()`（幂等）在 `createRoot` **之前**跑（`main.tsx`）：经窄口读进来，
 *    顺带**读穿式迁移**（老位置还留着这两格时搬进窄口）;
 * 3. Provider 的 `useState` 初始化器读 `userThemesSnapshot()` ⇒ 第一帧就带着他装过的主题
 *    （注入 `#celira-user-themes` 的是 `use-theme.tsx` 的 layout effect，首帧之前落下）；
 * 4. 写是**异步**的；`upsertUserTheme` / `removeUserTheme` 是"读改写"，所以它们自己先
 *    `await hydrateUserThemes()` —— **不假设调用方补过水**（否则一次过早的导入会把
 *    盘上别的主题整份覆盖掉）。
 *
 * ## 脏数据一律诚实丢弃
 *
 * 与 `wallpaper.ts` 同一条纪律：解析不了的记录**丢掉**，不半读半猜。
 */

import { readThrough, storage } from '../api/storage.ts'
import {
  parseThemeFile,
  serializeThemeFile,
  THEME_FILE_ERRORS,
  THEME_FILE_ERROR_TEXT,
  type ParsedThemeFile,
  type ThemeFileErrorCode,
  type TokenMap,
} from './themeFile.ts'
import { auditTheme, type AuditBase, type AuditFinding } from './themeAudit.ts'

export interface UserThemeRecord {
  /** 作用域 id（`user-…`）。重名即覆盖——「导出官方 → 改 → 再导入」靠它闭环。 */
  id: string
  name: string
  /** **规范化后**的主题文件原文；装载与导出都用它。 */
  css: string
}

const LIST_KEY = 'celira-user-themes'
const ACTIVE_KEY = 'celira-theme-user'

// ── 首帧快照（模块求值期只有缺省值）＋ 补水 ─────────────────────────────────

let themes: UserThemeRecord[] = []
let activeId: string | null = null

/** 这一刻已装的主题列表。**Provider 的初始化器读它**（同步 ⇒ 第一帧就带着它们）。 */
export function userThemesSnapshot(): UserThemeRecord[] {
  return themes
}

/** 这一刻生效的用户主题 id；`null` = 用官方预设。 */
export function activeUserThemeIdSnapshot(): string | null {
  return activeId
}

/** 读用户主题列表：非 JSON、非数组、形状不对、**解析不回来**的条目一律丢弃。 */
export function parseUserThemes(raw: string | null | undefined): UserThemeRecord[] {
  if (!raw) return []
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return []
  }
  if (!Array.isArray(parsed)) return []
  const out: UserThemeRecord[] = []
  for (const item of parsed) {
    if (!item || typeof item !== 'object') continue
    const record = item as Partial<UserThemeRecord>
    if (typeof record.css !== 'string') continue
    // 存进去过的东西也要能重新解析出来才算数（改坏了存储就是改坏了）。
    const reparsed = parseThemeFile(record.css)
    if (!reparsed.ok) continue
    out.push({
      id: reparsed.theme.id,
      name: typeof record.name === 'string' && record.name.trim() ? record.name : reparsed.theme.name,
      css: record.css,
    })
  }
  return out
}

/** 读"当前生效"那一格：空串 / 非字符串 / 读不到 ⇒ `null`（用官方预设）。 */
export function parseActiveUserThemeId(raw: string | null | undefined): string | null {
  return typeof raw === 'string' && raw ? raw : null
}

let hydrated: Promise<void> | null = null

/**
 * 补水（**幂等**）：把列表与"当前生效"经窄口读进快照。`main.tsx` 在 `createRoot` 之前 await 它。
 *
 * 失败降级成"一条都没有 / 用官方预设"并留痕 —— 读不到用户主题比拦住整个应用轻得多。
 */
export function hydrateUserThemes(): Promise<void> {
  hydrated ??= (async () => {
    try {
      const [listRaw, activeRaw] = await Promise.all([readThrough(LIST_KEY), readThrough(ACTIVE_KEY)])
      themes = parseUserThemes(listRaw)
      activeId = parseActiveUserThemeId(activeRaw)
    } catch (error) {
      themes = []
      activeId = null
      console.error('[theme] 用户主题没补上水（按"一条都没有"启动，下次启动再试）', error)
    }
  })()
  return hydrated
}

// ── 本机存储（写）────────────────────────────────────────────────────────────

/** 写失败只记一笔：这一档损失是"这次装的主题下次不在了"，不值得打断用户。 */
function warnWrite(ok: boolean) {
  if (!ok) console.warn('[theme] 用户主题没能落盘（这次仍然生效，下次启动会回到上一次记住的那份）')
}

/** 整份写列表（快照同时更新 —— 快照是**会话内的读面**，盘是**持久面**）。 */
export async function writeUserThemes(list: UserThemeRecord[]): Promise<boolean> {
  themes = list
  const ok = await storage.write(LIST_KEY, JSON.stringify(list))
  warnWrite(ok)
  return ok
}

/** 写"当前生效"那一格；`null` = 删掉这一格（回到官方预设）。 */
export async function writeActiveUserThemeId(id: string | null): Promise<boolean> {
  activeId = id
  if (id) {
    const ok = await storage.write(ACTIVE_KEY, id)
    warnWrite(ok)
    return ok
  }
  await storage.remove(ACTIVE_KEY)
  return true
}

/** 同一个 id 再导入 = **覆盖**（不是出错，也不是并存两条）。 */
export async function upsertUserTheme(record: UserThemeRecord): Promise<UserThemeRecord[]> {
  await hydrateUserThemes()
  const list = [...themes.filter((item) => item.id !== record.id), record]
  await writeUserThemes(list)
  return list
}

/** 删掉一条；若它正生效，一并从"当前生效"上摘下来（不留悬空 id）。 */
export async function removeUserTheme(id: string): Promise<UserThemeRecord[]> {
  await hydrateUserThemes()
  const list = themes.filter((item) => item.id !== id)
  await writeUserThemes(list)
  if (activeId === id) await writeActiveUserThemeId(null)
  return list
}

/**
 * **本机装的全部用户主题**的样式文本（给装载器用）——不是"当前生效那一份"。
 *
 * 为什么装全部而不是只装生效的：设置页要**在切换之前**就给出每一条的真样张
 * （`ThemeScopePreview` 用嵌套 `data-theme` 把它们各自关在一张卡片里）。
 * 只注入生效那一份，列表里那些**没生效的就没有样式可渲染**，样张只能是假的。
 *
 * ⚠ 安全性靠**作用域**而不是靠"只注入一份"：每份主题的规则都只带它自己的
 * `[data-theme='user-…']`（格式第一条硬要求），而 `<html>` 上只会写**生效那一个**
 * ⇒ 没生效的那些在文档里但**永不命中**。
 */
export function userThemeLibraryCss(list: UserThemeRecord[]): string {
  return list.map((item) => item.css.trim()).join('\n\n')
}

// ── 导入 / 导出 ─────────────────────────────────────────────────────────────

export type ImportOutcome =
  | { ok: true; record: UserThemeRecord; theme: ParsedThemeFile; findings: AuditFinding[] }
  | { ok: false; code: ThemeFileErrorCode; text: string }

/**
 * 导入一份主题文件：**格式不合就拒收**（那种文件装载不了），**体检结论不拦人**
 * （好不好看没法机械判定，能算的那几项照实报）。
 *
 * `base` 是**用户没写的令牌继承来的那一份**（当前生效的官方预设的令牌表）——
 * 少了它，一个只改了 `--primary` 的主题根本算不出对比度。
 */
export function importUserTheme(raw: string, base: AuditBase): ImportOutcome {
  const parsed = parseThemeFile(raw)
  if (!parsed.ok) {
    return { ok: false, code: parsed.code, text: THEME_FILE_ERROR_TEXT[parsed.code] }
  }
  const css = serializeThemeFile(parsed.theme)
  const record: UserThemeRecord = { id: parsed.theme.id, name: parsed.theme.name, css }
  return { ok: true, record, theme: parsed.theme, findings: auditTheme(parsed.theme, base) }
}

/** 导出一份用户主题（规范化的原文即文件）。 */
export function exportUserThemeCss(record: UserThemeRecord): string {
  return record.css
}

/** 官方预设 id → 导出用的用户主题 id（合法、稳定、可重复导入覆盖）。 */
export function officialExportId(presetId: string): string {
  const slug = (presetId || 'celira')
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return `user-${slug || 'theme'}`
}

/** 错误码封闭表的再导出（界面按码给人话，不认识就交出来原文）。 */
export { THEME_FILE_ERRORS, THEME_FILE_ERROR_TEXT }
export type { ThemeFileErrorCode, TokenMap }
