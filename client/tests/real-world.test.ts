// 「真实世界 vs 演示通道」判据测试。
//
// 本刀（TASK_56）修的根：**「这是不是真实世界」被写成了「这是不是 Host 模式」**。
// 真实世界有两条入口（本机 Host 会话 / core 直连），而演示通道
// （登录页「不连接，先看演示界面」）也写 `mode = 'core'`
// —— 所以判据不能只看 mode。
//
// **TASK_63 收窄**：Host 线退役 ⇒ 真实世界只剩 core 一支，判据退化成
// 「手里有没有真实凭据」（`token !== ''`）。上一条教训仍然成立且更要注意：
// 演示通道也是 `mode === 'core'`，所以**只看 mode 依旧会把演示态判成真实世界**。
//
// 这个文件钉住四件事：
//  1. `isRealWorldSession` 的判据本身（含"演示通道也是 core"这条要害）；
//  2. fixtures 回落的**总闸**在真实世界下确实关着（目录层 + SELF_ID 兜底）；
//  3. 关闸之后"查不到就是查不到"，不冒名、不顶替。
//  4. **fixtures 驱动的路由都挂在 `FixtureRouteGuard` 之下**（TASK_69 收尾补：
//     守卫的*逻辑*早有人钉，但"**哪些路由挂在它下面**"是另一件事——
//     `/settings/providers` 就在守卫之外活了很久，而路由树没人测过）。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import { readdirSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { isRealWorldSession } from '../src/api/prefs.ts'
import { resolveSelfId, systemMessageIdentity } from '../src/api/selfIdentity.ts'
import {
  fixtureFallbackAllowed,
  getParticipant,
  registerFixtureFallbackGate,
  tryGetParticipant,
} from '../src/api/participantDirectory.ts'
import { SELF_ID } from '../src/fixtures/data.ts'

/** 闸门是模块级状态：用完一律恢复默认（开），免得影响本文件后面的用例。 */
function withGate(allow: boolean, run: () => void) {
  registerFixtureFallbackGate(() => allow)
  try {
    run()
  } finally {
    registerFixtureFallbackGate(() => true)
  }
}

// ── ① 判据本身 ──────────────────────────────────────────────────────────────

test('真实世界 = 手里有真实凭据（TASK_56 立判据，TASK_63 收窄到只剩 token）', () => {
  assert.equal(isRealWorldSession({ mode: 'core', token: 'clr_hs_x' }), true, '直连连上了')
  // ★ 收窄的可见后果：同一个空 token，收窄前 `mode='host'` 判真（Host 走 Cookie，
  // token 本就是空），现在判假——Host 那一支不存在了。
  assert.equal(isRealWorldSession({ mode: 'core', token: '' }), false, '空 token = 没有凭据')
})

test('★ 演示通道也是 mode=core ⇒ 只看 mode 会把它判成真实世界', () => {
  // `LoginGate.browseDemo()` 干的正是这件事：`setPrefs('core', '')` + 去 /gallery。
  // 这条判据要是写错，演示通道会被"当成真实世界"而拒掉 fixtures（演示全空）；
  // 反过来只认 host 就会把 core 直连的真实用户放进来看到假数据（本刀的真 bug）。
  assert.equal(isRealWorldSession({ mode: 'core', token: '' }), false, 'core + 空 token = 演示通道')
})

// ── ② fixtures 回落的总闸 ───────────────────────────────────────────────────

test('闸门默认是开的（没注册 = 保持既有演示/开发行为）', () => {
  assert.equal(fixtureFallbackAllowed(), true)
})

test('真实世界关闸：fixture 目录层不再顶替未知 id', () => {
  withGate(false, () => {
    assert.equal(fixtureFallbackAllowed(), false)
    assert.equal(tryGetParticipant('p-inori'), null, 'fixture 自己的 id 也不许捞出来')
    assert.throws(() => getParticipant('p-inori'), /unknown participant/)
  })
})

test('演示通道开闸：fixtures 照旧（这是演示通道唯一的内容来源）', () => {
  withGate(true, () => {
    assert.equal(tryGetParticipant('p-inori')?.id, SELF_ID)
    assert.equal(getParticipant('p-inori').id, SELF_ID)
  })
})

// ── ③ 关闸之后：不冒名 ──────────────────────────────────────────────────────

test('真实世界无身份：resolveSelfId 返空（"还不知道我是谁"），绝不回退 SELF_ID', () => {
  withGate(false, () => {
    assert.equal(resolveSelfId(undefined), '', '无身份 → 空串，不是 p-inori')
    assert.equal(resolveSelfId(null), '')
    assert.equal(resolveSelfId(''), '')
    assert.equal(resolveSelfId('hm_real_1'), 'hm_real_1', '有真实身份时照旧用真实的')
  })
})

