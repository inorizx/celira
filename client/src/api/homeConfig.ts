/**
 * 本机**模型配置门**的读写层（TASK_77；刀2b 起管"清单 + 绑定"两个事实）。
 *
 * 门是 `node home.js up` 起的**窄写口** —— **家的门**（`node home.js doors`）的写端面：它管的是**执行端的
 * 私有事实**——Agent 思考用的那个模型的端点 / 钥匙 / 模型名 / 窗口。刀2b 之后它是两件事、两个文件
 * （原理：**钥匙串归房子，用哪把钥匙归住户**）：
 *
 * | 事实 | 归属 | 文件 |
 * |---|---|---|
 * | 这台机器上有哪些钥匙（**清单**） | 机器 | `<家>/model_sources.json` |
 * | 这一个 Agent 用哪一条（**绑定**） | Agent | `<环境根>/model_binding.json` |
 *
 * 本层只负责"取回来 / 送出去 + 形状守卫"，不负责呈现，也**不负责**判断该不该写。
 *
 * 六条纪律（与状态口那份对齐，另加两条是"写"带来的）：
 *
 * - **形状守卫 fail-closed**：认不出的响应一律当"读不到"，绝不半读半猜（读错一份配置比读不到
 *   更坏：页面会拿它当事实画给你看，而人照着错的去改）；**三合一缺一层也不接受**；
 * - **地址是同源相对路径**（本机那条）：页面**永远不知道口子开在哪个端口**——发布形态由
 *   `serve_client.mjs --config-port` 代转，源码形态由 `vite.config.ts` 的同一个键代转；
 *   **家口远程访问刀**起：也可以按**家条目**问一台远程的机器（绝对地址 + Bearer 家钥匙），
 *   路由与请求头都在 `homes.ts` 一处算（本机那个形状一个新字符都没变）；
 * - **不进世界侧传输层（`WorldApi`）、不随连接模式切换、不经 `getApi()`**：它不是公共世界里的事实，是**本机**的
 *   一格配置；演示通道里 `getApi()` 是 null，而这一页照样要能读本机的读数；
 * - **错误码是封闭表**（与门侧 `scripts/home/config-server.js` 的 `ERRORS` 逐字对齐）：
 *   门说了一个我们不认识的码 ⇒ **明确拒绝**（`.code` 原样带出来），不拿一句"保存失败"糊过去；
 * - **凭据不过手**：这份形状里**没有 key**（门永不回它），写入时它只作为请求体的一个字段
 *   过一次，本层不缓存、不落存储、不进 DOM；
 * - **动作语义分开**（刀2b）：新建 = **保存即启用**（`createHomeSource`）；改一条 = **不动绑定**
 *   （`updateHomeSource`）；换绑 / 解绑 = `bindHomeSource`；删一条 = **销毁式**（`deleteHomeSource`，
 *   那一条里的钥匙只有这一份）。
 */

import type { ModelKind } from './modelCatalog.ts'
import { sendRequest } from './transport.ts'
// **时刻那一格的人话**（中立 helper：读不出来一律给 `—`，绝不 `new Date()` 兜底成"现在"）。
import { formatRuntimeTime } from '../lib/time.ts'
import {
  configRequestUrl,
  homeRequestHeaders,
  localHomeEntry,
  type HomeEntry,
} from './homes.ts'

/** 门的**同源相对基址**（与 `/home-status` 并列：一眼能看出它属于"家"这一层）。 */
export const HOME_CONFIG_URL = '/home-config'

/**
 * **换绑 / 解绑**那一条路径（刀2b）。读绑定走 `GET /home-config` 的 `bound` 那一格 ——
 * 这里只写（别处不开第二条读面）。
 */
export const HOME_BINDING_URL = '/home-binding'

/**
 * **只读探测**那一条路径（2026-09-23 预设供应商层）。
 *
 * 它回答的是"**用这把钥匙，那一家肯不肯答**"——即"选供应商 → 贴 key → 获取模型列表"这一步。
 * **它不写任何东西**：探测即测试，保存（`POST /home-config`）才落盘。所以这条路径返回的
 * 是一份**试连读数**，不是 `HomeConfigView`（也就不会让人误以为"探过一次 = 存过一条"）。
 */
export const HOME_PROBE_URL = '/home-config/probe'

/**
 * ★ **"这条钥匙能调哪些型号"**那一条路径（2026-10-14）。
 *
 * 与 [`HOME_PROBE_URL`] 问对方的是**同一件事**（`GET <基址>/models`），唯一的差别是**钥匙从哪儿来**：
 * 探测那一格按纪律**只从请求体来**（"用**这一把**试一试"）；这一格反过来 —— 请求体里只有条目 `id`，
 * 钥匙用清单里**存着的那把**（页面手里**永远没有**它：key 只到那台机器为止、永不回浏览器）。
 *
 * 为什么必须有它：没有这一格，"这台机器上这把钥匙支持哪些型号"就只能靠人**重贴一次钥匙**才看得见，
 * 而"在 Agent 页选具体型号"这件事正需要那份候选。
 *
 * 三条纪律与探测**逐条相同**：**只读**（不落盘）、**响应里没有 key**、**连不上是读数不是错误**。
 */
export const HOME_CATALOG_URL = '/home-config/catalog'

/**
 * **思考档的封闭表**（支级覆盖那一格；2026-10-14）。
 *
 * ⚠ **权威是运行时的读侧**（`agent/src/model_source.rs` 的 `REASONING_EFFORTS`）；门把它镜像成
 * 写入口的预检，这一份是**页面画选项**用的。三处对表由两条用例钉住：门侧那份在本仓的
 * `scripts/home/tests/model-sources-binding.test.js`（连运行时与壳一起比三处），
 * 客户端这份在 `tests/home-config.test.ts`（与门那份逐字比）——**改一处不改另一处会红**。
 */
export const REASONING_EFFORTS: readonly string[] = [
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
]

/**
 * 一次读 / 写的超时。
 *
 * 写那一次比读慢：门收到体之后要**原子写文件 + 试连一次**（`GET <基址>/models`），
 * 试连自己的窗口是 8 秒 ⇒ 这里必须比它宽，否则页面会在门还在等供应商回话时先掐断，
 * 显示一个"没保存上"——而它其实存上了（那是**撒谎**，不是降级）。
 */
const READ_TIMEOUT_MS = 12000
const WRITE_TIMEOUT_MS = 20000

/** 配置的三种处境（与门侧同一套词）。 */
export type HomeConfigState = 'configured' | 'absent' | 'unreadable'

/**
 * 门侧那份**稳定错误词表**的镜像（`scripts/home/config-server.js` 的 `ERRORS`）。
 *
 * ⚠ 两侧**逐字对齐**：一个码在这里找不到，页面就不知道该怎么跟人说话 ⇒ 它会明确说
 * "不认识的错误码"，而不是编一句听起来合理的话。这就是"封闭表"在这里的全部意思。
 */
export const HOME_CONFIG_ERROR_CODES: readonly string[] = [
  'not_found',
  'method_not_allowed',
  'origin_not_allowed',
  'bad_request',
  'body_too_large',
  'invalid_endpoint',
  'invalid_model',
  'invalid_window',
  'invalid_protocol',
  // 2026-10-17：**这一份模型收不收得到媒体材料**那一格不是布尔（三态里"说了"的那两态之外）。
  'invalid_vision',
  // 刀2b：清单条目那三个"身份 / 名字"的形状错误（与门侧同一次改动）。
  'invalid_id',
  'invalid_name',
  'invalid_bind',
  // 刀2b：条目不在场 / 条目正被绑定（删它之前得先换绑）。
  'entry_not_found',
  'entry_bound',
  // 2026-10-12：这台机器上**一支住户都没有** ⇒ 换绑/解绑没有目标（不许静默成功）。
  'no_resident',
  // 2026-10-12：带了 `agentId` 而这一支不在这台机器上 ⇒ 明确拒绝，不退回"换每一支"。
  // ⚠ **2026-10-14 起本版的门不再产生这个码**（点名 + 形状合法就落在它的环境根上，
  // 见 `bindHomeSource`）；它留在表里是因为**旧版的门**（远程那台还跑着老版本）还这么答。
  'no_such_resident',
  // 2026-10-14：`agentId` 的形状不合法（它会变成一个目录名 ⇒ 空/空白/超长/带 `/` `\` `:` 都拒）。
  'bad_agent_id',
  // 2026-10-14：支级覆盖那一格（思考档）不在封闭表里 —— 认不得的取值门侧不收
  // （写进去的值会让宿主拒绝启动）。
  'invalid_effort',
  // ★ 2026-10-15 远程接上那一刀：这台机器**没允许**从外面接 Agent（家钥匙只回答"能不能进这台
  // 机器"，"允许我替你起进程"由那台机器上的一个开关单独授权，缺省关）。
  'remote_control_off',
  // ★ 同一刀：那只手跑完了，而那一支**没被接上**（"为什么"在 `lines` 里原样带出来）。
  'attach_failed',
  // TASK_77 回炉③：窗口留空、而试连也没能问出来（连不上 / 接口没给那一格）⇒ 这时才要人填。
  'window_unavailable',
  // 2026-09-23：**钥匙没被对方认**（401 / 403）—— 与上面那条**刻意分开**：
  // 两者要用户做的事**相反**（前者去换钥匙，后者才该手填窗口）。合并成一个码就是把方向指错。
  'credential_rejected',
  // 家口远程访问刀：**家钥匙**没带 / 不对（模式二的 401）—— 与 `credential_rejected`
  // （模型供应商那把）**也是两回事**：这把是"进这台机器"的钥匙，归属与补救都不同。
  'unauthorized',
  'key_required',
  'read_failed',
  'save_failed',
]

