/**
 * **家那两个口的判据表 —— 唯一权威位置**（2026-10-12 立）。
 *
 * ## 为什么要有它
 *
 * 家那两个口（读口 5190 / 写口 5191）有**两份实现**：
 * - **源码形态**：`node home.js doors`（`scripts/home/status-server.js` ＋ `config-server.js`）；
 * - **产品形态**：壳自己端（`app/src-tauri/src/home_doors.rs`）。
 *
 * 两份实现**不可能合成一份**（两种形态各需要一个宿主：CLI 得在没装 app 的机器上跑，
 * 壳得在自己进程里端这两个口）⇒ 那这条纪律就落在**同形**上（仓库早写着："两个生产者必须同形"）。
 *
 * ⚠ 而 2026-10-11 真撞上一次不同形：**来源那道与路由谁先判**两边反着 ⇒ 一个**外域**来源打
 * `/home-binding`（只收 PUT）在壳那边得 **403**、在 JS 那边得 **405 ＋ `Allow: PUT`** ✗
 * —— 等于把"这个端点存在、它认哪些动词"告诉了一个不该知道的来源，而且**谁答话取决于
 * 哪个生产者碰巧占着那两个口** ✗（那天产品形态门禁量的其实是源码形态的门，看着像代码坏了）。
 *
 * 那次是把 JS 改成与壳同形 ✓。但"再同步一次"还会再漂 ⇒ **判据表现在只有这一份**：
 * 两个门禁都跑它（`verify-ui.mjs` 量源码形态的门、`verify-ui-app.mjs` 量壳），
 * 谁想加一条判据都只能加在这里，**两边同时生效**。
 *
 * ## 表的形状
 *
 * `origin`：`own` = 这一形态自己的来源（壳是 `http://tauri.localhost`，源码形态是回环来源）；
 * `foreign` = 外域；`foreign-scheme` = 协议像壳、主机不是。
 * `expect`：**状态码本身就是判据**（仓库口径：`Allow` 只在来源过了之后才给）。
 */

/** 壳自己的来源（`home_doors.rs` 的 `SHELL_ORIGINS`）。 */
export const SHELL_ORIGIN = 'http://tauri.localhost'
/** 源码形态的"自己的来源"：回环（`home-key.js` 的 `localOriginAllowed` 认的就是回环）。 */
export const LOCAL_ORIGIN = 'http://localhost:5173'

/**
 * 六条判据。**两个生产者必须逐条给同样的答案**（状态码相同；`Allow` 只在 405 那两条上出现）。
 *
 * `door`：哪一只口 —— `status`（读口）／`config`（写口）。⚠ 两个口是**一个进程的两个端面**
 * （`home.js doors` 与壳都这样），但**基址不同** ⇒ 表里要点名它属于哪一只。
 */
export const DOOR_CASES = [
  {
    label: '读口认自己的来源 ⇒ 200',
    door: 'status',
    path: '/status',
    origin: 'own',
    expect: 200,
  },
  {
    label: '写口认自己的来源 ⇒ 200',
    door: 'config',
    path: '/home-config',
    origin: 'own',
    expect: 200,
  },
  {
    label: '绑定口是 PUT-only：用 GET 敲 ⇒ 405（来源过了才轮到方法）',
    door: 'config',
    path: '/home-binding',
    origin: 'own',
    expect: 405,
    allow: 'PUT',
  },
  {
    label: '★ 外域来源打只读端点 ⇒ 403（守卫没被放宽）',
    door: 'config',
    path: '/home-config',
    origin: 'foreign',
    expect: 403,
  },
  {
    label: '★ `tauri://` 但主机不是壳自己的 ⇒ 照旧 403',
    door: 'config',
    path: '/home-config',
    origin: 'foreign-scheme',
    expect: 403,
  },
  {
    label: '★ 外域来源打 PUT-only 端点 ⇒ 403（**不是** 405）—— 来源那道在路由之前',
    door: 'config',
    path: '/home-binding',
    origin: 'foreign',
    expect: 403,
  },
]

/** 把 `origin` 那一格翻成一个真的来源头（`own` 按形态给）。 */
export function originHeader(kind, ownOrigin) {
  if (kind === 'own') return ownOrigin
  if (kind === 'foreign') return 'https://evil.example'
  return 'tauri://evil.example'
}

/**
 * 跑一遍这张表，返回 `[{ case, status, allow }]`（连不上 ⇒ `status: 0`）。
 *
 * `bases` = `{ status, config }`（**那个形态**的两只口的基址），`ownOrigin` 见 `originHeader`。
 */
export async function runDoorCases(bases, ownOrigin, { timeoutMs = 8000 } = {}) {
  const results = []
  for (const item of DOOR_CASES) {
    const origin = originHeader(item.origin, ownOrigin)
    const base = bases[item.door]
    let status = 0
    let allow = null
    try {
      const response = await fetch(`${base}${item.path}`, {
        headers: origin ? { origin } : {},
        signal: AbortSignal.timeout(timeoutMs),
      })
      status = response.status
      allow = response.headers.get('allow')
    } catch {
      status = 0
    }
    results.push({ case: item, status, allow })
  }
  return results
}

/** 判据本体：逐条比状态码（＋ 405 那两条比 `Allow`）。返回说错话的那些。 */
export function doorCasesSayingWrongThing(results) {
  const wrong = []
  for (const { case: item, status, allow } of results) {
    if (status !== item.expect) {
      wrong.push(`${item.path}（${item.origin}）期望 ${item.expect}，实得 ${status}`)
      continue
    }
    if (item.allow && (allow ?? '').trim() !== item.allow) {
      wrong.push(`${item.path} 期望 Allow: ${item.allow}，实得 ${allow ?? '(没有)'}`)
    }
  }
  return wrong
}

/** 一行一条的读数（两个门禁都打它，好对照）。 */
export function describeDoorCases(results) {
  return results
    .map(({ case: item, status, allow }) => `${status}${allow ? ` Allow:${allow.trim()}` : ''} ← ${item.path}（${item.origin}）`)
    .join(' · ')
}
