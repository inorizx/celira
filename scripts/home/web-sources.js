'use strict'
/**
 * **web 源：清单 + 绑定**（2026-10-20）——「这台机器上有哪些外网凭据」与「这一支用哪一条」。
 *
 * | 事实 | 归属 | 文件 |
 * |---|---|---|
 * | 这台机器上有哪些外网凭据（**清单**） | **机器** | `<家>/web_sources.json` |
 * | 这一支用哪一条（**绑定**） | **Agent** | `<环境根>/web_binding.json` |
 *
 * 一句话原理：**钥匙串归房子，用哪把钥匙归住户**（与模型那一对同一条判决）。
 *
 * **读侧是 `agent/src/web_source.rs`**（权威）：本文件只写、只印，**不做第二套解析**。
 * 条目形状（`{ id, name, api_key, search, fetch }`）与那边的 `parse_entry` 逐键对齐；
 * `wire` 是**方言**（对面回执的形状），不是厂商名：
 *
 * - `results-json`：对面直接回一个结果列表（请求体 `{query, max_results}`）；
 * - `search-api`：一家的**搜索接口**（请求体 `{query, count, need_content}`，按**请求次数**计费）。
 *
 * **厂商名只在这个文件里**（`PROVIDERS` 那张表：厂商 → 方言 + 默认端点 + 默认模型）。判据：
 * 主体那一侧只认得"查"与"取"两个词，换一家只换这一行；Runtime 一个厂商名都不认识
 * （语言守卫钉着，见 `agent/tests/minimal_agent.rs`）。
 *
 * ⚠ **key 只到这台机器为止**：明文写进清单（暴露面与环境变量同级），**永不打印**（只印掩码）、
 * 永不进世界、永不回浏览器。
 */

const fs = require('node:fs')
const path = require('node:path')

const P = require('./paths')

/** 清单文件名（与 `model_sources.json` 同级）。 */
const LIST_FILE = 'web_sources.json'
/** 绑定文件名（环境根下）。 */
const BINDING_FILE = 'web_binding.json'
/** id 的形状：`ss-<序号>`（稳定短标识，用户看到的就是它）。 */
const ID_SHAPE = /^ss-\d+$/

/**
 * 厂商 → 一条搜索源该长什么样（**这张表是厂商名的唯一落点**）。
 *
 * `wire` 的两档见文件头。`endpoint` 是**默认值**，`add` 时写进清单（清单是权威，改了清单就改了配置）；
 * 换一家同方言的只换这一格。
 *
 * ⚠ **`search-api` 这一条按请求次数计费**（不是 token、不是结果条数；账号级有 QPS），默认端点抄的
 * 是 DeepSeek 官方 **Search API**（`POST /v1/tools/search`）；换一家同方言的只换端点。
 */
const PROVIDERS = {
  deepseek: {
    label: '深度求索',
    search: { wire: 'search-api', endpoint: 'https://api.deepseek.com/v1/tools/search' },
    fetch: { wire: 'fetch-doc', endpoint: 'https://api.deepseek.com/v1/tools/fetch' },
  },
  tavily: {
    label: 'Tavily',
    search: { wire: 'results-json', endpoint: 'https://api.tavily.com/search' },
    fetch: { wire: 'fetch-raw', endpoint: 'https://api.tavily.com/extract' },
  },
}

const SEARCH_WIRES = ['results-json', 'search-api']
const FETCH_WIRES = ['fetch-doc', 'fetch-raw']

/** 清单文件的绝对路径。 */
function listPath(homeDir) {
  return path.join(homeDir, LIST_FILE)
}

/** 绑定文件的绝对路径（`envDir` 就是环境根）。 */
function bindingPath(envDir) {
  return path.join(envDir, BINDING_FILE)
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
      /* 临时文件本来就没写成 */
    }
    throw error
  }
}

/**
 * 读清单：`{ entries, problem }`。
 *
 * **读不出来 ≠ 空清单**（与读侧同一条纪律）：前者带 `problem` 让人看见，后者才是真的没有。
 */
