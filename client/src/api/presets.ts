/**
 * **预设供应商表**（2026-09-23 主人两轮裁决：不能让用户手填 API 地址 —— 那是工程视角的表单，
 * 不是产品）。
 *
 * 主流产品的形状是「选供应商 → 贴 key → 获取模型列表 → 挑一个 → 保存」。这一张表就是那个
 * "选"的清单：**它是填表助手，不是配置，也不是目录服务**。
 *
 * 三条性质（写死在类型与注释里，别漂移）：
 *
 * - **它只提供"地址的默认值"**：`endpointTemplate` 是**完整线协议地址**
 *   （`…/v1/chat/completions` 形状）—— TASK_77 的教训：Rust 侧要的是连到具体接口上的完整地址，
 *   填成域名或基址会 404。所以这张表里的每一条都必须以 `completions` / `responses` 收尾
 *   （有机械判据：`tests/presets.test.ts`）。
 * - **它不是"已配置"**：选它**不写任何东西**，页面上必须明示这一点（`PRESET_DISCLAIMER`）。
 *   真正的配置只发生在保存那一刻，落在本机那份清单里。
 * - **静态数据**：不联网拉取、不自动更新、不做版本协商。供应商换地址时**随发版更新这张表**；
 *   用户也可以走「自定义」手填（那条路一直在）。
 *
 * 纯数据 + 纯函数，零依赖 —— 所以能被 `node --test` 直接钉住。
 */

export interface ProviderPreset {
  /** 稳定标识（只在本表内用，不上屏）。 */
  id: string
  /** 显示名。 */
  name: string
  /** **完整**线协议地址（`…/v1/chat/completions` 形状）。 */
  endpointTemplate: string
  /** 一句人话，帮用户选（不是广告语，也不承诺任何能力）。 */
  blurb: string
}

/**
 * 「自定义」那一项（清单的**最后一项**）。
 *
 * 它不是一个预设 —— 它是**降级通道**：手填端点（老表单那条路，一直保留）。
 * 单独给一个常量是为了让"这一项不是预设"在类型上就看得出来（它没有 `endpointTemplate`）。
 */
export const CUSTOM_PRESET_ID = 'custom'

/**
 * **预设 ≠ 已配置**的明示句。
 *
 * ⚠ 它**必须上屏**：这张表的危险性全在"用户以为选了就等于配好了"。判据见
 * `tests/presets.test.ts`（源级判据：这一页真的把它渲染出来了）。
 * 纯常量 = 页面与判据引同一处，不各写一份文案。
 */
export const PRESET_DISCLAIMER = '选这里只是把地址替你填好，不代表已经配好 —— 钥匙（API Key）要你自己贴。'

/**
 * 表格（顺序 = 左栏的呈现顺序）。国内常用的排前面。
 *
 * 地址口径：OpenAI 兼容端点，**完整到具体接口**。要改一条时记住两件事：
 * ① 末尾必须是线协议那一段（`chat/completions` 或 `responses`）；
 * ② 试连读的是 `GET <基址>/models`，门会把末尾那一段摘掉再拼（`modelsUrl`）。
 */
