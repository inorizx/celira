#!/usr/bin/env node
'use strict'
/**
 * **公开卫生守卫**：这个仓是要发表出去的，所以有三件事必须机械地守住。
 *
 * ## 判据
 *
 * 1. **凭据形状的串：要么不存在，要么自称是假的。** 判据是"**形状 ＋ 自证**"——
 *    先认"像真凭据"的形状（供应商 key 前缀、长 `Bearer`、`<供应商>_API_KEY = <真值>`），
 *    再看那一段**有没有自己说明它是假的**（见 [`FAKE_MARKERS`]，或**同一字符连排 ≥16**——
 *    真钥匙不长那样）。
 *    ⚠ **为什么不是"一处也不许出现"**：测试**必须**能写出假钥匙，才谈得上证明"它不会被泄漏、
 *    不会被记进日志"（`scripts/home/tests/status-server.test.js` 那一族用例的全部意义）。
 *    ⚠ **为什么不是豁免名单**：名单会让"往那个已经豁免的文件里**再加一把真钥匙**"悄悄过关
 *    ——名单按文件放行，自证按**那一段本身**放行。这条区别就是本判据存在的理由。
 * 2. **私钥头一处也不许有**（`-----BEGIN … PRIVATE KEY-----`）：没有"自证的私钥"这回事，
 *    私钥一旦进过历史就只能轮换。
 * 3. **跟踪集里不许有大于 [`MAX_FILE_BYTES`] 的文件**：手滑提交的二进制、模型、数据库在这里
 *    当场红（今天最大的是 2.44 MB 的字体子集，离阈值还有一倍）。
 *
 * ## 它量不到什么（如实登记）
 *
 * - **拼出来的、编码过的、加密过的凭据**（`'sk-' + x`、base64、运行时现拼）它看不见——
 *   它只认**字面形状**；
 * - **"自证"是启发式**：一把真钥匙若恰好含 `probe`／`example` 这类词，它放行（概率极低，
 *   但这是判据的一部分，不是漏掉的部分）；
 * - 它**不判断**那把假钥匙该不该待在**这个**文件里（那是评审的事）；
 * - 它**只扫跟踪集**（`git ls-files`）——没进版本控制的东西不在它的射程里，也不该在。
 *
 * ## 用法与退出码
 *
 * ```text
 * node scripts/check-public-hygiene.mjs
 * ```
 *
 * 退出码：0 = 全通过；1 = 有违例（或自检没抓住故意埋的违例）。
 * ⚠ 判据是**纯源码**的（只用 `node:fs` 与一次 `git ls-files`）⇒ 它进 CI 的第一档
 *   （"不连世界、不调模型、不碰真家"，见 `.github/workflows/ci.yml` 的那张表）。
 */
