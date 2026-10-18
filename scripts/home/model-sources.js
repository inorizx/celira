'use strict'
/**
 * **模型源清单**与**绑定**（刀2b）：清单归机器，绑定归 Agent。
 *
 * 一句话原理：**key 是用户的（机器级资产，多个 Agent 共享一个池）；"用哪一条"是每个 Agent
 * 的事实**（存它自己的环境根）。同一条裁决线：钥匙串归房子，用哪把钥匙归住户。
 *
 * 两个文件（**各管各的，不出现第三份解析缓存**）：
 *
 * | 文件 | 归属 | 形状 |
 * |---|---|---|
 * | `<家>/model_sources.json` | **机器** | `[{ id, name, endpoint, api_key, model, context_window, protocol?, reasoning_effort?, vision? }]` |
 * | `<环境根>/model_binding.json` | **Agent** | `{ "source": "<清单条目 id>" }` |
 *
 * 三条纪律：
 * 1. **一个权威**：清单的条目形状只在本模块定义（`normalizeEntries`），别人一律调这里；
 *    `id` 稳定短标识（`ms-1`…）、`name` 是用户起的名；**没有第三份解析缓存**——
 *    绑定到四件套的解析只有一处（agent 侧 `model_source.rs` 读文件时做，本层不代它缓存）。
 * 2. **迁移幂等**：老的单份 `<环境根>/model_source.json`（四件套）迁成清单第一条 + 该环境根的
 *    绑定指向它；迁完**把老文件挪走**（改名留档）——它是"旧权威"，留着就是**双权威**
 *    （agent 侧也把"老文件在场"当冲突，见 `model_source.rs` 的冲突规则③）。
 * 3. **fail-closed**：读不出来/形状不对 ⇒ **如实失败**，绝不回落或猜一份出来。
 */

const fs = require('node:fs')
const path = require('node:path')

const P = require('./paths')

/** 机器级清单的位置（家根下，与 `home.conf` 同级）。 */
const LIST_FILE = 'model_sources.json'
/** 老的单份配置（环境根下，四件套形状）——迁移的**输入**，迁完改名留档。 */
const LEGACY_FILE = 'model_source.json'
/** 每个 Agent 的绑定（环境根下）。 */
const BINDING_FILE = 'model_binding.json'
/** 迁移留档的后缀（老文件改成 `<名>.<后缀>`，不再是权威）。 */
const MIGRATED_SUFFIX = 'migrated'
/** id 的形状：`ms-<序号>`（稳定短标识；用户看到的就是它）。 */
const ID_SHAPE = /^ms-\d+$/

/** 清单文件的绝对路径。 */
function listPath(homeDir) {
  return path.join(homeDir, LIST_FILE)
}

/** 绑定文件的绝对路径（`envDir` 就是环境根）。 */
function bindingPath(envDir) {
  return path.join(envDir, BINDING_FILE)
}

/** 老单份配置的绝对路径（迁移输入）。 */
function legacyPath(envDir) {
  return path.join(envDir, LEGACY_FILE)
}

/** 原子写（同目录临时文件 + rename）：读侧永远读不到半行。 */
function writeAtomic(file, text) {
  const temp = path.join(path.dirname(file), `.${path.basename(file)}.${process.pid}.tmp`)
  try {
    fs.writeFileSync(temp, text, 'utf8')
    fs.renameSync(temp, file)
  } catch (error) {
    try {
      fs.unlinkSync(temp)
    } catch {
      // 临时文件本来就没写成 ⇒ 没什么可清的
    }
    throw error
  }
}

/**
 * 把一份**原始解析结果**收成清单形状（**这一层是条目形状的唯一权威**）。
 *
 * 三条：`id` 不认识就**丢掉这一条**（不猜、不改名）；`name` 空 ⇒ 用模型名顶（再空 ⇒ `未命名`）；
 * `api_key` 只在这一层进内存，`.mjs`/HTTP 层一律只回"配没配"。
 */
