/**
 * **模型候选的「分类 / 分组 / 搜索」** —— 纯函数，零依赖（可被 `node --test` 直接钉住）。
 *
 * ## 为什么要这一层
 *
 * 供应商给回来的候选可能很多（实测阿里百炼 **261 条**、它的原生端点 **517 条**），
 * 而且**混着各种东西**：对话模型、embedding、rerank、图像生成、语音识别/合成、OCR，
 * 还有平台自己没清理的内部运维项。原样倒给用户 ⇒ 他挑中一个 embedding 当对话模型，
 * **Agent 跑不起来**。所以要分类、要能搜、要能分组。
 *
 * ## 类型从哪里来（**分三层，优先用最硬的**）
 *
 * 1. **供应商给的元数据**（最硬）—— 有些原生端点直接给：
 *    百炼 `/api/v1/models` 的 `capabilities`（`["TG","VU","Reasoning"]`）与
 *    `inference_metadata.request_modality`（`["Image","Text"]`）。**给了就用它，不猜。**
 * 2. **命名约定**（兜底）—— 名字是供应商自己起的公开语义：`qwen3.7-text-embedding` 里的
 *    `embedding` 就是它在告诉你这是什么。本文件实现的就是这一层。
 * 3. 都没有 ⇒ **`chat`**（见下面的"保守默认"）。
 *
 * ## ★ 两条纪律
 *
 * - **保守默认 = `chat`。** 判不出来时**宁可当成对话模型列出来** ——
 *   **漏掉一个能用的模型，比多列一个不能用的更坏**（漏了用户选不到；多列了，他挑中一个
 *   embedding 也只是"存了之后跑不起来"，而且页面上标着类型，看得见）。
 * - **只排除"一定不是"的。** 拿不准的一律留（例如 `realtime` / `omni` / `audio` ——
 *   实时语音也可能是对话；`*-vl` 是视觉但**也能对话**）。
 *
 * ⚠ 本文件**不抄任何第三方实现的代码**：这里写的判据来自「模型命名的事实」
 * （哪些词表示哪类模型），是可独立得出的技术事实。
 */

/** 模型类别（互斥）。`chat` 是**保守默认**：判不出来就归这里。 */
export type ModelKind = 'chat' | 'embedding' | 'rerank' | 'image' | 'video' | 'audio' | 'ocr' | 'unknown'

/** 类别的中文短标签（给候选列表打标用；`chat` 不标 —— 那是默认，标了反而吵）。 */
export const MODEL_KIND_LABEL: Record<Exclude<ModelKind, 'chat'>, string> = {
  embedding: '向量',
  rerank: '重排',
  image: '图像',
  video: '视频',
  audio: '语音',
  ocr: '文字识别',
  unknown: '未知',
}

/**
 * **只列"一定不是对话模型"的词。**
 *
 * 顺序有意义：先判 embedding / rerank / ocr / audio / image —— 它们**互相不会混淆**，
 * 但都可能出现在同一个名字里（例如 `bge-m3` 是 embedding、`bge-reranker-v2` 是 rerank）。
 * **`rerank` 要排在 `embedding` 前面**：`bge-reranker` 里没有 embed，但
 * `jina-embeddings-v2-rerank` 这种同时带两个词，取更具体的那个。
 *
 * ⚠ **边界一律写 `[^a-z0-9]`**（`/i` 下即"非字母数字"）—— 不能只写 `[-_/]`：
 * 实测踩过 `black-forest-labs/FLUX.1-dev`，`flux` 后面跟的是 **`.`** ⇒ 用 `[-_/]` 就漏判了。
 */
const B = '[^a-z0-9]'
const KIND_RULES: ReadonlyArray<{ kind: ModelKind; re: RegExp }> = [
  // 重排：`rerank` / `re-rank` / `reranker` 三种写法都见过
  { kind: 'rerank', re: new RegExp(`rerank|re-rank|reranker`, 'i') },
  // 文字识别
  { kind: 'ocr', re: new RegExp(`(^|${B})ocr(${B}|$)|paddleocr`, 'i') },
  // 语音：**只排"一定不是对话"的** —— 识别（asr / whisper / transcribe）与合成（tts / speech）。
  // ⚠ **故意不含 `audio` / `voice` / `realtime`**：那类名字（如 `qwen-audio-3.0-realtime`）
  //    很可能是**实时语音对话** ⇒ 按"拿不准就留"的纪律**不排**。
  { kind: 'audio', re: new RegExp(`(^|${B})(asr|tts|whisper|speech)(${B}|$)|transcrib`, 'i') },
  // 图像：**只排"生成/编辑"**（`flux` / `diffusion` / `dall-e` …）。
  // ⚠ **故意不含 `-vl` / `vision`**：视觉理解模型（`qwen3-vl`、`*-vision`）**通常也能对话** ⇒ 不排。
  {
    kind: 'image',
    re: new RegExp(
      `(^|${B})(flux|diffusion|stable-diffusion|sd-xl|dall-e|dalle|cogview|janus|imagen|text-to-image|t2i|image-gen|image-edit)(${B}|$)`,
      'i',
    ),
  },
  // 视频：生成类（`t2v` / `kling` / `veo` / `sora` …）—— 与图像分开，因为**模态不同**。
  { kind: 'video', re: new RegExp(`(^|${B})(t2v|text-to-video|video-gen|kling|veo|sora|seedance|wan2)(${B}|$)|i2v`, 'i') },
  // 向量：`text-embedding` / `bge-` / `gte-` / `e5-` / `clip` / `voyage-` …
  {
    kind: 'embedding',
    re: new RegExp(`embedding|embed-|(^|${B})(bge|gte|e5|m3e|text2vec)(${B}|$)|jina-clip|voyage-|retrieval`, 'i'),
  },
]