/** 词表里的人话（**只翻译，不发明**）：每一条都是"为什么被拒绝"的直译。
 *
 * ⚠ 这些句子**会原样渲染在页面上** ⇒ 两条纪律：
 * 1. 不许出现 markdown 记号（写 `**强调**` 就会显示成星号）；
 * 2. **不许用工程黑话**（人话刀 2026-09-22）：门 / 供应商 / 标称窗口 / 线协议 / 这一格
 *    这些词都换成了用户读得懂的说法。**码本身（键名）一个字都没动** ——
 *    两侧封闭表比的是码，不是这句话。
 */
const ERROR_TEXTS: Record<string, string> = {
  not_found: '本机配置服务不认这条路径（可能是版本不一致）',
  method_not_allowed: '本机配置服务不接受这个动作',
  origin_not_allowed: '这个来源不被允许（只允许从本机访问）',
  bad_request: '请求读不懂（页面与本机服务版本不一致？）',
  body_too_large: '这份配置太大了（它只有几项）',
  // **不是门说的码**，是我们这一侧发不出去（连不上 / 超时取消）。放在同一个表里，
  // 是为了让界面只需要问一次"这个码怎么讲" —— 它照旧走 `known` 那条路，不会被当成"不认识"。
  network: '连不上那台机器（它没开机、地址不对，或者网络不通）',
  invalid_endpoint: '接口地址读不懂：要么是空，要么不是 http/https 地址',
  invalid_model: '模型名要么不填，要么是一段没有控制字符的文字（不超过 200 字节）',
  invalid_window: '上下文长度要么留空（自动获取），要么是一个正整数（0 不是一条可解释的声明）',
  invalid_protocol: '接口类型只认两种：chat-completions 或 responses',
  invalid_vision: '「这个模型能不能看图」要么不填，要么是 true / false',
  invalid_id: '这一条配置的编号读不懂（它长得像 ms-1 这样）',
  invalid_name: '名字得是一段文字',
  invalid_bind: '「存完要不要用它」只能是「要」或「不要」',
  entry_not_found: '这一条配置已经不在了（可能刚被删掉）—— 刷新一下看看',
  entry_bound: '这一条正在被这个 Agent 使用，先换成别的再删',
  no_resident: '这台机器上还没有住户 —— 先让一支 Agent 住进来，再指定它用哪一条',
  // ⚠ 这句要**指得出下一步**（2026-10-14）：本版的门不再回它，回它的是**旧版的门** ——
  // 那时唯一能做的就是把这一支先接上那台机器，再给它选。
  no_such_resident: '这台机器上没有这一支 —— 先把它接到这台机器上，再回来选',
  bad_agent_id: '这个 Agent 的编号读不懂（它不该带空格、斜杠或冒号）',
  invalid_effort: '思考档只认这七档：none / minimal / low / medium / high / xhigh / max',
  // ★ 远程接上那一刀的两句（都要**指得出下一步**）：
  remote_control_off:
    '这台机器没允许从别的机器接 Agent 上来 —— 在那台机器上跑一次 node home.js remote-control on 就能开（缺省是关的）',
  attach_failed: '那台机器没能把它接上 —— 下面那几行是它自己说的话',
  window_unavailable:
    '没能自动问出上下文长度（对方连不上，或它的模型列表里没有这一项）—— 这时才需要你手填',
  credential_rejected: '对方不认这把密钥 —— 先换一把能用的（密钥不对时，手填上下文长度也连不上）',
  // 家钥匙那一格（家口远程访问刀）：固定一句，与「钥匙不对」那条纪律同一句话（`homes.ts` 那份）。
  unauthorized: '钥匙不对，去那台机器上重新拿一把',
  key_required: '首次保存必须给密钥（还没有配置文件，没有可沿用的密钥）',
  read_failed: '配置文件读不出来',
  save_failed: '写不进那个配置文件（磁盘或权限？）',
}

/** 一个错误码的人话；**不认识就说不认识**（并把它原样交出来）。 */
export function homeConfigErrorText(code: string): string {
  return ERROR_TEXTS[code] ?? `本机配置服务回了一个我不认识的错误码：${code}`
}

/**
 * ★ **那台机器允不允许从别的机器接 Agent 上来**（2026-10-15 远程接上那一刀）。
 *
 * 四档，**各有各的下一步**（压成一档就是把"能开"和"开不了"说成同一件事）：
 *
 * | 档 | 意思 | 用户能做什么 |
 * |---|---|---|
 * | `on` | 允许（模式二还要家钥匙 ＋ TLS） | 接 |
 * | `off` | **这台有这条路，只是没开** | 在那台机器上跑一次 `node home.js remote-control on` |
 * | `unsupported` | **这扇门不提供这条路**（壳形态：只绑 loopback） | 换命令行形态那台，或在那台本机上接 |
 * | `unknown` | 老门 / 这一格读不到 | 先看清那台的读数（**不许**当成没开） |
 */
export type RemoteAttachReadout =
  | { kind: 'on' }
  | { kind: 'off' }
  | { kind: 'unsupported' }
  | { kind: 'unknown' }

export function remoteAttachReadout(view: HomeConfigView | null): RemoteAttachReadout {
  if (view === null) return { kind: 'unknown' }
  const value = view.remoteControl
  if (value === 'on') return { kind: 'on' }
  if (value === 'off') return { kind: 'off' }
  if (value === null) return { kind: 'unsupported' }
  return { kind: 'unknown' }
}

/** 那一档的人话（**一处拼句**；页面直接渲染）。 */
export function remoteAttachLine(readout: RemoteAttachReadout): string {
  switch (readout.kind) {
    case 'on':
      return '允许从别的机器接 Agent 上来'
    case 'off':
      return '没允许从别的机器接 Agent 上来 —— 在那台机器上跑一次 node home.js remote-control on 就能开'
    case 'unsupported':
      return '这台机器只在本机接 Agent（它不提供从外面接这一条路）'
    case 'unknown':
      return '这台机器没报这一格（版本旧，或读数没到）—— 看不清就先别动'
  }
}

/** 门拒绝时抛的这个（`code` 是门说的原话，可能不在封闭表里）。 */
export class HomeConfigError extends Error {
  readonly code: string
  /** 这个码在不在我们的封闭表里（不在 ⇒ 页面必须说"不认识"，不许编）。 */
  readonly known: boolean
  /**
   * 门拒绝时**顺手带回来的那份试连读数**。
   *
   * 目前只有 `window_unavailable` 那条路上有（门探了一次没问到窗口）——它正是"为什么没问到"
   * 的读数，页面要能把它显示出来（否则人只看到一句"要你手填"，不知道是不是网络问题）。
   */
  readonly probe: HomeConfigProbe | null
  /**
   * ★ 门拒绝时**那台机器自己说的话**（原样读数，一行都不吞）。
   *
   * 目前只有远程接上那一刀用得上：`attach_failed` 那一档，门把人话（`ERROR_TEXTS`）之外的
   * **机器原话**交出来 —— 而"为什么没接上"只在那几行里。空数组 = 这一次没有原话。
   */
  readonly lines: string[]

  constructor(code: string, probe: HomeConfigProbe | null = null, lines: string[] = []) {
    super(homeConfigErrorText(code))
    this.name = 'HomeConfigError'
    this.code = code
    this.known = HOME_CONFIG_ERROR_CODES.includes(code)
    this.lines = lines
    this.probe = probe
  }
}

/**
 * 试连读数（保存之后门替我们探的那一次）。
 *
 * ⚠ 它是**读数**不是判据：`ok: false` **不代表没保存**（门那边保存已经落定）。
 * 页面必须把这两件事分开说，否则人会把"这一家现在连不上"当成"我的配置没写进去"。
 */
export interface HomeConfigProbe {
  url: string
  ok: boolean
  /** 拿到了 HTTP 应答就是那个状态码；连不上/超时 ⇒ `null`。 */
  status: number | null
  /** `status` 为 `null` 时说明是哪一种没连上。 */
  reason: 'timeout' | 'unreachable' | null
  /**
   * **顺手读回来的标称窗口**（TASK_77 回炉③：窗口不是配置，是体检）。
   *
   * 只有 2xx 的回执才去看这一格；读到就是那个正整数，读不到就是 `null`
   * （连不上 / 目录里没有我们声明的那个模型 / 没有我们认得的那个字段名）。
   */
  contextWindow: number | null
  /** 窗口是从**哪一个字段**读到的（如实点名；没读到就是 `null`）。 */
  windowField: string | null
  /**
   * **这家有哪些模型**（刀2b：把"模型目录"还给页面挑）。
   *
   * 只有 2xx 的回执才去看这一格；读到就是那几个名字（保序、去重、封顶），
   * 读不到就是空数组 —— **它是读数，不是判据**：空数组不代表这家坏了。
   */
  models: string[]
  /**
   * **按模型名给的权威品类**（原生目录层，2026-09-24；可缺省）。
   *
   * 只有原生目录探成功才有这一格（兼容端点没有它 ⇒ 页面退回按名字判）。
   * 畸形条目**不做硬校验**：`classifyModel` 对认不出的值自动降级回名字规则 ——
   * 品类只影响标签，不该让它砸了整份读数。
   */
  modelKinds?: Record<string, ModelKind[]>
}

/**
 * 清单里的**一条**（刀2b：机器级事实 —— 这台机器上有哪些钥匙）。
 *
 * ⚠ `api_key` 不在这里：门只回"配没配"（`keyConfigured`），取值一个字节都不回浏览器。
 */