function normalizeEntries(raw) {
  if (!Array.isArray(raw)) return []
  const out = []
  for (const item of raw) {
    if (!item || typeof item !== 'object') continue
    const id = typeof item.id === 'string' ? item.id.trim() : ''
    if (!ID_SHAPE.test(id)) continue
    const endpoint = typeof item.endpoint === 'string' ? item.endpoint.trim() : ''
    const model = typeof item.model === 'string' ? item.model.trim() : ''
    const rawWindow = item.context_window
    const contextWindow =
      typeof rawWindow === 'number' && Number.isInteger(rawWindow) && rawWindow > 0 ? rawWindow : null
    if (!endpoint || !model || contextWindow === null) continue
    const name = typeof item.name === 'string' && item.name.trim() ? item.name.trim() : model || '未命名'
    const protocol =
      typeof item.protocol === 'string' && item.protocol.trim() ? item.protocol.trim() : null
    // **思考强度**（2026-10-08）：可选的一格。这一层**只收形状、不判语义**（认得的值是
    // agent 侧唯一读侧的封闭表说了算，与 `protocol` 同款）；⚠ 但它**必须原样带上** ——
    // 写者重写清单时漏掉它，就等于把用户填的那一格**删掉**。
    const reasoningEffort =
      typeof item.reasoning_effort === 'string' && item.reasoning_effort.trim()
        ? item.reasoning_effort.trim()
        : null
    // **这一份模型收不收得到媒体材料**（2026-10-17）：可选的一格，**fail-closed**——
    // 只有显式 `true` 才算支持（与 agent 侧 `FileSource::vision` 同一条判据）。
    // ⚠ 与 `reasoning_effort` **同一个病、同一条药**：写者重写清单时漏掉它，就等于把用户填的
    // 那一格**删掉**——而这里删掉的后果是"她静默地重新读不动图片"（同一类事故在本文件里
    // 已经发生过两次，见下面 `writeList` 与 `decideBinding` 的注释）。
    const vision = item.vision === true
    out.push({
      id,
      name,
      endpoint,
      api_key: typeof item.api_key === 'string' ? item.api_key : '',
      model,
      context_window: contextWindow,
      protocol,
      reasoning_effort: reasoningEffort,
      vision,
    })
  }
  return out
}

/**
 * 读清单。**三种处境分开说**（`absent` / `unreadable` / `loaded`）——
 * "没配"与"配了但读不出来"是两件事，混成一个"没配"会让人去重配一份本来在的文件。
 */
function readList(homeDir) {
  const file = listPath(homeDir)
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch {
    return { state: 'absent', entries: [] }
  }
  let parsed
  try {
    parsed = JSON.parse(text)
  } catch {
    return { state: 'unreadable', entries: [], reason: 'not_json' }
  }
  if (!Array.isArray(parsed)) return { state: 'unreadable', entries: [], reason: 'not_array' }
  const entries = normalizeEntries(parsed)
  // 解析后一条都不剩、而原文非空 ⇒ 形状全不认识（如实说，不假装"空清单"）。
  if (entries.length === 0 && parsed.length > 0) {
    return { state: 'unreadable', entries: [], reason: 'no_valid_entry' }
  }
  return { state: 'loaded', entries }
}

/** 写清单（原子）。**只写规范形状**（调用方给的条目一律先过 `normalizeEntries`）。 */
function writeList(homeDir, entries) {
  const document = normalizeEntries(entries).map((entry) => {
    const out = {
      id: entry.id,
      name: entry.name,
      endpoint: entry.endpoint,
      api_key: entry.api_key,
      model: entry.model,
      context_window: entry.context_window,
      // ⚠ 落盘时也要带上（漏了它 = 每次保存都把那一格删掉）。
      reasoning_effort: entry.reasoning_effort ?? null,
      // ⚠ 同上，**一个字都不能漏**（2026-10-17）：漏了它的后果不是"少一格配置"，是
      // "用户以为配好了、而她静默地读不动图片"——这种失效没有任何报错。
      vision: entry.vision === true,
    }
    if (entry.protocol) out.protocol = entry.protocol
    return out
  })
  writeAtomic(listPath(homeDir), `${JSON.stringify(document, null, 2)}\n`)
}

