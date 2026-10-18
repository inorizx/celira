// **本机状态层的两段式启动 ＋ 搬家读**：迁移判据的**接线**守卫（2026-10-08 立，同日第二刀铺满五族）。
//
// 运行：cd client && npm test
//
// 为什么这几条值得钉：它们错了都**看得见但看不出原因** ——
//   ① 模块求值期又出现同步读 ⇒ 壳里那一族**永远读不到家里的文件**（文件存储白接）；
//   ② 顺序错（先挂载再补水）⇒ 第一帧落默认值再跳回来（闪一下，且某些时序下补齐不了）；
//   ③ 漏了一族没在 `main.tsx` 点名 ⇒ 只有那一族的第一帧退回缺省值，**没有任何东西会提醒你**；
//   ④ 老位置（webview 的 `localStorage`）被某一族自己读 ⇒ 变成第二个权威（窄口说了不算）；
//   ⑤ 登出清错地方 ⇒ 壳里**家里那一族世界级数据没清掉**（cursor / 系统消息 / 房间偏好）。
//
// 纯判据（"该用哪一份"那三条）在 `storage.test.ts`（它住在窄口那一层，一处权威）。

import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'

const REPO = path.resolve(import.meta.dirname, '..')
const read = (relative: string) => fs.readFileSync(path.join(REPO, relative), 'utf8')

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（9 条 ＋ 收尾那条）。 */
const DECLARED_CASES = 10

function hydrationCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

/** 摘掉注释再判（注释里合法地讲这些名字，裸子串会误报 —— 与守卫那条同规）。 */
const stripComments = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

/** `src/` 下所有 `.ts` / `.tsx`（相对路径，正斜杠）。 */
function sourceFiles(dir = path.join(REPO, 'src')): string[] {
  const out: string[] = []
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) out.push(...sourceFiles(full))
    else if (/\.tsx?$/.test(entry.name)) out.push(path.relative(REPO, full).replace(/\\/g, '/'))
  }
  return out
}

// ── ① 模块求值期只有缺省值 ──────────────────────────────────────────────────

