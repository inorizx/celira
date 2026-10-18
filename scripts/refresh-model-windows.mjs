#!/usr/bin/env node
/**
 * 刷新「模型 → 上下文窗口」兜底表 —— **构建期工具，不是运行时依赖**。
 *
 * ## 为什么有这张表
 *
 * `/models` 这条接口的契约**只保证给模型 id**（OpenAI 定的形状）。实测 8 家里 4 家不给窗口
 * （**「不给」未必可靠：DeepSeek 一员当晚即被真响应推翻**，见下文 2026-09-23 晚补充），
 * 而给的都是"主动加料"的（聚合器必须展示对比；自研厂额外给元数据）。
 * 所以「探测拿不到」是常态，不是异常 —— 需要一份**兜底**。
 *
 * ## 数据来源
 *
 * LiteLLM 的 `model_prices_and_context_window.json`（**MIT**，仓库根目录那份；
 * `enterprise/` 目录是商业许可，与本文件无关）。它由社区维护、每周自动刷新，
 * Portkey / Helicone / OpenRouter / Continue 都从它拉。
 *
 * ## 通道（实测 2026-09-23）
 *
 *   fastly.jsdelivr.net  200 / 2.75 MB / **0.92 s**  ← 用这个
 *   raw.githubusercontent 200 / 2.75 MB / 65.8 s   （慢 71 倍，别用）
 *   cdn.jsdelivr / gcore.jsdelivr / raw.githack     全都不通
 *
 * ## 用法
 *
 *   node scripts/refresh-model-windows.mjs <litellm.json 路径> [输出路径]
 *
 * 拉取（另一步，故意分开：拉取失败不该让"生成"这一步看起来失败）：
 *   curl -sL -o /tmp/litellm.json \
 *     https://fastly.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json
 *
 * ## 纪律
 *
 * - **只摘我们预设表里有的那几家**（`providerMap`），不做全量 —— 表是兜底，不是目录服务。
 * - **只摘能读出窗口的**；读不出的条目留空没有意义（那是"未知"，不是"零"）。
 * - **只摘 chat 模式**（现在还只有对话模型这一格；将来扩模态时改这里重生成）。
 * - **精确匹配优先**：查表是"限家 + 逐字命中"，**绝不做模糊匹配**
 *   （实测：模糊会把 `mistral-large-latest` 命中成 32000，真值 256K —— 静默给错值）。
 */

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

/**
 * 我们预设表（`client/src/api/presets.ts`）里的家 → LiteLLM 的 provider 名。
 *
 * ⚠ 两个**不是同名**的，写死在这里（名字自动匹配会撞车）：
 * - `ark`（火山方舟）→ `volcengine`（注意：自动匹配会错配到 `darkbloom`）
 * - `zhipu`（智谱）  → `zai`（智谱的海外品牌 Z.ai）
 *
 * ⚠ 表里**没有**的家（实测）：`siliconflow` / `stepfun` / `hunyuan` / `baichuan`
 * ⇒ 这几家只能靠探测；探测拿不到时如实让用户填（**不猜**）。
 */
const providerMap = {
  deepseek: 'deepseek',
  openai: 'openai',
  anthropic: 'anthropic',
  dashscope: 'dashscope',
  moonshot: 'moonshot',
  minimax: 'minimax',
  openrouter: 'openrouter',
  groq: 'groq',
  together: 'together_ai',
  mistral: 'mistral',
  xai: 'xai',
  deepinfra: 'deepinfra',
  ark: 'volcengine',
  zhipu: 'zai',
}

/** 只认 chat 模式（LiteLLM 用 `mode` 区分 chat / embedding / rerank / audio / image…）。 */
const MODE = 'chat'

