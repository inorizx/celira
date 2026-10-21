import { Moon, Sun } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useTheme } from '@/hooks/theme-context'
import { cn } from '@/lib/utils'

export function ThemeToggle({
  size = 'icon',
  className,
}: {
  /** 命中面档：默认 36（`Gallery`/`Profile` 那些行内按钮），`icon-lg` = 40（图标栏）。 */
  size?: 'icon' | 'icon-lg'
  /** 只放调用点特有的外观（如图标栏那边的 `rounded-xl`）；尺寸由 `size` 决定。 */
  className?: string
}) {
  const { theme, toggleTheme } = useTheme()
  // 图标跟着命中面走：36 的按钮配 16 的图标，40 的配 20 —— 两处不能各写各的
  // （`AppShell` 的图标栏是 40/20，`Gallery`/`Profile` 用默认的 36/16）。
  const iconClass = size === 'icon-lg' ? 'size-5' : 'size-4'
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size={size}
          onClick={toggleTheme}
          aria-label="切换深浅主题"
          className={cn('cursor-pointer', className)}
        >
          {theme === 'dark' ? (
            <Sun className={iconClass} />
          ) : (
            <Moon className={iconClass} />
          )}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{theme === 'dark' ? '切换到浅色' : '切换到深色'}</TooltipContent>
    </Tooltip>
  )
}
