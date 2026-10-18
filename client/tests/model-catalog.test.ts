'use strict'
// 模型候选的「分类 / 分组 / 搜索」（`client/src/api/modelCatalog.ts`）。
//
// ⚠ 本文件的模型名**照真输出逐字抄**：
//  - 主人截图里阿里百炼返回的那批（`qwen3.7-text-embedding` / `fun-asr-flash-2026-06-15` …）
//  - 百炼**原生端点**给的那批（`happyhorse-1.1-t2v` 等）
//  - Cherry Studio 截图里那种带命名空间前缀的（`BAAI/bge-m3` / `deepseek-ai/DeepSeek-R1`）
// 不自己编名字 —— 编出来的会掩盖真实分布。

import { test } from 'node:test'
import assert from 'node:assert/strict'

import {
  buildCatalog,
  classifyModel,
  groupCandidates,
  namespaceOf,
  searchCandidates,
  toCandidate,
} from '../src/api/modelCatalog.ts'

// ---------------------------------------------------------------------------
// 分类
// ---------------------------------------------------------------------------

const CASES: ReadonlyArray<[string, string]> = [
  // 对话（保守默认，也是绝大多数）
  ['qwen3.7-flash', 'chat'],
  ['deepseek-chat', 'chat'],
  ['deepseek-flash', 'chat'],
  ['gpt-4o', 'chat'],
  ['kimi/kimi-k3', 'chat'],
  ['ZHIPU/GLM-5.2', 'chat'],
  ['MiniMax/MiniMax-M3', 'chat'],
  ['qwen3.7-max-2026-06-08', 'chat'],
  ['test-sre-gpu-auto-handle', 'chat'], // 平台内部运维项：判不出来 ⇒ 归 chat（**不猜着一刀切掉**）
  // 向量
  ['qwen3.7-text-embedding', 'embedding'],
  ['BAAI/bge-m3', 'embedding'],
  ['text-embedding-3-small', 'embedding'],
  ['Qwen/Qwen3-Embedding-8B', 'embedding'],
  ['jina-embeddings-v3', 'embedding'],
  // 重排（★ 必须排在 embedding 前面：`bge-reranker` 这种名字不能再被当成向量）
  ['BAAI/bge-reranker-v2-m3', 'rerank'],
  ['rerank-2', 'rerank'],
  // 文字识别
  ['qwen3.5-ocr', 'ocr'],
  ['PaddleOCR-VL', 'ocr'],
  // 语音（只排"一定不是对话"的）
  ['fun-asr-flash-2026-06-15', 'audio'],
  ['whisper-large-v3', 'audio'],
  ['gpt-4o-mini-tts', 'audio'],
  // 图像生成/编辑
  ['black-forest-labs/FLUX.1-dev', 'image'],
  ['qwen-image-2.0-pro-2026-06-22', 'chat'], // ⚠ 名字里没有"生成"词（`image` 只作为中间段）⇒ 判不出来 ⇒ chat（保守）
  // 视频生成
  ['happyhorse-1.1-t2v', 'video'],
  // ★ 拿不准的一律留（这些**很可能是对话**，不许排掉）
  ['qwen-audio-3.0-realtime-flash', 'chat'],
  ['qwen3.8-omni-flash-realtime', 'chat'],
  ['qwen3-vl-plus', 'chat'],
  ['glm-4v-plus', 'chat'],
  ['deepseek-v4-flash-vision-exp', 'chat'],
]

test('分类：按名字判类型（照真输出逐字）', () => {
  for (const [id, want] of CASES) {
    assert.equal(classifyModel(id), want, `${id} 应当判成 ${want}`)
  }
})

test('★ 保守默认：判不出来的一律 `chat`（漏掉能用的比多列一个更坏）', () => {
  for (const id of ['some-brand-new-model-x1', '神秘模型', 'abc/def-2', '2026-06-08']) {
    assert.equal(classifyModel(id), 'chat', `${id} 判不出来时应当归 chat`)
  }
})