/** 下一个可用 id（`ms-<最大序号+1>`）——**单调**，删条目后不回收（回收会撞历史绑定）。 */
function nextId(entries) {
  let max = 0
  for (const entry of entries) {
    const n = Number.parseInt(entry.id.slice('ms-'.length), 10)
    if (Number.isInteger(n) && n > max) max = n
  }
  return `ms-${max + 1}`
}

/**
 * 放一条进清单：**按 id 原地替换，新的追加在末尾**。
 *
 * 为什么不重排也不排序：清单在页面上是给人看的一列，顺序该稳定——`ms-1`（迁来的那条）
 * 永远在最前，其余按加入顺序。编辑一条顺手把它挪到末尾是没道理的行为。
 *
 * 返回 `{ ok: true, entries }` 或 `{ ok: false, reason: 'unreadable' | 'invalid_entry', detail }`。
 */
function upsertEntry(homeDir, entry) {
  const list = readList(homeDir)
  if (list.state === 'unreadable') return { ok: false, reason: 'unreadable', detail: list.reason }
  // ⚠ **页面没说的那一格，不许因此被删掉**（2026-10-17）：这里是**整条替换**，而来件是页面
  // 按它认识的几格拼的 ⇒ 页面还不认识的一格会在每一次保存里**静默消失**（同一类事故在本文件
  // 里已经发生过两次，见 `writeList` 与 `decideBinding` 的注释）。
  // ⇒ 来件**没声明** `vision` 时，从**旧条目**原样带过来。
  // 判据与 2026-10-09 那次同一条，区别是把保险做在**门这一侧**——它不依赖页面记得带上哪几格。
  // ⚠ 来件**显式**声明了（哪怕 `false`）就以它为准：那不是"没说"，那是"说了不支持"。
  const stored = list.entries.find((each) => each.id === entry?.id)
  const merged =
    entry && typeof entry === 'object' && entry.vision === undefined && stored
      ? { ...entry, vision: stored.vision === true }
      : entry
  const normalized = normalizeEntries([merged])[0]
  if (!normalized) return { ok: false, reason: 'invalid_entry' }
  const entries = list.entries
  const next = entries.some((each) => each.id === normalized.id)
    ? entries.map((each) => (each.id === normalized.id ? normalized : each))
    : [...entries, normalized]
  writeList(homeDir, next)
  return { ok: true, entries: next }
}

/**
 * **思考强度的封闭表**（支级覆盖那一格）。
 *
 * ⚠ **权威是运行时的读侧**（`agent/src/model_source.rs` 的 `REASONING_EFFORTS`）：认不得的取值
 * 到不了下游（宿主会拒绝启动）。本文件是**写入侧**的同表镜像 —— 两侧对表由
 * `tests/model-sources-binding.test.js` 的一条用例逐字比对（连壳那份门一起比三处），
 * 改一处不改另一处会红。
 */
const REASONING_EFFORTS = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

/** 型号那一格的字节上界（它进文件、不进路径；上界只是防呆）。 */
const MODEL_MAX_BYTES = 200

/**
 * **型号那一格的形状**（照运行时的读侧口径：**非空字符串**，`null`/不写 = 没有覆盖）。
 *
 * 允许空格与 `:` `/` 这类字符（"meta-llama/Llama-3-70b"、":free" 这些是**真的型号名**），
 * 只禁**控制字符**（含换行与 NUL —— 它们是写进 JSON 的病，而不是型号）。
 */
function modelShapeOk(value) {
  return (
    typeof value === 'string' &&
    value.trim() !== '' &&
    Buffer.byteLength(value, 'utf8') <= MODEL_MAX_BYTES &&
    // `Cc`（控制字符）—— 与壳那一侧 `char::is_control()` **同一类**（别写成只挡 ASCII 的
    // `\u0000-\u001f`：那样 C1 那一半两边给的答案就不一样了）。
    !/\p{Cc}/u.test(value)
  )
}

