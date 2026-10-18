import { cn } from '@/lib/utils'

import anthropicLogo from '@/assets/provider-logos/anthropic.svg?raw'
import arkLogo from '@/assets/provider-logos/ark.svg?raw'
import baichuanLogo from '@/assets/provider-logos/baichuan.svg?raw'
import dashscopeLogo from '@/assets/provider-logos/dashscope.svg?raw'
import deepinfraLogo from '@/assets/provider-logos/deepinfra.svg?raw'
import deepseekLogo from '@/assets/provider-logos/deepseek.svg?raw'
import groqLogo from '@/assets/provider-logos/groq.svg?raw'
import hunyuanLogo from '@/assets/provider-logos/hunyuan.svg?raw'
import minimaxLogo from '@/assets/provider-logos/minimax.svg?raw'
import mistralLogo from '@/assets/provider-logos/mistral.svg?raw'
import moonshotLogo from '@/assets/provider-logos/moonshot.svg?raw'
import openaiLogo from '@/assets/provider-logos/openai.svg?raw'
import opencodeLogo from '@/assets/provider-logos/opencode.svg?raw'
import openrouterLogo from '@/assets/provider-logos/openrouter.svg?raw'
import siliconflowLogo from '@/assets/provider-logos/siliconflow.svg?raw'
import stepfunLogo from '@/assets/provider-logos/stepfun.svg?raw'
import togetherLogo from '@/assets/provider-logos/together.svg?raw'
import xaiLogo from '@/assets/provider-logos/xai.svg?raw'
import zhipuLogo from '@/assets/provider-logos/zhipu.svg?raw'

/**
 * **预设供应商的标记**（真 logo）。
 *
 * ## 这些图是**别人的商标**，来源与许可见 `src/assets/provider-logos/NOTICE.md`
 *
 * 2026-09-23 主人问「**为什么别人的 agent 有图标呢？是不是不是用 svg 而是 png？**」
 * —— 一问戳穿了我上一版的说法。**别人用的是 AI 专用社区图标库**：
 * **LobeHub `lobe-icons`**（`@lobehub/icons`，MIT，1605+ 图标，LobeChat 自己就用它）。
 * **主格式就是 SVG**，PNG / WebP 只是同一个源导出的备胎 ⇒ 我们也能用，而且是 SVG。
 *
 * ⚠ 我上一版写"实测拿不到、只有 6/18、走自绘"——**那是残缺的读数**：我只查了**官方仓库**与
 * **Simple Icons（通用库）**两个源就报了覆盖率。**没穷举来源，覆盖率就是假读数。**
 * 真实读数（逐条 `curl` unpkg）：**单色 18/18、彩色 15/18**。
 *
 * ## 唯一红线：**运行时不发请求**
 *
 * 这些 SVG 用 `?raw` 在**构建期**读进 bundle（本地矢量、零请求）。私自改成运行时去外链取图，
 * 会给每个预设供应商开一条对外请求 —— 等于页面上写着"这台机器在准备连 DeepSeek"。
 * **这是安全边界，不是版权。**
 *
 * ## 设计纪律（改这个文件时守住）
 *
 * 1. **认不出 id ⇒ 回落首字母圆标**，**不许编形状**（编出来的会被当成真标记）；
 * 2. **上游换标就重抓**（`NOTICE.md` 有命令），别在这里手改 SVG 内容；
 * 3. 单色的那几家**不为好看硬上色**（OpenAI / Anthropic / xAI 的官方标本就是黑白）。
 */

/**
 * 上游**没有彩色版**、但我们有据可查其品牌色的那几家。
 *
 * 只有 Groq 一家：它的品牌色就是那个橙红（记在上游 `src/Groq/style.ts` 的 `COLOR_PRIMARY` 里），
 * 静态包里却没导出 `groq-color.svg`。文件本身是 `fill="currentColor"` ⇒ 这里给个字面色即可。
 *
 * 其余单色的三家（`openai` / `anthropic` / `xai`）**故意不给色**：它们的官方标本来就是黑白，
 * 跟着主题明暗走才对（暗色主题下黑标会看不见）。
 */
