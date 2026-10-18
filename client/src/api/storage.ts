/**
 * **本机持久存储的窄口** —— 状态层与"谁提供存储"之间唯一的一道缝。
 *
 * ## 为什么要有这个模块
 *
 * 前端的状态层（cursor / 系统消息 / 房间缓存 / 用户侧偏好）原先**直接调 `localStorage`**，
 * 于是"能不能存东西"这件事就跟**浏览器**绑死了。第二个壳（桌面 / 手机 app）来了以后，
 * 它没有 `localStorage`——它有的是文件或平台自己的存储。**缝不收在这里，就得改每一个调用点。**
 *
 * ## 三条纪律
 *
 * 1. **异步**：`localStorage` 是同步的，文件不是。接口从第一天起就是 `Promise`——
 *    今天定，调用点只多一个 `await`；将来定，就要把所有调用点改一遍。
 *    这条是**接口形状的唯一理由**，不是洁癖。
 * 2. **零平台分支**：本模块里**不许**出现 `if (platform)` / 能力探测 / 插件注册。
 *    换平台 = **换一份实现**，调用方一行不改。（`AGENTS.md` 明禁为想象中的扩展预建万能层。）
 * 3. **只放取值**：这里只管"键 → 字符串"，**不认识 JSON、不认识业务**。
 *    序列化归调用方——本模块一旦开始解结构，就会变成第二个领域权威。
 *
 * ## 今天的实现
 *
 * 只有一种：浏览器的 `localStorage`（`window.localStorage`）。**app 壳那一步换的就是这里**，
 * 换的时候本模块以外的文件一行都不用动。
 *
 * ## 一条边界（如实说）
 *
 * `localStorage` 有容量上限（各浏览器约 5–10 MiB）且**只在浏览器里存在**。所以本模块
 * 不是"通用 KV 存储"，它是"当前平台能给的那一份"。将来 app 的实现可以是文件、可以是
 * 平台存储——**判据只有一条：读写同样的键、返回同样的形状**。
 *
 * ## 第四条（2026-10-08 存储收尾）：**老位置**与**搬家读**
 *
 * 见文件末那一节。它是本文件里**唯一**一处"狭口以外还有一份老地方"的东西；那不是平台分支
 * （它不判"我在哪个平台"，只认"老位置是个历史事实"），理由与判据都写在那节开头。
 */

/**
 * 存储的窄口。**只有四个动作**，多一个都不加（没有消费者就不加）。
 *
 * 读不到、写失败一律**如实返回**（`null` / `false`），不抛错、不吞掉——
 * 调用方要能区分"没有这一格"与"读不出来"。
 */
export interface StoragePort {
  read(key: string): Promise<string | null>
  write(key: string, value: string): Promise<boolean>
  remove(key: string): Promise<void>
  /** 以 `prefix` 开头、属于**本作用域**的键（用于清理；不带前缀的键不在其中）。 */
  keys(prefix: string): Promise<string[]>
}

/**
 * 浏览器实现：`localStorage`。
 *
 * **为什么把 `window` 的存在性判在这里**：这是"当前平台"的判据，属于实现内部；
 * 调用方永远不该问"我在哪个平台"。
 *
 * 越界、配额满（`QuotaExceededError`）、被隐私模式禁用——一律 `write` 返回 `false`，
 * **不抛**。理由：写偏好失败不该让一整页崩掉；而"失败"这件事仍然如实回给调用方。
 */
export function browserStorage(target?: Storage): StoragePort {
  const store = target ?? (typeof globalThis.localStorage !== 'undefined' ? globalThis.localStorage : null)
  return {
    async read(key) {
      if (!store) return null
      try {
        return store.getItem(key)
      } catch {
        return null
      }
    },
    async write(key, value) {
      if (!store) return false
      try {
        store.setItem(key, value)
        return true
      } catch {
        return false
      }
    },
    async remove(key) {
      if (!store) return
      try {
        store.removeItem(key)
      } catch {
        /* 删不掉就删不掉：这不是调用方要处理的事 */
      }
    },
    async keys(prefix) {
      if (!store) return []
      const out: string[] = []
      for (let i = 0; i < store.length; i += 1) {
        const key = store.key(i)
        if (key !== null && key.startsWith(prefix)) out.push(key)
      }
      return out
    },
  }
}

/**
 * 当前平台的存储。**应用装配处只认这一个名字**——壳换实现，换的就是 `setStorage` 那一行
 * （2026-10-05：壳里那份实现落在 `shellStorage.ts`，装配在 `main.tsx` 第一行旁边）。
 *
 * ⚠ 它是一个**转发门面**（不是直接持有某一份实现）：调用方照旧 `import { storage }`，
 * 而"谁在提供存储"由装配处决定 —— 于是换平台不改任何调用点，也不需要在这里写平台分支
 * （本模块的第二条纪律：零平台分支）。
 */
let current: StoragePort = browserStorage()

/** 换一份实现（只有装配处该调它：有壳 ⇒ 壳的文件后端；浏览器 ⇒ 留默认）。 */
export function setStorage(next: StoragePort): void {
  current = next
}