export interface HomeSourceEntry {
  /** `ms-<序号>`（由本机服务给，单调不回收）。 */
  id: string
  /** 用户给它起的名字（空的话门用模型名顶）。 */
  name: string
  endpoint: string
  model: string
  /** 上下文长度（正整数）。 */
  contextWindow: number | null
  protocol: string | null
  /**
   * **这一份模型收不收得到媒体材料**（2026-10-17）：宿主声明，**fail-closed**（不写就是不支持）。
   *
   * ⚠ 它说的是**这个（供应商, 模型）对**——不是"这条钥匙"、也不是某个住户自己的设置：
   * 同一台机器上所有住户共用这一格。
   *
   * ⚠ **可选**：老门的回执里没有这一格，那不该让整份读数不可接受（下面"缺一格就整份不接受"
   * 防的是**半读**，不是**版本差**）。
   */
  vision?: boolean
  keyConfigured: boolean
}

/** 清单的三种处境（与门侧同一套词）。 */
export type HomeSourcesState = 'absent' | 'loaded' | 'unreadable'
/** 绑定的三种处境（与门侧同一套词）。 */
export type HomeBindingState = 'absent' | 'bound' | 'unreadable'

/**
 * ★ **这一份配置最后是谁改的**（2026-10-18）—— 门那一侧记的**见证**
 * （权威在 `scripts/home/model_witness.js`；壳那一份 `app/src-tauri/src/model_witness.rs` 逐条同形）。
 *
 * 它答的是**两档，而且两档必须分开说**：
 *
 * | 档 | 它知道什么 |
 * |---|---|
 * | **从页面那条路改的** | `same` ＋ **有时刻**（`atMs`） |
 * | **别处改的**（主体走 terminal 写、直接改文件、别的工具） | `changed`：**只知"变了"，不知是谁** |
 *
 * ⇒ 判据一句话：**它答的是「页面有没有写过」，不是「有没有人写过」。**
 *
 * ⚠ **判不了必须与"没改过"分开**：`no-witness`（没有上一条见证）与 `unreadable`
 * （这一份 / 这本账读不出来）**都不是**"没改过"。
 *
 * ⚠ **它不是封印**：本地见证对拥有整机全权的主体**不是密码学保证** —— 那个主体够得着那两份文件，
 * 也够得着这本账。它给的是**改动的可见性**。
 */
export interface ModelWitnessReading {
  state: 'same' | 'changed' | 'no-witness' | 'unreadable'
  /** **门最后一次写下这一份的时刻**（epoch ms）；判不了时是 `null`。 */
  atMs: number | null
  /**
   * 更细的那一格（`null` = 没有更细的话）：
   * `changed` ⇒ `content`（内容不一样）/ `missing`（不在了）/ `appeared`（又出现了）；
   * `unreadable` ⇒ `file`（这一份读不出来）/ `ledger`（见证账本读不出来）。
   */
  detail: string | null
  /** `changed/content` 时的**字节差**（算不出来 ⇒ `null`）。 */
  deltaBytes: number | null
}

/**
 * **某一支住户**的绑定（一个家住 N 支 ⇒ 一支一份 `model_binding.json`）。
 *
 * ⚠ 门在两处给的就是这一格（`config-server.js` 与壳的 `home_doors.rs` 逐字同形），
 * 本层只是把它接进来 —— **不派生第二份**。
 */
export interface HomeResidentBinding {
  /** 这一支的 Agent id；门读不到就是 `null`（那时它归不到哪一支，见 `residentModelReadout`）。 */
  agentId: string | null
  state: HomeBindingState
  /** `state === 'bound'` 时是清单里那一条的 id。 */
  bound: string | null
  /**
   * ★ **支级覆盖：这一支具体用哪个型号**（2026-10-14）。
   *
   * `null` = **没覆盖**（用它绑的那条清单条目里的型号）。它不是"读不到"——那个由整格的
   * `state: 'unreadable'` 说。
   *
   * 为什么它在**这一支**手里：**钥匙属机器，"用什么型号 / 什么档位"归主体** —— 运行时一直
   * 这么认（`agent/src/model_source.rs` 的 `apply_binding_overrides`），只是从前没有写者。
   *
   * 可选：**老门**（远程那台跑着旧版）不给这两格 ⇒ 调用方当"没覆盖"（那一档的语义与
   * "这一支还没选过覆盖"一致，不会说反话）。
   */
  model?: string | null
  /** 支级覆盖：**思考档**（七档封闭表，见 [`REASONING_EFFORTS`]）；`null` = 没覆盖。 */
  reasoningEffort?: string | null
  /**
   * ★ **这一支那份绑定最后是谁改的**（2026-10-18）：见证跟它见证的那一份走，**不另开一张
   * 平行的表**。可选：老门不给 ⇒ 读成"读不到这一格"（**不许**当成"没改过"）。
   */
  witness?: ModelWitnessReading
}

export interface HomeConfigView {
  /** **这次读数的时刻**（门读文件那一刻）。 */
  readAt: string
  /**
   * **由绑定那一条算出来的**派生状态（不是第二份权威：它就是"Agent 这次用哪条"的投影）。
   * `configured` = 绑了且解析得出；`absent` = 没绑；`unreadable` = 悬空绑定 / 清单读不出来。
   */
  state: HomeConfigState
  /** 关于钥匙，门的回答只有这一个布尔（**取值永不回来**）。 */
  keyConfigured: boolean
  endpoint: string | null
  model: string | null
  contextWindow: number | null
  protocol: string | null
  /** `unreadable` 的机械原因（`not_json` / `no_valid_entry` / `binding_dangling` …）。 */
  reason: string | null
  /** **清单**（机器级）：这台机器上有哪些钥匙。 */
  sourcesState: HomeSourcesState
  entries: HomeSourceEntry[]
  /** **绑定**（Agent 级）：这一个 Agent 用哪一条（`bound` 就是那一条的 id）。 */
  bindingState: HomeBindingState
  /**
   * ⚠ **这是"第一支"的那一条**（门在有多支时取的第一个）：单支的家它就是唯一事实，
   * **多支的家它只是其中一支的**。逐支那一格在场时，问某一支请走 `residentModelReadout`，
   * 问"这台机器上有没有人在用"请走 `inUseEntryIds` —— **别拿这一格当全称**。
   */
  bound: string | null
  /**
   * **各支的绑定**（2026-10-12 接）：一支一份文件 ⇒ 这是"哪一支用哪一条"的**唯一逐支读数**。
   *
   * ⚠ **可选**：老门（或远程那台跑的是旧版）不给这一格。缺场时调用方退回整机 `bound`
   * （单支的家两者等价）；给坏形状 ⇒ 整份读数拒收（见 `isHomeConfigView`）。
   */
  boundPerResident?: HomeResidentBinding[]
  /**
   * ★ **还没住户化、但已经先选好了**的那几支（2026-10-14「选择先于接上」）。
   *
   * 它还没被接到这台机器上（`<家>/agents/<id>/` 里只有 `model_binding.json`，没有
   * `agent.conf` / `identity.json`），而有人已经在它的页面上给它选了哪一条 —— 选择先落盘，
   * 接它的时候就不必再问"用哪一条"（那一问正是"选哪条 = 花谁的钱，不替它决定"停下来的地方）。
   *
   * ⚠ **它与 `boundPerResident` 是两件事，不许互相顶**：那一格的主语是**住户**
   * （机器页那一列写的就是"这台机器上的住户（N）"），它还不是住户 —— 混进去就是假话。
   * 问"这一支先选好了哪条"走 [`residentModelReadout`] 的 `homed: false` 那一档。
   *
   * 可选：老门不给这一格（⇒ 那一档读成"这份读数没给这一格"，页面如实说，
   * 不许把它说成"没指定"）；给坏形状 ⇒ 整份读数拒收。
   */
  boundUnhomed?: HomeResidentBinding[]
  /** 各支绑得不一样吗（**门算的**，本层只读）。缺场 = 老门。 */
  bindingDivergent?: boolean
  /**
   * ★ **清单那一份最后是谁改的**（2026-10-18；见 [`ModelWitnessReading`]）。
   *
   * 为什么它在**顶层**：这一格的形状与 `sourcesState` / `entries` 同族 —— 那两格说的就是
   * **清单**（机器级那两份文件里的这一份），而"逐支那一份"的见证跟着**它自己那一行**走。
   *
   * 可选：老门不给 ⇒ 读成"读不到这一格"，**不许**当成"没改过"。
   */
  listWitness?: ModelWitnessReading
  /**
   * ★ **见证账本的链条断在第几条**（0 起算；`null` = 完整）。可选：老门不给。
   *
   * 为什么它要上来：账是**只追加 ＋ 链值**的，而"链值"若没人读，它就是一个装饰 ——
   * 删掉中间一条记录，正是它该显形的那件事。
   */
  witnessChainBreakAt?: number | null
  /**
   * ★ **这台机器允不允许从别的机器接 Agent 上来**（2026-10-15 远程接上那一刀）。
   *
   * 三态，**各有各的意思，不许压成两态**：
   * - `'on'` ⇒ 允许（还要家钥匙 ＋ TLS，模式二才算数）；
   * - `'off'` ⇒ **这台机器有这条路，只是没开**（在那台机器上跑
   *   `node home.js remote-control on` 就能开 —— 页面要指得出这一句）；
   * - `null` ⇒ **这扇门不提供这条路**（壳形态的机器：它只绑 loopback，只在本机接 Agent）
   *   —— 与 `'off'` **不是一回事**：那个能开，这个开不了。
   *
   * 可选：老门不给这一格（⇒ 读成"不知道"，页面如实说，**不许**当成 `'off'`）。
   */
  remoteControl?: 'on' | 'off' | null
  /** 只有刚写成功才带：这一次动的是**哪一条**。 */
  entryId?: string
  /** 只有刚写成功才带：那一条是**新建的**，还是改的。 */
  creating?: boolean
  /** 只有刚保存过才带：**窗口那一格是哪儿来的**（人填的 / 试连问来的 / 查兜底表来的）。 */
  windowSource?: 'given' | 'probed' | 'table' | null
  /** 只有刚保存过（POST / PUT / DELETE）才带：那一次试连的读数。 */
  probe?: HomeConfigProbe | null
}