/**
 * **不摘的家**：已确认「探测（`GET <基址>/models`）本身就能拿到窗口」的那几家。
 *
 * 为什么排除：这张表的定位是**补探测的缺口**，不是"全量备份"。
 * 探测本来就拿得到的家，摘进来只是把表做大 —— **轮不到查表**。
 *
 * 证据（各家官方响应 schema / 文档，2026-09-23 核）：
 * - `openrouter`：`/api/v1/models` 的 schema 里有 `context_length`（顶层）与 `top_provider.context_length`
 * - `together`  ：`models.list()` 返回 `context_length`
 * - `moonshot`  ：官方文档明写「列出模型…包含模型 ID、**上下文长度**及能力标识」，字段 `context_length`
 * - `mistral`   ：官方 `/v1/models` 响应 schema 里有 `max_context_length`
 *
 * ⚠ 这不是永久判断。哪天某家改了接口、探测不再给窗口，把它从这张名单里移出来重生成即可
 * （表随包发，重生成＝发版时改一行）。
 *
 * ⚠ **待核**：`deepinfra` 也是聚合型托管（占本表 134 条 / 37% 体积），但**其 `/models` 是否给窗口未查证**
 * ⇒ 按"没查清先留着"处理（表只在探测失败时才查，多留无害；误删会卡住用户）。
 */
 * ⚠ **实测补充（2026-09-23 晚，主人保存 DeepSeek 时撞出来）**：
 * **`deepseek` 的 `/models` 实际是「给」窗口的** —— 那次保存 `windowSource` 报 `probed`、
 * 值是 `1048576`（而本表里同一个模型是 `1000000`，两个值不一样 ⇒ 更证明确是对方给的）。
 *
 * ⇒ 我原先据「DeepSeek 官方文档的响应 schema 只有 `id`/`object`/`owned_by`」判它**不给**，**判错了**。
 * **要害：接口「多给字段」并不违约** ⇒ 「文档只列了三个字段」**推不出**「响应只有三个字段」。
 * （当时以为是"两个独立来源印证"，其实文档与 SDK 类型**都是照契约写的静态证据**，不是实测响应。）
 *
 * 但**仍把 `deepseek` 留在摘录名单里**：给不给**可能分模型、也可能变**，而本表只在
 * 探测拿不到时才被查（探测给的一律优先）⇒ 多留一层保险，代价只是 10 条。
 */
const probedProviders = new Set(['openrouter', 'together', 'moonshot', 'mistral'])

/** 剥掉 `provider/` 前缀：各家 key 形态不统一（anthropic 是裸名，mistral 带前缀）。 */
const bareOf = (key, provider) => {
  const prefix = provider + '/'
  return key.startsWith(prefix) ? key.slice(prefix.length) : key
}

const [, , srcPath, outPathArg] = process.argv
if (!srcPath) {
  console.error('用法: node scripts/refresh-model-windows.mjs <litellm.json 路径> [输出路径]')
  process.exit(2)
}

const outPath = resolve(outPathArg ?? 'scripts/home/model-windows.json')
const table = JSON.parse(readFileSync(srcPath, 'utf8'))

const byLiteLLM = new Map() // litellm provider 名 → 我们的家 id
for (const [ours, theirs] of Object.entries(providerMap)) byLiteLLM.set(theirs, ours)

const providers = {}
const report = []

for (const [key, entry] of Object.entries(table)) {
  if (key === 'sample_spec' || !entry || typeof entry !== 'object') continue
  const ours = byLiteLLM.get(entry.litellm_provider)
  if (!ours) continue
  if (probedProviders.has(ours)) continue // 探测拿得到的家不摘（见上面证据）
  if (entry.mode !== MODE) continue

  const win = entry.max_input_tokens ?? entry.max_tokens
  if (typeof win !== 'number' || !Number.isInteger(win) || win <= 0) continue

  const name = bareOf(key, entry.litellm_provider)
  if (!name) continue

  providers[ours] ??= {}
  // 同名冲突（剥前缀后撞车）⇒ 保留**较小**的窗口：宁可保守，不虚报。
  const prev = providers[ours][name]
  providers[ours][name] = prev === undefined ? win : Math.min(prev, win)
}

for (const [ours, rows] of Object.entries(providers)) {
  report.push(`  ${ours.padEnd(12)} ${String(Object.keys(rows).length).padStart(4)} 个模型`)
}

const out = {
  _note:
    '模型 → 上下文窗口兜底表。探测（GET <基址>/models）拿不到窗口时才查这里。' +
    '查表必须「限家 + 逐字命中」，不做模糊匹配。',
  _source: 'https://github.com/BerriAI/litellm (MIT) — model_prices_and_context_window.json',
  _generatedBy: 'scripts/refresh-model-windows.mjs',
  _generatedAt: new Date().toISOString(),
  _channel: 'https://fastly.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json',
  _generatedFrom: srcPath,
  providers,
}

mkdirSync(dirname(outPath), { recursive: true })
writeFileSync(outPath, JSON.stringify(out, null, 2) + '\n', 'utf8')

const total = Object.values(providers).reduce((n, r) => n + Object.keys(r).length, 0)
console.log(`写入 ${outPath}`)
console.log(`共 ${Object.keys(providers).length} 家 / ${total} 个模型`)
console.log(report.join('\n'))
const missing = Object.keys(providerMap).filter((k) => !providers[k] && !probedProviders.has(k))
if (missing.length) console.log(`⚠ 兜底表里**没有**这几家（只能靠探测 + 用户手填）: ${missing.join(', ')}`)
console.log(`不摘（探测本身就拿得到窗口）: ${[...probedProviders].join(', ')}`)
