// 外观个性化纯逻辑测试（feat/appearance-personalization）。
//
// 证明外观系统的安全边界：主题/配色变体/字号 id 校验器拒绝非法值；
// 密度概念已整体移除；tokens.css 的字号档只覆盖语义文本令牌，
// 不缩放根字号/间距，也没有 data-density / --spacing 覆盖；
// 壁纸偏好解析对缺失、损坏、非法字段一律诚实回退默认值，
// 绝不让存储里的脏数据变成视觉或运行态异常。
// 运行：cd web && npm test

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import {
  FONT_SIZES,
  isFontSizeId,
  isThemePresetId,
  isThemeVariantId,
  THEME_PRESETS,
} from '../src/lib/themes.ts'
import {
  hydrateWallpaper,
  lastRoomSnapshot,
  parseLastRoom,
  parseWallpaperPref,
  saveLastRoom,
  saveWallpaperPref,
  wallpaperPrefSnapshot,
  OFFICIAL_WALLPAPERS,
  saveCustomWallpaper,
  validateWallpaperBlob,
  WallpaperRejectedError,
  WALLPAPER_MAX_BYTES,
} from '../src/lib/wallpaper.ts'
import { currentStorage, setStorage, type StoragePort } from '../src/api/storage.ts'
import {
  hydrateThemePrefs,
  parseFontSize,
  parseThemeMode,
  parseThemePreset,
  parseThemeVariantMap,
  saveThemeFontSize,
  saveThemeMode,
  saveThemeVariantMap,
  themePrefsSnapshot,
} from '../src/lib/themePrefs.ts'

test('isThemePresetId 只接受目录内 id', () => {
  for (const p of THEME_PRESETS) assert.equal(isThemePresetId(p.id), true)
  assert.equal(isThemePresetId('celira'), true)
  assert.equal(isThemePresetId('hacker-theme'), false)
  assert.equal(isThemePresetId(''), false)
  assert.equal(isThemePresetId(null), false)
  assert.equal(isThemePresetId(undefined), false)
})

test('isThemeVariantId 按主题校验各自的变体目录', () => {
  for (const p of THEME_PRESETS) {
    // 每个主题都有 'standard'（原色）作为默认变体
    assert.equal(p.variants[0].id, 'standard', `${p.id} 的首个变体必须是 standard`)
    for (const v of p.variants) assert.equal(isThemeVariantId(p.id, v.id), true)
    assert.equal(isThemeVariantId(p.id, 'magenta'), false)
    assert.equal(isThemeVariantId(p.id, null), false)
  }
  // 变体 id 按主题各自为政：celira 的 azure 不是 sakura 的合法值
  assert.equal(isThemeVariantId('celira', 'azure'), true)
  assert.equal(isThemeVariantId('sakura', 'azure'), false)
})

test('FONT_SIZES 目录：恰为 small / standard / large，standard 默认', () => {
  assert.deepEqual(
    FONT_SIZES.map((s) => s.id),
    ['small', 'standard', 'large'],
  )
  assert.deepEqual(
    FONT_SIZES.map((s) => s.name),
    ['小', '标准', '大'],
  )
  assert.equal(FONT_SIZES.find((s) => s.id === 'standard')?.id, 'standard')
})

test('isFontSizeId 只接受目录内 id', () => {
  for (const s of FONT_SIZES) assert.equal(isFontSizeId(s.id), true)
  assert.equal(isFontSizeId('standard'), true)
  assert.equal(isFontSizeId('compact'), false)
  assert.equal(isFontSizeId('huge'), false)
  assert.equal(isFontSizeId(''), false)
  assert.equal(isFontSizeId(null), false)
  assert.equal(isFontSizeId(undefined), false)
})

test('themes 目录不再导出密度概念', async () => {
  const themes = (await import('../src/lib/themes.ts')) as Record<string, unknown>
  assert.equal('DENSITIES' in themes, false)
  assert.equal('isDensityId' in themes, false)
  assert.equal('DensityId' in themes, false)
})