function readList(homeDir) {
  const file = listPath(homeDir)
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch (error) {
    if (error.code === 'ENOENT') return { entries: [], problem: null, file }
    return { entries: [], problem: `读不出来（${error.message}）`, file }
  }
  let parsed
  try {
    parsed = JSON.parse(text)
  } catch (error) {
    return { entries: [], problem: `不是合法 JSON（${error.message}）`, file }
  }
  if (!Array.isArray(parsed)) return { entries: [], problem: '清单应当是一个数组', file }
  return { entries: parsed, problem: null, file }
}

/** 读绑定：`{ source, problem }`；不在 ⇒ `source: null`。 */
function readBinding(envDir) {
  const file = bindingPath(envDir)
  let text
  try {
    text = fs.readFileSync(file, 'utf8')
  } catch (error) {
    if (error.code === 'ENOENT') return { source: null, problem: null, file }
    return { source: null, problem: `读不出来（${error.message}）`, file }
  }
  try {
    const parsed = JSON.parse(text)
    const source = typeof parsed?.source === 'string' ? parsed.source.trim() : ''
    if (source === '') return { source: null, problem: '缺 `source`', file }
    return { source, problem: null, file }
  } catch (error) {
    return { source: null, problem: `不是合法 JSON（${error.message}）`, file }
  }
}

/** 掩码：只露最后四位（**key 永不打印**）。 */
function maskKey(key) {
  const text = String(key ?? '')
  if (text.length <= 4) return '••••'
  return `••••${text.slice(-4)}`
}

/** 下一个 id：`ss-<最大序号+1>`（删掉中间那一条也不复用号）。 */
function nextId(entries) {
  let max = 0
  for (const entry of entries) {
    const id = typeof entry?.id === 'string' ? entry.id : ''
    if (!ID_SHAPE.test(id)) continue
    max = Math.max(max, Number(id.slice(3)))
  }
  return `ss-${max + 1}`
}

/** 形如 `http(s)://…` 的绝对地址（不在这一步判它通不通，那是运行期的事）。 */
function looksLikeAddress(value) {
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:'
  } catch {
    return false
  }
}

/** 住户（`<家>/agents/<id>/`）：用来在没给 `--agent` 时挑那一支。 */
function residents(homeDir) {
  const root = path.join(homeDir, 'agents')
  let names
  try {
    names = fs.readdirSync(root, { withFileTypes: true })
  } catch {
    return []
  }
  return names
    .filter((item) => item.isDirectory())
    .map((item) => item.name)
    .filter((name) => {
      const dir = P.agentEnvRoot(homeDir, name)
      return (
        fs.existsSync(path.join(dir, 'identity.json')) ||
        fs.existsSync(path.join(dir, 'agent.conf'))
      )
    })
    .sort()
}

/**
 * 这一条命令说的是哪一支（`--agent` 优先；没给时**恰好一支**才替人选）。
 *
 * 判据：绑定是**每支一份**的事实——给错一支就是把别人的配置改了，因此宁可要求说清楚。
 */
function targetAgent(homeDir, agentId) {
  if (agentId) return { id: agentId, dir: P.agentEnvRoot(homeDir, agentId) }
  const all = residents(homeDir)
  if (all.length === 1) return { id: all[0], dir: P.agentEnvRoot(homeDir, all[0]) }
  if (all.length === 0) return { error: '这个家里还没有住户（先用 node home.js up 安一支）' }
  return { error: `这个家有多支住户，用 --agent 点名一支：${all.join(' / ')}` }
}

/** 参数解析（取值型选项后面必须跟一个值；不认识就报错，不猜）。 */
function parseArgs(argv) {
  const out = { positional: [], agent: null, name: null, provider: null, key: null, json: false }
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index]
    if (token === '--json') {
      out.json = true
      continue
    }
    if (token.startsWith('--')) {
      const value = argv[index + 1]
      if (value === undefined || value.startsWith('--')) throw new Error(`${token} 后面要给一个值`)
      index += 1
      const key = token.slice(2)
      if (!(key in out) || key === 'json') throw new Error(`未知选项: ${token}`)
      out[key] = value
      continue
    }
    out.positional.push(token)
  }
  return out
}