test('★ 元数据优先：供应商说了算，不再拿名字猜', () => {
  // 名字像对话、但元数据说是 embedding ⇒ 信元数据
  assert.equal(classifyModel('qwen3.7-flash', ['embedding']), 'embedding')
  // 名字像 embedding、但元数据说是 chat ⇒ 信元数据
  assert.equal(classifyModel('text-embedding-3-small', ['chat']), 'chat')
  // 元数据只有 chat ⇒ chat
  assert.equal(classifyModel('whatever', ['chat']), 'chat')
  // 元数据是空的/歪的 ⇒ 退回按名字判
  assert.equal(classifyModel('qwen3.7-text-embedding', []), 'embedding')
  assert.equal(classifyModel('qwen3.7-text-embedding', null), 'embedding')
})

test('分类：空白/非字符串 ⇒ unknown（不是 chat —— 空名不是一个候选）', () => {
  assert.equal(classifyModel(''), 'unknown')
  assert.equal(classifyModel('   '), 'unknown')
})

// ---------------------------------------------------------------------------
// 分组
// ---------------------------------------------------------------------------

test('命名空间前缀 = 第一个 `/` 之前那一段；没有前缀 ⇒ 空串', () => {
  assert.equal(namespaceOf('kimi/kimi-k3'), 'kimi')
  assert.equal(namespaceOf('BAAI/bge-m3'), 'BAAI')
  assert.equal(namespaceOf('meta-llama/Llama-3.3-70B-Instruct'), 'meta-llama')
  assert.equal(namespaceOf('qwen-max'), '')
  assert.equal(namespaceOf('/leading-slash'), '') // 开头就是斜杠 ⇒ 不算前缀
})

test('分组：组名升序，且"没前缀的那一堆"排最前', () => {
  const items = ['kimi/kimi-k3', 'qwen3.7-flash', 'BAAI/bge-m3', 'qwen3-max'].map((id) => toCandidate(id))
  const groups = groupCandidates(items)
  assert.deepEqual(
    groups.map((g) => g.group),
    ['', 'BAAI', 'kimi'],
  )
  assert.deepEqual(
    groups[0]!.items.map((i) => i.id),
    ['qwen3.7-flash', 'qwen3-max'],
  )
})

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------

test('搜索：大小写不敏感的子串；空白查询原样返回', () => {
  const items = ['qwen3.7-flash', 'Qwen3-Max', 'kimi/kimi-k3'].map((id) => toCandidate(id))
  assert.deepEqual(
    searchCandidates(items, 'QWEN').map((i) => i.id),
    ['qwen3.7-flash', 'Qwen3-Max'],
  )
  assert.deepEqual(
    searchCandidates(items, 'k3').map((i) => i.id),
    ['kimi/kimi-k3'],
  )
  assert.equal(searchCandidates(items, '   ').length, 3)
  assert.equal(searchCandidates(items, '不存在的名字').length, 0)
})

// ---------------------------------------------------------------------------
// 一次算好：过滤 → 分类 → 分组
// ---------------------------------------------------------------------------

test('★ buildCatalog：默认只列对话类，并**如实报出被挡掉的条数**', () => {
  const raw = [
    'qwen3.7-flash',
    'qwen3.7-text-embedding',
    'qwen3.5-ocr',
    'fun-asr-flash-2026-06-15',
    'kimi/kimi-k3',
    'BAAI/bge-m3',
  ]
  const catalog = buildCatalog(raw)
  assert.equal(catalog.total, 6)
  assert.equal(catalog.shown, 2, '只列对话类：qwen3.7-flash + kimi/kimi-k3 这 2 条')
  assert.equal(catalog.hiddenNonChat, 4, '★ 被挡掉的必须如实报出来（界面要明说，否则用户以为那就是全部）')
  assert.deepEqual(
    catalog.groups.flatMap((g) => g.items.map((i) => i.id)).sort(),
    ['kimi/kimi-k3', 'qwen3.7-flash'],
  )
})