test('演示通道无身份：仍回退 SELF_ID（演示的既有展示行为不变）', () => {
  withGate(true, () => {
    assert.equal(resolveSelfId(undefined), SELF_ID)
  })
})

test('真实世界 + 没传 resolver：systemMessageIdentity 也不许改用 fixtures 作者名', () => {
  // 原先这条只靠"调用方记得传 resolver"自觉；真实世界下没传 ≠ 那就用假作者。
  withGate(false, () => {
    const fallback = () => ({ displayName: 'Courier' })
    const done = systemMessageIdentity('a-courier', undefined, undefined, fallback)
    assert.equal(done.authorName, '参与者', '没 resolver 就中性，不用 fixtures 名')
    assert.equal(done.isSelfAuthor, false, 'selfId 缺失时不许回退 SELF_ID 认成自己')
  })
})

test('真实世界 + 有 resolver：作者名只来自 resolver（未知 → 参与者）', () => {
  withGate(false, () => {
    const resolver = (id: string) => (id === 'hm_a' ? '真实我' : undefined)
    const self = systemMessageIdentity('hm_a', 'hm_a', resolver, tryGetParticipant)
    assert.equal(self.authorName, '真实我')
    assert.equal(self.isSelfAuthor, true)
    const other = systemMessageIdentity('hm_b', 'hm_a', resolver, tryGetParticipant)
    assert.equal(other.authorName, '参与者')
    assert.equal(other.isSelfAuthor, false)
  })
})

test('演示通道 + 没 resolver：fixtures 回退保持（既有行为）', () => {
  withGate(true, () => {
    const done = systemMessageIdentity('a-courier', undefined, undefined, tryGetParticipant)
    assert.equal(done.authorName, 'Courier')
    const self = systemMessageIdentity(SELF_ID, undefined, undefined, tryGetParticipant)
    assert.equal(self.isSelfAuthor, true)
  })
})

// ---------------------------------------------------------------------------
// 4. fixtures 驱动的**路由**也必须在守卫生成的那一页之下（TASK_69 收尾）
// ---------------------------------------------------------------------------

/**
 * **fixtures 驱动的页**：真实世界里不该让人看见演示内容的那几页。
 *
 * 清单是**封闭**的——哪一页算 fixtures 驱动，逐个点名并写清依据（都从 `@/fixtures/data`
 * 取内容，页面自身没有 mode 分支）。新增这种页时要**同时**加进这里（否则它可能又落在守卫之外）。
 *
 * ⚠ 第一栏是**路由声明里那个字符串**（不是完整地址）：`discover` 挂在 `/app` 之下，
 * 它的完整地址是 `/app/discover`。这条判据比的是"声明里那个 path 有没有出现在守卫块里"。
 */
const FIXTURE_DRIVEN_ROUTES: [path: string, page: string, why: string][] = [
  ['/gallery', 'GalleryPage', '陈列室本体（连接横幅三态那条 legacy 通道的宿主）'],
  ['discover', 'DiscoverPage', '发现页（完整地址 /app/discover；Core v1 无 Directory 契约，保持诚实拦截）'],
]

// ⚠ 2026-09-25 从这份清单里**移除**了 `/identity`（IdentityPage）：那条路由与页面已整体删除。
// 理由不是"判据放宽"，而是**那一页的承诺不实**——标题写着「你是谁？先创建你的 Human 身份」、
// 主按钮写着「创建身份并进入」，实际两个按钮都只是 `navigate('/app/chat')`，一个身份都不创建；
// 而真实身份是 CLI 签发的（`celira-im create-human`）。**清单是封闭的** ⇒ 页面没了就删行。
// 同时 `SplashPage` 的「进入」改为回正门 `/`（原先指向那条已删的路由）。

