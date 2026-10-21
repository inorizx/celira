import { ThemePresetCard, ThemeVariantPicker } from '@/components/celira/ThemePresetCard'
import { WallpaperPicker } from '@/components/celira/ChatWallpaper'
import { UserThemeSection } from '@/components/celira/UserThemeSection'
import { Segmented } from '@/components/celira/Segmented'
import { useTheme } from '@/hooks/theme-context'
import { useWallpaper } from '@/hooks/wallpaper-context'
import { FONT_SIZES, getThemeVariants, THEME_PRESETS } from '@/lib/themes'
import { OFFICIAL_WALLPAPERS } from '@/lib/wallpaper'

/** 外观设置：明暗模式、主题风格、配色、字号、聊天壁纸（全部本机呈现层偏好）。 */
export function SettingsAppearancePage() {
  const {
    theme,
    mode: themeMode,
    setMode: setThemeMode,
    preset,
    setPreset,
    variant,
    setVariant,
    fontSize,
    setFontSize,
  } = useTheme()
  const { pref: wallpaperPref } = useWallpaper()

  return (
    <>
      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <div className="flex items-center gap-2">
          <span className="text-body">明暗模式</span>
          <span className="ml-auto text-caption text-muted-foreground">
            {themeMode === 'system'
              ? `跟随系统（当前${theme === 'dark' ? '夜间' : '日间'}）`
              : theme === 'dark'
                ? '夜间'
                : '日间'}
          </span>
        </div>
        <Segmented
          label="明暗模式"
          value={themeMode}
          onChange={setThemeMode}
          options={[
            { id: 'system', label: '跟随系统' },
            { id: 'light', label: '日间' },
            { id: 'dark', label: '夜间' },
          ] as const}
        />
      </div>
      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <span className="text-body">主题风格</span>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {THEME_PRESETS.map((p) => (
            <ThemePresetCard
              key={p.id}
              presetId={p.id}
              name={p.name}
              description={p.description}
              active={preset === p.id}
              dark={theme === 'dark'}
              onSelect={() => setPreset(p.id)}
            />
          ))}
        </div>
        <p className="text-caption text-muted-foreground">
          只改变本机视觉，保存在本浏览器。
        </p>
      </div>
      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <div className="flex items-center gap-2">
          <span className="text-body">配色</span>
          <span className="ml-auto text-caption text-muted-foreground">
            {getThemeVariants(preset).find((v) => v.id === variant)?.name}
          </span>
        </div>
        <ThemeVariantPicker presetId={preset} variant={variant} onSelect={setVariant} />
        <p className="text-caption text-muted-foreground">
          每套主题自带几组配色，只换颜色；每个主题记住各自的选择。
        </p>
      </div>
      {/* 我的主题：外观这一层唯一的装载口（装的是数据，不是代码） */}
      <UserThemeSection />
      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <div className="flex items-center gap-2">
          <span className="text-body">字号</span>
          <span className="ml-auto text-caption text-muted-foreground">
            {FONT_SIZES.find((s) => s.id === fontSize)?.name}
          </span>
        </div>
        <Segmented
          label="字号"
          value={fontSize}
          onChange={setFontSize}
          options={FONT_SIZES.map((s) => ({ id: s.id, label: s.name }))}
        />
        <p className="text-caption text-muted-foreground">
          只改变阅读文字大小，与主题、配色自由组合。
        </p>
      </div>
      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <div className="flex items-center gap-2">
          <span className="text-body">聊天壁纸</span>
          <span className="ml-auto text-caption text-muted-foreground">
            {wallpaperPref.global.kind === 'none'
              ? '默认'
              : wallpaperPref.global.kind === 'official'
                ? (OFFICIAL_WALLPAPERS.find((w) => w.id === wallpaperPref.global.assetId)?.name ?? '官方壁纸')
                : '自定义图片'}
            {Object.keys(wallpaperPref.rooms).length > 0 &&
              ` · ${Object.keys(wallpaperPref.rooms).length} 个会话有专属`}
          </span>
        </div>
        <WallpaperPicker />
        <p className="text-caption text-muted-foreground">
          只应用于聊天页消息区；自定义图片只保存在本机浏览器，不会上传。
        </p>
      </div>
    </>
  )
}
