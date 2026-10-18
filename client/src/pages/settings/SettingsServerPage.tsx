import type { ReactNode } from 'react'
import { useNavigate } from 'react-router'
import { ArrowLeft, Server } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useHomeReading } from '@/components/celira/use-home-reading'
import { localServiceReading } from '@/components/celira/reading-label'

/**
 * 本机服务（Core 管理节，2026-09-23 骨架刀 ＋ 同日填肉；★ 2026-10-11 收成**一个主语**）。
 *
 * 定位（2026-09-23 裁决）：core 的管理面按「你跟世界的关系」长——
 * 自用 ⇒ 只有状态读数；搬云 ⇒ 加连接健康；开放世界 ⇒ 真·后台（用户/封禁/备份）。
 * 当前只做自用级：**只读读数**，全是既有的东西，不新造 API、不做任何写操作。
 *
 * ★ **这一页的主语是"这台机器"**（`routeRegistry` 里它写着 `planes: ['home']`）。
 * 2026-10-11 之前它叫「服务器」，还混着两行**世界侧**读数（服务器地址 / 账号）——
 * 于是同一页两个主语、导航里又挂在「这个世界」组下 ✗。现在：
 *
 * - **留下的两行**（都归这台机器）：
 *
 * | 那一行 | 来源 |
 * |---|---|
 * | 本机服务端 | `/home-status` 的 `im` 那一格（探活读数，判据在 `reading-label.ts`） |
 * | 这台机器上的活动 | `/home-status` 的 `agent.activities`（**本机记录**） |
 *
 * - **搬走的两行**：服务器地址与账号**归世界侧**，它们的权威在「通用」那一页
 *   （地址在那里**可编辑**）。这一页**不复述**它们（一个事实一个权威位置 —— 复述会漂，
 *   而且复述出来的那一份永远比权威旧）。
 *
 * 两条纪律：**读不到的行如实写「未知」**（不隐藏整行，也不把读不到说成一个事实）；
 * **零写操作、零管理功能**（封禁 / 备份 / 用户列表都不做 —— 等真实消费者）。
 *
 * ⚠ 「版本」那一格**没有做**：状态读数里没有任何版本读数的位置（`im.health` 是 HTTP
 * 状态码，不是版本），而本刀不许新造 API ⇒ 宁可少一行，也不拿一个别的东西冒充它。
 */

/** 一行读数：左边这一行说的是什么，右边是读数。 */
function ReadingLine({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4 text-body">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="min-w-0 break-all text-right">{children}</span>
    </div>
  )
}

export function SettingsServerPage() {
  const navigate = useNavigate()
  const { reading } = useHomeReading()
  const view = reading.kind === 'loaded' ? reading.view : null
  const activities = view && typeof view.agent.activities === 'number' ? view.agent.activities : null

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-4 p-6">
      <div className="flex items-center gap-2 md:hidden">
        <Button variant="ghost" size="icon" onClick={() => navigate('/settings')}>
          <ArrowLeft className="size-4" />
        </Button>
        <span className="text-title">本机服务</span>
      </div>

      <div className="hidden items-center gap-2 md:flex">
        <Server className="size-5 text-muted-foreground" />
        <h1 className="text-title">本机服务</h1>
      </div>
      <p className="text-caption text-muted-foreground">
        这台机器上的进程与本机记录，只读。
      </p>

      <div className="space-y-3 rounded-lg border bg-card p-4">
        <ReadingLine label="本机服务端">
          {/* 判据与机器页「运行」那一间**同一处**（`reading-label.ts` 的 `localServiceReading`）：
              有应答 ⇒ 在跑（哪怕它自报 500）；没答话 ⇒ **读不到**，不是"没在跑"。 */}
          {view === null ? '未知' : localServiceReading(view.im.health).text}
        </ReadingLine>

        {/* ⚠ 这一格的标签带**主语**（2026-10-11）：从前它叫「活动总数」——一个没有主语的数
            （"谁的活动？"）。它数的是**这台机器上的本机记录**，所以主语写在标签里。 */}
        <ReadingLine label="这台机器上的活动">
          {activities === null ? '未知' : `${activities} 次`}
        </ReadingLine>
      </div>

      <div className="space-y-1 text-caption text-muted-foreground">
        {/* 那两行**世界侧**读数（地址 / 账号）的权威在「通用」那一页（地址在那里可编辑）
            ⇒ 这里只给路，不复述读数（复述出来那一份永远比权威旧）。
            ⚠ 这句话**不许**写成「服务器地址…」：那一页的地址栏 aria-label 就叫那个，
            一写就等于在这一页又出现了一次"服务器地址"（测试与人都分不清是读数是引用）。 */}
        <p>连接设置（你连的世界）在「通用」那一页。</p>
        {reading.kind === 'unreachable' && (
          <p>本机状态读不到——读不到不等于它停了。</p>
        )}
      </div>
    </div>
  )
}