const MONO_INK: Record<string, string> = {
  groq: '#f55036', // token-check:allow 品牌身份色（取自上游 src/Groq/style.ts 的 COLOR_PRIMARY）
}

/** `presets.ts` 的 `id` ⇒ 上游 SVG 的原文（`NOTICE.md` 里有本地文件名 ↔ 上游 slug 的对照表）。 */
const LOGO_SVG: Record<string, string> = {
  deepseek: deepseekLogo,
  openai: openaiLogo,
  opencode: opencodeLogo,
  anthropic: anthropicLogo,
  siliconflow: siliconflowLogo,
  dashscope: dashscopeLogo,
  zhipu: zhipuLogo,
  ark: arkLogo,
  moonshot: moonshotLogo,
  minimax: minimaxLogo,
  stepfun: stepfunLogo,
  hunyuan: hunyuanLogo,
  baichuan: baichuanLogo,
  openrouter: openrouterLogo,
  groq: groqLogo,
  together: togetherLogo,
  mistral: mistralLogo,
  xai: xaiLogo,
  deepinfra: deepinfraLogo,
}

export interface ProviderMarkProps {
  /** 预设 id（`presets.ts` 里的那个）。认不出就画首字母。 */
  id: string
  /** 认不出 id 时的首字母来源（一般是名字）。 */
  name: string
  /** 边长（px）。默认 28 —— 左栏那一行的尺寸。 */
  size?: number
  className?: string
}

/**
 * 画一家的标记。
 *
 * ⚠ **认不出 id 时回落到首字母圆标**，而不是"随便画一个形状" —— 编出来的形状会
 * 让人以为那是这家的真标记（那是我们最不想要的误导）。
 */
export function ProviderMark({ id, name, size = 28, className }: ProviderMarkProps) {
  const svg = LOGO_SVG[id]
  const ink = MONO_INK[id]

  if (!svg) {
    /* 认不出这一家 ⇒ 首字母（不编形状）。外面那一行已经有名字了，这里不进无障碍树。 */
    return (
      <span
        aria-hidden
        className={cn(
          'flex shrink-0 items-center justify-center rounded-full bg-muted text-caption text-muted-foreground',
          className,
        )}
        style={{ width: size, height: size }}
      >
        {name.slice(0, 1)}
      </span>
    )
  }

  /*
   * 上游那份 SVG 自带 `width="1em" height="1em"` ⇒ 用 `fontSize` 控制它画多大，
   * 不用去改人家文件里的属性（上游换标时整份替换即可）。
   * `color` 只给 `MONO_INK` 里那几家（他们的文件是 `fill="currentColor"`）。
   */
  const style = {
    width: size,
    height: size,
    fontSize: Math.round(size * 0.8),
    ...(ink ? { color: ink } : {}),
  }

  /*
   * ⚠ **同一家会同时在两处渲染**（左栏那一行 ＋ 选中后右侧的面板头）⇒ 上游 SVG 里的
   * `<linearGradient id="…">` 会在 DOM 里**出现两次**（`id` 本该唯一；目前 qwen / minimax /
   * stepfun / baichuan 四家有渐变）。
   *
   * 2026-09-23 **实测：无害**，三条一起来：
   *   ① 每份 SVG 都**自带 `<defs>`** ⇒ 不是"引用了别人的定义"；
   *   ② 浏览器按**第一个**匹配解析 `url(#id)`，而两份定义**逐字相同**；
   *   ③ 渐变用的是 objectBoundingBox（随各自 bbox 缩放）⇒ 28px 与 40px 两处都正确。
   * 连最坏情况都验过：**把左栏那行用搜索过滤掉**（那一行的定义随之离开 DOM）后，
   * 面板头仍**自带**那份定义、照常上色。
   *
   * ⇒ **别为它加"运行时改写 id"的魔法**（那要在渲染期动上游内容，是新的脆弱点）。
   * 真会出问题的是"**同一家的两种外观同时在场**"（比如以后做选中态异色）——那时才要处理。
   */
  return (
    <span
      aria-hidden
      className={cn('flex shrink-0 items-center justify-center', className)}
      style={style}
      /* 内容是**构建期读进来的本仓文件**，不是运行时数据 ⇒ 没有注入面。 */
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  )
}