// 静态样式断言（读源码 tokens.css，不解析构建产物）：字号档只覆盖语义
// 文本令牌，绝不缩放根字号/间距；密度概念已从外观系统整体移除。
test('tokens.css 静态语义：无密度、无根字号/间距覆盖，字号档只覆盖语义文本令牌', () => {
  const css = readFileSync(new URL('../src/styles/tokens.css', import.meta.url), 'utf8')
  assert.equal(css.includes('data-density'), false)
  assert.doesNotMatch(css, /--spacing\s*:/, '不得由外观功能引入 --spacing 覆盖')
  for (const level of ['small', 'large']) {
    const block = css.match(new RegExp(`html\\[data-font-size='${level}'\\]\\s*\\{[^}]*\\}`))?.[0]
    assert.ok(block, `缺少 data-font-size='${level}' 字号档`)
    assert.doesNotMatch(block, /font-size\s*:/, `字号档 ${level} 不得缩放根字号`)
    for (const token of ['--text-display', '--text-title', '--text-body', '--text-caption', '--text-micro']) {
      assert.match(block, new RegExp(`${token}\\s*:`), `字号档 ${level} 必须覆盖 ${token}`)
    }
  }
})

test('官方壁纸目录：id 唯一且 url 指向 public 资源', () => {
  const ids = OFFICIAL_WALLPAPERS.map((w) => w.id)
  assert.equal(new Set(ids).size, ids.length)
  for (const w of OFFICIAL_WALLPAPERS) assert.match(w.url, /^\/wallpapers\//)
})

// ── 壁纸偏好：纯解析（2026-10-08 起它不再自己读存储，读由窄口 ＋ 快照负责）──

test('parseWallpaperPref：无内容时回退默认（无壁纸/中档蒙版）', () => {
  assert.deepEqual(parseWallpaperPref(null), {
    global: { kind: 'none', dim: 'medium', assetId: undefined },
    rooms: {},
  })
})

test('parseWallpaperPref：损坏 JSON 回退默认', () => {
  assert.deepEqual(parseWallpaperPref('{broken json'), {
    global: { kind: 'none', dim: 'medium', assetId: undefined },
    rooms: {},
  })
})

test('parseWallpaperPref：旧版扁平结构迁移为 global + rooms', () => {
  const pref = parseWallpaperPref(
    JSON.stringify({ kind: 'official', assetId: 'aurora', dim: 'high' }),
  )
  assert.deepEqual(pref, {
    global: { kind: 'official', assetId: 'aurora', dim: 'high' },
    rooms: {},
  })
})

test('parseWallpaperPref：非法 kind / dim 字段各自回退，合法字段保留', () => {
  const pref = parseWallpaperPref(JSON.stringify({ kind: 'hacked', dim: 'extreme', assetId: 'aurora' }))
  assert.equal(pref.global.kind, 'none')
  assert.equal(pref.global.dim, 'medium')
  assert.equal(pref.global.assetId, 'aurora')
  assert.deepEqual(pref.rooms, {})
})

test('parseWallpaperPref：合法新结构（含会话覆盖）原样读回', () => {
  const pref = parseWallpaperPref(
    JSON.stringify({
      global: { kind: 'official', assetId: 'night-grid', dim: 'low' },
      rooms: { 'r-core': { kind: 'custom' }, 'r-aria': { kind: 'none' } },
    }),
  )
  assert.deepEqual(pref, {
    global: { kind: 'official', assetId: 'night-grid', dim: 'low' },
    rooms: { 'r-core': { kind: 'custom', assetId: undefined }, 'r-aria': { kind: 'none', assetId: undefined } },
  })
})

test('parseWallpaperPref：会话覆盖里的脏数据被清理', () => {
  const pref = parseWallpaperPref(
    JSON.stringify({
      global: { kind: 'none', dim: 'medium' },
      rooms: { 'r-x': { kind: 'hacked', assetId: 42 } },
    }),
  )
  assert.deepEqual(pref.rooms['r-x'], { kind: 'none', assetId: undefined })
})

test('parseLastRoom：形状不对 / 坏 JSON ⇒ null（设置页就当没有这一格）', () => {
  assert.equal(parseLastRoom(null), null)
  assert.equal(parseLastRoom('{broken'), null)
  assert.equal(parseLastRoom(JSON.stringify({ id: 'r-1' })), null)
  assert.deepEqual(parseLastRoom(JSON.stringify({ id: 'r-1', name: '甲' })), { id: 'r-1', name: '甲' })
})

// ── 壁纸偏好：经窄口补水与落盘（2026-10-08 存储收尾第二刀）──────────────────

/** 一个记账用的假窄口。 */
function fakePort(store: Record<string, string>, log: string[]): StoragePort {
  return {
    async read(key) {
      log.push(`read:${key}`)
      return store[key] ?? null
    },
    async write(key, value) {
      log.push(`write:${key}`)
      store[key] = value
      return true
    },
    async remove(key) {
      delete store[key]
    },
    async keys(prefix) {
      return Object.keys(store).filter((k) => k.startsWith(prefix))
    },
  }
}

test('★ 壁纸走窄口：补水把盘上那份读进**快照**（第一帧就有值），写再落回**同名键**', async () => {
  const store: Record<string, string> = {
    'celira-wallpaper': JSON.stringify({ global: { kind: 'official', assetId: 'dunes', dim: 'high' }, rooms: {} }),
    'celira-last-room': JSON.stringify({ id: 'r-9', name: '九号房' }),
  }
  const log: string[] = []
  const before = currentStorage()
  setStorage(fakePort(store, log))
  try {
    await hydrateWallpaper()
    // ① 快照 = 盘上那份（Provider 的初始化器读它 ⇒ 第一帧就是用户自己的壁纸）
    assert.deepEqual(wallpaperPrefSnapshot().global, { kind: 'official', assetId: 'dunes', dim: 'high' })
    assert.deepEqual(lastRoomSnapshot(), { id: 'r-9', name: '九号房' })
    assert.deepEqual(log.slice(0, 2).sort(), ['read:celira-last-room', 'read:celira-wallpaper'])
    // ② 写回同一批键（键名一字不改 —— 改了就等于把用户已经选好的壁纸丢掉）
    await saveWallpaperPref({ global: { kind: 'none', dim: 'low' }, rooms: { 'r-1': { kind: 'official', assetId: 'aurora' } } })
    assert.equal(JSON.parse(store['celira-wallpaper']!).global.dim, 'low')
    await saveLastRoom({ id: 'r-10', name: '十号房' })
    assert.deepEqual(JSON.parse(store['celira-last-room']!), { id: 'r-10', name: '十号房' })
    assert.ok(!log.some((line) => line.startsWith('write:celira-wallpaper-legacy')), '不许另起键名')
  } finally {
    setStorage(before)
  }
})

// ── 主题四格的持久面（`lib/themePrefs.ts`，2026-10-08 存储收尾第二刀）─────────

test('parseThemeMode / parseFontSize / parseThemePreset：只收目录内的值，其余归默认', () => {
  assert.equal(parseThemeMode('light'), 'light')
  assert.equal(parseThemeMode('system'), 'system')
  for (const raw of ['dark', 'DARK', 'hacked', '', null, undefined]) {
    assert.equal(parseThemeMode(raw), 'dark', `明暗模式 ${String(raw)} 该归默认`)
  }
  for (const preset of THEME_PRESETS) assert.equal(parseThemePreset(preset.id), preset.id)
  for (const raw of ['hacker-theme', '', null, undefined]) {
    assert.equal(parseThemePreset(raw), 'celira', `预设 ${String(raw)} 该归默认`)
  }
  for (const size of FONT_SIZES) assert.equal(parseFontSize(size.id), size.id)
  for (const raw of ['huge', '', null, undefined]) {
    assert.equal(parseFontSize(raw), 'standard', `字号 ${String(raw)} 该归默认`)
  }
})

test('parseThemeVariantMap：脏数据诚实丢弃（非 JSON / 非对象 / 非法变体 id）', () => {
  assert.deepEqual(parseThemeVariantMap('{broken'), {})
  assert.deepEqual(parseThemeVariantMap('[]'), {})
  assert.deepEqual(parseThemeVariantMap(JSON.stringify({ celira: 'magenta', sakura: 'azure' })), {})
  // 合法的那几格留着（免得上面那几条只是"什么都读不出来"）
  assert.deepEqual(parseThemeVariantMap(JSON.stringify({ celira: 'azure', sakura: 'azure' })), {
    celira: 'azure',
  })
})

test('★ 主题四格走窄口：补水进**快照**（第一帧就是用户自己的配色），写回**同名键**', async () => {
  const store: Record<string, string> = {
    'celira-theme': 'light',
    'celira-theme-preset': 'paper',
    'celira-theme-variant': JSON.stringify({ paper: 'standard' }),
    'celira-font-size': 'large',
  }
  const log: string[] = []
  const before = currentStorage()
  setStorage(fakePort(store, log))
  try {
    await hydrateThemePrefs()
    assert.deepEqual(themePrefsSnapshot(), {
      mode: 'light',
      preset: 'paper',
      variantMap: { paper: 'standard' },
      fontSize: 'large',
    })
    await saveThemeMode('system')
    await saveThemeFontSize('small')
    await saveThemeVariantMap({ paper: 'standard', celira: 'rose' })
    assert.deepEqual(
      log.filter((line) => line.startsWith('write:')).sort(),
      ['write:celira-font-size', 'write:celira-theme', 'write:celira-theme-variant'],
      '写回的键名必须与读的是同一批（改了 = 用户已经选好的外观丢掉）',
    )
    assert.equal(themePrefsSnapshot().mode, 'system', '快照跟着写走（同一会话内再读也是新值）')
    assert.equal(themePrefsSnapshot().fontSize, 'small')
  } finally {
    setStorage(before)
  }
})

// ── 自定义壁纸本地校验（纯逻辑，不触达 IndexedDB）──
// loadCustomWallpaper 的「存储内容不合法 → fail closed 返回 null」依赖 IndexedDB，
// 无法在无浏览器的 node 单测里稳妥验证；其使用的判据与这里同一 validateWallpaperBlob。

test('validateWallpaperBlob：只接受允许的栅格 MIME 类型', () => {
  for (const type of ['image/png', 'image/jpeg', 'image/webp', 'image/gif']) {
    assert.deepEqual(validateWallpaperBlob({ type, size: 1024 }), { ok: true })
  }
})

test('validateWallpaperBlob：拒绝 SVG 与其它 MIME 类型', () => {
  for (const type of ['image/svg+xml', 'image/bmp', 'image/avif', 'image/x-icon', 'text/plain', 'application/pdf', '']) {
    assert.deepEqual(validateWallpaperBlob({ type, size: 1024 }), { ok: false, error: 'unsupported_type' })
  }
})

test('validateWallpaperBlob：拒绝超过上限的文件', () => {
  assert.deepEqual(validateWallpaperBlob({ type: 'image/png', size: WALLPAPER_MAX_BYTES + 1 }), {
    ok: false,
    error: 'file_too_large',
  })
  assert.deepEqual(validateWallpaperBlob({ type: 'image/png', size: WALLPAPER_MAX_BYTES }), { ok: true })
})

test('saveCustomWallpaper：未通过校验的 Blob 在触达 IndexedDB 前即被拒绝', async () => {
  await assert.rejects(
    () => saveCustomWallpaper(new Blob(['<svg/>'], { type: 'image/svg+xml' })),
    (err: unknown) => err instanceof WallpaperRejectedError && err.code === 'unsupported_type',
  )
  await assert.rejects(
    () => saveCustomWallpaper(new Blob([new Uint8Array(WALLPAPER_MAX_BYTES + 1)], { type: 'image/png' })),
    (err: unknown) => err instanceof WallpaperRejectedError && err.code === 'file_too_large',
  )
})
