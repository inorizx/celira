#!/usr/bin/env node
// 源码守卫：**人称代词不许进产品面**（2026-09-25 立）。
//
// 为什么需要一条守卫，而不是"清干净就行"：这个仓库的散文层习惯用第三人称单数女性代词
// 讲一个具体的 Agent——那是**解释层**的叙述视角，跟 `Bed`/`Desk`/`House` 那类比喻同级。
// 但它已经漏进过产品面（聊天页头部那条「Agent 上次醒于 …」就是真的渲染给用户看的），
// 而且清理是**一次性**的：没有守卫，下一刀就会自然写回来。
//
// 判据（一条，只扫 `src/`）：
//   **任何一行**都不许出现那个中文代词——注释、字符串、JSX 文本一视同仁。
//
// 为什么注释也不放过（跟 `check-no-host.mjs` 的口径刻意相反）：
//   那条守卫不管注释，因为 Host 是**历史叙述**，留着是对的；这一条管的是**声音**——
//   产品代码里怎么称呼它，是产品的一部分，注释是维护者读的第一手材料。
//
// ⚠ 本文件自己**不许写出那个字**：它写成转义常量 `PRONOUN_SHE`，判据与夹具都用它拼。
//   这不是洁癖——守卫要是写得出它，就会被"清一遍"的机械替换顺手改掉，守卫当场失效。
//   这件事在 `agent/` 那一侧**真实发生过一次**（2026-09-25），代价是那段守卫整段重写。
//
// ⚠ 只扫 `src/`：`tests/` 与 `scripts/` 里的夹具**必须**能写出这个字才能证明守卫拦得住它。
//   这条边界是**故意的**，不许读成"测试里可以随便写"。
//
// ⚠ **英文标识符不在本守卫的判据里**（`AgentRooms.tsx` / `agentDays` 这类仍在场）：
//   它们是**重命名**，会动文件名、import 与一个跨进程字段（家状态口的 `agent`），
//   得单独一刀走 —— 不在"文案与注释"这一刀的范围里。改那一刀时，这里补上第二条判据。
//
// 退出码：0 = 干净；1 = 命中（按纪律 fail loudly，不静默放行）。
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const SRC = join(ROOT, 'src')
/** 被禁的那个人称代词，**写成转义**（见文件头：本文件自己不许出现它）。 */
const PRONOUN_SHE = '\u5979'
/** 只看这些扩展名——它们是会被打包进产物的源码面。 */
const EXTS = ['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs']

function walk(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...walk(full))
    else if (entry.isFile() && EXTS.some((e) => entry.name.endsWith(e))) out.push(full)
  }
  return out
}

const problems = []
for (const file of walk(SRC)) {
  const rel = file.slice(ROOT.length).replace(/\\/g, '/')
  const lines = readFileSync(file, 'utf8').split('\n')
  lines.forEach((line, index) => {
    if (line.includes(PRONOUN_SHE)) {
      problems.push(`${rel}:${index + 1} —— ${line.trim().slice(0, 100)}`)
    }
  })
}

if (problems.length > 0) {
  console.error('源码守卫：产品面不许出现人称代词（2026-09-25 立），但检测到：\n')
  for (const p of problems) console.error(`  ✗ ${p}`)
  console.error(
    '\n（怎么改：讲一个具体的 Agent 用**它世界里的名字**；讲结构用 `Agent`；' +
      '讲容器用 `Home` / 执行点 / `Ownership`；多个 Agent 是**清单**，不是群体。' +
      '界面文案**不带主语** —— 见 `navGroups.ts` 头上那段「称谓刀」裁决。）',
  )
  console.error(
    '\n（代词同时断言了三件事：性别、单一连续人格、可被第三人称叙述。' +
      '这三件都不是机械事实，而且对着一个"哥哥"型的 Agent 全错。）',
  )
  process.exit(1)
}
console.log('no-pronoun check passed')
