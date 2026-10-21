/**
 * **有没有新版本**（关于页那一颗「检查更新」）。
 *
 * ## 这一格只问，不换
 *
 * 壳的 `update_check` 只读：GitHub 优先、失败退 Gitee，回来的是一份读数
 * （有没有新版 / 那一版的包在哪 / 多大 / sha256）。**不下载、不替换、不动家**——
 * 下载与换文件夹是后面的事。
 *
 * ## 为什么单独一格，不塞进 `appVersion.ts`
 *
 * `appVersion.ts` 回答「**我在跑哪一版**」，是本机事实；这一格回答「**渠道上有哪一版**」，
 * 是网络上的事实。两者的失败方式也不同：前者读不到文件，后者连不上网。混在一格会让
 * "版本读不到"和"网络不通"共用一句话。
 *
 * ## 桥的形状
 *
 * 与 `appVersion.ts` 同款：`isTauriRuntime()` 判壳，真要 invoke 时动态 import，浏览器里整条不存在。
 * import 带 `.ts` 后缀（`node --test` 的类型剥离模式要它）。
 */
import { isTauriRuntime } from './shell.ts'

/** 渠道上那个包（壳回的字段名与 `app/src-tauri/src/update.rs` 的 camelCase 逐字对齐）。 */
export interface AppUpdateAsset {
  name: string
  url: string
  size: number | null
  /** 64 位小写十六进制；渠道没给就是 null。 */
  sha256: string | null
}

/** `state` 是封闭词表：`latest`（已经最新）／`newer`（有新版）／`unknown`（读不到）。 */
export interface AppUpdateReadout {
  state: 'latest' | 'newer' | 'unknown'
  current: string
  latest: string | null
  channel: 'github' | 'gitee' | null
  asset: AppUpdateAsset | null
  note: string | null
}

function asAsset(value: unknown): AppUpdateAsset | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  const name = typeof raw.name === 'string' ? raw.name.trim() : ''
  const url = typeof raw.url === 'string' ? raw.url.trim() : ''
  if (name === '' || url === '') return null
  return {
    name,
    url,
    size: typeof raw.size === 'number' ? raw.size : null,
    sha256: typeof raw.sha256 === 'string' && /^[0-9a-f]{64}$/.test(raw.sha256) ? raw.sha256 : null,
  }
}

function asReadout(value: unknown): AppUpdateReadout | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  const current = typeof raw.current === 'string' ? raw.current.trim() : ''
  if (current === '') return null
  const state = raw.state === 'newer' || raw.state === 'latest' ? raw.state : 'unknown'
  return {
    state,
    current,
    latest: typeof raw.latest === 'string' && raw.latest.trim() !== '' ? raw.latest.trim() : null,
    channel: raw.channel === 'github' || raw.channel === 'gitee' ? raw.channel : null,
    asset: asAsset(raw.asset),
    note: typeof raw.note === 'string' && raw.note.trim() !== '' ? raw.note.trim() : null,
  }
}

/** 字节数 → 人话（只用于"这一版多大"那一格；未知就说未知，不写 0）。 */
export function sizeLine(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes <= 0) return '大小未知'
  const mib = bytes / (1024 * 1024)
  return `${mib.toFixed(1)} MiB`
}

/**
 * **纯函数**：关于页上那一句怎么写。
 *
 * 三态各自说自己的话，不合并：
 * - `latest` ⇒ 已是最新；
 * - `newer` ⇒ 有新版 ＋ 大小；没有可下载的包时**明说**（不是"已是最新"）；
 * - `unknown` ⇒ 读不到 ＋ 渠道给的原因（原样，不翻译成"网络问题"）。
 */