/**
 * **这一支在这台机器上选的是哪一条**（"绑定"那一层的**逐支**读法）—— 一个都不压：
 *
 * `homed` 说**它在这一格里的身份**（两个行集互斥，一支要么是住户、要么还没被接上）：
 * - `true`（缺省）：它是**住户** ⇒ 读 `boundPerResident`（机器页与它主页「机器」那一间的默认问法）；
 * - `false`：它**还没被接到这台机器上** ⇒ 读 `boundUnhomed`（"先给它选好的那条"）。
 *
 * ⚠ 这**不是同一个问题的两个答案**：两个行集的分工由门定（住户 / 还没住户化），
 * 而且**一次只读一格** —— 页面按"它住没住下"选哪一档，从不两格都读。
 *
 * | kind | 意思 | 调用方该说 |
 * |---|---|---|
 * | `unlisted` | 这门**没报这一格**（老门 / 远程那台是旧版） | 退回整机 `bound`（`homed` 时；旧口径**一字不变**） |
 * | `missing` | 报了那一格，但**这一支不在里面** | `homed`：「未知」——**不许拿整机那格顶**：那是**第一支**的；`!homed`：「还没给它选」 |
 * | `absent` | 这一支在、**没指定** | 「未指定」（这是一个事实，不是读不到） |
 * | `unreadable` | 这一支的绑定读不出来 | 「未知」（不许说成"未指定"） |
 * | `dangling` | 绑的那条**不在清单里** | 「未知」（病因是悬空） |
 * | `bound` | 好的一条 | 「模型：<名字>」 |
 *
 * ⚠ `agentId` 缺（`null` / 空串）也算 `missing`：**说不出是哪一支，就不许替它认一条**。
 */
export type ResidentModelReadout =
  | { kind: 'unlisted' }
  | { kind: 'missing' }
  | { kind: 'absent' }
  | { kind: 'unreadable' }
  | { kind: 'dangling'; id: string }
  | {
      kind: 'bound'
      entry: HomeSourceEntry
      /** ★ 支级覆盖的型号（`null` = 用它选的条目里那个）。 */
      model: string | null
      /** ★ 支级覆盖的思考档（`null` = 用它选的条目里那个）。 */
      reasoningEffort: string | null
    }

export function residentModelReadout(
  view: HomeConfigView,
  agentId: string | null | undefined,
  homed = true,
): ResidentModelReadout {
  const rows = homed ? view.boundPerResident : view.boundUnhomed
  if (rows === undefined) return { kind: 'unlisted' }
  const id = typeof agentId === 'string' && agentId.trim() !== '' ? agentId : null
  const mine = id === null ? undefined : rows.find((row) => row.agentId === id)
  if (mine === undefined) return { kind: 'missing' }
  if (mine.state === 'unreadable') return { kind: 'unreadable' }
  if (mine.state !== 'bound' || mine.bound === null) return { kind: 'absent' }
  const entry = view.entries.find((candidate) => candidate.id === mine.bound)
  if (!entry) return { kind: 'dangling', id: mine.bound }
  return {
    kind: 'bound',
    entry,
    // 缺场（老门）与 `null` 同义：**没覆盖** —— 那一档的语义与"还没选过覆盖"一致，不会说反话。
    model: mine.model ?? null,
    reasoningEffort: mine.reasoningEffort ?? null,
  }
}

/**
 * **那两格的尾巴**（"具体用哪个型号 / 什么思考档"）—— 一处拼句。
 *
 * ⚠ 为什么必须有它：条目里的型号与**实际在用的**型号可以是两个不同的东西（支级覆盖）。
 * 只报条目名/条目型号而不说覆盖，就是"页面上说的"与"实际在用的"对不上 —— 这一句正是
 * 把覆盖**说出来**的地方（没覆盖时它一个字都不多说）。
 *
 * 它是**内部**的一处：两个页面都通过下面那两个人话函数用同一句话，不各自拼一遍。
 */
function overrideSuffix(readout: ResidentModelReadout): string {
  if (readout.kind !== 'bound') return ''
  const parts: string[] = []
  if (readout.model !== null) parts.push(`型号：${readout.model}`)
  if (readout.reasoningEffort !== null) parts.push(`思考档：${readout.reasoningEffort}`)
  return parts.length > 0 ? `（${parts.join(' · ')}）` : ''
}

/**
 * [`residentModelReadout`] 的**人话**（一处拼句，两个页面共用）。
 *
 * ⚠ 为什么必须只有一处：机器页那一间与 Agent 页那一段**说的是同一件事**（这一支用哪条）——
 * 两处各拼一遍就是"两个答案"（`homes.ts` 的 `mindReadoutFor` 头上记着这条教训）。
 * 读数来路也只有一处（`useHomeEntries` 的那份 `config.view`）。
 */
export function residentModelLine(readout: ResidentModelReadout): string {
  switch (readout.kind) {
    case 'bound':
      return `正在用 ${readout.entry.name || readout.entry.model}${overrideSuffix(readout)}`
    case 'absent':
      // ⚠ **2026-10-15 改口径**：从前这句是「没指定（走这台机器的环境变量）」—— 那句话在
      // **命令行形态**里是真的，在 **app 形态**里是**假的**（`host_process::model_plan`：
      // app 不继承模型来源，"没绑定"就是"还没配" ⇒ 起不来）。页面分不清对面是哪一种形态，
      // 所以只说**事实**："还没指定用哪一条"；"起不起得来"由知道那一半的读数去说
      // （机器页「运行」那一间、以及接上那一刻的结局）。
      return '还没指定用哪一条'
    case 'unreadable':
      return '绑定读不出来'
    case 'dangling':
      return `绑的那条不在了（${readout.id}）`
    case 'missing':
      return '不在这份读数里'
    case 'unlisted':
      return '这份读数没给逐支那一格'
  }
}

/**
 * **"先给它选好的那条"的人话**（`homed: false` 那一档，只用在**它还没被接到这台机器上**时）。
 *
 * 与 [`residentModelLine`] 分开的理由是**事实不同**，不是口味：同一句话在两处会是假话 ——
 * "正在用 X"对一支还没住进来的 Agent 不成立（它没在跑，也就谈不上"正在用"）；
 * 反过来"还没给它选"也不该出现在住户那一栏。⇒ 两个函数，各说各的处境，
 * 而**读那一格的那一处实现只有一份**（`residentModelReadout`）。
 */
export function unhomedModelLine(readout: ResidentModelReadout): string {
  switch (readout.kind) {
    case 'bound':
      return `已经给它选好 ${readout.entry.name || readout.entry.model}${overrideSuffix(readout)}（接到这台机器上就用它）`
    case 'absent':
    // ⚠ `missing` 在**这一格**里就是"没选过"：这一格列的是"先选好的那几支"，
    //    它不在里面 = 它还没有先选好的那条（不是"读不到"，也不是"这台机器上没有它"）。
    case 'missing':
      return '还没给它选 —— 选一条，接上就用它'
    case 'unreadable':
      return '先选好的那条读不出来'
    case 'dangling':
      return `先选好的那条不在了（${readout.id}）`
    case 'unlisted':
      return '这台机器上的门是旧版：读不到"先给它选好"那一格'
  }
}

/**
 * ★ **见证那一格的人话**（2026-10-18）—— 「这两份配置最后是谁改的」。
 *
 * 判据一句话：**两档分开说，不合并** ——
 * **页面改的，有时刻**；**不是页面改的，就只知"变了"，不知是谁**。
 *
 * ⚠ 三条不许越界（与门那一侧同一份纪律）：
 * - **报"变了"不等于报"被篡改了"** —— 这一层只报事实，不判性质；
 * - **判不了 ≠ 没改过**：`no-witness` / `unreadable` 两档都必须把"判不了"说出口；
 * - **它不是封印**：本地见证对拥有整机全权的主体不是密码学保证（那个主体够得着那两份文件，
 *   也够得着这本账），它给的是**改动的可见性**。
 *
 * `null` / `undefined` = **门没给这一格**（老门 / 远程那台跑着旧版）⇒ 如实说读不到，
 * **不许**说成"没改过"。
 */
export function modelWitnessLine(reading: ModelWitnessReading | null | undefined): string {
  if (reading === null || reading === undefined) return '这台机器上的门是旧版：读不到"谁改过"这一格'
  // ⚠ 时刻读不出来时**不许**兜底成"现在"（那会把一个缺失的事实说成一个假的事实）。
  const moment =
    reading.atMs === null ? '一个读不出来的时刻' : formatRuntimeTime(new Date(reading.atMs).toISOString())
  switch (reading.state) {
    case 'same':
      return `页面在 ${moment} 改过，之后没变过`
    case 'changed': {
      const unknown = '是谁、什么时候，这条路上答不出来'
      if (reading.detail === 'missing') return `不在了（页面最后写于 ${moment}）—— ${unknown}`
      if (reading.detail === 'appeared') return `又出现了（页面在 ${moment} 摘掉过它）—— ${unknown}`
      const delta =
        reading.deltaBytes === null
          ? ''
          : `，${reading.deltaBytes >= 0 ? '+' : ''}${reading.deltaBytes} 字节`
      return `页面之后有人动过它（页面最后写于 ${moment}${delta}）—— ${unknown}`
    }
    case 'unreadable':
      return reading.detail === 'ledger'
        ? '见证账本读不出来 ⇒ 判不了（不是"没改过"）'
        : '这一份读不出来 ⇒ 判不了（不是"没改过"）'
    case 'no-witness':
      return '没有见证 ⇒ 判不了（不是"没改过"）'
  }
}

