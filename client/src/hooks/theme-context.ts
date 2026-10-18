import { createContext, useContext } from 'react'
import type { UserThemeRecord } from '@/lib/userTheme'
import type { ImportOutcome } from '@/lib/userTheme'

// ⚠ 这两个类型住在 `lib/themes.ts`（2026-10-08 挪过去的）：持久面 `lib/themePrefs.ts` 要按它
// 解析与落盘，而 lib 不许反向依赖 hooks。这里**再导出**，既有的
// `import { type ThemeMode } from '@/hooks/theme-context'` 一个都不用改。
import type { Theme, ThemeMode } from '@/lib/themes'

export type { Theme, ThemeMode }

export interface ThemeContextValue {
  theme: Theme
  mode: ThemeMode
  setMode: (mode: ThemeMode) => void
  toggleTheme: () => void
  preset: import('@/lib/themes').ThemePresetId
  setPreset: (preset: import('@/lib/themes').ThemePresetId) => void
  variant: import('@/lib/themes').ThemeVariantId
  setVariant: (variant: import('@/lib/themes').ThemeVariantId) => void
  fontSize: import('@/lib/themes').FontSizeId
  setFontSize: (size: import('@/lib/themes').FontSizeId) => void
  /**
   * 本机装载的用户主题（刀 1）。它们是**数据**，不是代码：装载不需要任何 API 承诺，
   * 卸载就是删掉一条记录。外观这一层因此是前端**唯一**的合法接缝
   * （`AGENTS.md` §前端边界的「扩展接缝」）。
   */
  userThemes: UserThemeRecord[]
  /** 当前生效的用户主题 id；`null` = 用官方预设（两者是同一个选择的两面）。 */
  activeUserThemeId: string | null
  /** 切到某条用户主题；传 `null` 回到官方预设那一套。 */
  activateUserTheme: (id: string | null) => void
  /** 导入一份主题文件：格式不合即拒收（装载不了），体检结论不拦人。 */
  installUserTheme: (raw: string) => Promise<ImportOutcome>
  deleteUserTheme: (id: string) => void
}

export const ThemeContext = createContext<ThemeContextValue | null>(null)

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext)
  if (!ctx) throw new Error('useTheme must be used within ThemeProvider')
  return ctx
}