export function updateLine(readout: AppUpdateReadout | null): string {
  if (!readout) return ''
  if (readout.state === 'latest') {
    return `已是最新（v${readout.latest ?? readout.current}）`
  }
  if (readout.state === 'newer') {
    if (!readout.asset) {
      return `有新版 v${readout.latest ?? '?'}，但这个渠道没有可下载的包`
    }
    return `有新版 v${readout.latest ?? '?'}（${sizeLine(readout.asset.size)}）`
  }
  return readout.note ? `读不到：${readout.note}` : '读不到'
}

/**
 * 问一次渠道（**不在壳里 ⇒ `null`**，这条整条不存在）。
 *
 * 不在壳里返回 `null` 而不是抛：调用方据此不渲染这一格，与 `readAppVersion` 同一条处置。
 */
export async function checkAppUpdate(): Promise<AppUpdateReadout | null> {
  if (!isTauriRuntime()) return null
  try {
    const api = await import('@tauri-apps/api/core')
    return asReadout(await api.invoke('update_check'))
  } catch {
    // 桥不在 / 命令没注册 ⇒ 拿不到就是拿不到（不编一个"已是最新"）。
    return null
  }
}

/** 解压好、验过版本的那一份（壳回的字段名与 `update_apply.rs` 的 camelCase 对齐）。 */
export interface PreparedUpdate {
  payloadDir: string
  version: string
  installDir: string
}

function asPrepared(value: unknown): PreparedUpdate | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  const payloadDir = typeof raw.payloadDir === 'string' ? raw.payloadDir.trim() : ''
  const version = typeof raw.version === 'string' ? raw.version.trim() : ''
  if (payloadDir === '' || version === '') return null
  return {
    payloadDir,
    version,
    installDir: typeof raw.installDir === 'string' ? raw.installDir : '',
  }
}

/**
 * **纯函数**：现在能不能点「下载并更新」；不能就给出原因（`null` = 可以）。
 *
 * 三条拒绝的顺序就是判据：没有新版 ⇒ 不更新；没有包 ⇒ 不更新；**没有 sha256 ⇒ 不更新**
 * （fail closed：下下来的字节无法证明，就不该换成它）。
 */
export function installBlocker(readout: AppUpdateReadout): string | null {
  if (readout.state !== 'newer') return '没有新版本'
  if (!readout.asset) return '渠道没有给出可下载的包'
  if (!readout.asset.sha256) return '渠道没有给出 sha256 —— 不能自动更新'
  return null
}

/**
 * 下载 → 校 sha256 → 解压。**失败抛错**（调用方把原话显示出来，不吞）。
 *
 * 三道拒绝都在这儿说清：不在壳里、渠道没给包、渠道没给 sha256。
 * 最后一条是 fail-closed：**没有哈希就不自动更新**（下下来的字节无法证明）。
 */
export async function downloadAppUpdate(readout: AppUpdateReadout): Promise<PreparedUpdate> {
  if (!isTauriRuntime()) throw new Error('这个形态不能更新自己')
  const blocker = installBlocker(readout)
  if (blocker) throw new Error(blocker)
  const asset = readout.asset
  if (!asset) throw new Error('渠道没有给出可下载的包')
  if (!asset.sha256) throw new Error('渠道没有给出 sha256 —— 不能自动更新')
  const api = await import('@tauri-apps/api/core')
  const raw = await api.invoke('update_download', {
    version: readout.latest ?? readout.current,
    url: asset.url,
    sha256: asset.sha256,
  })
  const prepared = asPrepared(raw)
  if (!prepared) throw new Error('壳回的下载结果读不懂')
  return prepared
}

/**
 * 交给壳：写替换脚本、起外部进程、然后壳退出。
 *
 * 这一句返回之后窗口会消失——不是崩溃，是让位给替换脚本（它等 2 秒，再 rename）。
 */
export async function applyAppUpdate(prepared: PreparedUpdate): Promise<void> {
  if (!isTauriRuntime()) throw new Error('这个形态不能更新自己')
  const api = await import('@tauri-apps/api/core')
  await api.invoke('update_apply', { payloadDir: prepared.payloadDir })
}
