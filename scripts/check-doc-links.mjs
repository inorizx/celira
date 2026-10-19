#!/usr/bin/env node
'use strict'
/**
 * **文档链接守卫**：跟踪集里每一份 `*.md` 的**相对链接与图片**都必须解析得到。
 *
 * ## 为什么需要它
 *
 * 仓里已经有一条判据管"引用指不指得到"（`check-docs.mjs` 判据四），但它的**射程只有词表**，
 * 而且只管 `文件:行号` 那一种写法。**前门那些 `[文字](./路径)` 与 `![图](./路径)` 没人查**——
 * 而它们正是仓外读者**唯一**能走的路：一条指不到的链接，对读者就是"这份文档不可信"。
 * 这一条与 `check-voice`／`check-docs` 同族：**判据只写在文档里，就得有东西拿它跟盘比。**
 *
 * ## 判据（一条，只认相对路径）
 *
 * 一份 `.md` 里每一条 `[文字](目标)` / `![替代](目标)`：
 * - **外部**（`http:` / `https:` / `mailto:` / `ftp:`）⇒ 不查（网不在盘上）；
 * - **纯锚点**（`#…`）⇒ 不查（同一份文件内）；
 * - **`outputs/…`** ⇒ **跳过并计数**：那个目录**整份不随本仓发表**，门后文档按名字引用它是
 *   **承重的**（`AGENTS.md`：一处引用就是那份文件"不得删除"的理由）⇒ 它不是"死链"，
 *   是**一条出处**（边界声明见 `README.md` 的「仓库地图与命名」末段）。⚠ 前门里出现
 *   **具体的** `outputs/X.md` 由 `check-docs.mjs` 判据五管，**不在这里重复**。
 * - **其余相对路径** ⇒ **必须在盘上**（去掉 `#锚点`、去掉 `<…>` 包裹、`%20` 解码之后再查）。
 *
 * ⚠ **代码块里的不算**（``` 围起来的例子常常写着并不存在的路径）。
 * ⚠ **它量不到**：目标文件里**有没有那个锚点**（`#某一节`）、外部 URL 活不活、链接的**文字**
 *   说得对不对。本脚本只做"文件在不在"这一层，不假装管更多。
 *
 * ## 用法与退出码
 *
 * ```text
 * node scripts/check-doc-links.mjs
 * ```
 *
 * 退出码：0 = 全通过；1 = 有指不到的链接（或自检没抓住故意埋的坏链接）。
 * ⚠ 纯源码判据（只用 `node:fs` 与一次 `git ls-files`）⇒ 进 CI 的第一档。
 */
import { existsSync, readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { dirname, join, normalize, relative, sep } from 'node:path'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
/** 门后那一格：不是死链，是一条出处（见文件头）。 */
const BEHIND_THE_DOOR = /^\.?\/?outputs\//
/** 外部目标：网不在盘上，不查。 */
const EXTERNAL = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i

/** 一行里所有的 `[文字](目标)` / `![替代](目标)`（含 `<…>` 包裹那种）。 */
function linksIn(line) {
  const out = []
  const pattern = /!?\[[^\]]*\]\(\s*(<[^>]*>|[^)\s]+)(?:\s+"[^"]*")?\s*\)/g
  let match
  while ((match = pattern.exec(line)) !== null) out.push(match[1])
  return out
}

/** 去掉 `<…>` 包裹、`#锚点`、`?查询`，并解码 `%20`。 */
function normalizeTarget(raw) {
  let target = String(raw).trim()
  if (target.startsWith('<') && target.endsWith('>')) target = target.slice(1, -1)
  target = target.split('#')[0].split('?')[0]
  try {
    target = decodeURIComponent(target)
  } catch {
    /* 解不开就按原样查（它本来就不像一个路径） */
  }
  return target
}

/**
 * 一份 md 文本里的**指不到的相对链接**（`(行号, 目标)`）。
 *
 * ⚠ 代码块（``` 围起来）里的不算——那里常常写着例子路径。
 */
