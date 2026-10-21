import { cn } from '@/lib/utils'

/**
 * 分段选择：外观（明暗 / 字号）、壁纸（作用范围 / 蒙版）、房间信息页签共用的那一个控件。
 *
 * ## 为什么要有它
 *
 * 这条表达式原来在 5 处各写了一遍（`bg-muted` 轨道 ＋ 选中片 `bg-card text-foreground shadow-sm`）。
 * 夜间 `--muted` 与 `--card` 的亮度差只有 **1.05:1**：轨道等于不存在，
 * 选中片又靠 `shadow-sm`——阴影压在这块底上投不出来。于是"我选中了哪个"只剩一条线索：字的亮度。
 *
 * ## 现在选中态只有一种说法
 *
 * 与会话列表选中行同族：**accent 底 ＋ accent-foreground 字 ＋ 加粗**。
 * 轨道靠 `ring-border` 的一条线站住 —— 暗色的容器**靠线分层，不靠把底调亮**
 * （判据与读数见 `src/styles/tokens.css` 的 `.dark` 注释）。
 *
 * 判据一句话：**"选中"不许靠阴影、不许只靠字的深浅** —— 那两样在夜间都不成立。
 */
export interface SegmentedOption<T extends string> {
  id: T
  label: string
}

export function Segmented<T extends string>({
  value,
  onChange,
  options,
  label,
  fill = true,
  dense = false,
  className,
}: {
  value: T
  onChange: (id: T) => void
  options: readonly SegmentedOption<T>[]
  /** 给读屏的分组名（`role="group"` 的 `aria-label`）。 */
  label: string
  /** 每格等分宽度（设置页与房间页签是满宽；壁纸那两处跟内容走）。 */
  fill?: boolean
  /** 紧凑档：容器与格子各收一档（壁纸那一族用）。 */
  dense?: boolean
  className?: string
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className={cn('flex gap-1 rounded-lg bg-muted ring-1 ring-border', dense ? 'p-0.5' : 'p-1', className)}
    >
      {options.map((option) => {
        const active = option.id === value
        return (
          <button
            key={option.id}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(option.id)}
            className={cn(
              'cursor-pointer rounded-md text-caption transition-colors',
              fill && 'flex-1',
              dense ? 'px-2.5 py-1' : 'px-2 py-1.5',
              active
                ? 'bg-accent font-medium text-accent-foreground'
                : 'text-muted-foreground hover:text-foreground',
            )}
          >
            {option.label}
          </button>
        )
      })}
    </div>
  )
}
