#!/usr/bin/env node
// 源码守卫：**演示数据（fixtures）只该被懂它的文件消费**。
//
// 为什么需要它，而不是"写进规矩就行"：
// `src/fixtures/` 只服务演示通道（未连接）。约定「组件不得绕过 api 层直连 fixtures」
// 早已成文，但长期只是一条纯约定、没有任何机械判据 —— 代价是真的：
// `pages/NewContactPage.tsx` 曾直接 import fixtures 的供应商/模型，既不在
// `FixtureRouteGuard` 下、页内也没有任何真实世界判据 ⇒ 真实世界用户看到演示数据，
// 而且那个假的模型选择器还卡着创建按钮。
//
// ⚠ 这条守卫**不判「凭什么」**：一个文件能看到演示数据，可能是合法的（它身处演示
//   通道、或它按会话闸门决定、或它做了能力探测、或"有真实数据就走真实分支"），
//   也可能是违规的 —— 四种形状混在一起，机器分辨不了；硬分辨只会误报。
//
// ⇒ 它的正确形状是：**把导入者清单冻结起来**。任何变化（新增一个导入者、或清单里
//   的文件不再导入）都让门禁变红，逼作者每次回答一遍：
//
//     这个文件凭什么能看到演示数据？它是演示通道自身，还是有真实世界判据？
//
// 判据（只扫 `src/`）：一个文件通过 静态 import / re-export / 动态 import() /
// require() 导入 `@/fixtures/data`（相对路径写法如 `../fixtures/data.ts` 同样算），
// 且它不在下面的封闭清单里 ⇒ 失败（多了谁）；清单里有、但已不再导入 ⇒ 失败（少了谁）。
//
// 退出码：0 = 干净；1 = 命中；2 = 用法/环境错误（读不到 src/、或扫到 0 个文件）。
import { readdirSync, readFileSync } from 'node:fs'
import { join, posix, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const SRC = join(ROOT, 'src')
/** 目标归一化后的 src 相对路径（去扩展名）：`src/fixtures/data.ts`。 */
const TARGET = 'fixtures/data'
/** 只看这些扩展名——与 `check-no-host.mjs` 的 EXTS 一致（会被打包进产物的源码面）。 */
const EXTS = ['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs']
/** 源码里 import 说明符可能带的扩展名（归一化时剥掉）。 */
const SPEC_EXT = /\.[cm]?[jt]sx?$/

/**
 * 封闭清单：**允许**导入 `@/fixtures/data` 的文件（src 相对路径 → 「凭什么」）。
 *
 * 每一行都必须能回答「凭什么」——它是演示通道自身，还是有真实世界判据。
 * ⚠ 新增导入者时**不许随手加一行**：先回答那个问题；答不出来（两者皆无）就是
 *   又一处漏洞，先修它，别把它塞进清单。
 */
const ALLOWED = new Map([
  // —— 演示通道自身的页面（路由在 FixtureRouteGuard 块内，真实世界被诚实门拦下）——
  ['pages/GalleryPage.tsx', '演示通道自身的陈列室：路由 /gallery 在 FixtureRouteGuard 内，真实世界被诚实门拦下'],
  // ⚠ 2026-09-26：`pages/AgentPage.tsx` 已从本清单移除 —— 它**不再导入演示数据**。
  // 该页原先读 fixtures 的后端绑定 / 模型，而那两样是**执行端事实**，不该出现在
  // **世界侧参与者页**上（且那一分区里还挂着一个没有读数来源的「在线」徽标）；
  // 整段已撤。见 `outputs/CLIENT_DEVIATION_AUDIT_2026-09-26.md` H9 与 B3。
  ['pages/DiscoverPage.tsx', '演示通道自身的页面：路由 /app/discover 在 FixtureRouteGuard 内（Core v1 无 Directory 契约）'],
  // —— 有真实世界判据：按会话闸门（isRealWorldSession / fixtureFallbackAllowed）决定 ——
  ['pages/ContactsPage.tsx', '真实世界判据：isRealWorldSession({mode,token}) 为真时一律走真实 contacts/ownedAgents，fixtures 只在演示通道分支使用'],
  ['api/mock.ts', '演示通道自身的传输层（2026-09-25 补回）：**只有 `store.connect()` 在没有真实凭据时**才会实例化它（那正是演示通道的定义），真实世界拿到的永远是 CoreApi ⇒ 这一层根本不会被构造。它的存在就是为了让"只想看页面"的人不装家、不连 core 也能把全站跑起来'],
  ['pages/ProfilePage.tsx', '真实世界判据：isRealWorldSession({mode,token}) 为真时用真实 me/ownedAgents，fixtures 只在演示通道分支使用'],
  ['api/selfIdentity.ts', '真实世界判据：fixtureFallbackAllowed() 闸门——真实世界无身份时返空串，绝不回退 fixtures 的 SELF_ID'],
  ['components/celira/AppShell.tsx', '真实世界判据：有真实 me 用真实身份；否则 isRealWorldSession 为真渲染中性非 fixtures 占位，只有演示通道才用 fixtures 自己'],
  ['components/celira/ContactDetailCard.tsx', '真实世界判据：isRealWorldSession 决定归属/模型走真实还是 fixtures；另按 supportsRoomLive(getApi()) 能力探测决定 Live 读数'],
  // —— 有真实世界判据：能力探测（supportsXxx(getApi())），没有该能力才走演示分支 ——
  ['components/celira/MessageInput.tsx', "能力探测：attachmentInputMode(getApi()?.mode) 为 'real' 时只用真实上传，demoAttachments 仅在演示通道（无 api）分支使用"],
  ['components/celira/NewRoomSheet.tsx', '能力探测：supportsRoomCreation(getApi()) 为真时只用真实 contacts/ownedAgents 建房，fixtures participants 仅在演示通道分支使用'],
  // —— 有真实世界判据：有真实数据就走真实分支（真实成员名表由调用方注入）——
  ['components/celira/MentionText.tsx', '真实世界判据：fixtureFallbackAllowed() 闸门——真实世界 names 缺失时不回退 fixtures 演示名（空名表中性渲染）；演示通道才用 fixtures 缺省名表'],
])

function walk(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...walk(full))
    else if (entry.isFile() && EXTS.some((e) => entry.name.endsWith(e))) out.push(full)
  }
  return out
}