export const PROVIDER_PRESETS: readonly ProviderPreset[] = [
  {
    id: 'deepseek',
    name: 'DeepSeek',
    endpointTemplate: 'https://api.deepseek.com/v1/chat/completions',
    blurb: '国产，性价比高，中文好',
  },
  {
    id: 'openai',
    name: 'OpenAI',
    endpointTemplate: 'https://api.openai.com/v1/chat/completions',
    blurb: '老牌，模型最全',
  },
  {
    id: 'anthropic',
    name: 'Anthropic',
    endpointTemplate: 'https://api.anthropic.com/v1/chat/completions',
    blurb: '走它的兼容接口；长文与代码见长',
  },
  {
    id: 'siliconflow',
    name: '硅基流动',
    endpointTemplate: 'https://api.siliconflow.cn/v1/chat/completions',
    blurb: '国产聚合：一把钥匙用很多开源模型',
  },
  {
    id: 'dashscope',
    name: '通义千问',
    endpointTemplate: 'https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions',
    blurb: '阿里云，国内直连稳',
  },
  {
    id: 'zhipu',
    name: '智谱 GLM',
    endpointTemplate: 'https://open.bigmodel.cn/api/paas/v4/chat/completions',
    blurb: '国产，有免费档可先试',
  },
  {
    id: 'ark',
    name: '火山方舟',
    endpointTemplate: 'https://ark.cn-beijing.volces.com/api/v3/chat/completions',
    blurb: '字节豆包，国内延迟低',
  },
  {
    id: 'moonshot',
    name: 'Moonshot',
    endpointTemplate: 'https://api.moonshot.cn/v1/chat/completions',
    blurb: '国产，长上下文见长',
  },
  {
    id: 'minimax',
    name: 'MiniMax',
    endpointTemplate: 'https://api.minimaxi.com/v1/chat/completions',
    blurb: '国产，多模态见长',
  },
  {
    id: 'stepfun',
    name: '阶跃星辰',
    endpointTemplate: 'https://api.stepfun.com/v1/chat/completions',
    blurb: '国产，多模态见长',
  },
  {
    id: 'hunyuan',
    name: '腾讯混元',
    endpointTemplate: 'https://api.hunyuan.cloud.tencent.com/v1/chat/completions',
    blurb: '腾讯云，国内直连',
  },
  {
    id: 'baichuan',
    name: '百川智能',
    endpointTemplate: 'https://api.baichuan-ai.com/v1/chat/completions',
    blurb: '国产，中文对话',
  },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    endpointTemplate: 'https://openrouter.ai/api/v1/chat/completions',
    blurb: '聚合很多家：一把钥匙挨个试',
  },
  {
    id: 'groq',
    name: 'Groq',
    endpointTemplate: 'https://api.groq.com/openai/v1/chat/completions',
    blurb: '出字极快，开源模型为主',
  },
  {
    id: 'together',
    name: 'Together',
    endpointTemplate: 'https://api.together.xyz/v1/chat/completions',
    blurb: '开源模型托管，型号多',
  },
  {
    id: 'mistral',
    name: 'Mistral',
    endpointTemplate: 'https://api.mistral.ai/v1/chat/completions',
    blurb: '欧洲的开源模型',
  },
  {
    id: 'xai',
    name: 'xAI',
    endpointTemplate: 'https://api.x.ai/v1/chat/completions',
    blurb: 'Grok 系列',
  },
  {
    id: 'deepinfra',
    name: 'DeepInfra',
    endpointTemplate: 'https://api.deepinfra.com/v1/openai/chat/completions',
    blurb: '开源模型，价格低',
  },
  {
    // **2026-10-06 加**（主人点名要这一家）。id 是**表内标识**（不上屏），而它**不许带连字符**
    // —— `ProviderMark.tsx` 的 `LOGO_SVG` 键是标识符，`presets.test.ts` 也按
    // `[a-z0-9]+` 解析那一块（判据：每家预设都必须有一份真标识）。显示名照实写「OpenCode Go」。
    //
    // ⚠ 端点与「Zen」**不是同一条**：Go 是订阅制那一档，它的地址是 `/zen/go/v1/...`；
    // 模型 id 也由那家自己定（例如 `deepseek-v4.1-flash`）。将来若要再加 Zen，那时才需要
    // 一个带连字符的 id —— 那会连带改 `LOGO_SVG` 的键法与那条解析判据（一次有理由的守卫改动）。
    id: 'opencode',
    name: 'OpenCode Go',
    endpointTemplate: 'https://opencode.ai/zen/go/v1/chat/completions',
    blurb: '订阅制网关，一批开放模型走 OpenAI 兼容端点',
  },
]

/**
 * **各家的「去哪儿拿钥匙」与官网**（外部链接，**不是配置**）。
 *
 * 为什么单独一张表、而不是塞进每一条预设里：
 * - 它是**同一类的纯外部事实**（各家公开的页面），跟 `endpointTemplate`（要进协议）不是一回事；
 * - 一张表一眼看全、好核对、好整批更新；塞进 17 条里则变成 17 处要同步的地方。
 *
 * ⚠ **维护口径与 `endpointTemplate` 一致：随发版更新**（厂商改版了就跟着改）。
 * 2026-09 抽验过一遍：**没有 404**（`403`/`000` 是反爬或网络，不代表死链 —— 浏览器打得开）。
 * 而且这些页面**不登录也能打开**（登录后才是里面的内容），所以这个链接对人是有用的。
 */
