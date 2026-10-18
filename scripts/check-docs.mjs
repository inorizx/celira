#!/usr/bin/env node
'use strict'
/**
 * **文档判据守卫**：把两条"只写在文档里的判据"变成会红的门禁。
 *
 * ## 这个守卫补的是哪一类洞
 *
 * **判据本身已经立好了，但没有任何机械判据拿它跟文档比。**
 * 这与 `check-boundary.mjs` 记的是同一类洞，只是那一次漏在端点上、这一次漏在词上：
 *
 * - **判据一（称谓）**：`docs/runtime/AGENT_VOCABULARY.md` §9.1④ 立了"对外入口首现必须定义那个
 *   第三人称代词"。2026-10-17 实测：`EXTENDING.md` 21 处、`agent/SKILL_PACK_GUIDE.md` 42 处、
 *   `agent/MCP_SERVER_GUIDE.md` 20 处**都没有定义**——仓外读者不知道它指谁，还会额外读出
 *   **性别**与**单一连续人格**两个本仓没做过的断言（理由见 `client/DESIGN_SYSTEM.md` 与
 *   `client/scripts/check-voice.mjs` 的文件头）。三处补上之后，**没有任何东西拦着下一刀再漏**。
 *   同一个仓已经付过一次学费：`check-voice.mjs` 头里写着"清理是**一次性**的：没有守卫，
 *   下一刀就会自然写回来"。
 * - **判据二（证据锚索引）**：`outputs/README.md` 规矩 4 立了"活文档引用本目录，一处引用一经
 *   写下，就是那份文件**不得删除**的理由"。2026-10-17 实测抓到一处漏登记
 *   （`AGENT_FILES_HOME_WORKORDER_2026-10-14.md` 被 `agent/README.md` 两处与
 *   `agent/src/terminal_app.rs` 引用，索引里没有它）——**漏登记的后果是它看起来"该焚"**，
 *   而焚是照着索引做的。这条守卫把"引用 ⊆ 登记"变成机械判据。
 * - **判据三（词表条目名必须活着）**：`docs/runtime/AGENT_VOCABULARY.md` 自称"**名字的权威
 *   位置**"，而**没有任何东西量过那些名字有没有人用**。2026-10-18 实测：§9 主表 28 条里
 *   **4 条只在词表里出现**（`屏上轨迹` 全仓 1 处——就是定义它自己那一行；`视野容量` 3 处；
 *   `操作栏` 6 处；`进 App 读全文` 1 处），而大家实际在用的是**另外的名字**（回执 824 /
 *   上下文窗口 38 / 工具面 51）⇒ **读者按表里的名字去代码里找，找不到**。
 *   判据与行内豁免机制见下方那一段。
 * - **判据四（`文件:行号` 引用必须解析得到）**：判据三管"条目名有没有人用"，**没有东西管
 *   "引用指不指得到"**。2026-10-18 实测：词表 21 条引用里 **2 条是坏的**——
 *   `agent/README.md:28` **少了 `../../`**、`agent/README.md:1747` **行号漂了**
 *   （那一行讲的是 `ImFailure` 六族）。⇒ 读者按引用去翻，翻到的是**另一句话**。
 *   ⚠ **射程只有词表，这是量出来的**：全仓 md 共 38 条这类引用，`agent/README.md` 的 13 条是
 *   **裸文件名**（基准隐含在 `agent/src/`）⇒ 放大射程会立刻产生 13 条假红。见下方那一段。
 *
 * ⇒ 四条判据都是"**声明即受约束**"：一个文件自称对外入口，它就接受判据一；
 * 一份 outputs 文件被活文档点名，它就接受判据二；一个词进了 §9 主表，它就接受判据三；
 * 一条引用写进词表，它就接受判据四。
 *
 * ## 判据方向（写死，别反过来）
 *
 * 1. **对外入口 ∧ 正文用了那个代词 ⇒ 文件头必须定义一次**。零处代词的文件免（不需要定义
 *    一个它没用的词）。
 * 2. **被活文档按名字引用的 `outputs/*.md` ⊆ `outputs/README.md` 的索引**。
 *    反方向（登记了、文件却不在盘上）**不报红**——那可能是已焚，也可能是"删了没记账"，
 *    **脚本分不出这两者**（已焚的名字也登记在索引里）。这一条要靠人读"已焚"那张表。
 * 3. **§9 主表的每行主名，在词表之外至少被引用一次**。⚠ **一行只判主名**——括号里那个是
 *    **别名**，别名不要求独立引用：要求它活着等于要求"两个名字都留着"，与 §9.1① 自相矛盾。
 *    确实只在词表里用的概念名，**在那一行写「豁免判据三」并把理由写在同处**。
 * 4. **词表里的每条 `文件:行号`：路径解析得到、行号在范围内**。⚠ **射程只有词表**——
 *    别的文档用裸文件名（基准隐含），放大射程就是一堆假红。有意保留的坏引用，
 *    **在那一行写「豁免判据四」并把理由写在同处**。
 *
 * ## 它量得到什么、量不到什么（如实登记）
 *
 * - 量得到：对外入口有没有**定义过**那个代词；被引用的 outputs 文件有没有**登记**；
 *   两张入口表与磁盘上的候选文档**是否恰好一一对上**（新加一个入口文档会自动要一个理由）；
 *   词表主名在**词表之外**还出不出现（"表名与实现分家"的下限探测）；
 *   词表里的 `文件:行号` 引用**指不指得到**。
 * - 量**不到**：定义写得**对不对**（脚本只认"代词与『指』同现"这个形状，认不出定义写歪了）；
 *   索引里那条**该不该**是证据锚；`outputs/README.md` 的 §1 与 §2 有没有归错类；
 *   **词表之外那个同名字指的是不是同一个东西**（`includes` 只回答"这个字符串还在不在别处
 *   出现"），也量不到那个名字用得**对不对**；
 *   ⚠ **判据四量不到"行号漂移"**——路径在、行号也在范围内、只是**内容换了**的那些，它
 *   **一声不响**（`agent/README.md:1747` 就是这一形态）。它只挡"指不到"，**挡不住"指错了"**。
 *   本脚本只做"有没有"这一层，不假装管更多。
 *
 * ⚠ **本文件不许写出那个字**：写成转义常量 `PRONOUN_SHE`，判据与夹具都用它拼。
 *   这不是洁癖——`check-voice.mjs` 的头里记着理由：守卫要是写得出它，就会被"清一遍"的
 *   机械替换顺手改掉，守卫当场失效。**这件事在 `agent/` 那一侧真实发生过一次。**
 *
 * ⚠ **本文件也在自己的扫描面里**（`.mjs` 在 `EXTS` 里，而扫描覆盖全仓）⇒ 自检夹具里的
 *   `outputs/*.md` **必须拼出来、不许写字面量**，否则守卫会抓到自己的夹具。实测踩过一次。
 *
 * 用法：
 *   node scripts/check-docs.mjs            # 检查（随 `npm run verify` 跑）
 *
 * 退出码：0 = 全通过；1 = 有违例（或自检没抓住故意埋的违例）。
 */
