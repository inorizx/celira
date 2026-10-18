import { useEffect, useLayoutEffect, useState, type ReactNode } from 'react'
import { isThemeVariantId, type FontSizeId, type ThemeMode, type ThemePresetId, type ThemeVariantId } from '@/lib/themes'
import {
  themePrefsSnapshot,
  saveThemeFontSize,
  saveThemeMode,
  saveThemePreset,
  saveThemeVariantMap,
  type VariantMap,
} from '@/lib/themePrefs'
import {
  activeUserThemeIdSnapshot,
  importUserTheme,
  removeUserTheme,
  upsertUserTheme,
  userThemeLibraryCss,
  userThemesSnapshot,
  writeActiveUserThemeId,
  type ImportOutcome,
  type UserThemeRecord,
} from '@/lib/userTheme'
import { auditBaseFor } from '@/lib/themeCatalog'
import { ThemeContext, type Theme } from '@/hooks/theme-context'

// 明暗模式、官方预设、配色变体、字号：**四格都住本机持久面**，而持久面归
// `lib/themePrefs.ts`（键 / 宽容解析 / 首帧快照 / 补水 / 写都在那里）。
//
// ⚠ 这里从前是**模块求值期同步读 `localStorage`**（`useState(readMode)` 这类初始化器）：
//   那是"首帧必须有值"与"存储窄口从第一天起就是异步的"撞在一起的地方。2026-10-08 存储收尾
//   第二刀把它拆成两半 —— **本文件只负责呈现**（DOM 属性、注入 `<style>`、动作），
//   真值由 `main.tsx` 在 `createRoot` 之前经窄口补进快照，这里的初始化器读快照（同步、一定有值）。
//
// 刀 1 增：**用户主题**（列表 / 当前生效）——同一族的持久面在 `lib/userTheme.ts`。
// 它和"官方预设"是**同一个选择的两面**：生效的用户主题在时，`data-theme` 写的是它的
// 作用域 id，配色变体自动让位（用户主题自带色彩，变体是官方预设的东西）。

/**
 * 注入**本机装的全部用户主题**样式的那个 `<style>` 的 id —— **唯一写者**，只有一个元素。
 *
 * 装全部而不是只装生效那一份（2026-10-08 刀 4）：设置页要在**切换之前**给出每条的真样张。
 * 生效哪一套**不由这个元素说** —— 由 `<html data-theme>` 说（两个事实两个位置）。
 * 没生效的那些在文档里但**永不命中**（作用域只带各自的 id）。
 */
const USER_STYLE_ID = 'celira-user-themes'

const MEDIA = '(prefers-color-scheme: dark)'