/** 当前的实现（给用例与装配处看；业务代码不许用它去问"我是谁"）。 */
export function currentStorage(): StoragePort {
  return current
}

export const storage: StoragePort = {
  read: (key) => current.read(key),
  write: (key, value) => current.write(key, value),
  remove: (key) => current.remove(key),
  keys: (prefix) => current.keys(prefix),
}

// ── 老位置（webview 的 `localStorage`）与搬家读 ─────────────────────────────
//
// ## 为什么需要它（2026-10-08 存储收尾，第二刀）
//
// 前端状态层原先住在 **webview 的 `localStorage`** 里：那是**浏览器内部**的东西 —— 不在家里、
// 不跟着家走、清一次站点数据就没了。窄口接上之后，同一个键就有了**两处可能**：**新家**（窄口；
// 壳里 = `<app_config_dir>/local-store.json`）与**老地方**（`localStorage`）。
//
// 用户升上来时**若不读老的**，他写过的草稿、选过的主题与壁纸、置顶过的会话就会**静默归零**。
// 那是我方换存储造成的损失，不该由用户承担 —— 连接偏好那一半（2026-10-08 第一刀）已经按
// 这条判据做过一次，这一刀把它推广到其余四族。
//
// ## 判据（三条，按顺序 —— `preferStoredRaw`，纯函数）
//
// 1. 窄口**读得到**（哪怕是空串这种合法值）⇒ **用它**，一个字都不问老地方；
// 2. 窄口读不到、老地方有 ⇒ **用老那份，并写进窄口**（下次它就有了）；
// 3. 两处都没有 ⇒ `null`（首访）。
//
// ⚠ **老位置只被本文件读**（`readThrough` 与下面两个清理动作）—— 这是全仓唯一的例外，
//   也是"读穿式迁移"这条判据**唯一**的落点。机械守卫见 `tests/local-state.test.ts`：
//   **`src/` 里 `localStorage` 这个标识符只许出现在本文件**。
// ⚠ 老位置**只读不写**：迁移是把老值写进**窄口**；老值原样留着，等登出清理（它不再被读）。

/** 老位置（webview 的 `localStorage`）。取不到（非浏览器平台 / 隐私模式）⇒ `null`。 */
function legacyBucket(): Storage | null {
  try {
    return typeof globalThis.localStorage !== 'undefined' ? globalThis.localStorage : null
  } catch {
    return null
  }
}

/** 从老位置读一格（读不到 / 被禁用 ⇒ `null`，绝不抛）。 */
function legacyRawOf(key: string): string | null {
  const store = legacyBucket()
  if (!store) return null
  try {
    return store.getItem(key)
  } catch {
    return null
  }
}

/**
 * **该用哪一份原文**（读穿式迁移的判据本体，一处）。
 *
 * ⚠ "窄口有值" 指的是**读得到**（哪怕是空串的合法值），不是"解析得出" —— 解析每个调用方
 *   自己负责（它对读不懂的内容各自退回默认值，本函数不替它判）。
 */
export function preferStoredRaw(
  portRaw: string | null,
  legacyRaw: string | null,
): { raw: string | null; migrate: boolean } {
  if (portRaw !== null) return { raw: portRaw, migrate: false }
  if (legacyRaw !== null) return { raw: legacyRaw, migrate: true }
  return { raw: null, migrate: false }
}

/**
 * **搬家读**：读一个键，顺带把老位置里那一格搬进窄口。**唯一允许读老位置的地方。**
 *
 * 搬家写失败**不算失败**：这一次仍用老值（`console.warn` 留痕），下次启动还会再试一次 ——
 * 读不到偏好比"没搬成"糟得多，绝不因为它拦下任何渲染。
 */
export async function readThrough(key: string): Promise<string | null> {
  const chosen = preferStoredRaw(await current.read(key), legacyRawOf(key))
  if (!chosen.migrate || chosen.raw === null) return chosen.raw
  if (!(await current.write(key, chosen.raw))) {
    console.warn(`[storage] 老位置里那一格（${key}）没能搬进新存储：这次先用着，下次启动再试`)
  }
  return chosen.raw
}

/** 老位置里以 `prefix` 开头的键（**登出清理**用；读不到就是空表）。 */
export function legacyKeysUnder(prefix: string): string[] {
  const store = legacyBucket()
  if (!store) return []
  const out: string[] = []
  try {
    for (let i = 0; i < store.length; i += 1) {
      const key = store.key(i)
      if (key !== null && key.startsWith(prefix)) out.push(key)
    }
  } catch {
    return []
  }
  return out
}

/** 删掉老位置里这些键（**登出清理**用；删不掉就删不掉，与 `StoragePort.remove` 同款）。 */
export function legacyRemove(keys: readonly string[]): void {
  const store = legacyBucket()
  if (!store) return
  for (const key of keys) {
    try {
      store.removeItem(key)
    } catch {
      /* 删不掉就删不掉：这不是调用方要处理的事 */
    }
  }
}