/**
 * 读绑定：`{ state, source, model, reasoningEffort }`（`absent` / `unreadable` / `bound`）。
 *
 * ⚠ **2026-10-14 加宽（支级覆盖上屏）**：绑定里那两格（`model` / `reasoning_effort`）从前
 * 读侧**看不见**（读回来只有 `source`）⇒ 页面就只能说"用哪一条来源"，说不出"**具体哪个型号**"，
 * 而运行时**一直**认这两格（`apply_binding_overrides`：绑定里的型号/档位赢过条目里的）。
 * 现在如实读出来（没写就是 `null`），并且**按运行时的同一口径 fail-closed**：
 * 型号不是非空字符串、档位不在封闭表里 ⇒ 这一份绑定读成 `unreadable`（**不是**"没写"）——
 * 那种文件宿主会拒绝启动，门必须说同一句话（两处对同一份文件给两个答案才是更坏的）。
 */
function readBinding(envDir) {
  const file = bindingPath(envDir)
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch {
    return { state: 'absent', source: null, model: null, reasoningEffort: null }
  }
  let parsed
  try {
    parsed = JSON.parse(text)
  } catch {
    return { state: 'unreadable', source: null, reason: 'not_json', model: null, reasoningEffort: null }
  }
  const source = parsed && typeof parsed.source === 'string' ? parsed.source.trim() : ''
  if (!source) {
    return { state: 'unreadable', source: null, reason: 'missing_source', model: null, reasoningEffort: null }
  }
  // `null` 与"没写"同义（写者清掉覆盖时就是这么写的）——与运行时的读侧同一条。
  const rawModel = parsed.model
  const model = rawModel === undefined || rawModel === null ? null : rawModel
  if (model !== null && !modelShapeOk(model)) {
    return { state: 'unreadable', source: null, reason: 'bad_model', model: null, reasoningEffort: null }
  }
  const rawEffort = parsed.reasoning_effort
  const reasoningEffort = rawEffort === undefined || rawEffort === null ? null : rawEffort
  if (reasoningEffort !== null && !REASONING_EFFORTS.includes(reasoningEffort)) {
    return {
      state: 'unreadable',
      source: null,
      reason: 'bad_reasoning_effort',
      model: null,
      reasoningEffort: null,
    }
  }
  return {
    state: 'bound',
    source,
    model: model === null ? null : model.trim(),
    reasoningEffort,
  }
}

/**
 * 写绑定（原子）。
 *
 * ⚠ **2026-10-09：重新绑定时要把"它说的那两格"原样带上**（`model` / `reasoning_effort`）。
 * 判据：**钥匙属机器，"用什么型号/什么档位"归主体** —— 那两格可以写进绑定（支级）来覆盖清单
 * 条目里的同名字段（换执行端时它还是它），而本函数**缺省**只拿到 `source`。漏了保留这一步，
 * 页面上重新绑一次就把那两格**悄悄抹掉**（与 `reasoning_effort` 那次"页面保存抹掉清单那一格"
 * 是**同族事故**，实测吃过一次）。缺那两格时写出来的仍是老形状（一字不变）。
 *
 * ★ **2026-10-14 加宽：那两格现在可以显式写**（`overrides`）—— 页面上的"选具体型号 / 思考档"
 * 就是这条路（在此之前运行时一直认这两格，而**没有任何写者**能写它们：功能有、出口没有）。
 * 三态语义（`undefined` / `null` / 字符串），一个都不许含糊：
 *
 * | `overrides.model` | 写出来的是 |
 * |---|---|
 * | **不给**（`undefined`） | **沿用旧值**（旧行为一字不变：重新绑定不抹掉它） |
 * | `null` | **清掉这一格**（回到"用条目里的型号"）——写 `null`，与运行时的"没写"同义 |
 * | 非空字符串 | 写成它（形状由调用方先过 [`modelShapeOk`] / 档位由 [`REASONING_EFFORTS`]） |
 */
function writeBinding(envDir, source, overrides = {}) {
  fs.mkdirSync(envDir, { recursive: true })
  let existing = null
  try {
    existing = JSON.parse(fs.readFileSync(bindingPath(envDir), 'utf8'))
  } catch {
    /* 没有旧的、或旧的读不懂 ⇒ 两格只能靠 overrides 给（照旧） */
  }
  /** 一格的三态：不给 ⇒ 沿用旧的（`null`/不写都算"没有"）；给了 ⇒ 照给的写（`null` = 清掉）。 */
  const decide = (key, given) => {
    if (given !== undefined) return given === null ? null : given
    const kept = existing && existing[key]
    return kept === undefined || kept === null ? null : kept
  }
  const out = { source }
  const model = decide('model', overrides.model)
  if (model !== null) out.model = model
  const effort = decide('reasoning_effort', overrides.reasoningEffort)
  if (effort !== null) out.reasoning_effort = effort
  writeAtomic(bindingPath(envDir), `${JSON.stringify(out, null, 2)}\n`)
}

