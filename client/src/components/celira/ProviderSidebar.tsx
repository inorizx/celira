// **左栏：一屏供应商**（主语＝这份清单）
//
// ⚠ **2026-10-12 第三刀：从 `pages/ModelProvidersPage.tsx` 整段搬出来的**（那一页 1662 行、
// 主函数 645 行，一页里混着三个主语）。**纯搬运**：正文逐字照搬，行为零变化。
// 这一处只说**清单**：有哪几家、每家此刻是什么状态、搜一搜筛掉谁。

import { useState } from 'react'
import { Search } from 'lucide-react'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import {
  CUSTOM_PRESET_ID,
  CUSTOM_ROW,
  PRESET_DISCLAIMER,
  presetById,
  type ProviderRowState,
} from '@/api/presets'
import { ProviderMark } from '@/components/celira/ProviderMark'

/** 左栏一行：一家供应商 + 它此刻的状态（判据在 `api/presets.ts` 的 `providerRowState`）。 */
export interface ProviderSidebarRow {
  id: string
  name: string
  blurb: string
  state: ProviderRowState
  /** 这一家在清单里有几条（多数时候是 0 或 1；>1 时左栏旁注一下）。 */
  count: number
}

/** 某一家显示用的名字：预设给预设名，「自定义」给固定名。 */
export function providerName(providerId: string): string {
  return presetById(providerId)?.name ?? CUSTOM_ROW.name
}

/** 某一家显示用的一句人话：预设给它的简介，「自定义」给降级通道那句。 */
export function providerBlurb(providerId: string): string {
  return presetById(providerId)?.blurb ?? CUSTOM_ROW.blurb
}

export function ProviderSidebar({
  rows,
  selectedId,
  boundProviderId,
  onSelect,
}: {
  rows: readonly ProviderSidebarRow[]
  selectedId: string | null
  boundProviderId: string | null
  onSelect: (providerId: string) => void
}) {
  const [query, setQuery] = useState('')
  const needle = query.trim().toLowerCase()
  // 搜索只筛**名字与简介**；「自定义」是出口，**不该被搜没**（同旧版那条纪律）。
  const shown = rows.filter(
    (row) =>
      row.id === CUSTOM_PRESET_ID ||
      !needle ||
      row.name.toLowerCase().includes(needle) ||
      row.blurb.toLowerCase().includes(needle),
  )
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-border p-3">
        <div className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="找供应商…"
            aria-label="搜索供应商"
            className="bg-muted pl-9"
          />
        </div>
      </div>
      <ScrollArea className="min-h-0 flex-1 [&_[data-radix-scroll-area-viewport]>div]:!block">
        <nav className="space-y-1 p-2" aria-label="预设供应商">
          <p className="px-2 py-1.5 text-micro text-muted-foreground">{PRESET_DISCLAIMER}</p>
          {shown.map((row) => {
            const active = row.id === selectedId
            const isBound = row.id === boundProviderId
            return (
              <button
                key={row.id}
                type="button"
                onClick={() => onSelect(row.id)}
                aria-pressed={active}
                className={cn(
                  'flex w-full cursor-pointer items-start gap-2.5 rounded-lg px-2.5 py-2.5 text-left transition-colors',
                  active ? 'bg-accent' : 'hover:bg-muted',
                )}
              >
                {/* 标记是这一家的**真 logo**（来源 / 许可 / 刷新法 → `src/assets/provider-logos/NOTICE.md`）；
                    认不出这一家就回落到首字母。 */}
                <ProviderMark id={row.id} name={row.name} size={28} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-body">{row.name}</span>
                  <span className="block truncate text-micro text-muted-foreground">{row.blurb}</span>
                </span>
                <span className="flex shrink-0 flex-col items-end gap-1">
                  {isBound && (
                    /* ⚠ 这一档最强：Agent **正在用的那条**就在这一家里。 */
                    <Badge
                      variant="outline"
                      className="text-micro border-presence-online/40 text-presence-online"
                    >
                      正在使用
                    </Badge>
                  )}
                  {!isBound && row.state === 'saved' && (
                    /* ⚠ 判据是**这一家里有条目**（地址归并），不是"这家配好了"。 */
                    <Badge variant="outline" className="text-micro text-muted-foreground">
                      已保存
                    </Badge>
                  )}
                  {row.count > 1 && (
                    <span className="text-micro text-muted-foreground">{row.count} 条</span>
                  )}
                </span>
              </button>
            )
          })}
          {shown.length === 0 && (
            <div className="px-3 py-6 text-center text-caption text-muted-foreground">
              没有匹配的供应商
            </div>
          )}
        </nav>
      </ScrollArea>
    </div>
  )
}