const USAGE = [
  '用法: node home.js web <list|add|use|remove|show> [参数]',
  '  list                        这台机器上有哪些搜索源（key 只印掩码）',
  '  add --provider <tavily|deepseek> --key <key>',
  '      [--name <名字>]',
  '                              往清单里加一条（端点/模型不给就用那一家默认的）',
  '  use <id> [--agent <id>]     这一支用哪一条（绑定写在环境根下）',
  '  remove <id>                 从清单里删一条（被绑定的不许删）',
  '  show [--agent <id>]         这一支现在用哪一条（以及清单/绑定的路径）',
  '  --home <家目录>              不给就用默认家',
].join('\n')

/** 一条条目的展示行（key 永远只露掩码）。 */
function describe(entry) {
  const search = entry.search ?? {}
  const fetch = entry.fetch ?? {}
  return `${entry.id}  ${entry.name ?? entry.id}\n      查 ${search.wire ?? '?'}  ${search.endpoint ?? '?'}\n      取 ${fetch.wire ?? '?'}  ${fetch.endpoint ?? '?'}\n      凭据 ${maskKey(entry.api_key)}`
}

function cmdList(homeDir) {
  const { entries, problem, file } = readList(homeDir)
  if (problem) {
    console.error(`[web] 清单读不出来（${file}）：${problem}`)
    return 1
  }
  console.log(`[web] 清单：${file}`)
  if (entries.length === 0) {
    console.log('[web] （空）—— 用 node home.js web add --provider … --key … 加一条')
    return 0
  }
  for (const entry of entries) console.log(describe(entry))
  return 0
}

function cmdAdd(homeDir, options) {
  if (!options.provider) {
    console.error(`[web] 缺 --provider（只有：${Object.keys(PROVIDERS).join(' / ')}）`)
    return 1
  }
  const preset = PROVIDERS[options.provider]
  if (!preset) {
    console.error(`[web] 不认识的 --provider ${options.provider}（只有：${Object.keys(PROVIDERS).join(' / ')}）`)
    return 1
  }
  if (!options.key || options.key.trim() === '') {
    console.error('[web] 缺 --key（凭据只落在这台机器上，不会打印）')
    return 1
  }

  const { entries, problem, file } = readList(homeDir)
  if (problem) {
    console.error(`[web] 清单读不出来（${file}）：${problem} —— 不覆盖它`)
    return 1
  }
  const entry = {
    id: nextId(entries),
    name: options.name ?? preset.label,
    api_key: options.key.trim(),
    search: { ...preset.search },
    fetch: { ...preset.fetch },
  }
  const rows = entries.filter((row) => row?.id !== entry.id)
  rows.push(entry)
  fs.mkdirSync(homeDir, { recursive: true })
  writeAtomic(file, `${JSON.stringify(rows, null, 2)}\n`)
  console.log(`[web] 已加入清单：${file}`)
  console.log(describe(entry))
  console.log('[web] 下一步：node home.js web use ' + entry.id + ' [--agent <id>]（选它给这一支用）')
  return 0
}

function cmdUse(homeDir, id, agentId) {
  if (!id || !ID_SHAPE.test(id)) {
    console.error('[web] 用法：node home.js web use <id> [--agent <id>]')
    return 1
  }
  const { entries, problem, file } = readList(homeDir)
  if (problem) {
    console.error(`[web] 清单读不出来（${file}）：${problem}`)
    return 1
  }
  const entry = entries.find((row) => row?.id === id)
  if (!entry) {
    console.error(`[web] 清单里没有 ${id}（有：${entries.map((row) => row?.id).join(' / ') || '一条都没有'}）`)
    return 1
  }
  const target = targetAgent(homeDir, agentId)
  if (target.error) {
    console.error(`[web] ${target.error}`)
    return 1
  }
  fs.mkdirSync(target.dir, { recursive: true })
  writeAtomic(bindingPath(target.dir), `${JSON.stringify({ source: id }, null, 2)}\n`)
  console.log(`[web] 这一支（${target.id}）现在用 ${id}（${entry.name ?? id}）`)
  console.log(`[web] 绑定：${bindingPath(target.dir)}`)
  console.log('[web] 生效点 = 下一次装配（宿主重启或下一次开机；跑着的那一段不换）')
  return 0
}