test('★ buildCatalog：`includeAll` 是那个「显示全部」的出口', () => {
  const raw = ['qwen3.7-flash', 'qwen3.7-text-embedding', 'qwen3.5-ocr']
  const catalog = buildCatalog(raw, { includeAll: true })
  assert.equal(catalog.shown, 3)
  assert.equal(catalog.hiddenNonChat, 0, '全列时没有"被挡掉"的')
  const byId = Object.fromEntries(catalog.groups.flatMap((g) => g.items).map((i) => [i.id, i]))
  assert.equal(byId['qwen3.7-text-embedding']!.kind, 'embedding', '全列时也要**带着类型标签**（用户才知道自己在挑什么）')
})

test('★ buildCatalog：搜索与"只列对话"叠加时，条数是**搜索之后**的', () => {
  const raw = ['qwen3.7-flash', 'qwen3.7-text-embedding', 'kimi/kimi-k3']
  const catalog = buildCatalog(raw, { query: 'embedding' })
  assert.equal(catalog.total, 3, 'total = 原始候选数')
  assert.equal(catalog.shown, 0, '搜到的那条是 embedding ⇒ 默认不列')
  assert.equal(catalog.hiddenNonChat, 1, '搜出来 1 条，被挡掉 1 条')
})

test('buildCatalog：空输入不炸（还没探测、或供应商一条都不给）', () => {
  const catalog = buildCatalog([])
  assert.deepEqual(catalog, { groups: [], total: 0, shown: 0, hiddenNonChat: 0 })
})

// ---------------------------------------------------------------------------
// 权威品类（`kinds`，2026-09-24 原生目录层）
//
// 门那边只有**表里有的家**探原生目录时才会多给这一格（按模型名 → 类别数组）。
// 这里要判的是：它**真的被消费了**，而且"没有它"这条老路一字不变。
// ---------------------------------------------------------------------------

test('★ buildCatalog：`kinds` 优先于名字 —— 元数据说是 chat 的，名字像 embedding 也要列出来', () => {
  const raw = ['qwen3.7-text-embedding', 'qwen3.7-flash', 'BAAI/bge-m3']
  const catalog = buildCatalog(raw, {
    kinds: { 'qwen3.7-text-embedding': ['chat'], 'BAAI/bge-m3': ['embedding'] },
  })
  const byId = Object.fromEntries(catalog.groups.flatMap((g) => g.items).map((i) => [i.id, i]))
  assert.equal(byId['qwen3.7-text-embedding']!.kind, 'chat', '★ 供应商说了算，不再拿名字猜')
  assert.equal(byId['qwen3.7-flash']!.kind, 'chat', '★ 名单里没有它 ⇒ 退回按名字判（不是"没给就当不存在"）')
  assert.equal(catalog.shown, 2)
  assert.equal(catalog.hiddenNonChat, 1, '★ 被挡掉的条数也要按元数据算（按名字算是 2 ⇒ 会多挡一个能用的）')
})

test('★ 负对照：`kinds` 缺失 / 空 / 只有名单外的键 ⇒ 与"完全不给"逐字一致（新格不改老路）', () => {
  const raw = ['qwen3.7-flash', 'qwen3.7-text-embedding']
  const base = buildCatalog(raw)
  assert.deepEqual(buildCatalog(raw, { kinds: {} }), base)
  assert.deepEqual(buildCatalog(raw, { kinds: { 'not-in-list': ['video'] } }), base)
  assert.deepEqual(buildCatalog(raw, { kinds: undefined }), base)
})

test('★ 元数据说 `unknown` ⇒ 退回按名字判（不把"未知"硬安上去挡住一个能用的模型）', () => {
  assert.equal(classifyModel('qwen3.7-text-embedding', ['unknown']), 'embedding')
  assert.equal(classifyModel('qwen3.7-flash', ['unknown', 'chat']), 'chat')
})