function brokenLinksIn(text, fileDir, exists = existsSync) {
  const out = []
  let inFence = false
  String(text)
    .split(/\r?\n/)
    .forEach((line, index) => {
      if (/^\s*(?:```|~~~)/.test(line)) {
        inFence = !inFence
        return
      }
      if (inFence) return
      for (const raw of linksIn(line)) {
        const target = normalizeTarget(raw)
        if (target === '' || EXTERNAL.test(target) || BEHIND_THE_DOOR.test(target)) continue
        const full = normalize(join(fileDir, target))
        if (!exists(full)) out.push({ line: index + 1, target })
      }
    })
  return out
}

/**
 * **锚点**：同文件里 `[文字](#某一节)` 指的那一节**还在不在**。
 *
 * 判据是"标题还在"：改一个标题会让所有指向它的锚点**静默失效**（读者点过去停在页首），
 * 而链接文字看上去毫无问题——这类坏法没有任何东西会报。今天全仓只有 4 处同文件锚点。
 *
 * ⚠ **松匹配**：把标题按 GitHub 的规矩粗加工（小写、空格换 `-`、去掉标点）之后，
 * 判据是"锚点是它的**子串**"——不追求逐字复刻 GitHub 的算法（复刻不准就会假红）。
 * ⚠ 跨文件带锚点的那两处**不在射程里**（要读另一份文件的标题；今天为 0 处需要）。
 */
function headingSlug(heading) {
  return String(heading)
    .replace(/^#+\s*/, '')
    .trim()
    .toLowerCase()
    .replace(/\s+/g, '-')
    .replace(/[^a-z0-9\u3400-\u9fff\uf900-\ufaff-]/g, '')
}

/** 同文件锚点里指不到的那几条（`(行号, 锚点)`）。 */
function brokenAnchorsIn(text) {
  const out = []
  const slugs = String(text)
    .split(/\r?\n/)
    .filter((line) => /^#{1,6}\s/.test(line))
    .map((line) => headingSlug(line))
  let inFence = false
  String(text)
    .split(/\r?\n/)
    .forEach((line, index) => {
      if (/^\s*(?:```|~~~)/.test(line)) {
        inFence = !inFence
        return
      }
      if (inFence) return
      for (const raw of linksIn(line)) {
        let target = String(raw).trim()
        if (target.startsWith('<') && target.endsWith('>')) target = target.slice(1, -1)
        if (!target.startsWith('#') || target.length < 2) continue
        let anchor = target.slice(1)
        try {
          anchor = decodeURIComponent(anchor)
        } catch {
          /* 解不开就按原样比 */
        }
        const wanted = anchor.trim().toLowerCase()
        if (!slugs.some((slug) => slug === wanted || slug.includes(wanted))) {
          out.push({ line: index + 1, anchor })
        }
      }
    })
  return out
}

/** 一份 md 里的同文件锚点（`#某一节`）——读数用（判据在 [`brokenAnchorsIn`]）。 */
function sameFileAnchorsIn(text) {
  let count = 0
  let inFence = false
  String(text)
    .split(/\r?\n/)
    .forEach((line) => {
      if (/^\s*(?:```|~~~)/.test(line)) {
        inFence = !inFence
        return
      }
      if (inFence) return
      for (const raw of linksIn(line)) {
        const target = String(raw).trim()
        if (target.startsWith('#') && target.length > 1) count += 1
      }
    })
  return count
}

/** 跟踪集里的 `*.md`（相对路径用 `/` 归一）。 */
function trackedMarkdown() {
  const raw = execFileSync('git', ['ls-files', '-z', '*.md'], { cwd: ROOT, encoding: 'utf8' })
  return raw
    .split('\0')
    .filter(Boolean)
    .map((rel) => rel.split(sep).join('/'))
}

/** 自检：坏链接**必须**被抓到，其余五类**必须**放行。 */
function selfCheck() {
  const missed = []
  // 夹具用的"盘"：只有路径里带「真的存在」这四个字的才算在盘上（于是坏链接必然指不到）。
  const onlyReal = (p) => String(p).includes('真的存在')
  const dir = ROOT
  if (brokenLinksIn('见 [那一份](./不存在的东西.md)', dir, onlyReal).length !== 1)
    missed.push('自检：指不到的相对链接**必须**被抓到')
  if (brokenLinksIn('见 [那一份](./真的存在.md)', dir, onlyReal).length !== 0)
    missed.push('自检：指得到的链接**不许**报红')
  if (brokenLinksIn('见 [外面](https://example.com/nope.md)', dir, onlyReal).length !== 0)
    missed.push('自检：外部 URL **不许**报红')
  if (brokenLinksIn('见 [这一节](#某一段)', dir, onlyReal).length !== 0)
    missed.push('自检：纯锚点**不许**报红')
  if (brokenLinksIn(`见 \`${'outputs'}/${'X.md'}\``, dir, onlyReal).length !== 0)
    missed.push('自检：门后的引用**不许**报红（它是一条出处）')
  if (brokenLinksIn('```\n[例子](./不存在的东西.md)\n```', dir, onlyReal).length !== 0)
    missed.push('自检：代码块里的例子**不许**报红')
  if (brokenLinksIn('见 [带锚点](./真的存在.md#L24)', dir, onlyReal).length !== 0)
    missed.push('自检：带 `#锚点` 的链接按文件查（锚点不在射程里）')
  if (brokenLinksIn('见 [真链接](<./真的存在.md>)', dir, onlyReal).length !== 0)
    missed.push('自检：尖括号包裹的路径**不许**报红')
  // 锚点自检（2026-10-19 加）：**必须**抓住改过名的标题，**必须**放行还在的那一节。
  const doc = '# 标题\n\n## Deployment shapes\n\n## 部署形态\n\n## 中文\n'
  if (brokenAnchorsIn(`${doc}\n见 [那一节](#deployment-shape)`).length !== 0)
    missed.push('锚点自检：还在的那一节（松匹配）**不许**报红')
  if (brokenAnchorsIn(`${doc}\n见 [那一节](#deployment-shapes-2020)`).length !== 1)
    missed.push('锚点自检：标题改过名之后指不到的锚点**必须**被抓到')
  if (brokenAnchorsIn(`${doc}\n见 [中文](#中文)`).length !== 0)
    missed.push('锚点自检：中文标题的锚点**不许**报红')
  return missed
}

const problems = []
const files = trackedMarkdown()
let behindCount = 0
let checked = 0
let anchors = 0
for (const rel of files) {
  let text = ''
  try {
    text = readFileSync(join(ROOT, rel), 'utf8')
  } catch {
    continue
  }
  for (const raw of text.split(/\r?\n/).flatMap((line) => linksIn(line))) {
    if (BEHIND_THE_DOOR.test(normalizeTarget(raw))) behindCount += 1
  }
  checked += 1
  anchors += sameFileAnchorsIn(text)
  for (const hit of brokenLinksIn(text, dirname(join(ROOT, rel)))) {
    problems.push(`${rel}:${hit.line} —— 指不到：\`${hit.target}\``)
  }
  for (const hit of brokenAnchorsIn(text)) {
    problems.push(`${rel}:${hit.line} —— 这一节不在了（锚点 \`#${hit.anchor}\`）`)
  }
}

const missed = selfCheck()
if (missed.length > 0) {
  console.error('[check-doc-links] 自检没过（守卫是装饰）：')
  for (const line of missed) console.error(`  ✗ ${line}`)
  process.exitCode = 1
} else if (problems.length > 0) {
  console.error(`[check-doc-links] ${problems.length} 条链接指不到：`)
  for (const line of problems) console.error(`  ✗ ${line}`)
  process.exitCode = 1
} else {
  console.log(
    `[check-doc-links] 通过：${checked} 份 md 的相对链接都解析得到 · ` +
      `同文件锚点 ${anchors} 条都指得到 · 门后引用 ${behindCount} 处（是一条出处，不是路径）· 自检全过`,
  )
}