import { readdirSync, readFileSync, existsSync } from 'node:fs'
import { join, relative, sep, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
/** 被禁的那个人称代词，**写成转义**（见文件头：本文件自己不许出现它）。 */
const PRONOUN_SHE = '\u5979'
/** 「文件头」= 前多少行。定义块住在文件头的引言里，不在正文。 */
const HEAD_WINDOW = 20
/** 只看这些扩展名——它们是"会引用 outputs 的活文档"的面。 */
const EXTS = ['.md', '.rs', '.ts', '.tsx', '.js', '.mjs', '.py']

/**
 * 对外入口：读者是**仓外**的人 ⇒ 受判据一约束。
 * 判据：正文用了那个代词，文件头必须定义一次。
 */
const DOC_ENTRY_FILES = [
  ['README.md', '仓外读者的第一份'],
  ['README.zh-CN.md', '同上，中文'],
  ['EXTENDING.md', '给想给 Celira 加东西的人'],
  ['examples/README.md', '能直接抄的样例'],
  ['im/server/README.md', '世界的部署文档（部署者在仓外）'],
  ['client/THEME_GUIDE.md', '主题作者'],
  ['agent/SKILL_PACK_GUIDE.md', '技能包作者'],
  ['agent/MCP_SERVER_GUIDE.md', 'MCP server 作者'],
]

/**
 * 内部：读者是维护者与主人，**共享上下文** ⇒ 不受判据一约束。
 * 那个代词在这里是**承重的内部行话**（`agent/README.md` 有 350+ 处），不是缺陷。
 */
const INTERNAL_DOCS = [
  ['AGENTS.md', '每轮自动注入的工作区指令'],
  ['ARCHITECTURE.md', '唯一 C4 系统地图，维护者读'],
  ['agent/README.md', '当前实现事实的活文档'],
  ['app/README.md', '桌面壳的实现状态'],
  ['client/README.md', '前端实现边界'],
  // ⚠ 第三格是**可选标记**（2026-10-18 加）：`true` ＝「这一条**可以不在盘上**」。
  // 为什么只有它：**证据库整目录不进公开仓**，而这份索引是它的门面 ⇒ 公开仓里它必然缺席，
  // 而在维护者那一侧它必须在。⇒ 这一格不是白名单（白名单是「什么都不查」），
  // 它是**把「这一条为什么可以不在」写在同一行里**，而且只给这一个条目。
  ['outputs/README.md', '证据库索引', true],
  ['docs/runtime/terminal/README.md', '终局设计入口'],
]

/** 扫描时跳过的路径（不是本仓的活文档：证据库自己、vendored 第三方、工具 scratch、构建产物）。 */
const SKIP = /(^|[\\/])(node_modules|target|\.git|outputs|dist-release|\.tmp-codex|\.workbuddy|\.trae|\.backups|\.pytest_cache|\.dsh-probe|references)([\\/]|$)/

/**
 * 草稿/临时文件：**不是入口文档，不受判据一管**。
 *
 * 判据一管的是"一个文件**自称对外入口**"——而自称的方式是**它被提交进仓**（`README.md` /
 * `*_GUIDE.md` / 根级 `*.md` 这些位置本身就是声明）。临时草稿没有这个声明：它是别人
 * 正在写的中间产物，下一刻可能就没了。
 *
 * 2026-10-17 实测：仓根一个 `.tmp_33t.md`（别的会话的草稿）被判据一判红 ⇒ **假阳性**。
 * ⇒ 按通行草稿命名排除，与 `.gitignore` 的常规集合同形。
 */
const SCRATCH_RE = /^(\.tmp|tmp_|\.#|#.*#|.*~$|.*\.tmp$|.*\.bak$|.*\.orig$)/

/** 候选入口文档：根级 `*.md`、任何 `README.md`、任何 `*_GUIDE.md`。草稿除外。 */
function isEntryCandidate(rel) {
  const base = rel.split('/').pop()
  if (SCRATCH_RE.test(base)) return false
  if (base === 'README.md') return true
  if (base.endsWith('_GUIDE.md')) return true
  return !rel.includes('/') && base.endsWith('.md')
}

function walk(dir, out = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const abs = join(dir, e.name)
    const rel = relative(ROOT, abs).split(sep).join('/')
    if (SKIP.test(rel)) continue
    if (e.isDirectory()) walk(abs, out)
    else if (EXTS.some((x) => e.name.endsWith(x))) out.push(rel)
  }
  return out
}

/** 判据一：对外入口用了那个代词 ⇒ 文件头必须定义一次。 */
function checkPronoun(rel, text, isOutward) {
  if (!isOutward || !text.includes(PRONOUN_SHE)) return []
  const head = text.split(/\r?\n/).slice(0, HEAD_WINDOW).join('\n')
  const defined =
    new RegExp(`${PRONOUN_SHE}[^。\\n]{0,40}指`).test(head) ||
    new RegExp(`(称谓|指)[^。\\n]{0,40}${PRONOUN_SHE}`).test(head)
  if (defined) return []
  return [
    `${rel}：正文用了那个第三人称代词，但文件头（前 ${HEAD_WINDOW} 行）没有定义它指谁 ⇒ ` +
      `仓外读者不知道它指谁，还会读出性别与"单一连续人格"两个本仓没做过的断言。` +
      `加一行（照另外三份对外入口的样子）：` +
      `⚠ **称谓**：本文用「${PRONOUN_SHE}」指**你的那一支 Agent**——叙述视角，不是名字，也不断言性别。` +
      `用法判据见 docs/runtime/AGENT_VOCABULARY.md §9.1④。`,
  ]
}

const REF_RE = /outputs\/([A-Za-z0-9_-]+\.md)/g

/** 从一段文本里取出它按名字引用的 outputs 文件。 */
function findOutputsRefs(text) {
  const out = new Set()
  let m
  REF_RE.lastIndex = 0
  while ((m = REF_RE.exec(text)) !== null) out.add(m[1])
  return [...out]
}

/** 判据二：被引用的 outputs 文件 ⊆ 索引里登记的名字。 */
function checkIndex(refs, registered) {
  const problems = []
  for (const r of refs) {
    if (r === 'README.md') continue // 索引不自列
    if (!registered.has(r)) {
      problems.push(
        `outputs/${r}：被活文档按名字引用，但 outputs/README.md 的索引里没有它 ⇒ ` +
          `一处引用就是"不得删除"的理由（规矩 4）。` +
          `**漏登记的后果是它看起来"该焚"，而焚正是照着索引做的。**`,
      )
    }
  }
  return problems
}

/** 索引里登记的名字 = `outputs/README.md` 里任何一行 ``| `X.md` |`` 的第一格。 */
function registeredNames(indexText) {
  const out = new Set()
  for (const line of indexText.split(/\r?\n/)) {
    const m = /^\|\s*`([A-Za-z0-9_-]+\.md)`\s*\|/.exec(line)
    if (m) out.add(m[1])
  }
  return out
}

// ---------------------------------------------------------------------------
// 判据三：词表条目名必须**活在词表之外**（2026-10-18 立）
// ---------------------------------------------------------------------------
//
// **它补的是哪一类洞**：词表是"名字的权威位置"，但**没有任何东西量过那些名字有没有人用**。
// 实测（2026-10-18）：§9 主表 28 条里有 4 条**只在词表里出现**——`屏上轨迹` 全仓 1 处
// （就是定义它自己那一行）、`视野容量` 3 处、`操作栏` 6 处、`进 App 读全文` 1 处，
// 而大家实际在用的是**另外的名字**（回执 824 / 上下文窗口 38 / 工具面 51）。
// ⇒ **词表记了一个没人用的名字，而活名不在表里**——读者按表里的名字去代码里找，找不到。
//
// **判据一句话**：§9 主表的每一行，其**主名**必须在词表之外**至少被引用一次**。
//
// ⚠ **一行只判一个主名**：括号里那个是**别名**，别名不要求独立引用——它本来就该退场
// （见 §9.1①）。要求别名也活着，等于要求"两个名字都留着"，与那条登记自相矛盾。
//
// ⚠ **豁免写在行里**（照 `INTERNAL_DOCS` 里 `outputs/README.md` 那条的先例）：确实只在
// 词表里用的概念名，就在**它自己那一行**写「豁免判据三」并把理由写在同处。
// **白名单是「什么都不查」，这个标记不是**——它要求理由跟着那一行走，人读得到。
//
// **量不到什么（如实登记）**：量不到"这个名字用得**对不对**"，也量不到"词表之外那个同名字
// 指的是不是同一个东西"——`includes` 只回答"这个字符串还在不在别处出现"。
// 它是**分家的下限探测**，不是语义一致性证明。

/** 名字的权威位置。**本文件只读它，不改它**。 */
const VOCAB_FILE = 'docs/runtime/AGENT_VOCABULARY.md'
/** 行内豁免标记（见上）。 */
const VOCAB_EXEMPT = '豁免判据三'

/**
 * 从词表 §9 主表取出每行的**主名**与它有没有行内豁免。
 *
 * 主名 = 名字列里第一个分隔符（`（` / `/` / `·`）之前的那一段，去掉强调与反引号。
 * **表读不到时返回 `null`**（调用方据此报红）——不返回空数组：空数组会假装"没有条目要查"。
 */
function vocabMainNames(text) {
  const lines = String(text ?? '').split(/\r?\n/)
  const headerAt = lines.findIndex((l) => /^\|\s*名字\s*\|\s*是什么\s*\|\s*权威在哪\s*\|/.test(l))
  if (headerAt < 0) return null
  const out = []
  for (let i = headerAt + 1; i < lines.length; i++) {
    const line = lines[i]
    if (!line.startsWith('|')) break
    if (/^\|[\s:-]+\|/.test(line)) continue
    const cells = line.split(/(?<!\\)\|/)
    if (cells.length < 4) continue
    const raw = cells[1].trim()
    const primary = raw
      .replace(/\*\*/g, '')
      .replace(/`/g, '')
      .split(/[（/·]/)[0]
      .trim()
    if (primary) out.push({ name: primary, exempt: line.includes(VOCAB_EXEMPT) })
  }
  return out
}

/** 判据三：主名在词表之外至少被引用一次；做不到就得在那一行写明理由。 */
function checkVocabLive(names, contents) {
  const problems = []
  for (const { name, exempt } of names) {
    if (exempt) continue
    const used = contents.some(([rel, text]) => rel !== VOCAB_FILE && text.includes(name))
    if (!used) {
      problems.push(
        `${VOCAB_FILE}：条目「${name}」在**词表之外一处都没有用过** ⇒ ` +
          `这是"表名与实现分家"：词表记了一个没人用的名字，而大家实际用的那个不在表里。` +
          `两条出路——**改成活名**（照「回执」行：表名全仓 1 处、活名 824 处 ⇒ 已改正），` +
          `或在那一行写「${VOCAB_EXEMPT}」并把理由写在同处。`,
      )
    }
  }
  return problems
}

// ---------------------------------------------------------------------------
// 判据四：词表里的 `文件:行号` 引用必须解析得到（2026-10-18 立）
// ---------------------------------------------------------------------------
//
// **它补的是哪一类洞**：判据三管"条目名有没有人用"，**没有东西管"引用指不指得到"**。
// 实测（2026-10-18）：词表 21 条 `文件:行号` 引用里 **2 条是坏的**——
// `agent/README.md:28` **少了 `../../`**（同文其他 4 处都写了）、
// `agent/README.md:1747` **行号漂了**（那一行讲的是 `ImFailure` 六族，真正的经历账在 `:133`）。
// ⇒ 读者按引用去翻，翻到的是**另一句话**。
//
// **判据一句话**：词表里的每条 `文件:行号`，**路径要解析得到、行号要在范围内**。
//
// ⚠ **射程只有词表，这是量出来的、不是省事**（2026-10-18；**同日晚些时候更正过一次**）：
// 全仓 md 共 38 条这类引用，`agent/README.md` 占 **12** 条，而那 12 条**基准不统一**——
// `core.rs:2672` / `mcp/app.rs:67` / `bin/agent_host.rs:445-448` 相对 **`agent/src/`**；
// `src/wake.rs:26` / `src/os.rs:1` 相对 **`agent/`**；`HOME_MANUAL.md:261` 相对 **`scripts/`**。
// 其中还有 **2 条是"引述历史"**：`AGENT_PHONE_MODEL.md:25` 与**划掉的** `src/screen.rs:1`
// ——那是 2026-09-25 只读审计那一行里的**旧名**（同一行写着"已解决、该文件今天叫…"），
// **故意指不到**，改了就是伪造记录。
// ⇒ 放大的话：既要**多基准**（判据退化成启发式），又要给"引述历史"开豁免（那一格只有人
// 读得出）——**两样都会让这条判据变软**。⇒ 只判**基准唯一**的那一份（词表：21 条，全部按
// 它自己所在目录解析）。
//
// ⚠ **本条自我更正**（2026-10-18）：早先这里写「`agent/README.md` 的 13 条是裸文件名、
// 基准隐含在 `agent/src/`」——**两处都不准**：数错一条，基准也不止一个，而 `src/screen.rs`
// 更是**引述历史**、根本不是活引用（差一点就把一行审计记录"修"成假话）。
//
// ⚠ **豁免写在行里**（与判据三同规）：确实指不到、但有意保留的引用，在**那一行**写
// 「豁免判据四」并写明理由。
//
// **量不到什么（如实登记，这一格很重要）**：它**量不到"那一行还是不是当初那一句话"**。
// `agent/README.md:1747` 那一条就是**行号漂移**——路径在、行号也在范围内，只是**内容换了**，
// 本判据**一声不响**。⇒ 它只挡"指不到"，**挡不住"指错了"**；后者只有人核。

/** `文件:行号` 引用：`path:NNN` / `path:NNN-MMM` / `path:NNN,MMM`。 */
const CITE_RE = /([A-Za-z0-9_./-]+\.(?:md|rs|js|mjs|ts|tsx|py)):(\d+(?:\s*[-,]\s*\d+)*)/g
/** 行内豁免标记（见上）。 */
const CITE_EXEMPT = '豁免判据四'

/** 把 `3-5,2` 那样的尾巴拆成行号数组。 */
function citeNumbers(tail) {
  return String(tail)
    .split(/[-,]/)
    .map((s) => Number.parseInt(s.trim(), 10))
    .filter((n) => Number.isInteger(n))
}

/**
 * 判据四：`文件:行号` 引用必须解析得到。
 *
 * `probe(absPath)` 返回该文件的**行数**，文件不在返回 `null` —— 由调用方给，
 * 于是自检可以塞一个假的进去（不必在盘上造文件）。
 */
function checkCitations(text, baseDir, probe) {
  const problems = []
  const seen = new Set()
  let m
  CITE_RE.lastIndex = 0
  while ((m = CITE_RE.exec(text)) !== null) {
    const rel = m[1]
    const raw = m[2]
    const key = `${rel}:${raw}`
    if (seen.has(key)) continue
    seen.add(key)
    const abs = join(baseDir, rel)
    const total = probe(abs)
    if (total === null) {
      problems.push(
        `引用 \`${rel}:${raw}\`：**路径解析不到**（按本文所在目录解析为 \`${abs}\`）⇒ ` +
          `读者按引用去翻，翻不到东西。两条出路——补全相对路径（照同文其他引用的写法），` +
          `或在那一行写「${CITE_EXEMPT}」并写明理由。`,
      )
      continue
    }
    for (const n of citeNumbers(raw)) {
      if (n < 1 || n > total) {
        problems.push(
          `引用 \`${rel}:${n}\`：**行号超出**（该文件只有 ${total} 行）⇒ ` +
            `文件缩过或引用写错。**注意本判据查不出"行号漂移"**——` +
            `路径在、行号也在范围内、只是内容换了的那些，它一声不响。`,
        )
      }
    }
  }
  return problems
}