/**
 * **账本链条断了**那一句（`null` / `undefined` = 没断，或老门不给这一格 ⇒ 不说）。
 *
 * 为什么要说：账是**只追加 ＋ 链值**的，而"链值"若没人读，它就是一个装饰 ——
 * 删掉中间一条记录，正是它该显形的那件事。
 */
export function modelWitnessChainLine(at: number | null | undefined): string | null {
  if (at === null || at === undefined) return null
  return `⚠ 见证账本的链条在第 ${at + 1} 条断了 —— 有人抽掉了中间的记录（这本账对拥有整机全权的人不是封印，它给的是改动的可见性）`
}

/**
 * **这个家有没有住户** —— 逐支那一格给的那个答案（`null` ＝ 门**没给**这一格）。
 *
 * - `[]`：门说**一支住户都没有**。这是一个**事实**（住户表为空），不是"读不到"；
 * - `null`：老门 / 这一格不在场 ⇒ **不知道**，不许拿它当"没有"。
 *
 * 为什么要单列一格：0 住户时"哪一支用哪一条""给某一支换成这条"这两句话**没有主语**
 * （机器级页面上刚说过"这台机器上还没有 Agent"），而页面该做的事与"读不到"**相反** ——
 * 前者是"先让一支住进来"，后者是"刷新看看 / 那台机器读不出来"。
 */
export function residentBindings(view: HomeConfigView): HomeResidentBinding[] | null {
  const rows = view.boundPerResident
  return Array.isArray(rows) ? rows : null
}

/**
 * **这一份读数里正在被用的是哪几条**（机器级问法的正确回答）。
 *
 * - **逐支那一格在场** ⇒ 任一支绑着的那几条（去重，按住户顺序；`state` 不是 `bound` 的不算）；
 * - **不在场**（老门）⇒ 退回整机 `bound` 那一条（单支的家两者等价）。
 *
 * ⚠ **不过滤清单**：绑着一条而那条**已经不在清单里**（悬空）也算"在用" —— 那正是
 * "这家配过、而那一支现在起不来"的处境；把它滤掉会让 [`modelSourceAbsent`] 说"还没配"，
 * 而用户此刻该修的是**悬空那一条**，不是照着一份空表再配一遍。
 *
 * ⚠ 与整机 `bound` 的差别正在这里：那一格是"**第一支**绑的那条"，**任一支读不出来就会把它顶成
 * `null`**；这一格问的是"**有没有人真在用**"。⇒ 多支的家不许拿 `bound` 当全称。
 */
export function inUseEntryIds(view: HomeConfigView): string[] {
  const rows = view.boundPerResident
  if (rows === undefined) return view.bound === null ? [] : [view.bound]
  const ids: string[] = []
  for (const row of rows) {
    if (row.state === 'bound' && row.bound !== null && !ids.includes(row.bound)) ids.push(row.bound)
  }
  return ids
}

/** [`inUseEntryIds`] 的**名字**（"现在在用哪几条"那几句话的读数；名字空就拿模型名顶，与门侧同口径）。 */
export function inUseEntryNames(view: HomeConfigView): string[] {
  const ids = new Set(inUseEntryIds(view))
  return view.entries.filter((entry) => ids.has(entry.id)).map((entry) => entry.name || entry.model)
}

/**
 * 这台机器**还没有模型配置**吗（首次的路用它决定要不要把人带过去）。
 *
 * ⚠ **两个门对"清单文件不在"给的是两个词**——这不是笔误，是两侧各自的处境：
 *
 * | 门 | 文件不在时给的读数 |
 * |---|---|
 * | web（`scripts/home/config-server.js` → `scripts/home/model-sources.js`） | `sourcesState: 'absent'` ＋ `entries: []` |
 * | app（壳自己端 `/home-config`，`app/src-tauri/src/home_doors.rs`） | `sourcesState: 'loaded'` ＋ `entries: []` |
 *
 * ⇒ 判据只能落在**两个门共有**的那两格上（`entries` 与"有没有人在用"），**不许认词**：
 * 认 `'absent'` 的话这一格在壳里**恒不成立**（而壳里正是用户真正在用的形态），
 * 而在 web 形态又会**假绿**（同一个判断在两条路上给两个答案）。
 *
 * 三条：
 * 1. **"配置坏了"不许认成"没配"**：清单读不出来时（`unreadable`）门给的也是空表 ——
 *    那时该做的是修那份文件，不是让人照着空表再配一条；
 * 2. **光看清单空还不够**：有住户正用它（`inUseEntryIds` 非空）说明这台机器上本来就有配置 ——
 *    两格一起看，才不会把"有钥匙、只是没绑给这一支"说成"还没配"；
 * 3. **它是判据不是读数**：`entries` 只问"空不空"，一条内容都不读（key 永不回浏览器）。
 *
 * ⚠ 它回答的是"**这台机器**上有没有模型配置"，不是"**这一支**绑好了没有"。
 * **读不到是另一回事**（调用方那侧的事：读不到一律当"不是"，见 `App.tsx`）。
 *
 * ★ **2026-10-12 收**：最后那一格从"整机 `bound` 是 `null`"改成"**没有任何一支在用它**"——
 * 多支的家这两句话**不一样**（一支绑着、另一支的绑定读不出来 ⇒ 旧判据说"还没配"，
 * 而它明明在用）。逐支那一格缺场时两者等价，一字不变。
 */
export function modelSourceAbsent(config: HomeConfigView): boolean {
  return (
    config.sourcesState !== 'unreadable' &&
    config.entries.length === 0 &&
    inUseEntryIds(config).length === 0
  )
}

/**
 * 页面要送出去的那一条（四件套 + 可选第五件 + 名字）。
 *
 * `apiKey` 留空 = **沿用门里已存的那一把**（只对"改一条"成立；新建必须给 ——
 * 新建时没有可沿用的东西，留空只会写出一条用不了的条目）。
 */
export interface HomeConfigDraft {
  name: string
  endpoint: string
  apiKey: string
  model: string
  contextWindow: string
  protocol: string
  /**
   * **这一份模型收不收得到媒体材料**（2026-10-17）：**三态**——
   * `undefined` = 页面**没说**（老表单、或没碰过这一格）⇒ 门从旧条目**原样带过来**；
   * `true` / `false` = 用户的明确决定。
   *
   * ⚠ **不许压成两态**（见 `declarationBody` 里那一句）。
   */
  vision?: boolean
}

/**
 * 草稿里的**文本格**：能直接喂 `<input value>` 的那几格。
 *
 * ⚠ `vision` **不在**里面：它是布尔，由**勾选框**自己接线。混进 `field()` 会让 `value` 的类型
 * 变成 `string | boolean | undefined`，而 `<input value>` 收不下布尔——那是类型系统在说
 * "这一格得另走一条线"。
 *
 * 用 `Exclude` 而不是手抄一串字面量：以后再加文本格时它**自动**在里面。
 */
export type HomeConfigTextField = Exclude<keyof HomeConfigDraft, 'vision'>

/**
 * ★ **换型号那一次的声明**（2026-10-17）。
 *
 * 判据一句话：**随模型走的声明，不许在换模型时沿用**。
 *
 * 这里的邻居 `contextWindow` 早就是这条判据（换型号必须重问 ⇒ 那一格留空，门自己去问或查表）；
 * 而 `vision` **没有可重问的来源** —— `GET /models` 不报模态，随包对照表也还没落地 ⇒ 只能由人
 * 重新表态。所以这一格的动作是"**关掉并请人重新勾**"，不是"猜一个"。
 *
 * ⚠ 为什么必须**显式**写 `false`、不能靠"没说"：线上那一格的 `undefined` 语义恰恰是
 * **"从旧条目原样带过来"**（门侧的保险，见 `declarationBody`）—— 那正是这个病本身：
 * 换成一个收不下图像块的模型，工具面照旧说"读得动图片"，直到真去读图那一次才炸在对方那里。
 * ⇒ 换型号时显式说不支持（fail-closed 那一侧），再由回执请人重新勾。
 * ⚠ 代价如实：这一格因此**不替人猜**新模型能不能看图。猜错的方向比"暂时读不动、由工具面
 * 如实报 `unsupported-content-type`"坏得多。
 */
export function modelChangeDraft(entry: HomeSourceEntry, model: string): HomeConfigDraft {
  return {
    name: entry.name,
    endpoint: entry.endpoint,
    // 留空 = 沿用那一条已存的那把钥匙（只有"改一条"成立）。
    apiKey: '',
    model,
    // 留空 = 让门**重新问一次**窗口：旧数字是上一个模型的，对新模型没有意义。
    contextWindow: '',
    protocol: entry.protocol ?? '',
    // 见上面那两段：不沿用旧值，显式落到"不支持"。
    vision: false,
  }
}

