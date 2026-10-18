/**
 * 官方主题预设目录。
 * 预设包含「身份信息」（id/名称/描述）与各自的配色变体目录；颜色与质感
 * 事实全部在 `src/styles/tokens.css` 登记——主题令牌按 [data-theme='…']
 * 作用域，配色变体按 [data-theme='…'][data-variant='…'] 追加覆盖层。
 * 新增主题或变体时两边同时加，组件代码不感知主题。
 *
 * 配色变体是**按主题手工策展**的色板：只覆盖主行动/品牌/焦点环/自己气泡
 * 渐变等「色彩声音」令牌，背景/文字/表面结构保持主题原样；组件样式与
 * 布局完全不变。'standard' = 不写 data-variant，使用主题原色。
 */

/**
 * 明暗模式词汇（2026-10-08）：'system' = 跟随系统。
 *
 * ⚠ 它住在这里而不是 `hooks/theme-context.ts`：持久面（`lib/themePrefs.ts`）要按它解析与
 * 落盘，而 **`lib/` 不许反向依赖 `hooks/`**。`theme-context.ts` 照旧再导出这两个类型，
 * 于是既有的 `import { type ThemeMode } from '@/hooks/theme-context'` 一个都不用改。
 */
export type Theme = 'dark' | 'light'
export type ThemeMode = Theme | 'system'

export const THEME_PRESETS = [
  {
    id: 'celira',
    name: '默认主题',
    description: '官方默认 · 柔和扁平',
    variants: [
      { id: 'standard', name: '原色' },
      { id: 'azure', name: '星蓝' },
      { id: 'rose', name: '蔷薇' },
    ],
  },
  {
    id: 'glass',
    name: '雾玻璃',
    description: '半透明磨砂 · 环境光感',
    variants: [
      { id: 'standard', name: '原色' },
      { id: 'aurora', name: '极光' },
      { id: 'dusk', name: '暮橙' },
    ],
  },
  {
    id: 'neon',
    name: '赛博朋克',
    description: '霓虹夜色 · 锐利边缘',
    variants: [
      { id: 'standard', name: '原色' },
      { id: 'cyber', name: '电青' },
      { id: 'acid', name: '酸绿' },
    ],
  },
  {
    id: 'paper',
    name: '纸张墨',
    description: '衬线印刷 · 朱砂印记',
    variants: [
      { id: 'standard', name: '朱砂' },
      { id: 'indigo', name: '靛青' },
      { id: 'pine', name: '松绿' },
    ],
  },
  {
    id: 'clay',
    name: '软陶',
    description: '柔雾触感 · 阴影浮雕',
    variants: [
      { id: 'standard', name: '原色' },
      { id: 'sage', name: '鼠尾草' },
      { id: 'terracotta', name: '陶橙' },
    ],
  },
  {
    id: 'comic',
    name: '漫画格',
    description: '粗描边 · 硬投影',
    variants: [
      { id: 'standard', name: '原色' },
      { id: 'hero', name: '英雄蓝' },
      { id: 'bubblegum', name: '泡泡糖' },
    ],
  },
  {
    id: 'contrast',
    name: '高对比',
    description: '纯粹黑白 · 清晰至上',
    variants: [
      { id: 'standard', name: '原色' },
      { id: 'amber', name: '琥珀' },
      { id: 'emerald', name: '翡翠' },
    ],
  },
  {
    id: 'pixel',
    name: '像素风',
    description: '复古游戏 · 8-bit',
    variants: [
      { id: 'standard', name: '原色' },
      { id: 'arcade', name: '街机蓝' },
      { id: 'gameboy', name: '绿豆' },
    ],
  },
  {
    id: 'sakura',
    name: '樱吹雪',
    description: '圆润贴纸 · 柔粉心事',
    variants: [
      { id: 'standard', name: '樱粉' },
      { id: 'matcha', name: '抹茶' },
      { id: 'wisteria', name: '紫藤' },
    ],
  },
  {
    id: 'dazzle',
    name: '炫光',
    description: '流光渐变 · 张扬灯带',
    variants: [
      { id: 'standard', name: '紫青' },
      { id: 'flare', name: '品红' },
      { id: 'pulse', name: '青柠' },
    ],
  },
] as const

export type ThemePresetId = (typeof THEME_PRESETS)[number]['id']

export function isThemePresetId(value: string | null): value is ThemePresetId {
  return THEME_PRESETS.some((p) => p.id === value)
}

/** 变体 id 按主题各自为政（同名不同色），统一用 string 承载。 */
export type ThemeVariantId = string

export function getThemeVariants(presetId: ThemePresetId) {
  return THEME_PRESETS.find((p) => p.id === presetId)!.variants
}

export function isThemeVariantId(presetId: ThemePresetId, value: string | null): value is ThemeVariantId {
  return getThemeVariants(presetId).some((v) => v.id === value)
}

/**
 * 字号档位目录：只覆盖语义文本令牌（--text-display/title/body/caption/micro），
 * 是纯阅读/无障碍偏好——不改变按钮、间距、图标、布局或触控几何。
 * 'standard' = 不写 data-font-size，使用默认字阶。
 */
export const FONT_SIZES = [
  { id: 'small', name: '小' },
  { id: 'standard', name: '标准' },
  { id: 'large', name: '大' },
] as const

export type FontSizeId = (typeof FONT_SIZES)[number]['id']

export function isFontSizeId(value: string | null): value is FontSizeId {
  return FONT_SIZES.some((s) => s.id === value)
}