/** 摘掉绑定（回到"这台机器上没绑模型源"那一路）。**查无也成功**（撤销式）。 */
function clearBinding(envDir) {
  try {
    fs.unlinkSync(bindingPath(envDir))
  } catch (error) {
    if (!error || error.code !== 'ENOENT') throw error
  }
}

/**
 * ★ **还没住户化、但已经先选好了**的那几支（2026-10-14「选择先于接上」那一刀）。
 *
 * 判据（**与住户判据同一把尺子**，见 `residents.js` 的 `residents()`）：`<家>/agents/<id>/`
 * 里有 `model_binding.json`，而**既没有** `agent.conf`**也没有** `identity.json`
 * —— 那就是"有人在她的页面上先给她选了哪一条，她还没被接到这台机器上"。
 *
 * 为什么它与绑定同属本模块：绑定的**落点**就是"她的环境根"，而环境根要等接她的第 2 步才建
 * ⇒ "先选好"这件事只多出一个**环境根先于住户出现**的形状，绑定本身的形状一字未改。
 *
 * ⚠ **它们不是住户**：调用方不许把它们并进"这台机器上的住户"那一格（那是假话）。
 * 本函数只回答"哪几个目录里有先选好的绑定"——**住户表由调用方给**（本模块不认识那一套）。
 *
 * `residentIds`：住户的 agentId 列表（缺席 / 空 = 这台机器上一支住户都没有）。
 * 顺序按 id 排（稳定，与住户表的次序无关）。
 */
function unhomedBindings(homeDir, residentIds) {
  const root = P.agentsRoot(homeDir)
  let entries
  try {
    entries = fs.readdirSync(root, { withFileTypes: true })
  } catch {
    // 没有 `agents/` 目录 = 一支都没有（含"先选好的"）——**空是一个真答案**，不是读不到。
    return []
  }
  const live = new Set((residentIds ?? []).map((id) => String(id)))
  const rows = []
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    if (entry.name.startsWith('.')) continue
    if (live.has(entry.name)) continue
    const envDir = path.join(root, entry.name)
    // 住户判据同一把尺子（`residents.js`）：有 `agent.conf` **或** `identity.json` 就是住户。
    if (fs.existsSync(path.join(envDir, P.AGENT_CONF))) continue
    if (fs.existsSync(path.join(envDir, 'identity.json'))) continue
    if (!fs.existsSync(bindingPath(envDir))) continue
    rows.push({ agentId: entry.name, envDir, binding: readBinding(envDir) })
  }
  rows.sort((left, right) => (left.agentId < right.agentId ? -1 : left.agentId > right.agentId ? 1 : 0))
  return rows
}

/**
 * **迁移**：老的单份 `<环境根>/model_source.json`（四件套）⇒ 清单第一条 + 该环境根绑定指向它。
 *
 * **幂等**（三种情形都安全重跑）：
 * - 老文件不在 ⇒ 什么都不做（`skipped`，"迁移过了 / 本来就没有"两件事的**共同结果**）；
 * - 老文件在、**绑定也在** ⇒ **不动**（双遗迹：既有绑定又有旧文件）——这是**要报裁**的处境，
 *   本函数只如实报 `conflict`，不自动选边（见施工单「疑点报裁」）；
 * - 老文件在、绑定不在 ⇒ 迁：写清单条目 + 写绑定 + **把老文件改名留档**（`<名>.migrated`）。
 *   若清单里已经有同 id 的条目（上次迁了一半？）⇒ **复用**它，不新建（这就是幂等的落点）。
 *
 * 返回 `{ action, detail, entry }`：`action ∈ migrated | skipped | conflict | failed`。
 */