const STATES: readonly string[] = ['configured', 'absent', 'unreadable']
const SOURCES_STATES: readonly string[] = ['absent', 'loaded', 'unreadable']
const BINDING_STATES: readonly string[] = ['absent', 'bound', 'unreadable']
// ★ **见证的四态**（2026-10-18；权威是门那一侧的 `scripts/home/model_witness.js` 的 `STATES`）：
//   `same`（页面改的、之后没变）/ `changed`（别处改的）/ `no-witness`（没有上一条 ⇒ 判不了）
//   / `unreadable`（这一份或这本账读不出来 ⇒ 判不了）。**后两态都不是"没改过"。**
const WITNESS_STATES: readonly string[] = ['same', 'changed', 'no-witness', 'unreadable']
const PROBE_REASONS: readonly string[] = ['timeout', 'unreachable']
const WINDOW_SOURCES: readonly string[] = ['given', 'probed', 'table']

/** 清单里一条的形状（缺一格 ⇒ 整份读数不接受：半读的清单会让人照着重填一份本来在的）。 */
function isSourceEntry(value: unknown): value is HomeSourceEntry {
  if (typeof value !== 'object' || value === null) return false
  const entry = value as Record<string, unknown>
  if (typeof entry['id'] !== 'string' || typeof entry['name'] !== 'string') return false
  if (typeof entry['endpoint'] !== 'string' || typeof entry['model'] !== 'string') return false
  const window = entry['contextWindow']
  if (window !== null && typeof window !== 'number') return false
  const protocol = entry['protocol']
  if (protocol !== null && typeof protocol !== 'string') return false
  // `vision`（2026-10-17）：**可选**——老门的回执里没有这一格。**在场就必须是布尔**：
  // 半个形状（字符串 / 数字）比缺席更坏，那说明两边对不上，而照着重填会写出错的清单。
  const vision = entry['vision']
  if (vision !== undefined && typeof vision !== 'boolean') return false
  return typeof entry['keyConfigured'] === 'boolean'
}

/**
 * 见证那一格的形状守卫（**可选、给了就整格校验**）。
 *
 * 为什么不能"多一个字段不管"：读它的那一处要拿它说"页面在 X 改的 / 有人动过它"——
 * 半读的这一格会把**判不了**说成**没改过**（那是这一格最坏的一种假话）。
 */
function isWitnessReading(value: unknown): value is ModelWitnessReading {
  if (typeof value !== 'object' || value === null) return false
  const reading = value as Record<string, unknown>
  if (typeof reading['state'] !== 'string' || !WITNESS_STATES.includes(reading['state'])) return false
  // 时刻：`null`（判不了）或一个**有限**的数（`NaN` 会让 `new Date(...)` 抛 —— fail-closed）。
  const atMs = reading['atMs']
  if (atMs !== null && !(typeof atMs === 'number' && Number.isFinite(atMs))) return false
  const detail = reading['detail']
  if (detail !== null && typeof detail !== 'string') return false
  const delta = reading['deltaBytes']
  if (delta !== null && !(typeof delta === 'number' && Number.isFinite(delta))) return false
  return true
}

/**
 * 逐支绑定那一格里的一行。
 *
 * `agentId` 允许 `null`（门读不到那一支的 id 时就是这么给的 —— 那一行**归不到任何一支**，
 * 所以它不参与"这一支用哪条"的裁决，只参与"这台机器上有没有人在用"）。
 */
function isResidentBinding(value: unknown): value is HomeResidentBinding {
  if (typeof value !== 'object' || value === null) return false
  const row = value as Record<string, unknown>
  const agentId = row['agentId']
  if (agentId !== null && typeof agentId !== 'string') return false
  if (typeof row['state'] !== 'string' || !BINDING_STATES.includes(row['state'])) return false
  const bound = row['bound']
  if (bound !== null && typeof bound !== 'string') return false
  // ★ 支级覆盖那两格（2026-10-14；**可选** —— 老门不给）：给了就必须是 `string | null`。
  //   思考档**不在这里收封闭表**：它是页面上的一格读数，收窄会让一份手改过的文件把**整页**
  //   读成"读不到"（那是把一个小毛病放大成看不见）；门侧写入口才是 fail-closed 的那一道。
  for (const key of ['model', 'reasoningEffort']) {
    const field = row[key]
    if (field !== undefined && field !== null && typeof field !== 'string') return false
  }
  // ★ 见证那一格（2026-10-18；**可选** —— 老门不给）：给了就整格校验。
  const witness = row['witness']
  if (witness !== undefined && !isWitnessReading(witness)) return false
  return true
}

function isProbe(value: unknown): value is HomeConfigProbe {
  if (typeof value !== 'object' || value === null) return false
  const probe = value as Record<string, unknown>
  if (typeof probe['url'] !== 'string') return false
  if (typeof probe['ok'] !== 'boolean') return false
  const status = probe['status']
  if (status !== null && typeof status !== 'number') return false
  const reason = probe['reason']
  if (reason !== null && (typeof reason !== 'string' || !PROBE_REASONS.includes(reason))) return false
  // 窗口那两格**必须给**（可以是 null）：缺了它就是"这份读数不知道窗口探没探"——
  // 页面会照着它往输入框里填，猜错的那个方向（把"没探到"当成"探到了但没值"）
  // 正好会让人以为"这一家没有窗口"，于是手填一个假的。⇒ fail-closed 成"读不到"。
  const window = probe['contextWindow']
  if (window !== null && typeof window !== 'number') return false
  const field = probe['windowField']
  if (field !== null && typeof field !== 'string') return false
  // 模型目录那一格**必须在场**（可以是空数组）：缺了它就是"不知道这家有哪些模型"，
  // 而页面会拿它画"照着挑一个" —— 猜错的方向正好是"这家一个模型都没有"（假的）。
  const models = probe['models']
  return Array.isArray(models) && models.every((name) => typeof name === 'string')
}

/**
 * 形状守卫：**只认已知形状**。
 *
 * 必需件（`readAt` / `state` / `keyConfigured`）缺一个就是"读不到"——尤其 `state`：
 * 缺了它页面就不知道该说"没配"还是"配了但读不出来"，只能猜，而猜错的那个方向
 * （把"读不出来"说成"没配"）会让人去重配一份本来在的文件。
 */
export function isHomeConfigView(value: unknown): value is HomeConfigView {
  if (typeof value !== 'object' || value === null) return false
  const view = value as Record<string, unknown>
  if (typeof view['readAt'] !== 'string') return false
  if (typeof view['state'] !== 'string' || !STATES.includes(view['state'])) return false
  if (typeof view['keyConfigured'] !== 'boolean') return false
  for (const key of ['endpoint', 'model', 'protocol', 'reason']) {
    const field = view[key]
    if (field !== null && field !== undefined && typeof field !== 'string') return false
  }
  const window = view['contextWindow']
  if (window !== null && window !== undefined && typeof window !== 'number') return false
  // 三合一的另外两层（刀2b）：清单与绑定。**缺一层就不接受**——只认半个三合一的页面
  // 会把"读不出来"画成"没有钥匙"（那是撒谎），而这两层正是它用来分辨的东西。
  if (typeof view['sourcesState'] !== 'string' || !SOURCES_STATES.includes(view['sourcesState'])) {
    return false
  }
  if (typeof view['bindingState'] !== 'string' || !BINDING_STATES.includes(view['bindingState'])) {
    return false
  }
  const bound = view['bound']
  if (bound !== null && bound !== undefined && typeof bound !== 'string') return false
  // 逐支那一格（2026-10-12）：**可选**（老门不给 ⇒ 调用方退回整机 `bound`），但**给了就整格校验**
  // —— 半读的逐支表会把"这一支用哪条"指到**别的支**的配置上，而那比读不到更坏。
  const perResident = view['boundPerResident']
  if (
    perResident !== undefined &&
    (!Array.isArray(perResident) || !perResident.every(isResidentBinding))
  ) {
    return false
  }
  // ★ **还没住户化、但已经先选好了**那一格（2026-10-14）：同样**可选、给了就整格校验**。
  // 为什么不能只"多一个字段不管"：读它的那一处要拿它说"先给它选好的那条是 X"——
  // 半读的这一格会把"选过了"说成"没指定"（与逐支那一格同族的假话）。
  const unhomed = view['boundUnhomed']
  if (unhomed !== undefined && (!Array.isArray(unhomed) || !unhomed.every(isResidentBinding))) {
    return false
  }
  const divergent = view['bindingDivergent']
  if (divergent !== undefined && typeof divergent !== 'boolean') return false
  // ★ 见证那一格（2026-10-18；**可选** —— 老门不给 ⇒ 读成"读不到这一格"）：给了就整格校验
  //   —— 半读的见证会把"判不了"说成"没改过"（与逐支那一格同族的假话）。
  const listWitness = view['listWitness']
  if (listWitness !== undefined && !isWitnessReading(listWitness)) return false
  const chainBreak = view['witnessChainBreakAt']
  if (
    chainBreak !== undefined &&
    chainBreak !== null &&
    !(typeof chainBreak === 'number' && Number.isInteger(chainBreak) && chainBreak >= 0)
  ) {
    return false
  }
  // ★ 远程接上那一格（2026-10-15）：**可选**（老门不给 ⇒ "不知道"），但给了就必须落在这三态里
  // —— 把 `'off'` 读成 `null`（或反过来）会让页面把"能开"说成"这机器不提供"，指错方向。
  const remoteControl = view['remoteControl']
  if (
    remoteControl !== undefined &&
    remoteControl !== null &&
    remoteControl !== 'on' &&
    remoteControl !== 'off'
  ) {
    return false
  }
  const entries = view['entries']
  if (!Array.isArray(entries) || !entries.every(isSourceEntry)) return false
  const windowSource = view['windowSource']
  if (
    windowSource !== undefined &&
    windowSource !== null &&
    (typeof windowSource !== 'string' || !WINDOW_SOURCES.includes(windowSource))
  ) {
    return false
  }
  if (view['probe'] !== undefined && view['probe'] !== null && !isProbe(view['probe'])) return false
  return true
}

