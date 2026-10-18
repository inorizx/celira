import { useState } from 'react'
import { Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { removeHomeEntry, type HomeEntry } from '@/api/homes'

/**
 * **一台机器的管理面**（地址 / 端口 / 钥匙 / 移除）——从 `/settings/homes` 那一页搬进来的
 * （2026-10-12：机器清单**只留一个门**）。
 *
 * ## 为什么搬
 *
 * 同一份机器清单从前有**两个门**：`/app/runtimes`（一级入口，读处境）与 `/settings/homes`
 * （设置里，管钥匙）。两页列同一批条目、各说一半 ⇒ 人要记得"加一台去设置、看状态回执行端"。
 * 现在：**清单只有一处**（一级入口），"加一台"跟着清单走（清单页那颗「添加」就地开表单），
 * 而**某一台的管理**跟着那台机器（在它自己那一页展开 —— 那颗「这台机器的设置」）。
 *
 * ⚠ **它只说管理面**：那一台"现在怎么样"由机器卡与三个房间说（**一处说一次**），
 * 这里不复述处境。
 *
 * ## 它守的三条（逐字承自那一页，别改）
 *
 * - **钥匙只进不出**：粘贴的那把存进 IndexedDB（唯一落盘处）、发请求时作 Bearer 头过一次；
 *   这里**永不回显**它（只说"已配"）；
 * - **本机那条不给移除按钮**（预置的第一条，删不得）；
 * - **移除不是撤销**：两次点击确认式（第一次只把后果摆出来），名字不许叫「删除」
 *   （判据与「删除 · 仅为我 ≠ 删除」同源；那台机器上那把钥匙**照旧有效**，失效只有 CLI 一条路）。
 */
export function MachineAdminPanel({ entry, onRemoved }: { entry: HomeEntry; onRemoved: () => void }) {
  /** 已经点过一次「移除」（**两次点击确认式**，4 秒后自己解除 —— 与「撤销 Agent」同一配方）。 */
  const [armed, setArmed] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)

  /**
   * **移除一条家条目**：两次点击确认式（第一次把后果摆出来，第二次才真的移除）。
   *
   * ⚠ **它不能叫"删除"，也不能一次点掉**（2026-10-11）。两件事实：
   * 1. `removeHomeEntry` 只删**本机记录**（`api/homes.ts`）—— 那台机器上那把钥匙**照旧有效**；
   * 2. 钥匙的失效只有一条路：**在那台机器上重跑 `issue-home-key` 覆盖摘要**
   *    （`scripts/home/home-key.js`：换一把 ⇒ 所有已配的连接下一请求即失效）。
   *
   * ⇒ 把"这台浏览器不再记它"读成"它进不来了"，是这一格最容易犯、代价又在**安全**上的错
   * （用户以为已经切断，而那台机器还认那把钥匙）。
   */
  const remove = async () => {
    if (entry.local) return
    if (!armed) {
      setActionError(null)
      setArmed(true)
      setTimeout(() => setArmed(false), 4000)
      return
    }
    setArmed(false)
    setActionError(null)
    try {
      await removeHomeEntry(entry.id)
      onRemoved()
    } catch {
      setActionError('没能移除（这台浏览器写不了本地存储）')
    }
  }

  return (
    <div className="space-y-2 rounded-lg border border-border bg-muted/40 px-4 py-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1 break-all text-caption text-muted-foreground">
          {entry.local
            ? '这台机器，页面直接读（不填地址与钥匙）'
            : `${entry.baseUrl} · 状态端口 ${entry.statusPort} · 配置端口 ${entry.configPort}${entry.key ? ' · 已配钥匙' : ''}`}
        </div>
        {!entry.local && (
          <Button
            variant={armed ? 'destructive' : 'ghost'}
            size="sm"
            className="shrink-0"
            aria-label={armed ? `再次点击确认移除 ${entry.name}` : `移除 ${entry.name}`}
            onClick={() => void remove()}
          >
            {armed ? '再次点击确认移除' : <Trash2 className="size-4" />}
          </Button>
        )}
      </div>
      {/* **红路径不许静默**（`tests/no-silent-red-path.test.ts` 那条纪律）：失败一律说出口。 */}
      {actionError && (
        <p role="alert" className="text-caption text-presence-failed">
          {actionError}
        </p>
      )}
      {/* **后果在动手之前说清**（与「撤销 Agent」那一区同一配方）：第一次点击只是把这一段
          摆出来，第二次才真的移除。那三句缺一句，用户就会把"本机不再记它"读成"它进不来了"。 */}
      {armed && (
        <p className="border-t border-border pt-2 text-caption text-muted-foreground">
          移除只是让<strong>这台浏览器</strong>不再记它；<strong>那台机器上那把钥匙仍然有效</strong>
          。想真的让它进不来：在那台机器上重跑{' '}
          <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-micro">
            node home.js issue-home-key
          </code>
          （换一把 ⇒ 旧钥匙立刻全部失效）。
        </p>
      )}
    </div>
  )
}