function migrateIfNeeded(homeDir, envDir) {
  const legacy = legacyPath(envDir)
  if (!fs.existsSync(legacy)) return { action: 'skipped', detail: '老文件不在（已迁过，或本来就没有）' }

  const binding = readBinding(envDir)
  if (binding.state !== 'absent') {
    return {
      action: 'conflict',
      detail: `老文件在场，而绑定也在场（${binding.state}）—— 双遗迹，不自动选边`,
    }
  }

  let raw
  try {
    raw = JSON.parse(fs.readFileSync(legacy, 'utf8'))
  } catch (error) {
    return { action: 'failed', detail: `老文件读不出来/不是 JSON：${error && error.message}` }
  }
  const single = normalizeEntries([
    {
      // ⚠ 形状必须**合法**（`ms-<序号>`）：`normalizeEntries` 会丢掉不认的 id，而这一条
      // 下一步就要成为 `ms-1` —— 给一个像 `ms-legacy` 这样的名字会被静静丢掉、迁移永远失败。
      id: 'ms-1',
      name: typeof raw.name === 'string' ? raw.name : '',
      endpoint: raw.endpoint,
      api_key: raw.api_key,
      model: raw.model,
      context_window: raw.context_window,
      protocol: raw.protocol,
    },
  ])[0]
  if (!single) return { action: 'failed', detail: '老文件不是四件套形状（缺件或类型不对）' }

  const list = readList(homeDir)
  if (list.state === 'unreadable') return { action: 'failed', detail: `清单读不出来（${list.reason}）` }
  const entries = list.entries
  const existing = entries.find((entry) => entry.id === 'ms-1')
  if (existing) {
    // 清单第一条已经是它（上次迁到一半）⇒ 只补绑定与留档，不重复建条目。
    writeBinding(envDir, 'ms-1')
    archiveLegacy(legacy)
    return { action: 'migrated', detail: '清单第一条已在，补上绑定与留档', entry: existing }
  }
  const entry = { ...single, id: 'ms-1', name: single.name || single.model || '默认' }
  writeList(homeDir, [entry, ...entries])
  writeBinding(envDir, entry.id)
  archiveLegacy(legacy)
  return { action: 'migrated', detail: '四件套已迁为清单第一条，绑定指向它', entry }
}

/** 老文件改名留档（**不再是权威**）。名字撞了就带序号，绝不覆盖。 */
function archiveLegacy(legacy) {
  let target = `${legacy}.${MIGRATED_SUFFIX}`
  let n = 1
  while (fs.existsSync(target)) {
    n += 1
    target = `${legacy}.${MIGRATED_SUFFIX}-${n}`
  }
  fs.renameSync(legacy, target)
  return target
}

/**
 * 删除清单条目。**被绑定挡住就拒绝**（"有 Agent 正绑定这一条"）——
 * 与"删除配置"同一条纪律：后果、原因、怎么恢复三件事都要说得清。
 * 返回 `{ ok: true }` 或 `{ ok: false, reason: 'not_found' | 'bound', boundTo }`。
 */
function removeEntry(homeDir, envDir, id) {
  const list = readList(homeDir)
  if (list.state === 'unreadable') return { ok: false, reason: 'unreadable', detail: list.reason }
  const entries = list.entries
  if (!entries.some((entry) => entry.id === id)) return { ok: false, reason: 'not_found' }
  const binding = readBinding(envDir)
  if (binding.state === 'bound' && binding.source === id) {
    return { ok: false, reason: 'bound', boundTo: binding.source }
  }
  writeList(
    homeDir,
    entries.filter((entry) => entry.id !== id),
  )
  return { ok: true }
}

module.exports = {
  LIST_FILE,
  BINDING_FILE,
  LEGACY_FILE,
  MIGRATED_SUFFIX,
  ID_SHAPE,
  listPath,
  bindingPath,
  legacyPath,
  normalizeEntries,
  readList,
  writeList,
  upsertEntry,
  nextId,
  readBinding,
  writeBinding,
  clearBinding,
  unhomedBindings,
  REASONING_EFFORTS,
  modelShapeOk,
  MODEL_MAX_BYTES,
  migrateIfNeeded,
  removeEntry,
  writeAtomic,
}