const PROVIDER_LINKS: Record<string, { website: string; apiKeyUrl: string }> = {
  deepseek: { website: 'https://www.deepseek.com/', apiKeyUrl: 'https://platform.deepseek.com/api_keys' },
  openai: { website: 'https://openai.com/', apiKeyUrl: 'https://platform.openai.com/api-keys' },
  anthropic: { website: 'https://www.anthropic.com/', apiKeyUrl: 'https://console.anthropic.com/settings/keys' },
  siliconflow: { website: 'https://siliconflow.cn/', apiKeyUrl: 'https://cloud.siliconflow.cn/account/ak' },
  dashscope: { website: 'https://bailian.console.aliyun.com/', apiKeyUrl: 'https://bailian.console.aliyun.com/?apiKey=1' },
  zhipu: { website: 'https://open.bigmodel.cn/', apiKeyUrl: 'https://open.bigmodel.cn/usercenter/apikeys' },
  ark: { website: 'https://www.volcengine.com/product/ark', apiKeyUrl: 'https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey' },
  moonshot: { website: 'https://platform.moonshot.cn/', apiKeyUrl: 'https://platform.moonshot.cn/console/api-keys' },
  minimax: { website: 'https://platform.minimaxi.com/', apiKeyUrl: 'https://platform.minimaxi.com/user-center/basic-information/interface-key' },
  stepfun: { website: 'https://platform.stepfun.com/', apiKeyUrl: 'https://platform.stepfun.com/interface-key' },
  hunyuan: { website: 'https://cloud.tencent.com/product/hunyuan', apiKeyUrl: 'https://console.cloud.tencent.com/hunyuan/api-key' },
  baichuan: { website: 'https://platform.baichuan-ai.com/', apiKeyUrl: 'https://platform.baichuan-ai.com/console/apikey' },
  openrouter: { website: 'https://openrouter.ai/', apiKeyUrl: 'https://openrouter.ai/settings/keys' },
  groq: { website: 'https://groq.com/', apiKeyUrl: 'https://console.groq.com/keys' },
  together: { website: 'https://www.together.ai/', apiKeyUrl: 'https://api.together.ai/settings/api-keys' },
  mistral: { website: 'https://mistral.ai/', apiKeyUrl: 'https://console.mistral.ai/api-keys' },
  xai: { website: 'https://x.ai/', apiKeyUrl: 'https://console.x.ai/' },
  deepinfra: { website: 'https://deepinfra.com/', apiKeyUrl: 'https://deepinfra.com/dash/api_keys' },
  // 2026-10-06：钥匙与订阅都从这一页走（官方文档原话是"订阅 Go 之后拿到 API key"）——
  // 那家**没有**一个公开的"密钥页"地址可指，所以这里指文档页（已核过可达）。
  opencode: { website: 'https://opencode.ai/', apiKeyUrl: 'https://opencode.ai/docs/go/' },
}

/**
 * 取一家的外部链接；**表里没有就返回 `null`**（不回落、不猜 —— 「自定义」本来就没有）。
 *
 * `website` 与 `apiKeyUrl` **都是给人点的**：前者是"这家是谁"，后者是
 * "钥匙去哪儿拿"。**用户卡住的往往正是后者** —— 他知道要贴 key，但不知道去哪儿申请。
 */
export function providerLinks(id: string | null | undefined): { website: string; apiKeyUrl: string } | null {
  if (!id) return null
  return PROVIDER_LINKS[id] ?? null
}

/** 按 id 取一条预设（找不到 ⇒ `null`：**不回落第一条**，那是猜）。 */
export function presetById(id: string | null | undefined): ProviderPreset | null {
  if (!id) return null
  return PROVIDER_PRESETS.find((preset) => preset.id === id) ?? null
}

/** 左栏一行（预设与「自定义」在呈现上是同一件事，所以统一成一种形状）。 */
export interface ProviderPickerRow {
  id: string
  name: string
  blurb: string
}

/** 「自定义」那一行：它不是预设，是**降级通道**（手填地址）。 */
export const CUSTOM_ROW: ProviderPickerRow = {
  id: CUSTOM_PRESET_ID,
  name: '自定义',
  blurb: '地址自己填（高级）',
}