function systemTheme(): Theme {
  return window.matchMedia(MEDIA).matches ? 'dark' : 'light'
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  // 首帧快照（同步）：`main.tsx` 已经补过水 ⇒ 第一帧就是用户自己的配色，不闪。
  const [mode, setMode] = useState<ThemeMode>(() => themePrefsSnapshot().mode)
  const [preset, setPresetState] = useState<ThemePresetId>(() => themePrefsSnapshot().preset)
  const [variantMap, setVariantMap] = useState<VariantMap>(() => themePrefsSnapshot().variantMap)
  // 当前变体 = 当前主题的记忆值，缺省回落该主题原色 'standard'。
  const variant: ThemeVariantId = variantMap[preset] ?? 'standard'
  const setVariant = (v: ThemeVariantId) => {
    if (!isThemeVariantId(preset, v)) return
    setVariantMap((m) => ({ ...m, [preset]: v }))
  }
  const [fontSize, setFontSize] = useState<FontSizeId>(() => themePrefsSnapshot().fontSize)
  // system 模式下跟踪系统明暗变化；解析值驱动 classList 与组件。
  const [systemNow, setSystemNow] = useState<Theme>(systemTheme)

  // 用户主题：列表来自本机持久面（快照）；**生效 id 必须在列表里**（悬空 id 一律当没有，
  // 与 `parseUserThemes` 丢弃坏记录同一条纪律：不留半生效的状态）。
  const [userThemes, setUserThemes] = useState<UserThemeRecord[]>(userThemesSnapshot)
  const [activeUserThemeId, setActiveUserThemeId] = useState<string | null>(activeUserThemeIdSnapshot)
  const activeUserTheme = userThemes.find((item) => item.id === activeUserThemeId) ?? null

  useEffect(() => {
    const mq = window.matchMedia(MEDIA)
    const onChange = () => setSystemNow(mq.matches ? 'dark' : 'light')
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [])

  const theme: Theme = mode === 'system' ? systemNow : mode

  useEffect(() => {
    document.documentElement.classList.toggle('dark', theme === 'dark')
    void saveThemeMode(mode)
  }, [theme, mode])

  // ⚠ 这两个是 **layout effect**（在浏览器绘制**之前**跑）：用户主题的样式与 `data-theme`
  // 必须同一相位落下 —— 一个用 `useEffect`（绘制之后）会让首帧先出官方配色再跳成用户的，
  // 那是可见的闪。两者分处不同相位同样会闪（CSS 到了但作用域还没写）。
  useLayoutEffect(() => {
    // 生效的用户主题优先；否则按官方预设（默认预设不写属性，保持 <html> 干净）。
    if (activeUserTheme) {
      document.documentElement.dataset.theme = activeUserTheme.id
    } else if (preset === 'celira') {
      delete document.documentElement.dataset.theme
    } else {
      document.documentElement.dataset.theme = preset
    }
    void saveThemePreset(preset)
  }, [preset, activeUserTheme])

  useEffect(() => {
    // 用户主题自带色彩 ⇒ 配色变体同时让位（它是官方预设那一套里的东西）。
    if (!activeUserTheme && variant !== 'standard') {
      document.documentElement.dataset.variant = variant
    } else {
      delete document.documentElement.dataset.variant
    }
    void saveThemeVariantMap(variantMap)
  }, [variant, variantMap, activeUserTheme])

  useEffect(() => {
    // 'standard' 不写属性（与预设/配色变体的默认值同一纪律）。
    if (fontSize === 'standard') delete document.documentElement.dataset.fontSize
    else document.documentElement.dataset.fontSize = fontSize
    void saveThemeFontSize(fontSize)
  }, [fontSize])

  // 用户主题的样式注入：**这一个 `<style>` 是唯一写者**，写的是**全部**已装主题。
  // 一条都没有时写空串（而不是删元素），于是"全删光了"与"从没装过"落到同一个状态。
  useLayoutEffect(() => {
    let el = document.getElementById(USER_STYLE_ID) as HTMLStyleElement | null
    const css = userThemeLibraryCss(userThemes)
    if (!css) {
      if (el) el.textContent = ''
      return
    }
    if (!el) {
      el = document.createElement('style')
      el.id = USER_STYLE_ID
      document.head.appendChild(el)
    }
    el.textContent = css
  }, [userThemes])

  const toggleTheme = () => setMode(theme === 'dark' ? 'light' : 'dark')

  /** 切官方预设 = 同时退出用户主题（两者是同一个选择的两面）。 */
  const setPreset = (next: ThemePresetId) => {
    setPresetState(next)
    if (activeUserTheme) {
      setActiveUserThemeId(null)
      void writeActiveUserThemeId(null)
    }
  }

  const activateUserTheme = (id: string | null) => {
    setActiveUserThemeId(id)
    void writeActiveUserThemeId(id)
  }

  const installUserTheme = async (raw: string): Promise<ImportOutcome> => {
    // 基底 = 当前生效的官方预设那一份：用户没写的令牌继承它，少了它算不出对比度。
    // 同步可得（包是急加载的）⇒ 这里没有"等样式"的窗口。
    const outcome = importUserTheme(raw, auditBaseFor(preset))
    if (outcome.ok) {
      // ⚠ 落盘是异步的（窄口），所以**等它回来再推状态**：列表与盘上那一份必须一致，
      //   否则紧接着的一次导入会在旧的快照上做"读改写"。
      setUserThemes(await upsertUserTheme(outcome.record))
      activateUserTheme(outcome.record.id)
    }
    return outcome
  }

  const deleteUserTheme = (id: string) => {
    if (activeUserThemeId === id) setActiveUserThemeId(null)
    void removeUserTheme(id).then(setUserThemes)
  }

  return (
    <ThemeContext.Provider
      value={{
        theme,
        mode,
        setMode,
        toggleTheme,
        preset,
        setPreset,
        variant,
        setVariant,
        fontSize,
        setFontSize,
        userThemes,
        activeUserThemeId: activeUserTheme?.id ?? null,
        activateUserTheme,
        installUserTheme,
        deleteUserTheme,
      }}
    >
      {children}
    </ThemeContext.Provider>
  )
}