/**
 * **命名空间前缀** = 名字里第一个 `/` 之前那一段（`kimi/kimi-k3` → `kimi`）。
 *
 * 为什么拿它当分组：**这是供应商自己划的命名空间**（不是我们猜的）。
 * 没前缀（`qwen-max`）⇒ 返回空串，归到"未分组"那一档。
 */
export function namespaceOf(id: string): string {
  const slash = id.indexOf('/')
  return slash > 0 ? id.slice(0, slash) : ''
}

/**
 * 判一个模型名是哪一类。**判不出来 ⇒ `chat`**（保守默认，见文件头）。
 *
 * @param id     模型名（用户填的 / 供应商目录里那个）
 * @param kinds  供应商元数据里直接给的类型（**优先**；给了就不看名字了）
 */
export function classifyModel(id: string, kinds?: readonly ModelKind[] | null): ModelKind {
  // ① 元数据优先：供应商说自己是什么，就信它（不再拿名字猜）
  if (Array.isArray(kinds)) {
    const known = kinds.find((k) => k !== 'chat' && k !== 'unknown')
    if (known) return known
    if (kinds.includes('chat')) return 'chat'
  }
  if (typeof id !== 'string' || !id.trim()) return 'unknown'
  // ② 命名约定兜底
  for (const rule of KIND_RULES) {
    if (rule.re.test(id)) return rule.kind
  }
  // ③ 保守默认
  return 'chat'
}

/** 一条候选（页面要用的那几个字段，全在这里算好，组件不再自己判断）。 */
export interface ModelCandidate {
  /** 原名，**逐字**（保存时写进清单的就是它）。 */
  id: string
  /** 是哪一类。 */
  kind: ModelKind
  /** 命名空间前缀（分组依据；空串 = 没前缀）。 */
  group: string
  /** 是不是"能当对话模型用"（`chat` 与 `unknown` 之外的都不算）。 */
  usable: boolean
}

/** 把一个模型名包成候选（分类 + 分组一次算好）。 */
export function toCandidate(id: string, kinds?: readonly ModelKind[] | null): ModelCandidate {
  const kind = classifyModel(id, kinds)
  return { id, kind, group: namespaceOf(id), usable: kind === 'chat' }
}

/**
 * **按组切分候选**（返回保持"组名升序 + 组内保持原顺序"）。
 *
 * 组名规则：有命名空间前缀就按它分组；没有的归到 `''`（页面上显示成「其他」或直接平铺）。
 * 空组不产出。
 */
export function groupCandidates(items: readonly ModelCandidate[]): Array<{ group: string; items: ModelCandidate[] }> {
  const buckets = new Map<string, ModelCandidate[]>()
  for (const item of items) {
    const bucket = buckets.get(item.group)
    if (bucket) bucket.push(item)
    else buckets.set(item.group, [item])
  }
  return [...buckets.entries()]
    .sort((a, b) => {
      // 没前缀的那一堆永远排最前（`qwen-max` 这种自研主力模型通常没前缀）
      if (a[0] === '') return -1
      if (b[0] === '') return 1
      return a[0].localeCompare(b[0])
    })
    .map(([group, list]) => ({ group, items: list }))
}

/**
 * **搜索**（大小写不敏感的子串匹配）。
 *
 * 空白查询 ⇒ 原样返回（不排序、不过滤）。**不做"模糊/拼音"** —— 那会给出用户看不懂的命中。
 */
export function searchCandidates(items: readonly ModelCandidate[], query: string): ModelCandidate[] {
  const q = String(query ?? '').trim().toLowerCase()
  if (!q) return [...items]
  return items.filter((item) => item.id.toLowerCase().includes(q))
}

/**
 * 一次算好页面要的东西：**过滤 → 分类 → 分组**。
 *
 * @param raw   供应商给回来的模型名（可能是空的：探测没给、或还没探测）
 * @param opts.query        搜索词
 * @param opts.includeAll   `false`（默认）⇒ **只列对话类**；`true` ⇒ 全列（那个「显示全部」的出口）
 * @param opts.kinds        元数据类型（按模型名给；缺就用名字判）
 */
export function buildCatalog(
  raw: readonly string[],
  opts: { query?: string; includeAll?: boolean; kinds?: Record<string, readonly ModelKind[]> } = {},
): { groups: Array<{ group: string; items: ModelCandidate[] }>; total: number; shown: number; hiddenNonChat: number } {
  const all = raw.map((id) => toCandidate(id, opts.kinds?.[id]))
  const searched = searchCandidates(all, opts.query ?? '')
  const visible = opts.includeAll ? searched : searched.filter((item) => item.usable)
  return {
    groups: groupCandidates(visible),
    total: all.length,
    shown: visible.length,
    // 被"只列对话类"挡掉的条数 —— 界面要**明说**有多少条没显示（否则用户以为那就是全部）
    hiddenNonChat: searched.length - visible.length,
  }
}