/**
 * 左栏要呈现的那些行：**预设在前、「自定义」最后**。
 *
 * 这一条（`自定义` 是**最后一项**）是裁决里点名的形状，所以它落在纯逻辑层、有机械判据
 * （`tests/presets.test.ts`）—— 而不是"组件里恰好那么写的"。
 */
export function pickerRows(presets: readonly ProviderPreset[] = PROVIDER_PRESETS): ProviderPickerRow[] {
  return [
    ...presets.map((preset) => ({ id: preset.id, name: preset.name, blurb: preset.blurb })),
    CUSTOM_ROW,
  ]
}

/**
 * 某一个地址是不是**某条预设**填出来的（用来在左栏标「已保存」）。
 *
 * ⚠ 判据是**地址相等**（清单里已有一条用了这个地址），不是"这个供应商被配过" ——
 * 页面必须照这个口径说那句话，别把它说成一个更强的事实。
 */
export function presetIdForEndpoint(
  endpoint: string,
  presets: readonly ProviderPreset[] = PROVIDER_PRESETS,
): string | null {
  const trimmed = endpoint.trim()
  if (!trimmed) return null
  return presets.find((preset) => preset.endpointTemplate === trimmed)?.id ?? null
}

/** 清单里一条（本模块只关心它归到哪一家，所以只要两个字段）。 */
export interface EndpointCarrier {
  id: string
  endpoint: string
}

/**
 * 把清单里的条目**按"一家"归并**（2026-09-23 主人裁决：左边一行 = 一家供应商）。
 *
 * 为什么要有这个函数：左栏的心智对象是"我要用 DeepSeek"（**一家**），而底下存的是
 * 条目（**一条** = 一次具体的配置）。这两个口径不一样，就需要一处**权威的归并**，
 * 而不是让组件各写一遍 `filter(...)`。
 *
 * 归并规则（**地址相等 ⇒ 同一家**）：
 * - 地址命中某条预设 ⇒ 归到那条预设的 id；
 * - 没命中的地址 ⇒ 归到 `CUSTOM_PRESET_ID`（"自定义"那一堆）—— 它也是真条目该待的地方，
 *   不能因为"名字对不上预设"就把它藏起来（那正是这刀要治的不诚实）。
 *
 * ⚠ 只做归并，**不排序、不去重、不判对错**：条目的权威顺序与身份仍在清单那一层。
 * 返回的是 `Map<家 id, 条目[]>`，且**保持传入顺序**（同一家内部，先存的在前）。
 */
export function groupEntriesByProvider<T extends EndpointCarrier>(
  entries: readonly T[],
  presets: readonly ProviderPreset[] = PROVIDER_PRESETS,
): Map<string, T[]> {
  const grouped = new Map<string, T[]>()
  for (const entry of entries) {
    const key = presetIdForEndpoint(entry.endpoint, presets) ?? CUSTOM_PRESET_ID
    const bucket = grouped.get(key)
    if (bucket) bucket.push(entry)
    else grouped.set(key, [entry])
  }
  return grouped
}

/**
 * 左栏一行**此刻的状态**（三个档，页面照这个强度说话）。
 *
 * - `bound`：**这一家里有正在被用着的条目**（最强，左栏要一眼看得出来）；
 * - `saved`：这一家里有条目、但没有一条在用；
 * - `none`：这一家在这台机器上还没有条目（选了就是"要配它"）。
 */
export type ProviderRowState = 'bound' | 'saved' | 'none'

/**
 * 按上面的三档判某一家现在是什么状态。
 *
 * ★ **2026-10-12：`inUseIds` 是一组，不是一个** —— 一个家住 N 支 ⇒ "正在用"可能落在好几条上
 * （判据与来源：`homeConfig.ts` 的 `inUseEntryIds`，**逐支口径**）。传单个 id 的那种写法在
 * 多支的家会漏掉后面那几支（"这一家有人在用"却画成"只是存过"）。
 */
export function providerRowState(
  entryIds: readonly string[],
  inUseIds: readonly string[],
): ProviderRowState {
  if (entryIds.length === 0) return 'none'
  return entryIds.some((id) => inUseIds.includes(id)) ? 'bound' : 'saved'
}