/**
 * 单遍扫描：剔除注释（换空格，保持下标不变），并记录每个字符串字面量的区间。
 *
 * 为什么不能只跑正则、也不能整文件子串匹配：注释与普通字符串里出现的
 * `import … from '…'` 不是真实的 import 语法，不能算（本仓就有先例——测试的**注释**里
 * 写过 `@/fixtures/data` 字样）。剔除注释后，再用"匹配起点是否落在字符串区间内"
 * 把这层也堵上。
 */
function stripComments(code) {
  const chars = code.split('')
  const stringRanges = []
  const n = code.length
  let i = 0
  while (i < n) {
    const c = code[i]
    if (c === '/' && code[i + 1] === '/') {
      while (i < n && code[i] !== '\n') {
        chars[i] = ' '
        i++
      }
      continue
    }
    if (c === '/' && code[i + 1] === '*') {
      chars[i] = ' '
      chars[i + 1] = ' '
      i += 2
      while (i < n && !(code[i] === '*' && code[i + 1] === '/')) {
        if (code[i] !== '\n') chars[i] = ' '
        i++
      }
      if (i < n) {
        chars[i] = ' '
        chars[i + 1] = ' '
        i += 2
      }
      continue
    }
    if (c === "'" || c === '"' || c === '`') {
      const start = i
      i++
      while (i < n) {
        if (code[i] === '\\') {
          i += 2
          continue
        }
        if (code[i] === c) {
          i++
          break
        }
        i++
      }
      stringRanges.push([start, i])
      continue
    }
    i++
  }
  return { code: chars.join(''), stringRanges }
}