// 自检：故意埋的违例**必须**被抓到（否则这个守卫就是装饰）
function selfCheck() {
  const missed = []
  const noDef = `${PRONOUN_SHE}开场看到一行读数。`
  const withDef =
    `> ⚠ **称谓**：本文用「${PRONOUN_SHE}」指**你的那一支 Agent**。\n\n` +
    `正文：${PRONOUN_SHE}开场看到一行读数。`

  if (checkPronoun('<self-check>', noDef, true).length !== 1)
    missed.push('称谓判据自检：没定义的那条**必须**被抓到')
  if (checkPronoun('<self-check>', withDef, true).length !== 0)
    missed.push('称谓判据自检：定义过的那条**不许**报红')
  if (checkPronoun('<self-check>', noDef, false).length !== 0)
    missed.push('称谓判据自检：内部文档**不许**被这条判据管')
  if (checkPronoun('<self-check>', '这里一个代词都没有。', true).length !== 0)
    missed.push('称谓判据自检：零代词的文件**不许**被要求定义')

  const refs = findOutputsRefs(
    // ⚠ 夹具**拼**出来、不写字面量：本文件也在扫描面里（`.mjs`），写「outputs/ + 文件名」这种
    //   字面量会让守卫抓到**自己的夹具**（实测踩过两次：第一版报夹具没登记；修的时候又在注释里
    //   写了一遍那个字面量，于是照旧报红）。
    `见 \`outputs/${'AAA.md'}\` 与 \`outputs/${'BBB.md'}\`，还有 \`outputs/${'AAA.md'}\``,
  )
  if (refs.length !== 2)
    missed.push(`引用解析自检：期望 2 条（去重），实得 ${refs.length} 条`)
  if (checkIndex(['AAA.md'], new Set(['BBB.md'])).length !== 1)
    missed.push('索引判据自检：没登记的那条**必须**被抓到')
  if (checkIndex(['AAA.md'], new Set(['AAA.md'])).length !== 0)
    missed.push('索引判据自检：登记过的那条**不许**报红')
  if (checkIndex(['README.md'], new Set()).length !== 0)
    missed.push('索引判据自检：索引自己**不许**被要求自列')
  if (registeredNames('| `X.md` | 活文档 |\n| 不是表格行 |\n') .size !== 1)
    missed.push('索引解析自检：只认 ``| `X.md` |`` 那种行')

  // 草稿排除自检（2026-10-17 加）：**必须**放行草稿，**必须**照旧抓住正经的入口候选。
  // 只测放行不测抓住，就会退化成"什么都不查"——那正是这条自检存在的理由。
  if (isEntryCandidate('.tmp_33t.md')) missed.push('草稿排除自检：`.tmp_*` 必须放行（实测踩过一次假阳性）')
  if (isEntryCandidate('tmp_notes.md')) missed.push('草稿排除自检：`tmp_*` 必须放行')
  if (isEntryCandidate('NOTES.md.bak')) missed.push('草稿排除自检：`*.bak` 必须放行')
  if (!isEntryCandidate('NOTES.md')) missed.push('草稿排除自检：正经的根级 *.md **不许**被放行')
  if (!isEntryCandidate('agent/SKILL_PACK_GUIDE.md')) missed.push('草稿排除自检：`*_GUIDE.md` **不许**被放行')
  if (!isEntryCandidate('im/server/README.md')) missed.push('草稿排除自检：任何 `README.md` **不许**被放行')

  // 判据三自检：**必须**抓住"只在词表里用的名字"，**必须**放行行内豁免的那条。
  // ⚠ 夹具里的名字**拼出来、不写字面量**：本文件也在扫描面里（`.mjs`），写死会让守卫抓到
  //   自己的夹具 —— 判据二那里为同一个坑踩过两次（连注释里写一遍都会红）。
  const DEAD = '只在词表' + '里的名字'
  const EXEMPTED = '另一个' + '名字'
  const planted =
    '| 名字 | 是什么 | 权威在哪 |\n|---|---|---|\n' +
    `| ${DEAD} | 一个没有实现的格子 | 无 |\n` +
    `| ${EXEMPTED} | 豁免了 | 无（${VOCAB_EXEMPT}：理由写在行里） |\n`
  const plantedNames = vocabMainNames(planted)
  if (plantedNames === null) missed.push('判据三自检：主名解析读不到表头（应当读得到）')
  else if (plantedNames.length !== 2)
    missed.push(`判据三自检：主名解析期望 2 条，实得 ${plantedNames.length}`)
  else {
    // 词表之外零引用 ⇒ 未豁免的那条必须红，豁免的那条必须不红。
    const onlyInVocab = [[VOCAB_FILE, `${DEAD} ${EXEMPTED}`]]
    const found = checkVocabLive(plantedNames, onlyInVocab)
    if (found.length !== 1)
      missed.push(`判据三自检：零引用时未豁免的那条**必须**被抓到（实得 ${found.length} 条）`)
    // 别处用过 ⇒ 不许红。
    const usedElsewhere = [['some.md', DEAD], [VOCAB_FILE, EXEMPTED]]
    if (checkVocabLive(plantedNames, usedElsewhere).length !== 0)
      missed.push('判据三自检：词表之外用过的那条**不许**报红')
    // 只在词表里出现（正是"屏上轨迹"那一形态）⇒ 必须红。
    if (checkVocabLive(plantedNames, [[VOCAB_FILE, DEAD], ['other.md', '无关正文']]).length !== 1)
      missed.push('判据三自检：只在词表里出现的那条**必须**被抓到')
  }
  if (vocabMainNames('这里根本没有表') !== null)
    missed.push('判据三自检：表读不到时**必须**返回 null（不假装通过）')

  // 判据四自检：坏路径与越界**必须**被抓到，解析得到的**不许**报红。
  // ⚠ 与判据二/三不同，本判据的扫描面**只有词表**（见它那一段的段头），所以夹具写在这里
  //   不会被自己抓到——**不需要**拼字符串（判据二为那个坑踩过两次）。
  {
    const okAbs = join('/base', 'ok.md')
    const probe = (abs) => (abs === okAbs ? 5 : null)
    const cases = [
      ['见 `./ok.md:3`。', 0, '解析得到的引用**不许**报红'],
      ['见 `./nope.md:1`。', 1, '路径解析不到**必须**被抓到'],
      ['见 `./ok.md:99`。', 1, '行号越界**必须**被抓到'],
      ['见 `./ok.md:3-9,2`。', 1, '`3-9,2` 里越界的那一个**必须**被抓到'],
      ['见 `./ok.md:1-5`。', 0, '范围全在界内**不许**报红'],
      ['见 `./ok.md:0`。', 1, '行号 0（越下界）**必须**被抓到'],
    ]
    for (const [text, want, why] of cases) {
      const got = checkCitations(text, '/base', probe).length
      if (got !== want) missed.push(`判据四自检：${why}（期望 ${want} 条，实得 ${got} 条）`)
    }
  }

  return missed
}

