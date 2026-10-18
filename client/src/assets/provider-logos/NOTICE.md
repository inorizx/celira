# 预设供应商的标记（**真 logo**）

这 19 个 SVG 是**各家自己的商标**，不是我们画的。这份文件说明它们从哪来、为什么可以用、怎么刷新。

## 来源

| | |
|---|---|
| 库 | **LobeHub `lobe-icons`** —— AI / LLM 品牌图标集（1605+ 图标 / 300+ 品牌） |
| 包 | `@lobehub/icons-static-svg` |
| 仓库 | <https://github.com/lobehub/lobe-icons> |
| 版本 | **v1.95.1**（抓取于 2026-09-23；`opencode.svg` 2026-10-06 从**同一版本**抓） |
| 许可 | **MIT**（`Copyright © 2023 LobeHub`） |

> 这也是「**为什么别人的 agent 有图标**」的答案：开源 AI 客户端基本都走这个路子
> （LobeChat 自己就用它）。**主格式是 SVG**，PNG / WebP 是同一个源导出的备胎。

## 上游 slug 映射（**本地文件名 = `presets.ts` 里的 `id`**）

| 本地文件 | 上游 slug | 变体 |
|---|---|---|
| `deepseek.svg` | `deepseek-color` | 彩色 |
| `openai.svg` | `openai` | **单色** |
| `anthropic.svg` | `anthropic` | **单色** |
| `siliconflow.svg` | `siliconcloud-color` | 彩色 |
| `dashscope.svg` | `qwen-color` | 彩色 |
| `zhipu.svg` | `zhipu-color` | 彩色 |
| `ark.svg` | `volcengine-color` | 彩色 |
| `moonshot.svg` | `kimi-color` | 彩色 |
| `minimax.svg` | `minimax-color` | 彩色 |
| `stepfun.svg` | `stepfun-color` | 彩色 |
| `hunyuan.svg` | `hunyuan-color` | 彩色 |
| `baichuan.svg` | `baichuan-color` | 彩色 |
| `openrouter.svg` | `openrouter-color` | 彩色 |
| `groq.svg` | `groq` | **单色**（色由组件补） |
| `together.svg` | `together-color` | 彩色 |
| `mistral.svg` | `mistral-color` | 彩色 |
| `xai.svg` | `xai` | **单色** |
| `deepinfra.svg` | `deepinfra-color` | 彩色 |
| `opencode.svg` | `opencode` | **单色**（色由主题给） |

> ★ **彩色 / 单色各几家，别对不上号**：上游**能拿到彩色变体的有 15 家**，
> 而这里只装了 **14 家彩色** —— `anthropic.svg` 是**故意留单色**的：那一行叫「Anthropic」，
> 摆它**自己**的标（黑白）比摆产品 Claude 的彩色标更准。
> 想换过来就抓 `claude-color.svg`，同时把上表那格改成「彩色」（`presets.test.ts` 有一条判据会盯着）。
> 其余四家单色各有原因：`openai` / `xai` 的官方标本来就是黑白，`groq` 上游没导出彩色版，
> **`opencode`（2026-10-06 加）上游也没有彩色版**（`opencode-color.svg` 实测 404）——
> 而它**没有**一个"有据可查"的品牌色可补（`groq` 那种情形），因此**不进 `MONO_INK`**：
> 让 `fill="currentColor"` 跟着主题走，别为好看硬上色。
> ⇒ 表里「变体」那一列**不是描述，是判据**：文件内容与它不一致就会红。

★ **slug 是"产品级"的，不能按公司名猜**：火山方舟 = `volcengine`（**不是** `bytedance`）、
硅基流动 = `siliconcloud`、通义千问 = `qwen`、Moonshot = `kimi`、xAI = `xai`（`<title>` 是 Grok）。

## 为什么可以用（合规）

- **版权**：多数厂商 logo 是简单几何图形，**不受版权保护**，使用无需授权；
- **商标**：这才是真正的约束。我们用它**指代那一家服务**（指示性使用 / nominative use），
  **合法且通行** —— LobeChat / LobeHub 自己就是这么大规模用的；
- **库的 MIT 覆盖的是这些文件本身**；logo 的商标权**仍归各自所有者**，本仓不主张任何权利。

⇒ 若某家来函表示不希望被这样引用，**摘掉那一个文件即可** —— 组件会回落到首字母圆标。

## ★ 红线：运行时不发请求

这些 SVG 是**构建期**被 `?raw` 读进 bundle 的（见 `../components/celira/ProviderMark.tsx`）
⇒ 页面运行**零网络请求**。

这条是**安全边界**，不是版权：跑起来时去拉别人的图，等于页面上写着"这台机器在准备连 DeepSeek"。

## 刷新方法

```bash
B=https://unpkg.com/@lobehub/icons-static-svg@latest/icons
curl -sL "$B/deepseek-color.svg" -o src/assets/provider-logos/deepseek.svg
curl -sL "$B/openai.svg"          -o src/assets/provider-logos/openai.svg
curl -sL "$B/opencode.svg"        -o src/assets/provider-logos/opencode.svg
```

**四家是单色的**，原因不同，别当成漏抓：

- `openai` / `anthropic` / `xai` —— 上游就没有彩色版，因为**它们的官方标本来就是黑白**
  （文件里是 `fill="currentColor"`，跟着主题明暗走，这是对的）；
- `groq` —— 上游也没彩色版，但它的品牌色有据可查（`#F55036`，取自上游 `src/Groq/style.ts`
  的 `COLOR_PRIMARY`）⇒ 由组件里的 `MONO_INK` 补上。
- `opencode`（2026-10-06 加）—— 上游没彩色版（实测 `opencode-color.svg` 404），
  也**没有**有据可查的品牌色 ⇒ **不进 `MONO_INK`**，就用 `currentColor`。

## 一份 SVG 会在 DOM 里出现两次（**已知，实测无害**）

同一家会**同时在两处渲染**：左栏那一行 ＋ 选中后右侧的面板头。
所以上游 SVG 里的 `<linearGradient id="…">` 会在 DOM 里出现 **2 次** —— 而 `id` 本该唯一
（目前带渐变的有四家：`dashscope` / `minimax` / `stepfun` / `baichuan`）。

**2026-09-23 实测：无害**，三条一起成立：

1. 每份 SVG 都**自带 `<defs>`** —— 不是"引用了别人的定义"；
2. 浏览器按**第一个**匹配解析 `url(#id)`，而两份定义**逐字相同**；
3. 渐变用 objectBoundingBox（随各自 bbox 缩放）⇒ 28px 与 40px 两处都正确。

最坏情况也验过：**把左栏那一行用搜索过滤掉**（它的定义随之离开 DOM）后，
面板头仍**自带**那份定义、照常上色 —— 没有悬空引用。

⇒ **别加"运行时改写 id"的魔法**（那要在渲染期动上游内容，是新脆弱点）。
真会出问题的是"**同一家的两种外观同时在场**"（比如以后做选中态异色），那时才需要处理。
