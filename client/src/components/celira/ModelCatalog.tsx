import { useMemo, useState } from 'react'
import { Search } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { buildCatalog, MODEL_KIND_LABEL, type ModelKind } from '@/api/modelCatalog'

/**
 * **这一家的模型目录**（从 `pages/ModelProvidersPage.tsx` 搬出来的，2026-10-12 第三刀）。
 *
 * 为什么单独一处：那一页 1602 行、主函数 645 行，里面混着**三个主语**（这份清单 / 某一家的形状 /
 * 试连读数）⇒ 按主语拆。这一处只说**这一家给了哪些模型**。
 *
 * 它是**读数**不是判据：空数组时整块不渲染（这家没给目录 ≠ 这家坏了）。
 * 换模型名会重问上下文长度 —— 那是随模型走的声明，不能沿用一个旧数字。
 * ⚠ **纯搬运**：文案、分支与那个 `useMemo` 的位置逐字照搬，行为零变化。
 */
export function ModelCatalog({
  entryId,
  models,
  kinds,
  disabled,
  onPick,
}: {
  entryId: string
  models: string[]
  /** 按模型名给的**权威品类**（原生目录层；缺省 ⇒ 按名字判，`buildCatalog` 自己兜）。 */
  kinds?: Record<string, ModelKind[]>
  disabled: boolean
  onPick: (entryId: string, model: string) => Promise<void>
}) {
  const [query, setQuery] = useState('')
  const [includeAll, setIncludeAll] = useState(false)
  /**
   * ★ **默认收起**（2026-10-16 主人当次："还是很混乱，不够符合直觉，太晦涩了"）。
   *
   * 病灶：它是**一次试连的产物**（读数），从前一探到就**自动铺满首屏**（实测 37 个芯片：
   * grok / kimi / glm / muse…）—— 而**主体**（这一条是什么：接口地址 / 密钥 / 这条的默认型号 /
   * 上下文长度）被顶到屏幕外，要滚很久才看得见 ✗。
   * 判据：**"这条能调哪些型号"是这一条的次要信息 ⇒ 收起；要挑的时候再展开**（展开后照旧有搜索）。
   */
  const [open, setOpen] = useState(false)
  // ⚠ 这个 `useMemo` 必须在下面那个 early return **之前** —— React 要求 hooks 顺序稳定。
  const catalog = useMemo(
    () => buildCatalog(models, { query, includeAll, kinds }),
    [models, kinds, query, includeAll],
  )
  if (models.length === 0) return null
  return (
    <div className="space-y-2 pt-1">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="ghost"
          size="sm"
          className="text-micro"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          {open ? '收起型号目录' : `这条能调哪些型号（${catalog.total}）`}
        </Button>
        {open && catalog.total > 8 && (
          <div className="relative ml-auto">
            <Search className="pointer-events-none absolute top-1/2 left-2 size-3 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="找模型…"
              aria-label="搜索模型"
              className="h-6 w-40 pl-7 text-micro"
            />
          </div>
        )}
      </div>
      {open && (
        <>
          <p className="text-micro text-muted-foreground">
            {catalog.total} 个模型（点一个就换成它）{includeAll ? '' : '—— 只列对话模型'}。
            换型号之后上下文长度会重新问一次。
          </p>
          <div className="max-h-56 space-y-2 overflow-auto">
            {catalog.groups.map(({ group, items }) => (
              <div key={group || '·'} className="space-y-1">
                {/* 命名空间前缀就是**分组标题**（`kimi/kimi-k3` ⇒「kimi」）；没前缀的不加标题，直接平铺。 */}
                {group && (
                  <p className="text-micro tracking-wider text-muted-foreground uppercase">{group}</p>
                )}
                <div className="flex flex-wrap gap-1.5">
                  {items.map((item) => (
                    <Button
                      key={item.id}
                      variant="outline"
                      size="sm"
                      className="font-mono text-micro"
                      disabled={disabled}
                      onClick={() => void onPick(entryId, item.id)}
                    >
                      {item.id}
                      {/* 非对话的才打标（`chat` 是默认，标了反而吵）。 */}
                      {item.kind !== 'chat' && (
                        <span className="ml-1 font-sans text-muted-foreground">
                          {MODEL_KIND_LABEL[item.kind]}
                        </span>
                      )}
                    </Button>
                  ))}
                </div>
              </div>
            ))}
            {catalog.shown === 0 && <p className="text-muted-foreground">没有匹配的模型。</p>}
          </div>
          {/* ★ 被挡掉的**必须明说** —— 否则用户以为那就是全部（他看不到的东西，就没法选）。 */}
          {(catalog.hiddenNonChat > 0 || includeAll) && (
            <Button
              variant="ghost"
              size="sm"
              className="text-micro"
              onClick={() => setIncludeAll((v) => !v)}
            >
              {includeAll ? '只看对话模型' : `还有 ${catalog.hiddenNonChat} 个非对话模型（向量 / 图像 / 语音…）`}
            </Button>
          )}
        </>
      )}
    </div>
  )
}
