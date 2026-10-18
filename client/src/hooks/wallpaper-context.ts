import { createContext, useContext } from 'react'
import type { WallpaperDim, WallpaperPref, WallpaperSelection } from '@/lib/wallpaper'

export interface WallpaperContextValue {
  pref: WallpaperPref
  customUrl: string | null
  resolveSelection: (roomId?: string) => WallpaperSelection
  resolveUrl: (roomId?: string) => string | null
  selectNone: (roomId?: string) => void
  selectOfficial: (assetId: string, roomId?: string) => void
  selectCustom: (roomId?: string) => void
  clearRoomOverride: (roomId: string) => void
  setDim: (dim: WallpaperDim) => void
  uploadCustom: (file: File, roomId?: string) => Promise<void>
  removeCustom: () => Promise<void>
}

export const WallpaperContext = createContext<WallpaperContextValue | null>(null)

export function useWallpaper(): WallpaperContextValue {
  const ctx = useContext(WallpaperContext)
  if (!ctx) throw new Error('useWallpaper must be used within WallpaperProvider')
  return ctx
}
