import { Check } from 'lucide-react'
import { cn } from '@/lib/utils'
import { getThemeVariants, type ThemePresetId, type ThemeVariantId } from '@/lib/themes'

/**
 * 主题**样张**：嵌套 `data-theme`（+ 可选 dark class）把目标作用域的令牌
 * **限制在这一块内部**——与 GalleryPage ThemePanel 同一机制，不切换全局主题也能真实预览。
 * 只用语义类，不出现任何色值。
 *
 * `scopeId` 收**任意字符串**：官方预设 id，或用户主题那份小样的**保留作用域 id**
 * （`themeFile.ts` 的 `PREVIEW_SCOPE_ID`）。要让样张正确，前提是 `<html>` 上**永远不写
 * 这个 id** —— 保留 id 那条纪律就是为此存在的。
 *
 * ⚠ `.theme-scope-body` 这个类名不是装饰：主题的形态层（字族等）会把 `body` 与它并列写，
 * 于是**样张里也吃得到那一层**。用户主题若只写 `body`，它的小样就看不到字族 —— 那是如实的限制。
 */
export function ThemeScopePreview({
  scopeId,
  dark,
  className,
}: {
  scopeId: string
  dark?: boolean
  className?: string
}) {
  return (
    <div
      data-theme={scopeId}
      aria-hidden
      className={cn(
        'theme-scope-body h-20 w-full overflow-hidden rounded-lg border border-border bg-background p-2',
        dark && 'dark',
        className,
      )}
    >
      <div className="mb-1.5 flex items-center gap-1.5 rounded-md bg-card px-2 py-1 shadow-sm">
        <span className="size-1.5 rounded-full bg-primary" />
        <span className="text-micro text-foreground">Aa 永</span>
        <span className="ml-auto size-1.5 rounded-full bg-agent" />
      </div>
      <div className="mb-1 h-3 w-3/5 rounded-lg rounded-tl-sm border border-border bg-bubble-peer" />
      <div className="ml-auto h-3 w-2/5 rounded-lg rounded-tr-sm bg-gradient-to-br from-bubble-self-from to-bubble-self-to" />
    </div>
  )
}

/** 官方预设的样张：作用域只能是预设目录里的 id（类型安全的外壳，视觉只有上面那一处）。 */
export function ThemePresetPreview({
  presetId,
  dark,
  className,
}: {
  presetId: ThemePresetId
  dark?: boolean
  className?: string
}) {
  return <ThemeScopePreview scopeId={presetId} dark={dark} className={className} />
}

/** 设置页主题选择卡：真实预览 + 名称描述 + 选中态。 */
export function ThemePresetCard({
  presetId,
  name,
  description,
  active,
  dark,
  onSelect,
}: {
  presetId: ThemePresetId
  name: string
  description: string
  active: boolean
  /** 预览跟随当前明暗模式 */
  dark?: boolean
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={active}
      className={cn(
        'w-full cursor-pointer space-y-2 rounded-xl border p-3 text-left transition-colors',
        active ? 'border-primary bg-accent' : 'border-border bg-card hover:bg-muted',
      )}
    >
      <ThemePresetPreview presetId={presetId} dark={dark} />
      <div className="flex items-center gap-2">
        <span className="text-body">{name}</span>
        {active && <Check className="size-4 text-primary" aria-label="当前主题" />}
      </div>
      <p className="text-caption text-muted-foreground">{description}</p>
    </button>
  )
}

/**
 * 配色变体选择器：列出当前主题手工策展的色板（各主题 variants 目录），
 * 色板用嵌套 data-theme + data-variant 作用域直接渲染该变体的
 * 主色→气泡渐变（与主题预览同一机制）；'standard' 不写 data-variant 属性，
 * 与全局默认值纪律一致。全部语义类，不出现色值。
 */
export function ThemeVariantPicker({
  presetId,
  variant,
  onSelect,
}: {
  presetId: ThemePresetId
  variant: ThemeVariantId
  onSelect: (variant: ThemeVariantId) => void
}) {
  return (
    <div className="flex flex-wrap gap-x-4 gap-y-2">
      {getThemeVariants(presetId).map((v) => (
        <button
          key={v.id}
          type="button"
          onClick={() => onSelect(v.id)}
          aria-pressed={variant === v.id}
          aria-label={`配色：${v.name}`}
          className="flex cursor-pointer flex-col items-center gap-1"
        >
          <span
            data-theme={presetId}
            data-variant={v.id === 'standard' ? undefined : v.id}
            className={cn(
              'flex size-8 items-center justify-center rounded-full bg-gradient-to-br from-brand to-bubble-self-to transition-shadow',
              variant === v.id ? 'ring-2 ring-ring ring-offset-2 ring-offset-card' : 'hover:opacity-85',
            )}
          >
            {variant === v.id && <Check className="size-3.5 text-primary-foreground" />}
          </span>
          <span className="text-caption text-muted-foreground">{v.name}</span>
        </button>
      ))}
    </div>
  )
}