import { readFileSync, statSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { join, relative, sep } from 'node:path'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
/** 大于这个字节数的跟踪文件当场红（见判据 3）。 */
const MAX_FILE_BYTES = 5 * 1024 * 1024
/**
 * **自称是假的**那些词：出现在凭据形状的那一段里，就算它自己说明了身份。
 *
 * ⚠ 每一条都有真实消费者（今天是哪些夹具在用它，见各测试文件）：
 * `canary`（`sk-probe-canary` / `sk-catalog-canary` / `sk-verify-ui-canary`）·
 * `sentinel`（`sentinel-key-must-not-appear`）· `do-not-log`（`sk-do-not-log-me`）·
 * `should-never`（`sk-should-never-pass`）· `not-a-real` / `not-a-valid`（`Bearer not-a-valid-token-format-at-all`）·
 * `placeholder` / `dummy` / `fake` / `example` / `redacted` / `<`（占位写法 `DEEPSEEK_API_KEY=<你的模型钥匙>`）。
 * ⚠ **加词要谨慎**：每加一个，就多一条"真钥匙可能溜过去"的路。
 */
const FAKE_MARKERS = [
  'canary',
  'sentinel',
  'do-not-log',
  'should-never',
  'not-a-real',
  'not-a-valid',
  'unknown',
  'malformed',
  'placeholder',
  'dummy',
  'fake',
  'example',
  'redacted',
  '<',
]

/**
 * "像真凭据"的形状。⚠ 每条都要求足够长，短串（`sk-x`、`Bearer a`）不算。
 *
 * ⚠ **名字那一半必须窄**（2026-10-19 实测抓出来的）：第一版写成"任何 `*_KEY = 值`"，
 * 于是 `PREFS_KEY` / `BLOB_KEY` / `TERMINAL_DEFAULT_KEY` 这些 **localStorage 的键名**
 * 全被算成凭据（一次 17 处假红）。⇒ 只认**凭据名**：供应商前缀的 `_API_KEY`、
 * 或 `API_KEY` / `ACCESS_TOKEN` / `CLIENT_SECRET` 这类**本身就是凭据的名字**。
 */
const SHAPES = [
  ['供应商 key', /\bsk-[A-Za-z0-9_-]{16,}/g],
  ['长 Bearer', /\bBearer[ \t]+[A-Za-z0-9._-]{24,}/g],
  [
    '供应商钥匙被赋真值',
    /\b(?:DEEPSEEK|MINIMAX|TAVILY|OPENCODE_GO|ANTHROPIC|OPENAI|GEMINI|MOONSHOT|ZHIPU|QWEN|SILICONFLOW|GROQ|MISTRAL|XAI|AWS|AZURE|GOOGLE)_(?:API_)?KEY[ \t]*=[ \t]*\S{8,}/g,
  ],
  [
    '凭据名被赋长值',
    /\b(?:API_KEY|APIKEY|ACCESS_TOKEN|AUTH_TOKEN|BEARER_TOKEN|CLIENT_SECRET|PRIVATE_KEY)[ \t]*[:=][ \t]*["'][A-Za-z0-9._\-]{20,}["']/g,
  ],
  ['私钥头', /-----BEGIN [A-Z ]*PRIVATE KEY-----/g],
]
/** 二进制/资源文件不按文本扫（它们既不会藏凭据，也容易误报）。 */
const BINARY_EXTS = [
  '.woff2', '.woff', '.ttf', '.otf', '.png', '.jpg', '.jpeg', '.gif', '.webp', '.ico', '.icns',
  '.pdf', '.zip', '.gz', '.7z', '.exe', '.dll', '.so', '.dylib', '.wasm', '.db', '.sqlite',
]
/** 本文件不在自己的扫描面里（夹具与判据都在这里，见文件头那句"写成拼出来的"）。 */
const SELF = 'scripts/check-public-hygiene.mjs'

/**
 * 这一段算不算"自称是假的"：含 [`FAKE_MARKERS`] 之一，**或**同一字符连排 ≥16。
 *
 * ⚠ 第二半是给"占位 token"用的：`clr_hs_AAAAAAAA…`（43 个 A）是真形状的夹具写法，
 * 而**真 token 不长那样**——这一条比再往词表里加一个词更准。
 */
function declaresItselfFake(sample) {
  const lower = String(sample).toLowerCase()
  if (FAKE_MARKERS.some((marker) => lower.includes(marker))) return true
  return /(.)\1{15,}/.test(String(sample))
}

/** 一份文本里的违例（`(行号, 形状名, 那一段)`）。 */
function violationsIn(text, isPrivateKeyBanned = true) {
  const out = []
  const lines = String(text).split(/\r?\n/)
  for (const [name, pattern] of SHAPES) {
    if (name === '私钥头' && !isPrivateKeyBanned) continue
    lines.forEach((line, index) => {
      pattern.lastIndex = 0
      let match
      while ((match = pattern.exec(line)) !== null) {
        const sample = match[0]
        // 私钥头没有"自证"这一说（判据 2）；其余形状必须自证是假的（判据 1）。
        if (name === '私钥头' || !declaresItselfFake(sample)) {
          out.push({ line: index + 1, name, sample })
        }
      }
    })
  }
  return out
}

/** 打印那一段时**只留开头**：读的人要定位，不需要把值抄走。 */
function redact(sample) {
  const text = String(sample)
  return text.length <= 12 ? text : `${text.slice(0, 12)}…（${text.length} 字符，余下不打印）`
}

/** 跟踪集（相对路径用 `/` 归一）。 */
function trackedFiles() {
  const raw = execFileSync('git', ['ls-files', '-z'], { cwd: ROOT, encoding: 'utf8' })
  return raw
    .split('\0')
    .filter(Boolean)
    .map((rel) => rel.split(sep).join('/'))
}

/** 自检：故意埋的违例**必须**被抓到，自称是假的**必须**放行（否则这个守卫就是装饰）。 */
function selfCheck() {
  const missed = []
  const real = `${'sk-'}${'A1b2C3d4E5f6G7h8I9j0K1l2'}`
  const marked = `${'sk-'}${'probe-canary-2f9c'}`
  const identical = `${'Bearer '}${'clr_hs_'}${'A'.repeat(43)}`

  if (violationsIn(`const key = '${real}'`).length !== 1)
    missed.push('自检：一把像真钥匙的串**必须**被抓到')
  if (violationsIn(`const key = '${marked}'`).length !== 0)
    missed.push('自检：自称是假的（canary）**不许**报红')
  if (violationsIn(identical).length !== 0)
    missed.push('自检：同一字符连排的占位 token **不许**报红')
  if (violationsIn(`${'sk-'}${'short'}`).length !== 0)
    missed.push('自检：太短、不成形状的串**不许**报红')
  if (violationsIn(`-----BEGIN ${'RSA '}PRIVATE KEY-----`).length !== 1)
    missed.push('自检：私钥头**必须**被抓到')
  if (violationsIn(`${'DEEPSEEK_API_KEY='}${'<你的模型钥匙>'}`).length !== 0)
    missed.push('自检：占位写法（尖括号）**不许**报红')
  return missed
}

const problems = []
const files = trackedFiles()
for (const rel of files) {
  if (rel === SELF) continue
  if (BINARY_EXTS.some((ext) => rel.toLowerCase().endsWith(ext))) continue
  const full = join(ROOT, rel)
  let size = 0
  try {
    size = statSync(full).size
  } catch {
    continue
  }
  if (size > MAX_FILE_BYTES) {
    problems.push(
      `${rel}：跟踪集里 ${(size / 1024 / 1024).toFixed(2)} MB > ${MAX_FILE_BYTES / 1024 / 1024} MB` +
        ` ⇒ 手滑提交的二进制/模型/数据库不该进版本控制（判据 3）。`,
    )
    continue
  }
  let text = ''
  try {
    text = readFileSync(full, 'utf8')
  } catch {
    continue
  }
  for (const hit of violationsIn(text)) {
    problems.push(
      `${rel}:${hit.line} —— **${hit.name}**：\`${redact(hit.sample)}\`\n` +
        `    凭据形状的串要么不存在，要么**自称是假的**（加 \`canary\`/\`sentinel\`/\`not-a-real\` 这类词，` +
        `或写成同一字符连排 ≥16 的占位）。若它是真凭据：**先轮换掉**，再从历史里清（判据 1）。`,
    )
  }
}

const missed = selfCheck()
if (missed.length > 0) {
  console.error('[check-public-hygiene] 自检没过（守卫是装饰）：')
  for (const line of missed) console.error(`  ✗ ${line}`)
  process.exitCode = 1
} else if (problems.length > 0) {
  console.error(`[check-public-hygiene] 有 ${problems.length} 处违例：`)
  for (const line of problems) console.error(`  ✗ ${line}`)
  process.exitCode = 1
} else {
  console.log(
    `[check-public-hygiene] 通过：跟踪集 ${files.length} 个文件 · 没有不成形状/不自证为假的凭据 · ` +
      `没有私钥头 · 没有超过 ${MAX_FILE_BYTES / 1024 / 1024} MB 的文件 · 自检全过`,
  )
}