hydrationCase('★ 模块求值期只有缺省值：五族都不许在求值期读存储', () => {
  const store = read('src/api/store.ts')
  assert.match(store, /let prefs: ConnectionPrefs = DEFAULT_PREFS/, '连接偏好：缺省值')
  assert.match(store, /let roomPrefs: RoomPrefs = EMPTY_ROOM_PREFS/, '房间偏好：缺省值')
  assert.ok(
    !/^let prefs = loadPrefs\(\)/m.test(store) && !/^let roomPrefs = loadRoomPrefs\(/m.test(store),
    '模块求值期不许再同步读（那正是"文件存储接不进来"的原因）',
  )
  // 另外三族的快照也必须是"求值期只有缺省值"：快照变量初始化时不许出现任何 await/读口。
  const themePrefs = read('src/lib/themePrefs.ts')
  assert.match(themePrefs, /let snapshot: ThemePrefs = \{ \.\.\.DEFAULT_THEME_PREFS/, '主题：快照从缺省值起步')
  const drafts = read('src/lib/messageDrafts.ts')
  assert.match(drafts, /const drafts = new Map<string, string>\(\)/, '草稿：快照从空表起步')
  const wallpaper = read('src/lib/wallpaper.ts')
  assert.match(wallpaper, /let snapshot: WallpaperPref = defaultPref\(\)/, '壁纸：快照从缺省值起步')
})

hydrationCase('★ 每一族的补水都是**幂等**的（补两次不会互相踩）', () => {
  for (const file of [
    'src/api/store.ts',
    'src/lib/themePrefs.ts',
    'src/lib/userTheme.ts',
    'src/lib/wallpaper.ts',
    'src/lib/messageDrafts.ts',
  ]) {
    assert.match(read(file), /hydrated \?\?=/, `${file} 的补水该是 memo 形状（hydrated ??= …）`)
  }
})

// ── ② 启动顺序：装配 → 补水（五族）→ 挂载 ───────────────────────────────────

hydrationCase('★ 启动顺序：先 await 装配与补水，**再**挂载', () => {
  const main = read('src/main.tsx')
  const storageAt = main.indexOf('await installShellStorage()')
  const hydrateAt = main.indexOf('await hydrateLocalState()')
  const renderAt = main.indexOf('createRoot(')
  assert.ok(storageAt > 0 && hydrateAt > 0 && renderAt > 0, '三段都要在')
  assert.ok(storageAt < hydrateAt, '装配在前（补水要经过那条窄口）')
  assert.ok(hydrateAt < renderAt, '补水在挂载之前（第一帧就是用户自己的连接）')
  // 失败也要挂载：白屏比"没读到偏好"糟得多。
  assert.match(main, /catch \(error\) \{[\s\S]{0,200}console\.error/, '两条失败都要留痕迹')
})

hydrationCase('★ 五族**每一族**都在 `createRoot` 之前点名（漏一族 = 那一族的第一帧退回缺省值）', () => {
  const main = read('src/main.tsx')
  const renderAt = main.indexOf('createRoot(')
  for (const call of [
    'await hydrateLocalState()',
    'hydrateThemePrefs()',
    'hydrateUserThemes()',
    'hydrateWallpaper()',
    'hydrateDrafts()',
  ]) {
    const at = main.indexOf(call)
    assert.ok(at > 0, `main.tsx 里没有 ${call} —— 那一族不会被补水`)
    assert.ok(at < renderAt, `${call} 排在 createRoot 之后 —— 第一帧拿不到它`)
  }
})

// ── ③ 搬家读的调用点白名单（新增持久面必须显式登记）────────────────────────

hydrationCase('★ 凡"搬家读"的模块都在名单里（新增一族必须同时改这里与 `main.tsx`）', () => {
  const expected = [
    'src/api/roomPrefs.ts',
    'src/api/store.ts',
    'src/lib/messageDrafts.ts',
    'src/lib/themePrefs.ts',
    'src/lib/userTheme.ts',
    'src/lib/wallpaper.ts',
  ]
  const found = sourceFiles()
    .filter((file) => file !== 'src/api/storage.ts')
    .filter((file) => stripComments(read(file)).includes('readThrough('))
    .sort()
  assert.deepEqual(
    found,
    expected,
    '搬家读的调用点变了：新增一处 ⇒ 它必须在 `main.tsx` 里点名补水（并把本名单一起改）',
  )
})

// ── ④ 老位置（webview 的 `localStorage`）只被窄口读 ─────────────────────────

hydrationCase('★ `localStorage` 这个标识符**只许出现在 `api/storage.ts`**（老位置不是第二个权威）', () => {
  const offenders = sourceFiles().filter((file) => stripComments(read(file)).includes('localStorage'))
  assert.deepEqual(
    offenders,
    ['src/api/storage.ts'],
    `这些文件自己去读老位置了：${offenders.join(', ')} —— 读存储一律走 api/storage.ts 的窄口`,
  )
})

hydrationCase('★ 老位置的读只有一处：`readThrough`（别的读法一律不许）', () => {
  const storage = stripComments(read('src/api/storage.ts'))
  assert.match(storage, /export async function readThrough\(/, '搬家读要在')
  assert.match(storage, /export function preferStoredRaw\(/, '判据本体要在（纯函数，storage.test.ts 钉着）')
  // 老位置本身只在这三个函数里被碰：搬家读 ＋ 列出 ＋ 删除（后两个只服务登出清理）。
  assert.match(storage, /function legacyRawOf\(key: string\)/, '老位置的读只有这一处')
  const getItemCalls = (storage.match(/\.getItem\(/g) || []).length
  // ① 浏览器实现那一份 `read`（它是窄口本身）② `legacyRawOf` 里那次。
  assert.equal(getItemCalls, 2, `老位置被读了 ${getItemCalls} 处 —— 只该是"窄口实现 ＋ 搬家读"两处`)
})

// ── ⑤ 登出：清窄口那一族，老位置只清残留 ────────────────────────────────────

hydrationCase('★ 登出清的是**窄口那一族**，老位置只清残留（且从此没人往它写）', () => {
  const store = read('src/api/store.ts')
  assert.match(store, /for \(const k of await storage\.keys\(prefix\)\) await storage\.remove\(k\)/, '按前缀清窄口')
  assert.match(store, /legacyRemove\(legacyKeysUnder\(prefix\)\)/, '老位置那份残留顺手清掉')
  assert.ok(
    !/legacyBucket\(\)\?\.setItem|legacyBucket\(\)\.setItem/.test(store),
    '不许往老位置写（那会变成第二个权威）',
  )
  assert.ok(!stripComments(store).includes('localStorage'), 'store 自己不许碰老位置（一律经窄口那两个动作）')
})

// ── ⑥ 第一帧读的是快照（呈现面不再自己读存储）──────────────────────────────

hydrationCase('★ 呈现面读**快照**：Provider 与组件里没有一处自己读存储', () => {
  for (const [file, needle] of [
    ['src/hooks/use-theme.tsx', 'themePrefsSnapshot()'],
    ['src/hooks/use-theme.tsx', 'userThemesSnapshot'],
    ['src/hooks/use-wallpaper.tsx', 'wallpaperPrefSnapshot'],
    ['src/components/celira/ChatWallpaper.tsx', 'lastRoomSnapshot'],
    ['src/components/celira/MessageInput.tsx', 'draftFor(room.id)'],
  ] as const) {
    assert.ok(read(file).includes(needle), `${file} 该读首帧快照：${needle}`)
  }
  // 旧名字一个都不许留：它们是"模块求值期同步读"那一代的符号。
  const gone = ['loadUserThemes(', 'loadActiveUserThemeId(', 'loadWallpaperPref(', 'loadLastRoom(', 'persistDrafts(']
  for (const file of sourceFiles()) {
    const source = stripComments(read(file))
    for (const symbol of gone) {
      assert.ok(!source.includes(symbol), `${file} 里还留着旧符号 ${symbol}`)
    }
  }
})

hydrationCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