// ⚠ 静态/`export…from` 的"子句"用 `[^'"]` 而不是 `[\s\S]`：它必须能跨行
//   （本仓多行 import 遍地都是），但**不能跨过引号**——否则会把下一条语句的字符串
//   误当成它的说明符。`check-no-host.mjs` 的正则用的是 `[^'"\n]*?`（显式排除换行），
//   对多行写法一条都匹配不到，这里**刻意不照抄**。
const SPEC_PATTERNS = [
  /(?:^|\n)[ \t]*import\b\s*(?:type\s+)?[^'"]{0,2000}?\bfrom\s*['"]([^'"]+)['"]/g,
  /(?:^|\n)[ \t]*import\s*['"]([^'"]+)['"]/g,
  /(?:^|\n)[ \t]*export\b\s*(?:type\s+)?[^'"]{0,2000}?\bfrom\s*['"]([^'"]+)['"]/g,
  /\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)/g,
  /\brequire\s*\(\s*['"]([^'"]+)['"]\s*\)/g,
]

/** 取文件里所有真实 import 说明符（静态 / re-export / 动态 / require 四种写法）。 */
function importSpecifiers(raw) {
  const { code, stringRanges } = stripComments(raw)
  const insideString = (idx) => stringRanges.some(([s, e]) => idx >= s && idx < e)
  const specs = []
  for (const re of SPEC_PATTERNS) {
    re.lastIndex = 0
    for (const m of code.matchAll(re)) {
      if (insideString(m.index)) continue
      specs.push(m[1])
    }
  }
  return specs
}

/** 把说明符归一成 src 相对路径（去扩展名）；不是 src 内的目标则返回 null。 */
function normalizeSpec(spec, importerRel) {
  let candidate
  if (spec.startsWith('@/')) candidate = spec.slice(2)
  else if (spec.startsWith('./') || spec.startsWith('../')) {
    candidate = posix.normalize(posix.join(posix.dirname(importerRel), spec))
  } else return null
  return candidate.replace(SPEC_EXT, '')
}

function readSrcFiles() {
  try {
    return walk(SRC)
  } catch (error) {
    console.error(`用法/环境错误：读不到源码目录 ${SRC} —— ${error.message}`)
    process.exit(2)
  }
}

const files = readSrcFiles()

// 门禁纪律：`PASSED` 但扫到 0 个文件是**空跑**——看着绿，其实什么都没看。
if (files.length === 0) {
  console.error(
    `用法/环境错误：在 ${SRC} 下没有扫到任何 ${EXTS.join('/')} 源文件——守卫没东西可看。` +
      '请检查 ROOT/SRC 是否指对了地方。（空跑不等于通过。）',
  )
  process.exit(2)
}

const importers = new Set()
for (const file of files) {
  const rel = relative(SRC, file).split('\\').join('/')
  const raw = readFileSync(file, 'utf8')
  for (const spec of importSpecifiers(raw)) {
    if (normalizeSpec(spec, rel) === TARGET) {
      importers.add(rel)
      break
    }
  }
}

const added = [...importers].filter((p) => !ALLOWED.has(p)).sort()
const stale = [...ALLOWED.keys()].filter((p) => !importers.has(p)).sort()

if (added.length > 0 || stale.length > 0) {
  console.error('源码守卫：演示数据（fixtures）只该被懂它的文件消费，但导入者清单变了：\n')
  if (added.length > 0) {
    console.error('  ✗ 新增的导入者（不在清单里）：')
    for (const p of added) console.error(`      ＋ src/${p}`)
    console.error(
      '\n  你必须回答一次：**这个文件凭什么能看到演示数据？**\n' +
        '    —— 它是演示通道自身的页面（路由在 FixtureRouteGuard 块内），\n' +
        '       还是有真实世界判据（isRealWorldSession / fixtureFallbackAllowed / 能力探测 / 有真实数据就走真实分支）？\n' +
        '    答完把答案写进 scripts/check-fixtures-imports.mjs 清单里那一行（路径 + 一句人话理由）。\n' +
        '    两个都不是 ⇒ 那是又一处漏洞，先修它，**别把它塞进清单**。',
    )
  }
  if (stale.length > 0) {
    console.error('\n  ✗ 陈旧的清单条目（清单里有，但已不再导入）：')
    for (const p of stale) console.error(`      － src/${p}`)
    console.error(
      '\n  清单是封闭的：文件已经不导入了，就把 scripts/check-fixtures-imports.mjs ' +
        '清单里那一行删掉。',
    )
  }
  process.exit(1)
}

console.log('fixtures import check passed')
