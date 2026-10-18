import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import {
  clearCustomWallpaper,
  loadCustomWallpaper,
  wallpaperPrefSnapshot,
  OFFICIAL_WALLPAPERS,
  saveCustomWallpaper,
  saveWallpaperPref,
  type WallpaperDim,
  type WallpaperPref,
  type WallpaperSelection,
} from '@/lib/wallpaper'
import { WallpaperContext, type WallpaperContextValue } from '@/hooks/wallpaper-context'

export function WallpaperProvider({ children }: { children: ReactNode }) {
  // 首帧快照（同步）：`main.tsx` 已经补过水 ⇒ 第一帧就是用户自己的壁纸，不闪。
  // 自定义那张图（IndexedDB 的 Blob）不在这里，见下面那个 effect —— 它是二进制，挂载后异步取。
  const [pref, setPref] = useState<WallpaperPref>(wallpaperPrefSnapshot)
  const [customUrl, setCustomUrl] = useState<string | null>(null)
  const urlRef = useRef<string | null>(null)

  const replaceCustomUrl = useCallback((blob: Blob | null) => {
    if (urlRef.current) URL.revokeObjectURL(urlRef.current)
    urlRef.current = blob ? URL.createObjectURL(blob) : null
    setCustomUrl(urlRef.current)
  }, [])

  // 启动时尝试恢复已上传的自定义壁纸；读不到就诚实回退为无壁纸。
  useEffect(() => {
    let cancelled = false
    void loadCustomWallpaper()
      .then((blob) => {
        if (cancelled || !blob) return
        replaceCustomUrl(blob)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [replaceCustomUrl])

  useEffect(() => () => {
    if (urlRef.current) URL.revokeObjectURL(urlRef.current)
  }, [])

  const apply = (next: WallpaperPref) => {
    setPref(next)
    // 落盘是异步的（窄口）：界面不等它；写不进去由 `saveWallpaperPref` 记一笔日志。
    void saveWallpaperPref(next)
  }

  const applySelection = (sel: WallpaperSelection, roomId?: string) => {
    if (roomId) apply({ ...pref, rooms: { ...pref.rooms, [roomId]: sel } })
    else apply({ ...pref, global: { ...pref.global, ...sel } })
  }

  const selectNone = (roomId?: string) => applySelection({ kind: 'none' }, roomId)
  const selectOfficial = (assetId: string, roomId?: string) =>
    applySelection({ kind: 'official', assetId }, roomId)
  const selectCustom = (roomId?: string) => {
    if (customUrl) applySelection({ kind: 'custom' }, roomId)
  }

  const clearRoomOverride = (roomId: string) => {
    const rooms = { ...pref.rooms }
    delete rooms[roomId]
    apply({ ...pref, rooms })
  }

  const setDim = (dim: WallpaperDim) => apply({ ...pref, global: { ...pref.global, dim } })

  const uploadCustom = async (file: File, roomId?: string) => {
    await saveCustomWallpaper(file)
    replaceCustomUrl(file)
    applySelection({ kind: 'custom' }, roomId)
  }

  const removeCustom = async () => {
    await clearCustomWallpaper()
    replaceCustomUrl(null)
    // 所有引用 custom 的选择（全局 + 各会话覆盖）都诚实回退为无壁纸。
    const rooms: WallpaperPref['rooms'] = {}
    for (const [roomId, sel] of Object.entries(pref.rooms)) {
      rooms[roomId] = sel.kind === 'custom' ? { kind: 'none' } : sel
    }
    apply({
      global: pref.global.kind === 'custom' ? { ...pref.global, kind: 'none' } : pref.global,
      rooms,
    })
  }

  const resolveSelection = useCallback(
    (roomId?: string): WallpaperSelection =>
      (roomId && pref.rooms[roomId]) || pref.global,
    [pref],
  )

  const urlFor = useCallback(
    (sel: WallpaperSelection): string | null => {
      if (sel.kind === 'official') {
        return OFFICIAL_WALLPAPERS.find((w) => w.id === sel.assetId)?.url ?? null
      }
      if (sel.kind === 'custom') return customUrl
      return null
    },
    [customUrl],
  )

  const value = useMemo<WallpaperContextValue>(
    () => ({
      pref,
      customUrl,
      resolveSelection,
      resolveUrl: (roomId?: string) => urlFor(resolveSelection(roomId)),
      selectNone,
      selectOfficial,
      selectCustom,
      clearRoomOverride,
      setDim,
      uploadCustom,
      removeCustom,
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 动作函数随 pref/customUrl 重建即可
    [pref, customUrl, resolveSelection, urlFor],
  )

  return <WallpaperContext.Provider value={value}>{children}</WallpaperContext.Provider>
}