// ⚠ TASK_77 从这份清单里**移除**了 `/settings/providers`（ModelProvidersPage）：
// 它不再是 fixtures 页——重写之后它读的是**本机模型门**（`/home-config`）、写的是本机那格
// 私有配置，数据源与 `@/fixtures/data` 再无关系。因此它从守卫之下搬出来，与 `/app/runtime`、
// `/app/experience` 同款：非真实世界由**页面自身**按 `isRealWorldSession` 处理
// （读显示照实、写动作关着）。**清单是封闭的** ⇒ 这一行删掉是"证据变了"，不是"判据放宽"。
//
// ⚠ **2026-09-26 从这份清单里移除 `/agent/:participantId`（AgentPage）** —— 理由同上：
// 证据变了，不是判据放宽。这一页原先**整页**读 fixtures（假模型 / 假绑定 / 写死的「在线」），
// 而**真实世界有三处入口链到它**（消息头像 / 右键菜单「查看主页」/ 联系人资料卡）⇒
// 真实用户点得到、点进去撞门。这是 2026-09-26 前端审计里最重的一条硬伤。
//
// 那一刀把它**重写成真正的世界侧参与者页**：拆掉写死的「在线」与 fixtures 分区、
// 「发消息」不再跳演示房号（改成真的建/进直聊房）、漏网的内行词清掉 ⇒ 它现在**只读 core**
// （参与者目录 + `getAgentProfile` 能力探测），演示通道由目录回落自己处理。
// 页面不再是 fixtures 驱动 ⇒ 从守卫之下搬出来（`routeRegistry.ts` 同步改成 `side: 'world'`）。
//
// ⚠ 这条移除的**反面**同样被钉着：`tests/route-admission.test.ts` 会读 `App.tsx` 的真实路由表
// 逐条对表，`demo ⇔ 在守卫下` 对不上就红 —— 所以"搬出来"与"改声明"必须同一刀完成。

test('★ fixtures 驱动的路由都挂在 FixtureRouteGuard 之下（路由树也要有人钉）', () => {
  const source = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8')

  // 守卫块 = `element: <FixtureRouteGuard />,` 紧跟它的 `children: [ … ]`。
  //
  // ⚠ 这是**文本级**判断（不是链接期证明，与 agent 侧那些结构守卫同族）：守卫块里目前
  //    没有嵌套数组，所以非贪婪匹配到的第一个 `]` 就是它自己的收尾。哪天有人往里塞嵌套
  //    结构，这条会**响亮地红**（`guarded` 里认不出那个 path），不会静默放过。
  const guarded = [...source.matchAll(/element: <FixtureRouteGuard \/>,\s*children: \[([\s\S]*?)\]/g)]
    .map((match) => match[1])
    .join('\n')
  assert.ok(guarded.includes('path:'), `守卫块至少要认出一个（否则正则已经过期）: ${guarded}`)

  for (const [path, page, why] of FIXTURE_DRIVEN_ROUTES) {
    assert.ok(
      guarded.includes(`path: '${path}'`),
      `${path}（${why}）必须挂在 FixtureRouteGuard 之下——真实世界不该看到 ${page} 的演示内容`,
    )
  }
})

// ---------------------------------------------------------------------------
// 5. "真实世界"不许由 `mode` 判定（唯一权威是 isRealWorldSession，TASK_69 收尾）
// ---------------------------------------------------------------------------

test('★ 页面/组件不许拿 `mode` 当"真实世界"判据（唯一权威是 isRealWorldSession）', () => {
  const root = new URL('../src/', import.meta.url)
  const files: string[] = []
  const walk = (dir: URL) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) walk(new URL(`${entry.name}/`, dir))
      else if (/\.tsx?$/.test(entry.name)) files.push(fileURLToPath(new URL(entry.name, dir)))
    }
  }
  walk(root)
  // 别空跑：扫描面太小说明这段 walk 自己坏了（与家里那条纪律同源）。
  assert.ok(files.length > 50, `扫描面太小（${files.length} 个文件）——这段扫描可能已经失效`)

  // 判据：**以"真实世界"命名的那个变量，右侧不许是 `mode` 的比较**。
  //
  // 依据是成文的：`api/prefs.ts` 写着判据的**唯一权威位置**是 `isRealWorldSession`，
  // 并把"把 `mode === 'core'` 一律当真实世界"点名为错误——因为**演示通道也是
  // `mode === 'core'`**（`browseDemo()` 写的就是 `setPrefs('core', '')`），
  // 拿 mode 当真实世界会把它一起掐掉（TASK_56 那个 bug 的镜像）。
  // 真事：`ContactsPage` / `MyAgentsPage` / `ContactDetailCard` 三处就这么活了很久。
  const NAMED_FROM_MODE =
    /(?:const|let|var)\s+\w*(?:isReal|profileReal|realWorld)\w*\s*=\s*mode\s*===/
  for (const file of files) {
    const hit = readFileSync(file, 'utf8')
      .split('\n')
      .find((line) => NAMED_FROM_MODE.test(line))
    assert.equal(hit, undefined, `${file} 拿 mode 判"真实世界"：${hit?.trim()}`)
  }

  // 反向：那三个曾经踩过的点必须**真的**在用权威判据——免得这条判据被"删掉概念"骗过去。
  for (const file of [
    'pages/ContactsPage.tsx',
    'pages/MyAgentsPage.tsx',
    'components/celira/ContactDetailCard.tsx',
  ]) {
    const source = readFileSync(fileURLToPath(new URL(file, root)), 'utf8')
    assert.ok(source.includes('isRealWorldSession'), `${file} 应当用 isRealWorldSession 判真实世界`)
  }
})
