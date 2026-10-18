// **右侧：这一家的配置面板**（主语＝某一家的形状）
//
// ⚠ **2026-10-12 第三刀：从 `pages/ModelProvidersPage.tsx` 整段搬出来的**（那一页 1662 行、
// 主函数 645 行，一页里混着三个主语）。**纯搬运**：正文逐字照搬，行为零变化。
// 这一处只说**某一家**：它在这台机器上的条目、表单、试连、保存、移除、解绑。

import type { ChangeEvent } from 'react'
import { Eye, EyeOff, Plus, RefreshCw, Save, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { formatRuntimeTime } from '@/lib/time'
import { providerLinks, type ProviderPreset } from '@/api/presets'
import { ProviderMark } from '@/components/celira/ProviderMark'
import {
  probeUrlFor,
  type HomeConfigDraft,
  type HomeConfigTextField,
  type HomeConfigProbe,
  type HomeConfigView,
  type HomeSourceEntry,
} from '@/api/homeConfig'
import { ProbeReadingLine, ProbeUrlHint } from '@/components/celira/ModelProbeReading'
import { providerName, type ProviderSidebarRow } from '@/components/celira/ProviderSidebar'

/**
 * 「获取模型列表」那一步的读数。
 *
 * 三态分开（**试连失败是读数不是错误**）：门拒绝这次请求（形状 / 没给钥匙）才是 `refused`；
 * 而"连上了但对方回 401"、"超时"都是 `reading` —— 它们**不影响保存**，页面要把这两件事分开说。
 */
export type ProbeState =
  | { kind: 'idle' }
  | { kind: 'busy' }
  | { kind: 'reading'; probe: HomeConfigProbe }
  | { kind: 'refused'; message: string }

/**
 * **右侧：这一家的配置面板**（面板头承担身份 + 状态 + 保存 —— 旧版 `ProviderConfigPanel` 的形状）。
 *
 * 结构（自上而下）：
 * ```
 * [口] DeepSeek                          [正在使用]  [保存]
 *   这一家在这台机器上有一把钥匙（或：还没开始配）
 * 名字 / 接口地址 / 密钥 / 获取模型列表 / 模型名 / 上下文长度 / 高级
 * ```
 *
 * 三条纪律照旧（刀2b 与预设层一字未动）：
 * - **预设那条路地址只读**（替你填好的）；要自己填地址就选「自定义」；
 * - **获取模型列表 ≠ 保存**：探测不落盘，页面上明说；
 * - **一家多把是罕见边角**：面板底部留一颗次要的「再加一把钥匙」，主路径不受它影响。
 */
export function ProviderPanel({
  rows,
  providerId,
  providers,
  editingEntry,
  inUseIds,
  inUseNames,
  view,
  draft,
  canWrite,
  busy,
  showKey,
  setShowKey,
  field,
  setProtocol,
  onVision,
  autoWindow,
  tableWindow,
  windowProbeFailed,
  preset,
  probeState,
  onFetchModels,
  onPickModel,

  onSave,
  enableOnSave,
  usersOf,
  onNewEntry,
  onOpenEntry,
  onRemoveEntry,
  removing,
  onConfirmRemove,
  onCancelRemove,
  onReload,
}: {
  rows: readonly ProviderSidebarRow[]
  providerId: string
  /** 这一家在清单里的条目（0 条 ⇒ 还没开始配）。 */
  providers: readonly HomeSourceEntry[]
  /** 正在改的那一条（`null` ⇒ 在给这一家新加）。 */
  editingEntry: HomeSourceEntry | null
  /**
   * **正在被用着的那几条**（`homeConfig.ts` 的 `inUseEntryIds`：逐支口径）。
   *
   * ⚠ 从前这里是**一个** `bound: string | null` —— 那是门那边"**第一支**绑的那条"。
   * 一个家住 N 支时它会漏掉后面那几支（"这一家有人在用"被画成"只是存过"）。
   */
  inUseIds: readonly string[]
  /** 同上那几条的**名字**（"谁在用"那一句要用）。 */
  inUseNames: readonly string[]
  view: HomeConfigView
  draft: HomeConfigDraft
  canWrite: boolean
  busy: boolean
  showKey: boolean
  setShowKey: (value: boolean) => void
  field: (key: HomeConfigTextField) => {
    value: string
    disabled: boolean
    onChange: (event: ChangeEvent<HTMLInputElement>) => void
  }
  setProtocol: (protocol: string) => void
  /**
   * ★ **这一份模型收不收得到媒体材料**（2026-10-17）。
   *
   * 它**不走 `field()`**：那一格是布尔，而 `field()` 只服务文本格（`<input value>` 收不下
   * 布尔 —— 见 `HomeConfigTextField` 那一段），所以勾选框自己接一条线。
   *
   * ⚠ 调用方打开**已有条目**时必须按 `entry.vision ?? false` 初始化（表单总是"说了"）：
   * 否则用户碰不到"明确关掉"这一态，而那一态是**三态**里唯一能把已经开着的关掉的那一态。
   */
  onVision: (value: boolean) => void
  autoWindow: boolean
  tableWindow: boolean
  windowProbeFailed: boolean
  preset: ProviderPreset | null
  probeState: ProbeState
  onFetchModels: () => void
  onPickModel: (name: string) => void
  onSave: () => void
  /**
   * ★ **存完顺手给它用吗**（2026-10-15）：调用方按"这台机器上**恰一支**住户"给
   * （0 支没有目标、2 支以上就是替别人决定）—— 按钮上那句话跟着它走。
   */
  enableOnSave: boolean
  /** **这一条有谁在用**（名字，读数）—— 换绑是**那一支**页面上的动作，这一页只说读数。 */
  usersOf: (id: string) => string[]
  onNewEntry: () => void
  onOpenEntry: (entry: HomeSourceEntry) => void
  onRemoveEntry: (id: string) => void
  removing: string | null
  onConfirmRemove: (id: string) => void
  onCancelRemove: () => void
  onReload: () => void
}) {
  const row = rows.find((each) => each.id === providerId)
  const name = row?.name ?? providerName(providerId)
  const creating = editingEntry === null
  /**
   * 窗口那一格的**条件 caption**（2026-09-23 文案减重刀）。
   *
   * 那格底下原先常驻一段讲解（"留空就行…它是声明不是实测…换模型时必须重填"）—— 那是在用户
   * **没问**的时候先把机制讲一遍。但里面藏着**一个真事实**：改过模型名之后，格子里那个数字
   * 是**上一个模型的**，而保存时会**自动重问**。
   * ⇒ 只在它真的变得相关的那一刻说（**在改一条已有的** ＋ 模型名跟已存的不一样 ＋ 格子非空）。
   * 三件缺一都不显示（新加一条没有"上一个模型"这回事）。
   */
  const staleWindow =
    editingEntry !== null && draft.model !== editingEntry.model && draft.contextWindow.trim() !== ''
  /**
   * 「这个模型能看图」那一格的**条件 caption**（2026-10-17）——与 `staleWindow` **同一条判据**：
   * 随模型走的声明，在**模型名变了之后**就是**上一个模型的答案**。
   *
   * 为什么只在"这一格正说**能**看图"时说：`false` 是安全的那一侧（fail-closed），换型号后它
   * 照旧是"读不动"，不需要提醒；真正会骗人的是**上一个模型的那个"能"**。
   *
   * ⚠ 走**模型目录**换型号（`useModel`）不走这一格 —— 那条路在写的时候就把这一格复位了，
   * 并由回执说清。这里管的是**手填型号名**那条路：所见即所存，缺的只是"它随模型走"这件事。
   */
  const staleVision =
    editingEntry !== null && draft.model !== editingEntry.model && draft.vision === true
  /**
   * 这一条**谁在用**（读数，不是动作）—— 2026-10-15 主人裁：**模型配置归机器侧**。
   *
   * 从前这里是一枚「正在使用」徽标 ＋ 一颗「换成这条」（`bindHomeSource(id)` **不带 agentId**
   * ⇒ 一次换的是**这台机器上每一支**：没有主语的动作）。现在换绑归**那一支**的页面
   * （那里点得名、说得清"给谁换"），这一页只把**名字写出来**。
   */
  const users = usersOf(editingEntry?.id ?? '')
  const usersLabel =
    users.length > 0
      ? `${users.join('、')} 在用`
      : providers.length > 0
        ? '没人用'
        : '还没配'
  const keyConfigured = editingEntry?.keyConfigured ?? false
  /** 拉回来的模型目录（**只有 2xx 且真列了东西**才算数；空数组不是错误，是"这家没给"）。 */
  const fetched = probeState.kind === 'reading' ? probeState.probe : null
  const models = fetched !== null && fetched.ok ? fetched.models : []
  const canFetch = true

  return (
    <div className="space-y-6">
      {/* ---- 面板头：身份 + 状态徽标 + 保存（动作集中右上角 —— 旧版那个形状）---- */}
      <div className="flex items-center gap-3">
        {/* 跟左栏那一行**同一个标记** ⇒ 点谁进来、这里就是谁（身份不靠读名字才认得）。 */}
        <ProviderMark id={row?.id ?? ''} name={name} size={40} />
        <div className="min-w-0">
          {/* ★ **标题是"这一条"的名字**（2026-10-16）：从前它写的是**这一家**（「OpenCode Go」），
              而点进来的那一条叫「OpenCode Go · DeepSeek V4.1 Flash（…）」⇒ 标题与点击对象对不上 ✗。
              新建那一条时才用这一家的名字（那时还没有"这一条"）。 */}
          <div className="text-title">{editingEntry?.name ?? name}</div>
          <div className="text-caption text-muted-foreground">
            {editingEntry ? `${name}${row?.blurb ? ` · ${row.blurb}` : ''}` : (row?.blurb ?? '')}
          </div>
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-3">
          <Badge
            variant="outline"
            className={cn(
              users.length > 0 && 'border-presence-online/40 text-presence-online',
              users.length === 0 && 'text-muted-foreground',
            )}
          >
            {usersLabel}
          </Badge>
          <Button onClick={onSave} disabled={!canWrite || busy} className="gap-1.5">
            <Save className="size-4" />
            {/* ★ 存完要不要**顺手给它用**（2026-10-15）：**恰一支住户**才说"给它用"
                —— 0 支没有目标、2 支以上就是替别人决定（那颗按钮只说"保存"）。 */}
            {busy ? '正在保存…' : creating ? (enableOnSave ? '保存并给它用' : '保存') : '保存这一条'}
          </Button>
        </div>
      </div>

      {/* ★ **同址的兄弟收成一行**（2026-10-16 主人当次："还是很混乱…太晦涩了"）。
          从前这里是一张「这一家的 2 条」表（每行带徽标/型号/改这条/删掉），而**同一副屏幕上**
          左栏已经是"这台机器上的钥匙"、右栏又是"这一条的表单" ⇒ 三副眼镜看同一块地方 ✗。
          现在只留一行：**同址还有 N 条**（点一下切过去），危险动作跟着那一条自己的面板走。 */}
      {providers.length > 1 && (
        <p className="flex flex-wrap items-center gap-2 text-micro text-muted-foreground">
          <span>同一个地址还有 {providers.length - 1} 条：</span>
          {providers
            .filter((entry) => entry.id !== editingEntry?.id)
            .map((entry) => (
              <Button
                key={entry.id}
                variant="ghost"
                size="sm"
                className="text-micro"
                disabled={!canWrite || busy}
                onClick={() => onOpenEntry(entry)}
              >
                {entry.name}（{inUseIds.includes(entry.id) ? `${usersOf(entry.id).join('、')} 在用` : '没人用'}）
              </Button>
            ))}
        </p>
      )}
      {/* ★ **删掉这一条**（销毁式）：从前它分散在两处（"一家一条"与"一家多条"各一份拷贝），
          现在**只一处**、只对**当前这一条**（`editingEntry`）—— 新加那条自然没有它。 */}
      {editingEntry !== null && (
        <section className="flex flex-wrap items-center gap-2">
          {removing === editingEntry.id ? (
            <div className="w-full space-y-3 rounded-lg border border-destructive/40 bg-card px-3 py-3">
              <p className="text-caption text-destructive">
                这一条里的密钥<strong>只有本机这一份</strong>，删掉就没了 —— 以后要用得重新填一遍。
                {inUseIds.includes(editingEntry.id)
                  ? '还有 Agent 正用着它，本机服务会拒绝删除：先在它自己的页面上换成别的。'
                  : '删掉它不影响任何 Agent 正在用的那一条。'}
              </p>
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  variant="destructive"
                  size="sm"
                  className="gap-1.5"
                  disabled={busy}
                  onClick={() => onConfirmRemove(editingEntry.id)}
                >
                  <Trash2 className="size-4" />
                  {busy ? '正在删除…' : '确认删除这一条'}
                </Button>
                <Button variant="ghost" size="sm" disabled={busy} onClick={onCancelRemove}>
                  取消
                </Button>
              </div>
            </div>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              className="gap-1.5 text-muted-foreground"
              disabled={!canWrite || busy}
              onClick={() => onRemoveEntry(editingEntry.id)}
            >
              <Trash2 className="size-3.5" />
              删掉这一条
            </Button>
          )}
        </section>
      )}

      {/* ---- 名字 ---- */}
      <section className="space-y-2">
        <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">名字</div>
        <Input {...field('name')} placeholder="例如 主钥匙（留空就用模型名）" autoComplete="off" />
      </section>

      {/* ---- 接口地址 ----
          两条路的样子**刻意不同**：
          - 预设那条路：地址是**替你填好的**，所以它不摆成一个输入框 —— 摆成一条**读数**
            （完整地址 ＋「预设地址 · 不用你填」），「自定义」那条路才是真的要你填（输入框）。
          ⚠ **2026-10-16 换掉了那身绿**（主人当次："这种接口地址，为啥是绿的，看起来很不合理？"）。
          从前它是 `presence-online`（= **在线**那个绿）＋ 一个勾 ＋「已就绪」徽标 —— 而这一格
          **只说明"这格不用你填"**，**没有验证过任何东西**（对方通不通、钥匙对不对，一概未知）：
          绿＋勾＋"已就绪"在替一件**没发生的事**作证 ✗；同一屏上「没人用」还挂着，更自相矛盾。
          判据（本仓早就写着的那两条）：**试连是读数不是判据**、**预设 ≠ 已配置** ⇒ 这身绿属于
          **读数**（试连成功 / 有人在用 / 已保存），不属于"预设替你填好了"。 */}
      <section className="space-y-2">
        <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">接口地址</div>
        {preset ? (
          <>
            <div className="flex items-start gap-2.5 rounded-lg border border-border bg-muted/40 px-3 py-2.5">
              <code className="min-w-0 flex-1 font-mono text-caption break-all text-foreground">
                {preset.endpointTemplate}
              </code>
              <Badge variant="outline" className="shrink-0 text-micro text-muted-foreground">
                预设地址 · 不用你填
              </Badge>
            </div>
            <ProbeUrlHint url={probeUrlFor(preset.endpointTemplate)} />
          </>
        ) : (
          <>
            <Input
              {...field('endpoint')}
              placeholder="https://…/v1/chat/completions"
              autoComplete="off"
            />
            <p className="px-1 text-micro text-muted-foreground">要填完整地址，不是只到域名。</p>
            {/* ★ 自定义那条路是**边填边看**：地址一改这行就跟着变 —— 填错当场暴露。 */}
            <ProbeUrlHint url={probeUrlFor(field('endpoint').value)} />
          </>
        )}
      </section>

      {/* ---- 密钥 ---- */}
      <section className="space-y-2">
        <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">密钥</div>
        <div className="relative">
          <Input
            type={showKey ? 'text' : 'password'}
            {...field('apiKey')}
            placeholder={
              keyConfigured
                ? '留空 = 沿用已存的那把钥匙'
                : creating
                  ? '粘贴这一家的 API Key（新加的这条必须给）'
                  : '粘贴这一家的 API Key'
            }
            autoComplete="off"
            className="pr-10"
          />
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setShowKey(!showKey)}
            aria-label={showKey ? '隐藏密钥' : '显示密钥'}
            className="absolute top-1/2 right-1 -translate-y-1/2"
          >
            {showKey ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
          </Button>
        </div>
        <p className="px-1 text-micro text-muted-foreground">
          它只写进本机那份清单（明文，与环境变量同级）；页面不回显、浏览器不存、日志不记。
        </p>
        {/* ★ 用户最容易卡住的一步：「我知道要贴 key，但去哪儿申请？」—— 直接给门。
            只在**认得这一家**时给（自定义那条路不知道是哪一家 ⇒ 不给，不猜）。 */}
        {providerLinks(preset?.id) && (
          <p className="px-1 text-micro text-muted-foreground">
            还没有钥匙？
            <a
              href={providerLinks(preset?.id)!.apiKeyUrl}
              target="_blank"
              rel="noreferrer noopener"
              className="text-foreground underline underline-offset-2"
            >
              去 {preset?.name} 申请一把
            </a>
          </p>
        )}
      </section>

      {/* ---- 获取模型列表（`POST /home-config/probe`）----
          贴完钥匙的下一步。⚠ 它**不写任何文件**：探测即测试，保存才落盘（主人拍过的裁决）
          —— 所以"贴一把假 key 试一下"在这里是安全的，页面也要把这一点说出来。 */}
      {canFetch && (
        <section className="space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              className="gap-1.5"
              disabled={!canWrite || busy || !draft.endpoint.trim() || probeState.kind === 'busy'}
              onClick={onFetchModels}
            >
              <RefreshCw className="size-3.5" />
              {probeState.kind === 'busy' ? '正在获取…' : '获取模型列表'}
            </Button>
            <span className="text-micro text-muted-foreground">
              只试连一次，不会保存任何东西
            </span>
          </div>
          {probeState.kind === 'reading' && <ProbeReadingLine probe={probeState.probe} />}
          {probeState.kind === 'refused' && (
            <p role="alert" className="text-caption text-destructive">
              {probeState.message}
            </p>
          )}
        </section>
      )}

      {/* ---- 这条的默认型号 ----
          ★ **2026-10-15 分名**（主人裁：模型配置归机器侧）：这一格是**条目**的属性
          ——"这把钥匙默认用哪个型号"；而"**这一支**具体用哪个型号 / 什么思考档"是**支级覆盖**，
          在**那一支自己的页面**上选。两件事同名会读混 ⇒ 这里写全"这条的默认型号"。 */}
      <section className="space-y-2">
        <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">
          这条的默认型号
        </div>
        <p className="px-1 text-micro text-muted-foreground">
          这把钥匙默认用哪个型号；<strong>某一支</strong>具体用哪个、什么思考档，在那一支自己的页面上选。
        </p>
        {/* 拿到了目录就**给一个下拉**（预设层那个形状）；没拿到也不挡路 —— 下面那格照旧能手填。 */}
        {models.length > 0 && (
          <select
            aria-label="从这家列的模型里挑一个"
            value={models.includes(draft.model) ? draft.model : ''}
            disabled={!canWrite || busy}
            onChange={(event) => onPickModel(event.target.value)}
            className="w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-body"
          >
            <option value="">从这家列的模型里挑一个…</option>
            {models.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </select>
        )}
        <Input {...field('model')} placeholder="例如 deepseek-chat" autoComplete="off" />
      </section>

      {/* ---- 上下文长度 ---- */}
      <section className="space-y-2">
        <div className="flex items-center gap-2 px-1">
          <span className="text-micro tracking-wider text-muted-foreground uppercase">上下文长度</span>
          {autoWindow && (
            <Badge variant="outline" className="border-presence-online/40 text-presence-online">
              已自动获取
            </Badge>
          )}
          {tableWindow && (
            <Badge variant="outline" className="border-muted-foreground/40 text-muted-foreground">
              来自对照表
            </Badge>
          )}
        </div>
        <Input
          {...field('contextWindow')}
          inputMode="numeric"
          placeholder={windowProbeFailed ? '例如 131072（自动获取失败，请手填）' : '留空 = 保存时自动获取'}
          autoComplete="off"
        />
        {staleWindow && (
          <p className="px-1 text-micro text-muted-foreground">
            这个数字还是上一个模型的，清空再保存会自动重问。
          </p>
        )}
      </section>

      {/* ---- 媒体材料（`vision`）----
          ★ **2026-10-17**：与「上下文长度」**同族** —— 两格都是**宿主声明的**（"这个模型是
          什么样"），不是我们探测出来的；判据见 `agent/README.md` §6 解冻 ④。
          ⚠ **不勾的后果必须写在勾旁边**（本仓纪律「人不许比系统知道得更少」）：这一格是
          **开合能力门**的那一格 —— 关着的时候，工具面既不摆出"读得动图片"这句话，也**不报
          "图片读不了"以外的第二种答案**。那两句话就是这一格的全部意义，所以它们是常驻的
          （不是"安静时刻讲机制"：它们在描述**此刻**这一格的值）。
          ⚠ 勾选框与 `field()` 那条线**分开**：`field()` 只服务文本格（见 `HomeConfigTextField`）。 */}
      <section className="space-y-2">
        <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">媒体材料</div>
        <label className="flex cursor-pointer items-start gap-3 rounded-lg border border-border bg-card px-3 py-2.5">
          <div className="min-w-0 flex-1 space-y-1">
            <div className="text-body">这个模型能看图</div>
            <p className="text-caption text-muted-foreground">
              不勾：读附件时图片会被拒绝（
              <code className="rounded bg-muted px-1 py-0.5 font-mono">unsupported-content-type</code>
              ），它读附件的那条工具说明里也不会列出那四种媒体类型（png / jpeg / webp / gif）。
            </p>
            <p className="text-caption text-muted-foreground">
              这是<strong>这台机器</strong>的事实，不是某一支自己的设置 ——
              同一台机器上跑着的 Agent 都按它走。
            </p>
          </div>
          <Switch
            className="mt-0.5"
            aria-label="这个模型能看图"
            checked={draft.vision === true}
            disabled={!canWrite || busy}
            onCheckedChange={(value) => onVision(value)}
          />
        </label>
        {/* 与「上下文长度」那条提示同族：**型号名变了**之后，这一格就是上一个模型的答案
            （只在它正说"能"时说 —— 见 `staleVision` 的判据）。 */}
        {staleVision && (
          <p className="px-1 text-micro text-muted-foreground">
            这个勾还是上一个模型的答案 —— 换了型号要自己确认一下。
          </p>
        )}
      </section>

      {/* 接口类型是**进阶项**：默认收起（缺省就是最常见那一种，摆在外面会让人以为"必须选一个"）。 */}
      <details className="space-y-2">
        <summary className="cursor-pointer text-micro tracking-wider text-muted-foreground uppercase">
          高级
        </summary>
        <div className="mt-2 space-y-2">
          <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">
            接口类型（可选）
          </div>
          <div className="flex gap-2">
            {[
              { value: '', label: '默认（chat-completions）' },
              { value: 'chat-completions', label: 'chat-completions' },
              { value: 'responses', label: 'responses' },
            ].map((option) => (
              <Button
                key={option.value || 'default'}
                variant={draft.protocol === option.value ? 'default' : 'outline'}
                size="sm"
                disabled={!canWrite || busy}
                onClick={() => setProtocol(option.value)}
              >
                {option.label}
              </Button>
            ))}
          </div>
          <p className="px-1 text-micro text-muted-foreground">
            不给就用默认那一种。选{' '}
            <code className="rounded bg-muted px-1 py-0.5 font-mono">responses</code> 时，接口地址要填
            <strong>完整地址</strong>（例：…/v1/responses）。
          </p>
        </div>
      </details>

      {/* ---- 面板底：这一家的收尾（次要动作，不抢主路径）---- */}
      <section className="flex flex-wrap items-center gap-2 border-t border-border pt-4">
        {/* ★ **同一个东西只叫一个名字**（2026-10-17 主人当场问："啥叫再加一条"）：
            这一页上面把这东西叫**钥匙**（「这台机器上的钥匙」「加一把钥匙」），
            这里却叫"一条" ⇒ 读的人不知道"一条"是什么 ✗。⇒ 统一成「钥匙」（量词"把"），
            并给一句**什么时候需要它**（否则"再加一把"听起来像重复配置）。 */}
        <Button
          variant="ghost"
          size="sm"
          className="gap-1.5 text-muted-foreground"
          disabled={!canWrite || busy}
          onClick={onNewEntry}
          title="同一个供应商再加一把：另一个密钥，或同一个地址按型号分开记"
        >
          <Plus className="size-3.5" />
          这一家再加一把钥匙
        </Button>
        <div className="ml-auto flex items-center gap-2">
          <span className="text-micro text-muted-foreground">
            更新时间 {formatRuntimeTime(view.readAt)}
          </span>
          <Button variant="ghost" size="sm" className="gap-1.5" onClick={onReload} disabled={busy}>
            <RefreshCw className="size-3.5" />
            重新读
          </Button>
        </div>
      </section>

      {/* ★ **"谁用哪一条"不在这里换**（2026-10-15 主人裁：模型配置归机器侧）。
          从前这一块是「解除指定」（`bindHomeSource(null)` —— 同样**不带 agentId**：
          一次解除的是**这台机器上每一支**）。现在这一页只说读数，指一条真路。 */}
      <p className="text-micro text-muted-foreground">
        {inUseNames.length === 0
          ? '这台机器上还没有谁在用这些钥匙 —— 配好之后去那一支的页面上挑它用哪一把。'
          : `谁用哪把钥匙、用哪个型号，在**那一支自己的页面**上换${
              inUseNames.length === 1 ? `（现在用的是「${inUseNames[0]}」）` : `（现在有 ${inUseNames.length} 条各有住户在用）`
            }。`}
      </p>
    </div>
  )
}

