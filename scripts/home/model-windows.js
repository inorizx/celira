'use strict'

/**
 * **兜底表查询** —— 探测拿不到窗口时，在这一层补一手。
 *
 * ## 定位
 *
 * 这张表**只补探测（`GET <基址>/models`）的缺口**，不是全量模型目录，也不是运行时依赖：
 * 数据由 `scripts/refresh-model-windows.mjs` 在**发版时**从 LiteLLM 的公开表（MIT）摘录一次，
 * 随包发。运行时**零网络**（实测：从国内拉那份表，5 个 CDN 域名只通 1 个 ⇒ 不能靠运行时拉）。
 *
 * ## ★★ 铁律：只做「限家 + 精确命中」，**绝不做模糊匹配**
 *
 * 为什么这么严 —— 实测过（2026-09-23，43 个常见模型名）：
 *
 * | 查法 | 命中率 | 会不会给错值 |
 * |---|---|---|
 * | 模型名精确查 | 19% | 否（但太低） |
 * | **模糊匹配**（任意 key 末段相等） | **74%** | ★★ **会** |
 * | **限定供应商后查** | **65%** | 否 |
 *
 * 模糊给错值的实例（**都是"看着正常"的数字，最难发现**）：
 * `mistral-large-latest → 32000`（真值 256K）、`qwen-turbo → 129024`（文档标 1M）、
 * `grok-4 → 131072`（真值 256K）。同一批名字改用限家查 ⇒ `mistral-large-latest → 262144` **正确**。
 *
 * ⇒ **给错值比留空更坏**：留空用户会去填；错值没人会核对，而 Agent 就会拿错的窗口跑。
 * ⇒ 所以这里 **miss 一律返回 `null`**（上层据此让用户填），**不回落、不猜、不取第一条**。
 *
 * ## 为什么必须限家
 *
 * 窗口是**「(供应商, 模型) 对」的属性**，不是「模型」的属性。实测同一个 `deepseek-v4-flash`：
 *
 *   32768(tensormesh) / 163840(pinstripes) / 200000(libertai) / 1000000(8家) /
 *   1048576(6家) / 1049000(wandb)   —— **差 32 倍**
 *
 * ⇒ 同名跨家取到的值**没有意义**。`providerId` 是必填参数，缺了直接 miss。
 */

const fs = require('node:fs')
const path = require('node:path')

/**
 * 由 `scripts/refresh-model-windows.mjs` 生成；随包发（15 KB，gzip 3.2 KB）。
 *
 * ⚠ **别挪进 `data/` 子目录**：`.gitignore` 有一条 `data/`（本意是忽略**运行数据**，
 * 黑盒测试的 `./data` 会落在 package 下）⇒ 放进那儿会被**静默忽略**、进不了仓库，
 * 兜底就悄悄失效了。放在模块旁边最不容易踩。
 */
const DEFAULT_TABLE = path.join(__dirname, 'model-windows.json')

/**
 * 建成一张可查的表。**纯函数**（吃 `providers` 对象），便于测试注入，不碰文件系统。
 *
 * @param {Record<string, Record<string, number>>} providers 家 id → { 模型名 → 窗口 }
 */
function createWindowTable(providers) {
  const byProvider = providers && typeof providers === 'object' ? providers : Object.create(null)
  return {
    /**
     * @param {string} providerId 预设的家 id（如 `deepseek`）。**必填** —— 缺了就是 miss。
     * @param {string} modelName  用户填/挑的模型名，**逐字**匹配。
     * @returns {number|null} 正整数窗口，或 `null`（=「这一格我不知道」，交给上层让用户填）
     */
    windowFor(providerId, modelName) {
      if (typeof providerId !== 'string' || !providerId) return null
      if (typeof modelName !== 'string') return null
      const name = modelName.trim()
      if (!name) return null
      const rows = byProvider[providerId]
      if (!rows || typeof rows !== 'object') return null
      const value = rows[name] // ★ 精确命中：`Object.prototype` 上的名字不该被当成模型
      return typeof value === 'number' && Number.isInteger(value) && value > 0 ? value : null
    },

    /** 表里有没有这一家（给上层判断"该不该指望兜底"用）。 */
    hasProvider(providerId) {
      return typeof providerId === 'string' && !!byProvider[providerId]
    },

    /** 家清单（只读快照，给测试与诊断用）。 */
    providerIds() {
      return Object.keys(byProvider)
    },
  }
}

/**
 * 从磁盘读那张表。**读不动就当空表**（`windowFor` 全 miss ⇒ 退回"让用户填"）。
 *
 * 为什么不抛：这张表是**兜底**。它缺失/损坏时，正确行为是"退回到没有兜底"，
 * 而不是让整个保存流程挂掉 —— 用户仍然可以手填。
 */
function loadWindowTable(filePath = DEFAULT_TABLE) {
  try {
    const raw = JSON.parse(fs.readFileSync(filePath, 'utf8'))
    return createWindowTable(raw && raw.providers)
  } catch {
    return createWindowTable(null)
  }
}

/** 表是什么时候生成的（给诊断用；读不到就 `null`）。 */
function tableGeneratedAt(filePath = DEFAULT_TABLE) {
  try {
    const raw = JSON.parse(fs.readFileSync(filePath, 'utf8'))
    return typeof raw._generatedAt === 'string' ? raw._generatedAt : null
  } catch {
    return null
  }
}

module.exports = { createWindowTable, loadWindowTable, tableGeneratedAt, DEFAULT_TABLE }
