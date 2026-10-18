/**
 * **壳里的存储实现** —— `storage.ts` 那道窄口在 app 形态下的后端（2026-10-05）。
 *
 * ## 为什么要有它
 *
 * 前端状态层原先只住 webview 的 `localStorage`：那是**浏览器内部**的东西 —— 不在家里、不跟着家走、
 * 清一次站点数据就没了。窄口早就留好了（四个动作、全异步、零平台分支），缺的正是壳这一侧。
 * 现在四个动作各叫一条壳命令，落在 `<app_config_dir>/local-store.json`（见
 * `app/src-tauri/src/local_store.rs` 的文件头：落点、形状、坏文件让位、单值上限都在那里）。
 *
 * ## 三条纪律（与窄口逐字对齐）
 *
 * 1. **零平台分支**：本模块**只**知道"有个 `invoke` 能叫壳的命令"（注入进来的），
 *    不认识 `window`、不认识 Tauri 的全局；"要不要装"由装配处判（`installShellStorage`）；
 * 2. **失败如实返回**：读不到 ⇒ `null`、写失败 ⇒ `false`、`keys` 读不到 ⇒ 空表 —— **不抛**
 *    （窄口的接口纪律：调用方要能区分"没有这一格"与"读不出来"，但都不能变成一次崩溃）；
 * 3. **一个字节都不解结构**：只搬"键 → 字符串"。
 */

import { isTauriRuntime } from './shell.ts'
import { setStorage, type StoragePort } from './storage.ts'

/** 叫壳命令的那个东西（注入进来 ⇒ 用例里给个假的就能直测）。 */
export type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>

/**
 * 壳里的实现。**注入 `invoke`**，所以这一份是纯逻辑（用例里不碰真壳）。
 *
 * `write` 那一格的判据是**严格布尔**：壳回的必须是 `true` 才算写进去了 —— 回别的形状（`undefined`、
 * 字符串、对象）一律当"没写进去"（**认不出的形状不许说成成功**）。
 */
export function shellStorage(invoke: Invoke): StoragePort {
  return {
    async read(key) {
      try {
        const value = await invoke<string | null>('local_store_read', { key })
        return typeof value === 'string' ? value : null
      } catch {
        return null
      }
    },
    async write(key, value) {
      try {
        return (await invoke<boolean>('local_store_write', { key, value })) === true
      } catch {
        return false
      }
    },
    async remove(key) {
      try {
        await invoke('local_store_remove', { key })
      } catch {
        /* 删不掉就删不掉：这不是调用方要处理的事（与 `browserStorage` 同款） */
      }
    },
    async keys(prefix) {
      try {
        const out = await invoke<string[]>('local_store_keys', { prefix })
        return Array.isArray(out) ? out.filter((each) => typeof each === 'string') : []
      } catch {
        return []
      }
    },
  }
}

/**
 * 装配：**有壳才换**（判据是 `isTauriRuntime`，与传输面同一个判据 —— 不判平台名、不判构建变量）。
 *
 * 浏览器里这一行是空操作 ⇒ 同一份产物在浏览器里照旧走 `localStorage`。
 * 动态 `import` 让 Tauri 的 API 只在壳里被加载（web 产物不被污染，与 `installShellTransport` 同款）。
 */
export async function installShellStorage(): Promise<void> {
  if (!isTauriRuntime()) return
  const { invoke } = await import('@tauri-apps/api/core')
  setStorage(shellStorage(invoke))
}