function main() {
  const problems = []
  const files = walk(ROOT)
  const outward = new Map(DOC_ENTRY_FILES)
  const internal = new Map(INTERNAL_DOCS)

  // 0) 两张表自洽：候选入口文档**恰好**在一张表里 ⇒ 新加一个入口文档会自动要一个理由。
  for (const rel of files.filter(isEntryCandidate)) {
    const inOut = outward.has(rel)
    const inInt = internal.has(rel)
    if (inOut && inInt)
      problems.push(`${rel}：同时在 DOC_ENTRY_FILES 与 INTERNAL_DOCS 里，必须恰好在一张表里`)
    else if (!inOut && !inInt)
      problems.push(
        `${rel}：是入口候选（根级 *.md / README.md / *_GUIDE.md），但两张表里都没有它 ⇒ ` +
          `登记进 DOC_ENTRY_FILES（读者是仓外的人）或 INTERNAL_DOCS（读者是维护者与主人），并写一句理由`,
      )
  }
  for (const [rel, , optional] of [...DOC_ENTRY_FILES, ...INTERNAL_DOCS]) {
    if (optional && !existsSync(join(ROOT, rel))) continue
    if (!existsSync(join(ROOT, rel))) problems.push(`${rel}：登记在表里，文件却不在`)
  }

  // 1) 称谓判据
  for (const rel of outward.keys()) {
    if (!existsSync(join(ROOT, rel))) continue
    problems.push(...checkPronoun(rel, readFileSync(join(ROOT, rel), 'utf8'), true))
  }

  // 2) 证据锚索引判据
  //
  // ⚠ **不适用就报 `N/A`**（2026-10-18 加）：公开仓里**没有证据库**（那是维护者那一侧的东西，
  // 整目录不发表）⇒ 这里以前直接 `readFileSync` 会抛 ENOENT，**把整条门禁崩掉**
  // （实测：`Error: ENOENT: … <仓根>\outputs\README.md`，就是下面那一行）。
  // 按本仓的门禁纪律：**不适用报 `N/A`，既不算通过也不算失败**。
  // ⚠ 但**判据一照旧要跑**——它管的是称谓，与证据库在不在无关 ⇒ 这里只跳过判据二、**不改退出码**。
  const indexPath = join(ROOT, 'outputs', 'README.md')
  const hasIndex = existsSync(indexPath)
  let refs = new Set()
  let registered = new Set()
  if (!hasIndex) {
    console.log(
      `[check-docs] 判据二 N/A：本仓没有证据库索引（${indexPath}）⇒ 证据锚索引判据不适用`,
    )
  } else {
    registered = registeredNames(readFileSync(indexPath, 'utf8'))
    for (const rel of files) {
      if (rel === 'outputs/README.md') continue
      for (const r of findOutputsRefs(readFileSync(join(ROOT, rel), 'utf8'))) refs.add(r)
    }
    problems.push(...checkIndex([...refs], registered))
  }

  // 3) 词表条目名判据
  //
  // ⚠ **没有词表就报 `N/A`**（与判据二同规）：不适用既不算通过也不算失败。
  // ⚠ 但**表在、表头读不到时要红**——那是"判据无法执行"，不是"不适用"。
  const vocabPath = join(ROOT, VOCAB_FILE)
  let vocabNames = null
  let citeCount = 0
  if (!existsSync(vocabPath)) {
    console.log(`[check-docs] 判据三 N/A：本仓没有词表（${vocabPath}）`)
  } else {
    vocabNames = vocabMainNames(readFileSync(vocabPath, 'utf8'))
    if (vocabNames === null) {
      problems.push(
        `${VOCAB_FILE}：读不到 §9 主表（表头「| 名字 | 是什么 | 权威在哪 |」）⇒ ` +
          `判据三无法执行，**不假装通过**`,
      )
    } else {
      const contents = files.map((rel) => [rel, readFileSync(join(ROOT, rel), 'utf8')])
      problems.push(...checkVocabLive(vocabNames, contents))
    }
  }

  // 4) 词表引用判据
  //
  // ⚠ 射程**只有词表**（理由与实测见判据四那一段的段头：全仓 38 条里有 13 条是裸文件名、
  //    基准隐含，放大射程会立刻产生 13 条假红）。
  if (vocabNames !== null) {
    const vocabText = readFileSync(vocabPath, 'utf8')
    const vocabDir = dirname(vocabPath)
    citeCount = (vocabText.match(CITE_RE) || []).length
    const probe = (abs) =>
      existsSync(abs) ? readFileSync(abs, 'utf8').split(/\r?\n/).length : null
    problems.push(...checkCitations(vocabText, vocabDir, probe))
  }

  const missed = selfCheck()
  const all = [...problems, ...missed]
  if (all.length) {
    console.error(`[check-docs] ${all.length} 处不合格：`)
    for (const p of all) console.error(`  ✗ ${p}`)
    process.exit(1)
  }
  console.log(
    `[check-docs] 通过：对外入口 ${outward.size} 份（称谓判据）· ` +
      (hasIndex
        ? `被引用的 outputs 文件 ${refs.size} 份 ⊆ 索引 ${registered.size} 个名字`
        : '判据二 N/A（本仓没有证据库）') +
      ' · ' +
      (vocabNames
        ? `词表 ${vocabNames.length} 条主名都在词表之外活着（判据三）· ` +
          `${citeCount} 条 \`文件:行号\` 引用都解析得到（判据四）`
        : '判据三／四 N/A（本仓没有词表）') +
      ' · 自检全过',
  )
}

main()