function cmdRemove(homeDir, id) {
  if (!id || !ID_SHAPE.test(id)) {
    console.error('[web] 用法：node home.js web remove <id>')
    return 1
  }
  const { entries, problem, file } = readList(homeDir)
  if (problem) {
    console.error(`[web] 清单读不出来（${file}）：${problem}`)
    return 1
  }
  if (!entries.some((row) => row?.id === id)) {
    console.error(`[web] 清单里没有 ${id}`)
    return 1
  }
  // **被绑定的不许删**：删掉清单那一条，绑定就成了悬空指针（读侧会 fail-closed 拒绝启动搜索）。
  const bound = residents(homeDir).filter((name) => readBinding(P.agentEnvRoot(homeDir, name)).source === id)
  if (bound.length > 0) {
    console.error(`[web] ${id} 正被这些住户绑定：${bound.join(' / ')} —— 先让它们换一条（web use）`)
    return 1
  }
  const rows = entries.filter((row) => row?.id !== id)
  writeAtomic(file, `${JSON.stringify(rows, null, 2)}\n`)
  console.log(`[web] 已从清单删掉 ${id}（${file}）`)
  return 0
}

function cmdShow(homeDir, agentId) {
  const target = targetAgent(homeDir, agentId)
  if (target.error) {
    console.error(`[web] ${target.error}`)
    return 1
  }
  const binding = readBinding(target.dir)
  console.log(`[web] 住户：${target.id}`)
  console.log(`[web] 清单：${listPath(homeDir)}`)
  console.log(`[web] 绑定：${binding.file}`)
  if (binding.problem) {
    console.error(`[web] 绑定读不出来：${binding.problem}（读侧 fail-closed ⇒ 这一支没有「查」）`)
    return 1
  }
  if (!binding.source) {
    console.log('[web] 没绑定 ⇒ 走环境变量那一条老路（部署者的配置方式）')
    return 0
  }
  const { entries } = readList(homeDir)
  const entry = entries.find((row) => row?.id === binding.source)
  if (!entry) {
    console.error(`[web] 绑定指向 ${binding.source}，而清单里没有它（读侧会拒绝）`)
    return 1
  }
  console.log(`[web] 这一支用：`)
  console.log(describe(entry))
  return 0
}

/** `node home.js web <子命令>`。 */
function cli(argv) {
  let homeDir
  try {
    homeDir = resolveHome(argv)
  } catch (error) {
    console.error(`[web] ${error.message}`)
    return 1
  }
  const rest = argv.filter((token, index) => {
    if (token === '--home') return false
    return argv[index - 1] !== '--home'
  })
  const [subcommand, ...tail] = rest
  if (!subcommand || subcommand === '--help' || subcommand === '-h') {
    console.log(USAGE)
    return 0
  }
  let options
  try {
    options = parseArgs(tail)
  } catch (error) {
    console.error(`[web] ${error.message}`)
    return 1
  }
  switch (subcommand) {
    case 'list':
      return cmdList(homeDir)
    case 'add':
      return cmdAdd(homeDir, options)
    case 'use':
      return cmdUse(homeDir, options.positional[0], options.agent)
    case 'remove':
      return cmdRemove(homeDir, options.positional[0])
    case 'show':
      return cmdShow(homeDir, options.agent)
    default:
      console.error(`[web] 未知子命令: ${subcommand}`)
      console.error(USAGE)
      return 1
  }
}

/** `--home <目录>` 优先，否则默认家。 */
function resolveHome(argv) {
  const at = argv.indexOf('--home')
  if (at < 0) return P.homeDir()
  const value = argv[at + 1]
  if (typeof value !== 'string' || value.trim() === '') throw new Error('--home 后面要给一个目录')
  return value.trim()
}

module.exports = {
  BINDING_FILE,
  LIST_FILE,
  PROVIDERS,
  FETCH_WIRES,
  SEARCH_WIRES,
  bindingPath,
  cli,
  listPath,
  maskKey,
  nextId,
  readBinding,
  readList,
}
