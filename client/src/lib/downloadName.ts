/**
 * **下载的文件名**（壳里落盘用；纯逻辑，`node --test` 直测）。
 *
 * 三条判据都在这一个模块里（落点由 capability 的作用域管，不在这里）：
 *
 * 1. **文件名是不可信输入**：它来自对方（可能是模型生成的），可能带路径成分、控制字符、
 *    Windows 非法字符与保留名、结尾的点与空格 —— 一律消毒；
 * 2. **重名不覆盖**：加 ` (N)`。覆盖用户已有的文件是**不可逆操作**；
 * 3. **限长不切字符**：按**字节**算，且不许切在多字节字符中间。
 *
 * ⚠ 为什么放在页面这一侧而不是壳里：2026-10-08 那一刀原本把这三条写在 Rust（`downloads.rs`），
 * 但落盘机制最终走的是 **fs 插件**（Tauri 的下载钩子在本壳里不触发，账见 `app/README.md` §3.1）
 * ⇒ 名字这一格跟着落盘那一格走。壳那边只剩**权限**：capability 只放开 `$DOWNLOAD/**`。
 */

/** 名字消毒不出任何东西时的兜底名。 */
const FALLBACK_NAME = '附件'

/** 名字上限（**字节**，给 ` (N)` 后缀留余量）。 */
const MAX_NAME_BYTES = 180

/** 消毒后的文件名：只留最后一段、去非法字符与结尾的点/空格、避开 Windows 保留名。 */
export function sanitizeFileName(raw: string): string {
  // 名字里带路径（`../../x`、`C:\x`、`/etc/x`）一律当路径处理掉 —— 只取最后一段。
  const last = raw.split(/[\\/]/).pop() ?? ''
  let cleaned = ''
  for (const ch of last) {
    const code = ch.codePointAt(0) ?? 0
    const illegal =
      code < 0x20 || code === 0x7f || '<>:"|?*'.includes(ch)
    cleaned += illegal ? '_' : ch
  }
  // Windows 不吃结尾的点与空格（`a.txt.` 落盘会变成 `a.txt`）—— 自己去干净，别让它改我们的名字。
  const trimmed = cleaned.replace(/[. ]+$/, '')
  const named = trimmed === '' ? FALLBACK_NAME : trimmed
  return isReserved(named) ? `_${named}` : named
}

/**
 * 落点的文件名：`raw` 消毒后可用就用它；**已被占**就加 ` (N)`（`exists` 是注入的，
 * 于是这一条是纯逻辑：真机上喂 fs 插件的 `exists`，测试里喂一个集合）。
 */
export async function nextFileName(
  raw: string,
  exists: (candidate: string) => Promise<boolean>,
): Promise<string> {
  const name = capName(sanitizeFileName(raw))
  if (!(await exists(name))) return name
  const { stem, suffix } = splitExtension(name)
  for (let n = 1; n < 1000; n += 1) {
    const candidate = `${stem} (${n})${suffix}`
    if (!(await exists(candidate))) return candidate
  }
  // 999 个同名还不罢休：仍然**不覆盖**，换一个带时刻的名字。
  return `${stem}-${Date.now()}${suffix}`
}

/** Windows 保留名（`CON` / `NUL` / `COM1`…）：加个前缀，别造出一个系统打不开的名字。 */
function isReserved(name: string): boolean {
  const stem = (name.split('.')[0] ?? name).replace(/\s+$/, '')
  const upper = stem.toUpperCase()
  if (['CON', 'PRN', 'AUX', 'NUL'].includes(upper)) return true
  return (
    upper.length === 4 &&
    (upper.startsWith('COM') || upper.startsWith('LPT')) &&
    upper[3] >= '1' &&
    upper[3] <= '9'
  )
}

/** 按**字节**限长，保留扩展名，且**不切在多字节字符中间**。 */
function capName(name: string): string {
  if (byteLength(name) <= MAX_NAME_BYTES) return name
  const { stem, suffix } = splitExtension(name)
  const budget = MAX_NAME_BYTES - byteLength(suffix)
  let out = ''
  for (const ch of stem) {
    if (byteLength(out) + byteLength(ch) > budget) break
    out += ch
  }
  return `${out === '' ? FALLBACK_NAME : out}${suffix}`
}

/** 拆成（主名，`.扩展名`）；没有扩展名时第二项是空串。 */
function splitExtension(name: string): { stem: string; suffix: string } {
  const at = name.lastIndexOf('.')
  if (at <= 0 || at === name.length - 1) return { stem: name, suffix: '' }
  return { stem: name.slice(0, at), suffix: name.slice(at) }
}

/** UTF-8 字节数（`TextEncoder` 在浏览器与 Node 里都有）。 */
function byteLength(text: string): number {
  return new TextEncoder().encode(text).length
}
