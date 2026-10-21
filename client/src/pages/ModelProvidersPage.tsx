import { useCallback, useEffect, useState, type ChangeEvent } from 'react'
import { useNavigate, useParams } from 'react-router'
import { ArrowLeft, AlertTriangle, Loader2, Plus, RefreshCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { CommandLine } from '@/components/celira/CommandLine'
import { cn } from '@/lib/utils'
import { hasRealWorldTransport, useAppStore, useConnection } from '@/api/store'
import { isRealWorldSession } from '@/api/prefs'
// ★ **三个主语各成一处**（2026-10-12 第三刀：这一页从前 1662 行、主函数 645 行）：
//   试连读数（`ModelProbeReading`）／这一家的模型目录（`ModelCatalog`）／
//   左栏那份清单（`ProviderSidebar`）＋右侧某一家的形状（`ProviderPanel`）。
//   这一页只剩**编排**：把清单、面板、通知接起来。
import { probeText } from '@/components/celira/ModelProbeReading'
import { ModelCatalog } from '@/components/celira/ModelCatalog'
import { ProviderSidebar, providerBlurb, providerName, type ProviderSidebarRow } from '@/components/celira/ProviderSidebar'
import { ProviderPanel, type ProbeState } from '@/components/celira/ProviderPanel'
import {
  HomeConfigError,
  createHomeSource,
  deleteHomeSource,
  fetchHomeConfig,
  inUseEntryIds,
  inUseEntryNames,
  modelChangeDraft,
  probeHomeSource,
  residentBindings,
  updateHomeSource,
  type HomeConfigDraft,
  type HomeConfigTextField,
  type HomeConfigProbe,
  type HomeConfigView,
  type HomeSourceEntry,
} from '@/api/homeConfig'
import type { HomeEntry } from '@/api/homes'
import { useHomeEntryList } from '@/components/celira/use-home-entries'
import {
  CUSTOM_PRESET_ID,
  CUSTOM_ROW,
  PROVIDER_PRESETS,
  groupEntriesByProvider,
  presetById,
  presetIdForEndpoint,
  providerRowState,
} from '@/api/presets'

/**
 * **模型配置**（TASK_77 重写；刀2b 升成清单管理）。
 *
 * 它管的是**这台机器**上的钥匙（清单）——"**这一个 Agent 用哪一条**"是另一件事，在
 * 它的模型房间（`pages/RuntimesPage.tsx` 那一页的「模型」房间）里换。原理一句话：
 * **钥匙串归房子，用哪把钥匙归住户**。
 *
 * ⚠ **2026-09-26 修陈旧注释**：这里原先写着「所以这一页的每一行都有一个「正在使用」的徽章
 * 看得见，但**换绑不在这里做**」—— 而**这一页后来就在换绑**（`/home-binding` 那条路：
 * 行上的「换成这条」/「解除指定」）。注释停在刀2b 那一刻，与代码已经相反。
 *
 * 现在两件事的真实分工是：**清单**（有哪些钥匙：端点 / 密钥 / 模型名 / 上下文长度）
 * 与**绑定**（这一个 Agent 用哪一条）**都在这一页**，但**是两个动作、两个入口**，
 * 界面上分得开（加一条 = 进清单**并**指定给 Agent；「换成这条」= 只动绑定）——
 * 这样人就不会以为"改一条 = 换一条"。
 *
 * 它从前是「模型服务」——一份 fixtures 驱动的**假供应商目录**（搜索、浏览目录、勾选要
 * 启用哪些模型），**配不了任何东西**。现在是配置面：端点 / 密钥 / 模型名 / 上下文长度
 * （+ 可选接口类型）逐条存进本机那份清单。
 *
 * ---
 * ## 形状：**点谁配谁**（2026-09-23 主人第三次裁决，退回 2026-09-02 那版 `15f1fae`）
 *
 * 主人的原话意思：**「没有加一条的这种选项……以前那个页面就很人性化，现在太笨重」**。
 * 前两版我一直在"加一条"这个动作上打转（先是把两级面板塞进居中定宽，再是给它一个全高
 * 侧边栏）——**病根不在布局，在那个动作本身**：一按「加一条」，整页重心就压到"填一张工程
 * 表单"上。旧版不是这么干的。
 *
 * 现在的形状（一句）：**左栏常驻一屏供应商，点谁右边就配谁**。
 *
 * ```
 * ┌────────────────┬──────────────────────────────────┐
 * │ 找供应商…       │ [D] DeepSeek     [正在使用] [保存] │
 * │ 预设 ≠ 已配置…  │ 名字 / 接口地址 / 密钥 /           │
 * │ ● DeepSeek 正在使用│ [获取模型列表] / 模型名 /         │
 * │ ○ OpenAI  已保存 │ 上下文长度 / 高级                  │
 * │ ○ Anthropic      │ 这一家再加一条 ·  重新读 · 解除指定 │
 * │ …（预设表）…     │                                    │
 * │ ○ 自定义         │                                    │
 * └────────────────┴──────────────────────────────────┘
 * ```
 *
 * 三条边界（别漂移）：
 * - **左栏一行 = 一家**（不是"清单里的一条"）：判据是**地址相等** ⇒ 同一家
 *   （`api/presets.ts` 的 `groupEntriesByProvider`，一处权威）。**一家可以有多条**
 *   （同一家两个 key），那是罕见边角 —— 所以它不在左栏拆成多行，而是收进右侧面板里；
 * - **获取模型列表 ≠ 保存**：那一步走 `POST /home-config/probe`，**不写任何文件**
 *   （"探测即测试，保存才落盘"是主人拍过的裁决）—— 所以"贴了把假 key 试一下"是安全的；
 * - **预设 ≠ 已配置**：左栏那句明示必须上屏；「已保存」标记按**地址相等**认，不是"这个
 *   供应商配好了"。
 *
 * 四条纪律：
 * - **写动作要真实世界**（`isRealWorldSession`：手里有没有真实凭据）：演示通道不写本机配置。
 *   读显示照「家」那个房间的先例——门开着就照实显示；
 * - **失败不许静默**：任何动作失败一律把服务那句人话摆出来（`role="alert"`），
 *   不制造假成功、也不把返回值丢掉；
 * - **试连是读数不是判据**：它失败了保存照样成立（服务那边已经落定），页面必须把这两件事
 *   分开说 —— 否则人会把"这一家现在连不上"当成"我的配置没写进去"；
 * - **两个"删除"不是一回事**：**解除指定**是撤销式（Agent 改用环境变量里的设置，清单一条不少）；
 *   **删掉一条**是**销毁式**（那条里的密钥只有本机这一份，删了就没了）—— 后果不同，
 *   所以要各自说清。正被指定的那条**删不掉**（先换成别的）。
 *
 * ⚠ **一家一条**是最常见的情形 ⇒ 那种情形下右侧就是**那一条的参数**（点进去就看见，
 * 不是一张空表单）；**一家零条**时右侧是预填好地址的"开始配"；**一家多条**时才多一块
 * "这一家的 N 条"列表 —— 罕见边角不出现在主路径上。
 *
 * 页面**不知道口子开在哪个端口**：它只问同源的 `/home-config` 与 `/home-binding`
 * （代转的活由 `serve_client.mjs` 或 vite preview 干，见 `api/homeConfig.ts`）。
 */

const EMPTY_DRAFT: HomeConfigDraft = {
  name: '',
  endpoint: '',
  apiKey: '',
  model: '',
  contextWindow: '',
  protocol: '',
}

/** 把清单里的一条灌进表单；**密钥永远是空的**（服务不回它，页面也无从知道）。 */
function draftOf(entry: HomeSourceEntry): HomeConfigDraft {
  return {
    name: entry.name,
    endpoint: entry.endpoint,
    apiKey: '',
    model: entry.model,
    contextWindow: entry.contextWindow === null ? '' : String(entry.contextWindow),
    protocol: entry.protocol ?? '',
    // ★ **打开一条已有的 ⇒ 勾按这一条自己说的初始化**（2026-10-17）。
    //
    // ⚠ `?? false` 那一半不是多余的：老门的回执里**没有**这一格（`HomeSourceEntry.vision`
    //   是可选），而"读不到"按门的同一条判据落到**不支持**（fail-closed）。
    // ⚠ 这一句让**表单一旦打开就总是"说了"**（`true` / `false`，没有第三种）—— 正是为了
    //   让用户碰得到"明确关掉"那一态：不这样，取消勾选发出去的体里会**少一格**，门把它读成
    //   "没说"、再从旧条目原样带回来 ⇒ **这一格永远关不掉**，而且没有任何报错
    //   （判据与门那一侧同一句，见 `api/homeConfig.ts` 的 `declarationBody`）。
    vision: entry.vision ?? false,
  }
}

type Reading =
  | { kind: 'loading' }
  | { kind: 'unreachable' }
  | { kind: 'loaded'; view: HomeConfigView }

/**
 * 表单此刻在编什么：**哪一家**（`providerId`）＋ 是改它已有的哪条（`entryId`）还是给它新加一条。
 *
 * 2026-09-23 换成这个模型（主人裁决）：旧版是「点左栏一家 ⇒ 右边配这一家」——
 * **左栏的心智对象是"一家供应商"，不是"清单里的一条"**。但底下存的确实是条目
 * （一家可以有零到多条），所以这里两层都留着：
 *
 * - `providerId` 决定左栏谁高亮、右栏显示哪一家；
 * - `entryId` 有值 ⇒ 在改那一条；`null` ⇒ 在给这一家**新加**一条。
 *
 * ⚠ **没有"什么都没在编"这一态**：页面一进来就选中第一家（或清单里的第一家）。
 * 旧版就是这个行为（`selected` 恒有一家）；"空面板"那种状态正是让人觉得笨重的东西之一。
 */
type Editing = { providerId: string; entryId: string | null }



interface ModelProvidersPageProps {
  onBack?: () => void
}

/**
 * **某一台机器的模型配置**（`/app/runtimes/:entryId/config`；2026-10-11 按机器寻址）。
 *
 * ## 为什么地址里必须有机器
 *
 * 判据一句话：**配置跟着它的对象走，不跟着"设置"走。** 这一页管的是**某一台机器**上的
 * 钥匙清单（清单落 `<家>/model_sources.json`），而"用哪一条"是**住户**的事
 * （`<环境根>/model_binding.json`，在机器页的「模型」那一间换）——"钥匙串归房子，
 * 用哪把钥匙归住户"。旧地址 `/settings/providers` 里**没有机器上下文**，于是它自己
 * 长了一行"配哪台机器"的选择器；现在**地址就是那台机器**，那一行随之退场
 * （旧地址保留成别名：带 `?home=` 落那台、没有上下文落机器列表 —— 见 `App.tsx`）。
 *
 * ⚠ **认不出那台机器就如实说，绝不回落本机**（2026-10-11 修）：旧实现是
 * `find(...) ?? localHomeEntry()` —— 地址里点名了一台不在列表里的机器时，页面会**默默去配本机**，
 * 而用户以为自己在配那台。判据与机器页那句「没有这台机器」同款（**地址里点名了谁，
 * 页面就说谁**；说不出来就说说不出来）。
 */
export function ModelProvidersPage({ onBack }: ModelProvidersPageProps) {
  const navigate = useNavigate()
  const params = useParams<{ entryId?: string }>()
  const { mode, token } = useConnection()
  // **唯一权威判据**：手里有没有真实凭据（`api/prefs.ts`）。演示通道不写本机配置。
  const canWrite = isRealWorldSession({ mode, token })

  // **哪台机器由地址说**（`/app/runtimes/:entryId/config`）。
  const { entries: homeEntries, settled: homesSettled } = useHomeEntryList()
  const entryId = params.entryId ?? ''
  const entry: HomeEntry | null =
    homeEntries.find((candidate) => candidate.id === entryId) ?? null
  const goBack = onBack ?? (() => navigate(entry ? `/app/runtimes/${entry.id}` : '/app/runtimes'))
  const whereLabel = entry === null ? '这台机器' : entry.local ? '本机' : entry.name

  const [reading, setReading] = useState<Reading>({ kind: 'loading' })
  /**
   * 现在在配哪一家。
   *
   * ⚠ **一进来就有值**（`null` 只在"还没读到清单"那一瞬）：读完之后落到第一家。
   * 旧版（2026-09-02）就是这样 —— 空着不动是"笨重"的一半来源。
   */
  const [editing, setEditing] = useState<Editing | null>(null)
  const [draft, setDraft] = useState<HomeConfigDraft>(EMPTY_DRAFT)
  const [showKey, setShowKey] = useState(false)
  /** 正在跑的那个动作（字符串是给文案用的）：一次只许一个。 */
  const [busy, setBusy] = useState<string | null>(null)
  /** 动作失败：**服务那句人话**（`role="alert"`）。 */
  const [actionError, setActionError] = useState<string | null>(null)
  /**
   * 动作成功侧的读数（含那一次试连）。
   *
   * `saved` 是**这两件事必须分开说**的那一格：试连失败**不等于**没保存（服务那边已经落定），
   * 而"没能自动问出上下文长度"那一路上**确实没保存**——同一句"试连读数"要配不同的话。
   */
  const [notice, setNotice] = useState<{
    text: string
    probe: HomeConfigProbe | null
    saved: boolean
    /** 只有真保存过才带：窗口那一格的**值与来路**（人填 / 试连 / 随包对照表）。 */
    windowSource?: 'given' | 'probed' | 'table' | null
    windowValue?: number | null
  } | null>(null)
  /** 自动问窗口失败过（那之后 helper 降级成"这时才需要你填"）。 */
  const [windowProbeFailed, setWindowProbeFailed] = useState(false)
  /** **删掉一条**的确认门口；带 id ⇒ 正在确认那一行。 */
  const [removing, setRemoving] = useState<string | null>(null)
  /** 「获取模型列表」那一步的读数（**不落盘**：它不产生任何持久事实）。 */
  const [probeState, setProbeState] = useState<ProbeState>({ kind: 'idle' })
  /**
   * ★ **"加一把钥匙"那一步开着吗**（2026-10-15）。
   *
   * 从前左栏**常驻**一屏供应商（模板库）—— 于是"这台机器上有什么"与"世上还有哪些家"并排摆着，
   * 读的人分不清哪一栏才是**这台机器的事实**。现在：左栏常态列**这台机器上的钥匙**，
   * 预设表只在**这一步**出现（选一个起点，然后填参数）。
   */
  const [adding, setAdding] = useState(false)

  const view = reading.kind === 'loaded' ? reading.view : null
  /** **世界侧的 Agent 名单**（这一页只拿它把 id 说成人名 —— "谁在用"那一格是读数）。 */
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const entries = view?.entries ?? []
  const bound = view?.bound ?? null
  const boundEntry = entries.find((entry) => entry.id === bound) ?? null
  /**
   * **正在被用着的那几条**（`homeConfig.ts` 的 `inUseEntryIds`：**逐支**口径）。
   *
   * ⚠ 与 `bound` 那一格的区别：那是门取的"**第一支**绑的那条"（一个家住 N 支时它只是其中一支的）。
   * 这一页问的是"**这台机器上有没有人在用它**" —— 徽标、删除提示、保存提示都该走这一组。
   */
  const inUseIds = view ? inUseEntryIds(view) : []
  const inUseNames = view ? inUseEntryNames(view) : []
  /**
   * **这台机器上有几支住户**（`residentBindings`：`null` = 门没给这一格 ⇒ 不知道）。
   *
   * 它决定两件事，都是"恰一支才算没有选择"那把尺子：
   * ① 存完要不要**顺手启用**（恰一支 ⇒ 是；0 支没有目标、2 支以上就是替别人决定）；
   * ② 面板上那颗保存按钮怎么说（"保存并给它用" / "保存"）。
   */
  const residentCount = (view ? residentBindings(view) : null)?.length ?? null
  /**
   * **这个家有没有住户**（`residentBindings`：`[]` = 门说一支都没有；`null` = 门没给这一格 ⇒ 不知道）。
   *
   * 为什么要它：这一页管的是**这台机器的钥匙串**（机器级事实，0 住户时也成立），
   * 而"指定给某一支用"是**住户级**动作 —— 0 住户时它**没有目标** ⇒ 那颗按钮不摆（见 `ProviderPanel`）。
   */
  const noResident = (view ? residentBindings(view) : null)?.length === 0
  /**
   * 清单**按"一家"归并**（判据在 `api/presets.ts` 的 `groupEntriesByProvider`，一处权威）。
   * 左栏一行 = 一家；点它就是配这一家。
   */
  const byProvider = groupEntriesByProvider(entries)
  /** 当前选中那一家的预设（「自定义」⇒ `null`：它没有预设地址，要自己填）。 */
  const selectedPreset = presetById(editing?.providerId)
  /**
   * 左栏**冒头**那一家：**第一条**在用的那一条归在哪一家（**只用于排序**，不是断言 ——
   * 多支的家可能有好几条在用，判据归 `providerRowState(ids, inUseIds)`）。
   */
  const boundProviderId =
    boundEntry === null ? null : (presetIdForEndpoint(boundEntry.endpoint) ?? CUSTOM_PRESET_ID)
  /**
   * 窗口那一格**是不是自动来的、以及是哪条路来的**（读数都来自门，不是页面自己猜的）。
   *
   * - `probed` = 保存时向服务方问来的 ⇒ 旁注「已自动获取」；
   * - `table`  = 对方没给、从**随包那份对照表**里按家查到的 ⇒ 旁注「来自对照表」。
   *
   * 两条路**必须分开标注**：后者不是对方说的，用户有权知道该不该信（帮填 ≠ 权威值）。
   */
  const autoWindow = view !== null && view.windowSource === 'probed'
  const tableWindow = view !== null && view.windowSource === 'table'
  /** 这一家现在在编的那一条（`null` ⇒ 新加）；只认真的在清单里的 id。 */
  const editingEntry =
    editing?.entryId == null ? null : (entries.find((entry) => entry.id === editing.entryId) ?? null)
  /**
   * 左栏那些行：**清单里已存过的家排前面，然后是预设表其余的家，「自定义」永远最后**。
   *
   * 排序判据（一处权威，本页只算一次）：用户真正在用的是清单里那几家，它们该在上面；
   * 还没配过的预设跟在后面（那是"可以加哪些"）；「自定义」是出口，永远最后。
   */
  const sidebarRows: ProviderSidebarRow[] = (() => {
    const saved = Array.from(byProvider.keys())
    const savedSet = new Set(saved)
    const rows: ProviderSidebarRow[] = []
    // 已存的：先按"Agent 正在用的那家"冒头，其余按清单顺序。
    const ordered = [...saved].sort((a, b) => {
      if (a === boundProviderId) return -1
      if (b === boundProviderId) return 1
      return 0
    })
    for (const id of ordered) {
      const ids = (byProvider.get(id) ?? []).map((entry) => entry.id)
      rows.push({
        id,
        name: providerName(id),
        blurb: providerBlurb(id),
        state: providerRowState(ids, inUseIds),
        count: ids.length,
      })
    }
    // 未配过的预设（保持预设表顺序）。
    for (const preset of PROVIDER_PRESETS) {
      if (savedSet.has(preset.id)) continue
      rows.push({
        id: preset.id,
        name: preset.name,
        blurb: preset.blurb,
        state: 'none',
        count: 0,
      })
    }
    if (!savedSet.has(CUSTOM_PRESET_ID)) {
      rows.push({
        id: CUSTOM_PRESET_ID,
        name: CUSTOM_ROW.name,
        blurb: CUSTOM_ROW.blurb,
        state: 'none',
        count: 0,
      })
    }
    return rows
  })()

  const load = useCallback(async () => {
    setReading({ kind: 'loading' })
    setActionError(null)
    // ⚠ **认不出那台机器就什么都不读**（`entry === null`）：绝不拿本机顶上 ——
    //   那会让"地址里点名的那台"与"实际读的那台"不是同一个（2026-10-11 修）。
    if (entry === null) return
    // 演示通道没有家在跑（与 `use-home-entries` / `use-home-reading` 同一道闸）：
    // 不打 `/home-config`，如实"读不到"——而不是打出一串 502 再显示同一句话。
    if (!hasRealWorldTransport()) {
      setReading({ kind: 'unreachable' })
      return
    }
    try {
      const fresh = await fetchHomeConfig(undefined, entry)
      setReading({ kind: 'loaded', view: fresh })
    } catch {
      // 服务没开 / 超时 / 形状认不出 —— 一律"读不到"，**不填一份空读数冒充当下**。
      setReading({ kind: 'unreachable' })
    }
    // `entry` 换一台机器就该重读（条目对象每次渲染现算，所以按 id 钉依赖）。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entry?.id])

  useEffect(() => {
    void load()
  }, [load])

  /**
   * 读完之后**落到第一家**（2026-09-23）：一进来就摆一张要手填地址的空表单是"没选中"；
   * 而**什么都不选**就是旧版让人说"笨重"的那种感觉 —— 页面进来该已经站在某一家上。
   *
   * ⚠ 只在**还没有选**的时候落一次（`editing === null`）：用户点过哪一家就停在哪儿，
   * 读回来的新清单不许把他的选择顶掉。顺序：**清单里已存的排前面**（那是他真正在用的），
   * 然后是预设表的第一家（还没配过任何东西时的默认落脚点）。
   */
  useEffect(() => {
    if (view === null || editing !== null) return
    const firstSaved = entries[0]
    if (firstSaved) {
      const providerId = presetIdForEndpoint(firstSaved.endpoint) ?? CUSTOM_PRESET_ID
      setEditing({ providerId, entryId: firstSaved.id })
      setDraft(draftOf(firstSaved))
      return
    }
    setEditing({ providerId: PROVIDER_PRESETS[0]!.id, entryId: null })
    setDraft({ ...EMPTY_DRAFT, name: PROVIDER_PRESETS[0]!.name, endpoint: PROVIDER_PRESETS[0]!.endpointTemplate })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view])

  /** 一次动作的统一收口：成功接上服务回的那份三合一，失败摆人话。 */
  const run = async (label: string, action: () => Promise<HomeConfigView>, after?: (view: HomeConfigView) => void) => {
    setBusy(label)
    setActionError(null)
    setNotice(null)
    try {
      const fresh = await action()
      setReading({ kind: 'loaded', view: fresh })
      setWindowProbeFailed(false)
      after?.(fresh)
      return fresh
    } catch (error) {
      // ⚠ **不许静默**：拒绝时带一句人话（封闭词表），照原样显示；不认识的码也如实说。
      setActionError(
        error instanceof HomeConfigError ? error.message : `${label}没成功（${whereLabel}的服务没有响应）`,
      )
      if (error instanceof HomeConfigError && error.code === 'window_unavailable') {
        setWindowProbeFailed(true)
        if (error.probe) {
          // ⚠ 这一路**没保存**（一份没有上下文长度的配置 Agent 读不了）⇒ `saved: false`，话说清楚。
          setNotice({ text: '这次没能自动问出上下文长度，所以还没有保存', probe: error.probe, saved: false })
        }
      }
      return null
    } finally {
      setBusy(null)
    }
  }

  /** 保存表单里那一条：改它已有的那条（**不动指定**）或给这一家新加一条（**保存即启用**）。 */
  const saveDraft = async () => {
    if (entry === null) return
    if (editingEntry !== null) {
      // 带上门当前选中的**是哪一家**：窗口留空且试连也没问到时，门会拿它去随包那份对照表里**限家**查。
      const saved = await run('保存', () =>
        updateHomeSource(editingEntry.id, draft, editing?.providerId, entry),
      )
      if (saved) {
        // 改一条**不会换绑**（`updateHomeSource` 送的是 `bind: false`）⇒ 服务回的读数里
        // "有没有人在用它"照旧成立。它是不是**刚改的这条**，决定后面那句说不说 —— 不猜；
        // 判据走**逐支那一组**（`inUseEntryIds`），不是门取的"第一支那条"。
        const inUse = inUseEntryIds(saved).includes(editingEntry.id)
        setNotice({
          // 0 住户时"起用这一条"没有主语（**保存**照旧成立：钥匙串是这台机器的）—— 如实分开说。
          // ★ 2026-10-16 改口径：**只对"正在用它的那几支"成立**（逐支绑定；从前那句
          //   "Agent 下次活动起用新参数"在家里有没选它的支时是**半真话**）。
          text:
            residentBindings(saved)?.length === 0
              ? '已保存。这台机器上还没有 Agent —— 等一支住进来，再去它自己的页面上选这一条。'
              : inUse
                ? '已保存。正在用它的那几支，下一次活动起用新参数。'
                : '已保存。现在没有哪一支在用它 —— 谁用它，在那一支自己的页面上选。',
          probe: saved.probe ?? null,
          saved: true,
          windowSource: saved.windowSource ?? null,
          // 那一格**实际落盘**的是什么 —— 提示里要如实说，不拿试连读数顶（查表那条路上它是 null）。
          windowValue: saved.entries.find((entry) => entry.id === saved.entryId)?.contextWindow ?? null,
        })
      }
      return
    }
    const saved = await run('保存', () =>
      // ★ **存完要不要顺手给它用**（2026-10-15）：**恰一支住户**才顺手（那不是替谁决定 ——
      //   没有别人可决定）；0 支没有目标、2 支以上就是替别人决定 ⇒ 只进清单。
      createHomeSource(draft, editing?.providerId, entry, residentCount === 1),
    )
    if (saved) {
      // ⚠ **不关面板**：保存完还停在这一级 —— 左边那份清单就是这条流的收尾读数。
      // ★ 「加一把钥匙」那一步到此收场（新那条已经进了清单，选择器留着只会让人以为还要再选一次）。
      setAdding(false)
      // 顺手**把钥匙从表单里清掉**：它已经进了清单，留在页面的内存里没有第二个理由。
      setDraft((current) => ({ ...current, apiKey: '' }))
      setProbeState({ kind: 'idle' })
      setNotice({
        text:
          residentCount === 1
            ? '已保存，它下一次活动起用这一条；正在进行的那次不变。'
            : residentCount === 0
              ? '已保存。这台机器上还没有 Agent —— 等一支住进来，再去它的页面上选这一条。'
              : '已保存。这台机器上有好几支 Agent —— 谁用这一条，在**它们各自**的页面上选（不替它们决定）。',
        probe: saved.probe ?? null,
        saved: true,
        windowSource: saved.windowSource ?? null,
        windowValue: saved.entries.find((entry) => entry.id === saved.entryId)?.contextWindow ?? null,
      })
    }
  }

  /**
   * ★ **换绑那一族动作从这一页撤掉了**（2026-10-15 主人裁：**模型配置归机器侧**）。
   *
   * 从前这里有「换成这条」与「解除指定」，而它们走的是 `bindHomeSource(id, entry)` ——
   * **不带 `agentId`** ⇒ 点一下换的是**这台机器上每一支**：一个**没有主语**的动作
   * （与 2026-10-15 在机器页撤掉的那一颗同族）。
   * 判据：这一页的主语是**机器**（这台上有哪几把钥匙）；"哪一支用哪条"归**那一支**的页面，
   * 那里点得名、也说得清"给谁换"。
   *
   * 这一页保留的只是**读数**：每一行写"谁在用"（名字，见 `usersOfEntry`）。
   */

  /**
   * **这一条有谁在用**（名字）—— 只读，不是动作。
   *
   * 名字从**世界**侧那份 Agent 列表来（`ownedAgents`）；读不到名字时**不猜、也不吞**：
   * 那几支照旧占一行，只是名字显示成编号（"读不到名字 ≠ 没有这一支"）。
   */
  const usersOfEntry = (id: string): string[] => {
    const rows = view ? (residentBindings(view) ?? []) : []
    return rows
      .filter((row) => row.state === 'bound' && row.bound === id)
      .map((row) =>
        row.agentId === null
          ? '（没有身份名的那一支）'
          : (ownedAgents.find((each) => each.id === row.agentId)?.displayName ??
            `${row.agentId.slice(0, 8)}…`),
      )
  }

  /** 删掉清单里的一条（**销毁式**）。正被指定 ⇒ 服务拒绝（封闭词表里那句人话）。 */
  const removeEntry = async (id: string) => {
    if (entry === null) return
    const saved = await run('删除', () => deleteHomeSource(id, entry), () => setRemoving(null))
    if (saved) {
      setNotice({ text: `已删除：那一条从${whereLabel === '本机' ? '这台机器' : whereLabel}的清单里拿掉了。`, probe: null, saved: true })
    }
  }

  /**
   * 照着**这一家自己的模型目录**换模型名（刀2b：模型目录还原真功能）。
   *
   * ⚠ 换模型名**必须重问上下文长度**（那是随模型走的声明）：所以这一格**留空**再存 ——
   * 服务会顺手向那一家问一次新的窗口（问不到就不保存，并把为什么摆在屏幕上）。
   * 密钥留空 = 沿用那一条已存的那把。
   *
   * ★ **「这个模型能看图」也随模型走**（2026-10-17）：它跟窗口同族，但**没有可重问的来源**
   * （`GET /models` 不报模态）⇒ 那一条判据落在 `api/homeConfig.ts` 的 `modelChangeDraft`
   * （显式回到"不支持" ＋ 回执请人重新勾），**不在这里就地拼**。判据有离线用例。
   */
  const useModel = async (id: string, name: string) => {
    if (entry === null) return
    const target = entries.find((each) => each.id === id)
    if (!target) return
    // ★ 这一格**在改之前**是不是开着（落盘的 ＋ 表单上刚勾的都算）：回执要如实说"它被关掉了"。
    //   ⚠ 只算落盘那一半会漏掉一种静默：人刚勾上、还没保存就去点目录换型号 —— 那个勾会被丢掉。
    const hadVision = target.vision === true || draft.vision === true
    // ⚠ 目录这条路**不走草稿**（它自己拼体），所以"随模型走的那几格"由 `modelChangeDraft` 一处说。
    const saved = await run('换模型', () =>
      updateHomeSource(id, modelChangeDraft(target, name), undefined, entry),
    )
    if (!saved) return
    // ★ **界面 = 落盘**（2026-10-17）：这一次真写下去的几格拨回表单。
    //   ⚠ 从前这里一格都不拨 ⇒ 表单还摆着**旧型号名**（下一次点保存就把刚换的型号写回去）
    //   与旧窗口数字，而回执已经在说"已改成 X" —— 页面在自相矛盾。
    const written = saved.entries.find((each) => each.id === id)
    setDraft((current) => ({
      ...current,
      model: name,
      contextWindow: written?.contextWindow == null ? '' : String(written.contextWindow),
      vision: written?.vision === true,
    }))
    setNotice({
      text: hadVision
        ? `已改成「${name}」：上下文长度重新问过一次；「这个模型能看图」随型号走，已经回到不支持 —— 要用就重新勾一下。`
        : `已改成「${name}」：上下文长度重新问过一次。`,
      probe: saved.probe ?? null,
      saved: true,
    })
  }

  /** 表单字段的统一接线（禁写 / 忙的时候一律只读）。
   *
   * ⚠ 它**只服务文本格**（`HomeConfigTextField`）：草稿里还有一格不是字符串（`vision`，布尔），
   * 它由**勾选框**自己接线——见那个类型的文档。 */
  const field = (key: HomeConfigTextField) => ({
    value: draft[key],
    disabled: !canWrite || busy !== null,
    onChange: (event: ChangeEvent<HTMLInputElement>) =>
      setDraft((current) => ({ ...current, [key]: event.target.value })),
  })

  /**
   * 左栏选一家：**只改"在配哪一家"**，不写任何东西（写发生在保存那一刻）。
   *
   * 这一家的落脚点（`entryId`）：
   * - 这一家**已经有条目** ⇒ 落到**第一条**（多数时候也只有一条）—— 点一下就该看见它的参数，
   *   而不是一张空表单；
   * - 这一家还没配过 ⇒ `entryId: null` + 地址预填（预设给模板，「自定义」留空要自己填）。
   *
   * ⚠ **换一家就把钥匙清掉**：一家的一把钥匙不该被悄悄递给另一家。
   * 代价是"点错了再点回来要重贴一次" —— 那是该付的代价。
   */
  const selectProvider = (providerId: string) => {
    const existing = byProvider.get(providerId) ?? []
    const first = existing[0] ?? null
    setEditing({ providerId, entryId: first?.id ?? null })
    if (first) {
      setDraft(draftOf(first))
    } else {
      const preset = presetById(providerId)
      setDraft({
        ...EMPTY_DRAFT,
        name: preset?.name ?? '',
        endpoint: preset?.endpointTemplate ?? '',
      })
    }
    setActionError(null)
    setNotice(null)
    setWindowProbeFailed(false)
    setProbeState({ kind: 'idle' })
    setRemoving(null)
  }

  /**
   * 这一家里**再新加一条**（同一家两个 key / 两个模型那种）。
   *
   * ⚠ 它**不是**主路径 —— 主路径是"点左边一家就配它"。这是面板里那颗不显眼的次要按钮，
   * 因为它解决的是罕见边角（一家多条），不该把主路径变复杂。
   */
  const addAnotherInProvider = () => {
    if (editing === null) return
    const preset = presetById(editing.providerId)
    setEditing({ providerId: editing.providerId, entryId: null })
    setDraft({
      ...EMPTY_DRAFT,
      name: preset?.name ?? '',
      endpoint: preset?.endpointTemplate ?? '',
    })
    setActionError(null)
    setNotice(null)
    setWindowProbeFailed(false)
    setProbeState({ kind: 'idle' })
  }

  /** 在这一家里换成另一条（一家多条时用）。 */
  const openEntryInProvider = (entry: HomeSourceEntry) => {
    if (editing === null) return
    setEditing({ providerId: editing.providerId, entryId: entry.id })
    setDraft(draftOf(entry))
    setActionError(null)
    setNotice(null)
    setWindowProbeFailed(false)
    setProbeState({ kind: 'idle' })
  }

  /**
   * ★ **点左栏一条钥匙 ⇒ 配这一条**（2026-10-15：左栏的主语换成了"这台机器上的钥匙"）。
   *
   * 它做两件事：把"在配哪一家"落到这一条所属的那一家（一家多条时右侧才知道摆哪些兄弟），
   * 再把这一条**本身**选上 —— 于是右侧进来就是**这一条的参数**（不是一张空表单）。
   */
  const openEntry = (each: HomeSourceEntry) => {
    const providerId = presetIdForEndpoint(each.endpoint) ?? CUSTOM_PRESET_ID
    setEditing({ providerId, entryId: each.id })
    setDraft(draftOf(each))
    setAdding(false)
    setActionError(null)
    setNotice(null)
    setWindowProbeFailed(false)
    setProbeState({ kind: 'idle' })
    setRemoving(null)
  }

  /**
   * **获取模型列表**（`POST /home-config/probe`）：拿这把钥匙去那一家问一次。
   *
   * ⚠ 它**不写任何文件** —— 所以"贴一把假 key 试一下"是安全的；要落盘得走 [`saveDraft`]。
   * 拒绝（形状不对 / 没给钥匙）落到 `refused`；连不上、对方回 401 都是 `reading`
   * （**读数不是错误**：它们不影响后面的保存）。
   */
  const fetchModels = async () => {
    if (entry === null) return
    if (!draft.endpoint.trim() || probeState.kind === 'busy') return
    setProbeState({ kind: 'busy' })
    setActionError(null)
    try {
      // 这一刻选中的是哪家（预设 id）：门拿它查原生目录表——查不中自动退兼容端点。
      const providerId = editing?.providerId
      const probe = await probeHomeSource(
        {
          endpoint: draft.endpoint,
          apiKey: draft.apiKey,
          ...(draft.protocol ? { protocol: draft.protocol } : {}),
          ...(providerId && providerId !== 'custom' ? { provider: providerId } : {}),
        },
        entry,
      )
      setProbeState({ kind: 'reading', probe })
    } catch (error) {
      setProbeState({
        kind: 'refused',
        message:
          error instanceof HomeConfigError
            ? error.message
            : `${whereLabel}的服务没有响应（获取模型列表失败）`,
      })
    }
  }

  // ⚠ **认不出那台机器 ⇒ 如实说，绝不回落本机**（见文件头那条判据）。
  //    它必须排在**所有 hook 之后**（hooks 不许在条件分支里），所以只能放在这里。
  if (entry === null) {
    return (
      <div className="flex h-full min-h-0 flex-col bg-background">
        <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
          <Button variant="ghost" size="icon" onClick={goBack} aria-label="返回我的机器">
            <ArrowLeft className="size-4" />
          </Button>
          <div className="text-title">模型配置</div>
        </header>
        <div className="flex flex-1 items-center justify-center px-6 text-center">
          {homesSettled ? (
            <div className="space-y-2">
              <p className="text-body">没有这台机器</p>
              <p className="text-caption text-muted-foreground">
                地址里那台机器不在这份列表里（可能已被移除，或地址写错了）—— 所以这里什么都不配，
                <strong>也不会转去配本机</strong>。
              </p>
              <Button variant="outline" size="sm" onClick={() => navigate('/app/runtimes')}>
                回到我的机器
              </Button>
            </div>
          ) : (
            <p className="text-body">正在读取你的机器…</p>
          )}
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
        <Button variant="ghost" size="icon" onClick={goBack} aria-label="返回这台机器">
          <ArrowLeft className="size-4" />
        </Button>
        <div>
          <div className="text-title">模型配置</div>
          {/* ★ 主语是**这台机器**（2026-10-15 主人裁）：这一页只说"这台上有哪几把钥匙"；
              "哪一支用哪条 / 用哪个型号"归那一支自己的页面 —— 指路只指真能走的路。 */}
          <div className="text-caption text-muted-foreground">
            {entry.local
              ? '这台机器上配好的钥匙（清单）。谁用哪把钥匙、用哪个型号 —— 在那一支自己的页面上选'
              : `${entry.name}（${entry.baseUrl}）上配好的钥匙（清单）。谁用哪把钥匙、用哪个型号 —— 在那一支自己的页面上选`}
          </div>
        </div>
      </header>

      {/* ⚠ **"配哪台机器"那一行没有了**（2026-10-11）：**地址就是那台机器**
          （`/app/runtimes/:entryId/config`）—— 留着它等于让页面上同时有两个答案。
          要配另一台：回机器列表，进那一台。 */}

      {/*
        ⚠ **页面级两层**（2026-09-23 二次回炉，形状回到 2026-09-02 那版 `15f1fae`）：
        左栏**常驻**（一屏供应商，点谁配谁）＋ 右侧那一家自己的配置面板。
        **「加一条」这个动作没有了** —— 左栏每一项本身就是入口（主人原话："没有加一条的这种
        选项……以前那个页面就很人性化，现在太笨重"）。

        跟旧版的两处必要差异（旧版底下是 fixtures 假数据）：
        1. 左栏列的是**这台机器上真能配的那些家**：清单里已存过的 ＋ 预设表里可加的；
        2. 每行带**真状态**（正在使用 / 已保存 / 未配置）—— 旧版是 `hideStatus` 全隐。
      */}
      <div className="flex min-h-0 flex-1">
        <aside className="hidden w-72 shrink-0 flex-col border-r border-border bg-card md:flex">
          {/* ★ **左栏的主语是"这台机器"**（2026-10-15 主人裁：模型配置归机器侧）。
              常态：这台机器上的钥匙（一条一行）；「加一把钥匙」那一步才摆预设表
              —— 模板库回到它该在的位置（**一个选择器**，不是常驻的"目录"）。 */}
          {adding ? (
            <div className="flex min-h-0 flex-1 flex-col">
              <div className="flex items-center gap-2 border-b border-border px-3 py-2">
                <span className="text-caption text-muted-foreground">选一家（起点，保存才算配上）</span>
                <Button variant="ghost" size="sm" className="ml-auto" onClick={() => setAdding(false)}>
                  取消
                </Button>
              </div>
              <ProviderSidebar
                rows={sidebarRows}
                selectedId={editing?.providerId ?? null}
                boundProviderId={boundProviderId}
                onSelect={selectProvider}
              />
            </div>
          ) : (
            <MachineKeyList
              entries={entries}
              selectedId={editing?.entryId ?? null}
              usersOf={(id) => usersOfEntry(id)}
              unreadable={view?.sourcesState === 'unreadable'}
              readable={view !== null}
              onAdd={() => setAdding(true)}
              onOpen={(each) => openEntry(each)}
            />
          )}
        </aside>

        <main className="min-w-0 flex-1">
          <ScrollArea className="h-full [&_[data-radix-scroll-area-viewport]>div]:!block">
            <div className="mx-auto w-full max-w-3xl space-y-6 px-4 py-6 md:px-6">
              {reading.kind === 'loading' && (
                <div className="flex items-center justify-center gap-2 py-16 text-caption text-muted-foreground">
                  <Loader2 className="size-4 animate-spin" aria-label={`正在读取${whereLabel}的配置`} />
                  正在读取{whereLabel}的配置…
                </div>
              )}

              {reading.kind === 'unreachable' && (
                <section className="space-y-3 rounded-xl border border-border bg-card p-5">
                  <div className="flex items-center gap-2">
                    <AlertTriangle className="size-4 text-presence-timeout" aria-hidden />
                    <span className="text-body">读不到{whereLabel}的配置</span>
                  </div>
                  <p className="text-caption text-muted-foreground">
                    这一页配的是<strong>{whereLabel}</strong>的模型配置。联系不上它时这里读不到、
                    也存不下 —— 这不是 Agent 出问题了。
                  </p>
                  {entry.local && (
                    <div className="space-y-2">
                      <CommandLine command="node home.js up" note="起这台机器上的世界（仓根跑）。" />
                      <CommandLine
                        command="node home.js doors"
                        note="只起本机数据服务（读口 ＋ 写口，一个进程）。"
                      />
                    </div>
                  )}
                  <Button variant="outline" size="sm" onClick={() => void load()}>
                    <RefreshCw className="mr-1.5 size-3.5" />
                    重试
                  </Button>
                </section>
              )}

              {/* ★ **右侧只在两种时候摆表单**（2026-10-15）：正在看**一条已有的钥匙**，
                  或者正在**加一把**（`adding`）。别的时刻（这台机器上一条钥匙都没有）摆的是
                  **一句话 ＋ 一个出口** —— 凭空摆一张"这一家的空表单"会让人以为已经在配某一家了。 */}
              {reading.kind === 'loaded' && view && editing !== null && editingEntry === null && !adding && (
                <section className="space-y-3 rounded-xl border border-border bg-card p-5">
                  <div className="text-body">
                    {entries.length === 0 ? '这台机器上还没有配好的钥匙' : '左边点一条，看它的参数'}
                  </div>
                  <p className="text-caption text-muted-foreground">
                    这一页配的是<strong>{whereLabel}</strong>自己的钥匙串（端点 / 密钥 / 这条的默认型号 /
                    上下文长度 / 这个模型能不能看图）。谁用哪把钥匙、用哪个型号，在**那一支自己的页面**上选。
                  </p>
                  <Button variant="outline" size="sm" className="gap-1.5" onClick={() => setAdding(true)}>
                    <Plus className="size-3.5" />
                    加一把钥匙
                  </Button>
                </section>
              )}

              {reading.kind === 'loaded' && view && editing !== null && (editingEntry !== null || adding) && (
                <>
                  {/* 清单 / 指定的**读不出来**：这是"配置坏了"，不是"没配"——两种处境的话不一样。 */}
                  {view.state === 'unreadable' && (
                    <p
                      role="alert"
                      className="rounded-lg border border-destructive/40 bg-card px-3 py-2 text-caption text-destructive"
                    >
                      {whereLabel}这一格配置在场，但读不出来 —— Agent 起不来就是因为这个。
                      {view.reason === 'binding_dangling'
                        ? '某一支指定的那一条在清单里找不到了（可能刚被删掉）：在它的页面上重新选一条。'
                        : '修好它，或者去那一支的页面上重新选一条。'}
                    </p>
                  )}

                  {!canWrite && (
                    <p className="rounded-lg border border-border bg-muted px-3 py-2 text-caption text-muted-foreground">
                      <strong>演示模式</strong>：只能看，不能存；要改先登录。
                    </p>
                  )}

                  {actionError && (
                    <p
                      role="alert"
                      className="rounded-lg border border-destructive/40 bg-card px-3 py-2 text-caption text-destructive"
                    >
                      {actionError}
                    </p>
                  )}

                  {/* ⚠ **回执挪到主体之下**（2026-10-16 主人当次："还是很混乱…太晦涩了"）：
                      从前它排在**最上面**，于是一进这一条，第一眼是"上一次动作的回执 ＋ 一坨试连
                      拉回来的模型芯片"，而**主体**（接口地址 / 密钥 / 这条的默认型号 / 上下文长度）
                      被顶到屏幕外。判据：**先说这一条是什么，再说刚才那一下的结果**。
                      ⚠ `actionError`（失败）不挪 —— 失败必须在最显眼处。 */}
                  {/* 0 住户：这一页管的**钥匙串**照旧配得下去，而"谁用哪一条"这一页不说 ——
                      说清为什么（免得人以为页面坏了），并给一条真路。 */}
                  {noResident && (
                    <p className="text-caption text-muted-foreground">
                      这台机器上还没有 Agent —— 先让一支住进来（在「我的 Agent」里创建，
                      再在它自己的页面上点「接到这台机器上」），然后**在它的页面上**选用哪一条。
                      现在配的都是这台机器自己的钥匙。
                    </p>
                  )}

                  {/* ---- 这一家的配置面板（面板头承担身份 + 状态 + 保存 —— 旧版那个形状）---- */}
                  <ProviderPanel
                    rows={sidebarRows}
                    providerId={editing.providerId}
                    providers={byProvider.get(editing.providerId) ?? []}
                    editingEntry={editingEntry}
                    inUseIds={inUseIds}
                    inUseNames={inUseNames}
                    view={view}
                    draft={draft}
                    canWrite={canWrite}
                    busy={busy !== null}
                    showKey={showKey}
                    setShowKey={setShowKey}
                    field={field}
                    setProtocol={(protocol) => setDraft((current) => ({ ...current, protocol }))}
                    onVision={(vision) => setDraft((current) => ({ ...current, vision }))}
                    autoWindow={autoWindow}
                    tableWindow={tableWindow}
                    windowProbeFailed={windowProbeFailed}
                    preset={selectedPreset}
                    probeState={probeState}
                    onFetchModels={() => void fetchModels()}
                    onPickModel={(name) => setDraft((current) => ({ ...current, model: name }))}
                    onSave={() => void saveDraft()}
                    enableOnSave={residentCount === 1}
                    usersOf={(id) => usersOfEntry(id)}
                    onNewEntry={addAnotherInProvider}
                    onOpenEntry={openEntryInProvider}
                    onRemoveEntry={setRemoving}
                    removing={removing}
                    onConfirmRemove={(id) => void removeEntry(id)}
                    onCancelRemove={() => setRemoving(null)}
                    onReload={() => void load()}
                  />

                  {/* ★ **刚才那一下的结果**（回执 ＋ 试连读数 ＋ 这条能调哪些型号）——
                      排在**主体之后**（主体先说"这一条是什么"）。 */}
                  {notice && (
                    <div className="space-y-1 rounded-lg border border-presence-online/40 bg-card px-3 py-2 text-caption">
                      <p className="text-presence-online">{notice.text}</p>
                      {notice.probe && (
                        <p className={notice.probe.ok ? 'text-presence-online' : 'text-muted-foreground'}>
                          连接测试：{probeText(notice.probe)}（{notice.probe.url}）
                          {notice.saved
                            ? notice.probe.ok
                              ? ' ✓'
                              : ' —— 这一家现在连不上，但配置已经保存了'
                            : ''}
                        </p>
                      )}
                      {notice.saved &&
                        notice.windowValue != null &&
                        (notice.windowSource === 'probed' || notice.windowSource === 'table') && (
                          <p className="text-muted-foreground">
                            {notice.windowSource === 'table'
                              ? `上下文长度 ${notice.windowValue} 来自随包那份对照表 —— 不是对方说的，你可以改`
                              : `服务方给的上下文长度：${notice.windowValue}`}
                          </p>
                        )}
                      {/* **模型目录**（刀2b）：这一条能调哪些型号 —— **默认收起**（见 `ModelCatalog`）。 */}
                      {notice.saved && notice.probe !== null && view.entryId !== undefined && (
                        <ModelCatalog
                          entryId={view.entryId}
                          models={notice.probe.models}
                          kinds={notice.probe.modelKinds}
                          disabled={!canWrite || busy !== null}
                          onPick={useModel}
                        />
                      )}
                    </div>
                  )}

                  <p className="text-micro text-muted-foreground">
                    改动只影响<strong>下一次活动</strong>：正在进行的那一次不受影响。改完不用重启。
                  </p>
                </>
              )}
            </div>
          </ScrollArea>
        </main>
      </div>
    </div>
  )
}

/**
 * **短名**：列表里只显示名字的**第一段**（`·` 分段；再长的用括号/破折号截）。
 *
 * 为什么：主人把备注写进了名字（"OpenCode Go · DeepSeek V4.1 Flash（同端点同钥匙；2026-10-14
 * 因 ms-1 偶发超时接入）"）—— 那是**他的数据**，页面不该替他改，但**列表**该只给一句能认出来的
 * 短名（全名在详情里一字不改地显示，编辑框里也是完整的）。
 */
function shortKeyName(name: string): string {
  const head = name.split('（')[0]?.split('(')[0] ?? name
  return head.length > 28 ? `${head.slice(0, 28)}…` : head.trim()
}

/**
 * ★ **左栏：这台机器上的钥匙**（2026-10-15 主人裁：模型配置归机器侧）。
 *
 * 一行 = **清单里的一条**（一台机器上的一把钥匙）—— 名字 · 默认型号 · 窗口 · **谁在用**。
 * 三个判据是有意这么排的：
 *
 * 1. **第一行是"这台机器上有什么"**（机器事实），不是"世上还有哪些家"（模板库）——
 *    后者只在「加一把钥匙」那一步出现（一个选择器）；
 * 2. **"谁在用"是读数不是动作**（写名字；没人用就写"没人用"）—— 换绑归**那一支**的页面，
 *    因为只有那里点得名、说得清"给谁换"（这一页从前那颗「换成这条」不带主语：一次换每一支）；
 * 3. **读不到就说读不到**（清单读不出来 / 还没读到）—— 不许画一份空清单冒充"这台机器上什么都没有"。
 */
function MachineKeyList({
  entries,
  selectedId,
  usersOf,
  readable,
  unreadable,
  onAdd,
  onOpen,
}: {
  entries: HomeSourceEntry[]
  selectedId: string | null
  usersOf: (id: string) => string[]
  /** 读到过清单了吗（`false` = 还在读/读不到）。 */
  readable: boolean
  /** 清单在场但读不出来（那是"配置坏了"，不是"没配"）。 */
  unreadable: boolean
  onAdd: () => void
  onOpen: (entry: HomeSourceEntry) => void
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center gap-2 border-b border-border px-3 py-2">
        <span className="text-caption text-muted-foreground">
          这台机器上的钥匙{readable && !unreadable ? `（${entries.length}）` : ''}
        </span>
        <Button variant="ghost" size="sm" className="ml-auto gap-1" onClick={onAdd}>
          <Plus className="size-3.5" />
          加一把钥匙
        </Button>
      </div>
      <ScrollArea className="min-h-0 flex-1">
        <div className="space-y-1 p-2">
          {unreadable && (
            <p className="px-2 py-3 text-caption text-destructive">
              这台机器上的钥匙清单读不出来 —— 先修它（Agent 起不来就是因为这个）。
            </p>
          )}
          {!unreadable && readable && entries.length === 0 && (
            <p className="px-2 py-3 text-caption text-muted-foreground">
              这台机器上还没有配好的钥匙 —— 点右上角「加一把钥匙」。
            </p>
          )}
          {!unreadable && !readable && (
            <p className="px-2 py-3 text-caption text-muted-foreground">正在读这台机器的清单…</p>
          )}
          {!unreadable &&
            entries.map((each) => {
              const users = usersOf(each.id)
              return (
                <button
                  key={each.id}
                  type="button"
                  aria-pressed={selectedId === each.id}
                  onClick={() => onOpen(each)}
                  className={cn(
                    'w-full space-y-1 rounded-lg border border-transparent px-3 py-2 text-left hover:bg-muted',
                    selectedId === each.id && 'border-border bg-muted',
                  )}
                >
                  {/* ★ **名字只显示第一段**（2026-10-16）：主人把备注写进了名字
                      （"OpenCode Go · DeepSeek V4.1 Flash（同端点同钥匙；2026-10-14 …）"）⇒
                      左栏被截成"（同"，而详情里又整段重复。判据：**列表用短名，全名在详情里**。 */}
                  <span className="block truncate text-body">{shortKeyName(each.name)}</span>
                  {/* ★ **型号带前缀**：两行都写 `deepseek-v4.1-flash`（连着 Mimo 那条）时，
                      读者会以为配错了 —— 加上"型号"两个字，它就是**那一格**的值，不是名字的重复。 */}
                  <span className="block truncate text-caption text-muted-foreground">
                    型号 <code className="font-mono">{each.model}</code>
                  </span>
                  <span className="block truncate text-micro text-muted-foreground">
                    {users.length > 0 ? `${users.join('、')} 在用` : '没人用'}
                  </span>
                </button>
              )
            })}
        </div>
      </ScrollArea>
    </div>
  )
}

/**
 * **左栏：一屏供应商**（2026-09-23 二次回炉，形状回到 2026-09-02 那版 `15f1fae`）。
 *
 * 一行 = **一家**（判据在 `api/presets.ts` 的 `groupEntriesByProvider`：地址相等 ⇒ 同一家）。
 * 点一行 = **配这一家** —— 这就是这一版跟"加一条按钮"的区别：**入口本身是清单**，
 * 不需要先声明"我要加东西"。
 *
 * 三档状态各说各的话（判据是 `providerRowState`，不是组件自己猜的）：
 * - 「正在使用」= 这一家里有那条被指定的（最强，绿）；
 * - 「已保存」= 这一家里有条目、但没被用着（中性）；
 * - 不标 = 这一家在这台机器上还没有条目（点了就是开始配它）。
 *
 * ⚠ **不写任何东西**：点一行只改"在配哪一家"，写发生在保存那一刻。
 */