/** 读一次当前配置（三合一：清单 + 绑定 + 派生）。失败（门没开 / 超时 / 形状不认识 / 门拒绝）一律抛。
 *
 * 家口远程访问刀：`entry`（缺省＝本机）决定这份读数**问哪一台机器** —— 同源代转（本机）
 * 还是绝对地址 + Bearer 家钥匙（远程），分流在 `homes.ts` 一处发生。
 */
export async function fetchHomeConfig(
  init?: { signal?: AbortSignal },
  entry: HomeEntry = localHomeEntry(),
): Promise<HomeConfigView> {
  return request('GET', undefined, HOME_CONFIG_URL, init, undefined, entry)
}

/**
 * **新建一条**（`POST /home-config`）：id 由本机服务给（`ms-<n>`）。
 *
 * **保存即启用**（这条路的缺省语义与老页面"保存"逐字一致）：新存的那条会被绑给这个 Agent
 * —— 人刚填完一把钥匙，接着就要用它。要"只存不用"得走"改一条"那条路（它**不动绑定**）。
 *
 * ⚠ 新建**必须给密钥**（没有可沿用的东西）；窗口**可以留空** —— 门会顺手问服务方一次，
 * 还问不到就**按家查随包那份兜底表**；两条都没有才回 `window_unavailable`（那时保存不成立，
 * 如实请你手填）。钥匙不对另报 `credential_rejected`（那是另一回事：该去换钥匙）。
 */
export async function createHomeSource(
  draft: HomeConfigDraft,
  provider?: string,
  entry: HomeEntry = localHomeEntry(),
  /**
   * ★ 存完**要不要顺手把这一条给住户用**（2026-10-15「模型配置归机器侧」那一刀）。
   *
   * 不给 ⇒ **门的缺省（`bind: true`）**，老调用点一字不变。
   * 给了 ⇒ 照给的写。配置页给的是**"这台机器上恰有一支住户"**：
   * 那时"存完就用它"不是替谁决定（没有别人可决定）✓；**0 支或 2 支以上 ⇒ 不给**
   * （0 支没有目标、2 支以上就是替别人决定 —— 与"恰一条才自动绑"那条同一把尺子）。
   */
  bind?: boolean,
): Promise<HomeConfigView> {
  const body = { ...declarationBody(draft, provider), ...(bind === undefined ? {} : { bind }) }
  return request('POST', body, HOME_CONFIG_URL, undefined, undefined, entry)
}

/**
 * **改一条**（`PUT /home-config` 点名 `id`）：`bind: false` —— **不动绑定**。
 *
 * 为什么：清单归机器（这台上有哪些钥匙），绑定归 Agent（这一个用哪一条）。
 * 改一条备用钥匙的参数**不该**顺手把 Agent 正在用的那条换掉 —— 那是模型房间的动作。
 */
export async function updateHomeSource(
  id: string,
  draft: HomeConfigDraft,
  provider?: string,
  entry: HomeEntry = localHomeEntry(),
): Promise<HomeConfigView> {
  return request(
    'PUT',
    { ...declarationBody(draft, provider), id, bind: false },
    HOME_CONFIG_URL,
    undefined,
    undefined,
    entry,
  )
}

/**
 * **换绑**（`PUT /home-binding`）；`null` = **解除绑定**（回到环境变量那一路）。
 *
 * 生效语义与别处一致：**下一次活动**用新的一条，正在进行的那一次不受影响。
 *
 * `agentId`（2026-10-12 住户级覆盖；2026-10-14「选择先于接上」）：
 * - **不给** ⇒ 这台机器上的**每一支**都换（家级默认 —— 一个家住 N 支就写 N 份）；
 * - **给了** ⇒ **只给这一支**换（"住户用哪把钥匙归住户"）；这一支**还没被接到这台机器上**
 *   （还没有环境根）也照写 —— 落在 `<家>/agents/<id>/model_binding.json`，
 *   于是**选择可以先于接上**（接它的时候就不必再问"用哪一条"）。
 *   ⚠ 形状不合法的 `agentId` 由门回 `bad_agent_id`（它会变成一个目录名）；
 *   旧版的门对"不在场的这一支"回 `no_such_resident` —— 那一档的人话见 `ERROR_TEXTS`。
 *
 * `overrides`（★ 2026-10-14 支级覆盖）：**这一支具体用哪个型号 / 什么思考档** ——
 * 运行时一直认这两格（[`HomeResidentBinding`] 那两格的说明），这一层是它们的出口。三态：
 *
 * | 值 | 写出来的是 |
 * |---|---|
 * | **不给**（`undefined`） | 那两格**不动**（换来源时沿用原来的覆盖） |
 * | `null` | **清掉那一格**（回到"用条目里的型号/档位"） |
 * | 字符串 | 写成它（型号的字节上界与档位的封闭表由门预检） |
 *
 * ⚠ **只在点名 `agentId` 时能带**（那两格是支级事实；家级默认带它就是"每一支同一个型号"，
 * 门会回 `bad_request`）—— 这一层连着 `agentId` 一起判，不把一笔注定被拒的请求发出去。
 */
export async function bindHomeSource(
  id: string | null,
  entry: HomeEntry = localHomeEntry(),
  agentId: string | null = null,
  overrides?: { model?: string | null; reasoningEffort?: string | null },
): Promise<HomeConfigView> {
  const body: Record<string, string | null> = { source: id ?? '' }
  if (agentId !== null) body.agentId = agentId
  if (overrides && agentId !== null) {
    // 只在**真的有那一格**时才带上它：`undefined` = 不动（见上面那张三态表）。
    if (overrides.model !== undefined) body.model = overrides.model
    if (overrides.reasoningEffort !== undefined) body.reasoningEffort = overrides.reasoningEffort
  }
  return request('PUT', body, HOME_BINDING_URL, undefined, undefined, entry)
}

/**
 * ★ **这条钥匙能调哪些型号**（`POST /home-config/catalog`，2026-10-14）。
 *
 * 请求体只有条目 `id`：**钥匙由门从清单里读**（它永不回浏览器）。返回的是**试连读数**
 * （与 [`probeHomeSource`] **同一份形状**：`ok` / `status` / `reason` / `models` …），
 * 所以"连不上"照样是一份读数，不是错误 —— 只有**请求形状**不对（条目不在 / 没给 id）才抛。
 *
 * ⚠ **它不写任何东西**（与探测同一条纪律：探测即测试，保存才落盘）。
 */
export async function fetchHomeCatalog(
  id: string,
  entry: HomeEntry = localHomeEntry(),
): Promise<HomeConfigProbe> {
  const parsed = await send('POST', { id }, HOME_CATALOG_URL, undefined, undefined, entry)
  const probe = parsed && typeof parsed === 'object' ? (parsed as { probe?: unknown }).probe : null
  // 形状守卫与探测那一格同款：认不出就是"读不到"，**不半读半猜**。
  if (!isProbe(probe)) throw new Error('模型配置门回了一份认不出的目录读数')
  return probe
}

/**
 * **删一条**（`DELETE /home-config?id=`）。
 *
 * ⚠ 它是**销毁式**的，不是撤销式：那一条里的密钥**只有这一份**，删了就没了（要重新填一遍）。
 * 门那边还有一道：**正被绑定的那条不许删**（先换绑）—— 拒绝时回 `entry_bound`。
 */
export async function deleteHomeSource(
  id: string,
  entry: HomeEntry = localHomeEntry(),
): Promise<HomeConfigView> {
  return request('DELETE', undefined, HOME_CONFIG_URL, undefined, { id }, entry)
}

/**
 * ★ **接上一支 Agent** 那一条路径（2026-10-15 远程接上那一刀）。
 *
 * 为什么它不在 `/home-config` 底下：那几条管的都是**模型配置**（读与写文件），而这一条要在
 * **那台机器上建环境根、签凭据、起进程** —— 那是**执行期控制**。所以它自成一格路径，
 * 而判据是这台机器自己的准入（家钥匙 ＋ TLS）**再加一道单独的授权**
 * （那台机器上的 `remote_control` 开关，**只能在本地打开**）。
 *
 * ⚠ **本机那一档不走这条路**：本机由壳那只手做（那颗「接到这台机器上」按钮，与从前一字不变）。
 */
export const HOME_CONTROL_ATTACH_URL = '/home-control/attach'

/**
 * 一次接上的**结局**（**封闭表**：与那台机器报的、以及壳那份逐字相同；
 * 改一边就要改另一边 —— 有对表用例）。
 */
export const ATTACH_OUTCOMES: readonly string[] = [
  /** 接上了，而且进程起来了。 */
  'homed',
  /** 接上了，但**先没起它**（没模型那一档 —— 不是失败，也不是"跑起来了"）。 */
  'homed-not-started',
  /** 本来就已经接过 ⇒ **什么都没动**（幂等）。 */
  'already-homed',
  /** 没接上（`lines` 里是那台机器自己说的话）。 */
  'failed',
]

/** 一次接上的读数（**那台机器报的**，不是我们推的）。 */
export interface AttachReadout {
  outcome: string
  agentId: string
  envRoot: string | null
  pid: number | null
  lines: string[]
}

