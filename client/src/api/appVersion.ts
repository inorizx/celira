/**
 * **这一份 app 自己的身份**：它在跑哪一版（关于页那一行）。
 *
 * ## 为什么单独一格，不塞进 `shellLocal.ts`
 *
 * 判据：**把 im 服务端换成微信/QQ，这一格还成立吗？** 成立 —— 它说的是"这个 app 是哪个构建"，
 * 不是世界的事实，也不是**家**面的事实（家面那些模块管的是"本机世界起没起来 / 家里有哪些机器"）。
 * 塞进 `shellLocal.ts` 会让读到它的页面在 `route-planes` 里被算成"读了家面"（实测：那两条守卫
 * 当场把我推回这里），所以它的权威就在本文件。
 *
 * ## 号从哪儿来
 *
 * 壳的 `app_version_readout` 读 **exe 旁边的 `VERSION`**（发布包里那份，三个发布单元同一个号）；
 * 源码形态没有那个文件 ⇒ 退回**编译期**的号，并把 `source` 标成 `build`。
 * ⚠ 这一格从前是页面上**写死的** `v0.1.0`：发一版就撒一次谎。读不到就说"版本读不到"。
 *
 * ## 桥的形状
 *
 * 与 `notifications.ts` 同款（**不走** `shellLocal.ts`）：`isTauriRuntime()` 判"有没有壳"，
 * 真要 invoke 时再动态 import —— 浏览器里这条整条不存在，一个字节都不多做。
 *
 * ⚠ **import 要带 `.ts` 后缀**：这一格会被 `node --test` 的类型剥离模式直接加载（`client/tests/
 * app-version.test.ts`），无后缀在那边解析不了（实测 `ERR_MODULE_NOT_FOUND`）；`shellLocal.ts`
 * 里那条同款 import 也是带后缀的。
 */
import { isTauriRuntime } from './shell.ts'

/** 壳回的版本读数（字段名与 `app/src-tauri/src/app_version.rs` 的 serde camelCase 逐字对齐）。 */
export interface AppVersionReadout {
  version: string
  /** 封闭词表：`package`（exe 旁边的 `VERSION`）／`build`（编译期那个号）。 */
  source: 'package' | 'build'
}

function asAppVersion(value: unknown): AppVersionReadout | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  const version = typeof raw.version === 'string' ? raw.version.trim() : ''
  if (version === '') return null
  return { version, source: raw.source === 'package' ? 'package' : 'build' }
}

/**
 * **纯函数**：关于页上那一行怎么写（`null` = 读不到 ⇒ 如实说，**不编一个号**）。
 *
 * 只有**源码形态**才标出"编译期"：发布包里那个号就是包的名字，不需要多一句话；源码形态那个号
 * 未必等于你手里那个包 —— 标出来读者才知道该怎么读。
 */
export function appVersionLine(readout: AppVersionReadout | null): string {
  if (!readout) return '版本读不到'
  return readout.source === 'build' ? `v${readout.version}（编译期）` : `v${readout.version}`
}

/** 这个 app 自己在跑哪一版（**不在壳里 ⇒ `null`**，浏览器里这条路整条不存在）。 */
export async function readAppVersion(): Promise<AppVersionReadout | null> {
  if (!isTauriRuntime()) return null
  try {
    const api = await import('@tauri-apps/api/core')
    return asAppVersion(await api.invoke('app_version_readout'))
  } catch {
    // 拿不到桥 = 这个壳里没有这条路 ⇒ 当"读不到"处置（**不抛**：关于页不该因为一格读数塌掉）。
    return null
  }
}
