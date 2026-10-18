import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'

/**
 * 本机只读状态口的端口（TASK_61）。
 *
 * ⚠ **这一段只服务 `npm run dev`（开发者热更新那条路）**。2026-10-01 第六刀起，产品那条路
 * （`node home.js up`）**两种形态都由 `scripts/serve_client.mjs` 托管**（源码形态
 * `--dist client/dist`）——代转路由表只有那一份，`vite preview` 已经退役。
 * 这里留着是为了"改前端时还能 `npm run dev` 看到真页面"：跑之前自己把这几个环境变量设好
 * （`CELIRA_IM_URL` / `CELIRA_STATUS_PORT` / `CELIRA_CONFIG_PORT`）。
 *
 * 与产品同一条口径：**页面永远不知道口子开在哪个端口**，只知道同源的 `/home-status/*`。
 * 不设就兜默认口（家的默认值，见 `scripts/home/paths.js` 的 `DEFAULT_STATUS_PORT`）。
 */
const statusPort = process.env.CELIRA_STATUS_PORT ?? '5190'
const statusTarget = `http://127.0.0.1:${statusPort}`

/**
 * **模型配置门**的端口（TASK_77）。
 *
 * 与状态口同一条口径：页面只知道同源的 `/home-config`。产品那条路由
 * `serve_client.mjs --config-port` 代转（那个值来自 `home.conf` 的 `config_port`）；
 * 这里的环境变量只给 `npm run dev` 用（见上面那段）。
 * 不设就兜默认口（`scripts/home/paths.js` 的 `DEFAULT_CONFIG_PORT`）。
 */
const configPort = process.env.CELIRA_CONFIG_PORT ?? '5191'
const configTarget = `http://127.0.0.1:${configPort}`

export default defineConfig({
  server: {
    proxy: {
      '/v1': {
        target: process.env.CELIRA_IM_URL ?? 'http://127.0.0.1:8080',
        changeOrigin: true,
        ws: true,
      },
      // 状态口那两条路由**逐条写出来**（与 `serve_client.mjs` 的 STATUS_ROUTES 同形）：
      // 穷举而不是泛转前缀 —— 口子哪天多长一条路由，不该经由这里**静默**变成同源可达。
      //
      // ⚠ **这里只服务 `npm run dev`**：产品那条路（`up`）走 `serve_client.mjs` 的那张路由表。
      // 两张表要同时改这件事是本刀**没有**消掉的债（vite 是第三方 CLI，读不到我们的表）；
      // 记在这里，别以为改了一处就够。
      '/home-status/status': { target: statusTarget, changeOrigin: true, rewrite: () => '/status' },
      '/home-status/healthz': { target: statusTarget, changeOrigin: true, rewrite: () => '/healthz' },
      // 模型配置门那几条路径同款（TASK_77；**v1 收口三小债**补齐后两条）：`GET` 读、
      // `PUT` 保存+试连 / 换绑、`DELETE` 摘除、`POST` 只读探测，方法原样透传
      // （`rewrite` 只改路径，不动动词）。**不泛转前缀**，理由同上。
      // ⚠ 从前只写了 `/home-config` 前缀这一条：它（靠前缀匹配）能通 `/home-config/probe`，
      // 而 `/home-binding` 全缺 ⇒ 源码形态里「换绑 / 解除指定」落到 SPA 回退（拿到一份
      // `200` 的 `index.html`，页面按"读不懂的回执"报错）。两条都**逐条**列出，不靠前缀。
      '/home-config': { target: configTarget, changeOrigin: true },
      '/home-config/probe': { target: configTarget, changeOrigin: true },
      // ★ 2026-10-14：只读目录那一格（`POST /home-config/catalog`）——与上面两条同款**逐条**列出。
      // ⚠ **两张表要同时改**（这条债见上面那段）：`serve_client.mjs` 的 `CONFIG_ROUTES` 是产品
      //    形态那一份；漏了这一条，页面上「看这家的型号…」拿到的是 HTML（`verify-ui` 抓得到）。
      '/home-config/catalog': { target: configTarget, changeOrigin: true },
      '/home-binding': { target: configTarget, changeOrigin: true },
    },
  },
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },
})