/**
 * **在那一台机器上接上这一支**（`entry` 决定问哪一台：本机＝同源代转，外地＝绝对地址 ＋ 家钥匙）。
 *
 * 三种回法，一种都不许假装成功：
 * - 成功 ⇒ 结局读数（`homed` / `homed-not-started` / `already-homed`）；
 * - 那台机器拒绝 ⇒ 抛 [`HomeConfigError`]（`remote_control_off` / `bad_agent_id` /
 *   `attach_failed` / `unauthorized` …）—— `attach_failed` 那一档带上**机器原话**（`lines`）；
 * - 结局词不认识 ⇒ 抛（一个不认识的结局**不许**当成功，那正是"把拒绝说成成功"那一类错）。
 */
export async function attachAgentOnMachine(
  agentId: string,
  entry: HomeEntry = localHomeEntry(),
): Promise<AttachReadout> {
  const parsed = await send('POST', { agentId }, HOME_CONTROL_ATTACH_URL, undefined, undefined, entry)
  const view = parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : null
  const outcome = typeof view?.['outcome'] === 'string' ? view['outcome'] : ''
  if (!ATTACH_OUTCOMES.includes(outcome)) {
    throw new Error(`那台机器回了一个我不认识的结局：${outcome || '（空）'}`)
  }
  return {
    outcome,
    agentId: typeof view?.['agentId'] === 'string' ? view['agentId'] : agentId,
    envRoot: typeof view?.['envRoot'] === 'string' ? view['envRoot'] : null,
    pid: typeof view?.['pid'] === 'number' ? view['pid'] : null,
    lines: Array.isArray(view?.['lines'])
      ? (view['lines'] as unknown[]).filter((each): each is string => typeof each === 'string')
      : [],
  }
}

/** 只读探测要送出去的那一份（`apiKey` 只走请求体：不落盘、不进 URL）。 */
export interface HomeProbeInput {
  endpoint: string
  apiKey: string
  /** 可选线协议（门只做形状预检；探测打的是 `/models`，与线协议无关）。 */
  protocol?: string
  /** 可选模型名：给了就顺手把**它**的标称窗口读回来。 */
  model?: string
  /**
   * 可选**家**（预设 id，如 `dashscope`）：原生目录层的查找键。
   * 门查不中就退兼容端点（一字不变），所以「自定义」也可以不传或随便传 —— 不会 4xx。
   */
  provider?: string
}

/**
 * **只读探测**（`POST /home-config/probe`）：拿这把钥匙去那一家问一次模型目录。
 *
 * 三条纪律与门侧逐字对齐（见 `scripts/home/config-server.js` 那段注释）：
 * - **不落盘**：这一趟不产生任何持久事实；要存就走 [`createHomeSource`]；
 * - **key 只进不出**：请求体里过一趟，响应里没有它（`HomeConfigProbe` 的形状里本来就没有这一格）；
 * - **试连失败是读数不是错误**：连不上 / 超时照样返回一份读数（`ok: false` + `reason`），
 *   由调用方如实显示；只有**请求形状**不对才抛 `HomeConfigError`。
 */
export async function probeHomeSource(
  input: HomeProbeInput,
  entry: HomeEntry = localHomeEntry(),
): Promise<HomeConfigProbe> {
  const parsed = await send(
    'POST',
    {
      endpoint: input.endpoint,
      api_key: input.apiKey,
      ...(input.protocol ? { protocol: input.protocol } : {}),
      ...(input.model ? { model: input.model } : {}),
      ...(input.provider ? { provider: input.provider } : {}),
    },
    HOME_PROBE_URL,
    undefined,
    undefined,
    entry,
  )
  const probe = parsed && typeof parsed === 'object' ? (parsed as { probe?: unknown }).probe : null
  // 形状守卫与别处同款：认不出就是"读不到"，**不半读半猜**。
  if (!isProbe(probe)) throw new Error('模型配置门回了一份认不出的探测读数')
  return probe
}

/**
 * **试连会打向的地址** —— 页面拿它给用户看一眼（「你填的这个，会变成这个」）。
 *
 * 口径与门侧 `scripts/home/config-server.js` 的 `modelsUrl` **同一条**（**那边是权威**）：
 * 我们让用户填的是**完整线协议地址**（`…/v1/chat/completions`），而试连要去的是**模型目录**
 * ⇒ 把末尾那一段线协议摘掉，再拼 `/models`；端点本来就是基址时原样拼（**摘不到就不摘，不猜**）。
 *
 * ⚠ **两份实现必须一致**（一份在门、一份在页面）⇒ `tests/presets.test.ts` 里有一条判据
 * **逐项比对**两侧的输出（跨侧 import 的先例就在那个文件里）。**改这边要同步那边。**
 */
export function probeUrlFor(endpoint: string): string {
  const trimmed = String(endpoint ?? '')
    .trim()
    .replace(/\/+$/, '')
  if (!trimmed) return ''
  const stripped = trimmed.replace(/\/(chat\/)?completions$/, '').replace(/\/responses$/, '')
  return `${stripped}/models`
}

/**
 * 表单 → 请求体（`apiKey` 空 = 沿用；其余原样）。
 *
 * `provider` = 当前选中的**是哪一家**（预设 id，如 `deepseek`；`custom` 表示自定义那条路）。
 * 门只在「窗口留空、且试连也没问到」时才用它 —— 拿去**随包那份兜底表**里**限家**查一条。
 * 不给（或给 `custom`）⇒ 门不查表、如实请你手填：自定义那条路**不知道是哪一家**，
 * 而"不知道是哪一家还去查"会**静默给错值**（实测：`mistral-large-latest` 被模糊命中成 32000，真值 256K）。
 */
function declarationBody(draft: HomeConfigDraft, provider?: string) {
  return {
    endpoint: draft.endpoint,
    apiKey: draft.apiKey,
    model: draft.model,
    contextWindow: draft.contextWindow,
    ...(draft.name.trim() ? { name: draft.name } : {}),
    ...(draft.protocol ? { protocol: draft.protocol } : {}),
    // ⚠ **三态，不许照抄上面那两行的惯例**：`false` 是 falsy，写成 `draft.vision ? … : {}` 会让
    // "**明确关掉**"这一态**消失**——门把它读成"没说"，再从旧条目带过来 ⇒ **这一格永远关不掉**，
    // 而且没有任何报错。判据与门那一侧同一句：**"没说"与"说了不支持"是两件事**。
    ...(draft.vision === undefined ? {} : { vision: draft.vision }),
    ...(provider ? { provider } : {}),
  }
}

async function request(
  method: 'GET' | 'POST' | 'PUT' | 'DELETE',
  body?: unknown,
  base: string = HOME_CONFIG_URL,
  init?: { signal?: AbortSignal },
  query?: Record<string, string>,
  entry: HomeEntry = localHomeEntry(),
): Promise<HomeConfigView> {
  const parsed = await send(method, body, base, init, query, entry)
  if (!isHomeConfigView(parsed)) throw new Error('模型配置门回了一份认不出的形状')
  return parsed
}

/**
 * 发一次请求，并把门的**拒绝**接成 [`HomeConfigError`]（**一处权威**）。
 *
 * 清单读写与只读探测共用这一段：错误码的翻译、超时的窗口、拒绝时顺手带回来的试连读数
 * —— 三件事都只写一遍（两份就会漂移，而漂移的那一份会让某条路少说一句话）。
 *
 * 家口远程访问刀：地址与请求头**按条目算**（`homes.ts` 一处分流）——本机那条一字不变，
 * 远程那条是绝对地址 + Bearer 家钥匙。
 */
async function send(
  method: 'GET' | 'POST' | 'PUT' | 'DELETE',
  body?: unknown,
  base: string = HOME_CONFIG_URL,
  init?: { signal?: AbortSignal },
  query?: Record<string, string>,
  entry: HomeEntry = localHomeEntry(),
): Promise<unknown> {
  const timeout = AbortSignal.timeout(method === 'GET' ? READ_TIMEOUT_MS : WRITE_TIMEOUT_MS)
  const signal = init?.signal ? AbortSignal.any([init.signal, timeout]) : timeout
  const payload = body === undefined ? undefined : JSON.stringify(body)
  const target = configRequestUrl(entry, base)
  const url = query ? `${target}?${new URLSearchParams(query).toString()}` : target
  const headers = {
    ...homeRequestHeaders(entry),
    ...(payload === undefined ? {} : { 'content-type': 'application/json' }),
  }
  let res
  try {
    // 出网走 `./transport`（与 homeStatus 同一条路）：壳里由 Rust 侧发出。
    res = await sendRequest({
      url,
      method,
      headers,
      body: payload,
      signal,
    })
  } catch {
    // 连不上 / 超时取消：家通道的失败分类由调用方按 `network` 讲。
    throw new HomeConfigError('network')
  }
  let parsed: unknown = null
  try {
    parsed = await res.json()
  } catch {
    parsed = null
  }
  if (!res.ok) {
    // 门拒绝：用**它说的那个码**（不在封闭表里也照原样带出来 ⇒ 页面说"不认识"）。
    // 顺带把它**带回来的试连读数**接住（`window_unavailable` 那条路上有），以及
    // **那台机器自己说的话**（`attach_failed` 那条路上有 —— 为什么没接上只在那几行里）。
    const refusal = parsed as { error?: unknown; probe?: unknown; lines?: unknown } | null
    const code = refusal?.error
    const probe = refusal?.probe !== undefined && refusal.probe !== null && isProbe(refusal.probe)
      ? refusal.probe
      : null
    const lines = Array.isArray(refusal?.lines)
      ? refusal.lines.filter((each): each is string => typeof each === 'string')
      : []
    throw new HomeConfigError(
      typeof code === 'string' && code ? code : `http_${res.status}`,
      probe,
      lines,
    )
  }
  return parsed
}
