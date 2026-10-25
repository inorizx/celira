// **群简介**的纯逻辑（契约 `CORE_V1_ROOM_PROFILE.md`）。
//
// 这里只放三件可直测的事：写请求的固定形状（路径 / 方法 / snake_case 正文）、
// 正文的**前端早校验**（1..2000 UTF-8 字节、禁 U+0000、trim 后非空）、以及
// 「单房读面 → 视图模型」的映射与可编辑判据。无 React、无 DOM、无持久化。
//
// 三条纪律：
// - **字节口径与 Core 同一条**：`TextEncoder` 数的就是 UTF-8 字节（中文 3、Emoji 4），
//   不是 `String.length`（UTF-16 码元）—— 用后者会把 2000 字节的正文放行成 4000 字节。
// - **原样保存**：这里不 trim、不做 Unicode 归一化。`trim` 只用于「是不是只有空白」的裁决
//   （契约 §3.1 / I7）。
// - **前端校验只是早反馈**：Core 仍是唯一权威裁决；409 / 403 / 400 的人话由
//   `api/contractErrors.ts` 给出（本模块不另造第二张文案表）。
import type { Room } from './domain'
import type { RoomProfileView } from './contracts'

/** 正文上限（UTF-8 字节，契约 §3.1）。 */
export const ROOM_PROFILE_MAX_BYTES = 2000

/** `PUT /v1/rooms/{room_id}/profile` 的路径（shared by 传输层与测试）。 */
export function roomProfilePath(roomId: string): string {
  return `/v1/rooms/${encodeURIComponent(roomId)}/profile`
}

export interface RoomProfileWriteInput {
  baseVersion: number
  description: string
}

export interface RoomProfilePutRequest {
  path: string
  method: 'PUT'
  body: {
    base_version: number
    description: string
  }
}

/**
 * 写请求的固定形状：`base_version` 是 snake_case（服务端 `deny_unknown_fields`），
 * `description` 原样带上（不 trim —— 首尾空白是提交正文的一部分）。
 *
 * ⚠ 幂等键不在这里：它由 `core.ts` 每次调用新生成（与其它写端点同规），
 * 不进本函数的返回值，也就不可能被调用方复用或写进日志。
 */
export function buildRoomProfilePutRequest(
  roomId: string,
  input: RoomProfileWriteInput,
): RoomProfilePutRequest {
  return {
    path: roomProfilePath(roomId),
    method: 'PUT',
    body: {
      base_version: input.baseVersion,
      description: input.description,
    },
  }
}

/** 一段正文的 UTF-8 字节数（与 Core 的 BLOB 长度口径一致）。 */
export function roomProfileByteLength(description: string): number {
  return new TextEncoder().encode(description).length
}

/**
 * 正文的**早校验**；合法返回 `null`。
 *
 * 与 Core 的三条正文规则逐条对应（契约 §3.1）：禁 U+0000、1..2000 UTF-8 字节、
 * `trim` 后非空。顺序是「先看非法字符，再看长度，最后看空白」—— 一行只有空白时
 * 长度必然合法，先报「不能为空」比报字节数有用。
 */
export function roomProfileDraftError(description: string): string | null {
  if (description.includes('\u0000')) return '简介包含不支持的字符（U+0000），请删除后重试'
  const bytes = roomProfileByteLength(description)
  if (bytes > ROOM_PROFILE_MAX_BYTES) {
    return `简介超过 ${ROOM_PROFILE_MAX_BYTES} 字节上限（当前 ${bytes} 字节），请删减后保存`
  }
  if (description.trim().length === 0) return '简介不能为空'
  return null
}

/**
 * `GET /v1/rooms/{id}` 的 `profile` → `Room.profile`。
 *
 * `null` / 缺字段都映射成 `null`：「没有简介」与「这台 Core 还没给这一格」在**读面**
 * 上是同一件事（都是没读到内容）。真正的「读取失败」由 store 的 `keep previous` 承担，
 * 不会走到这里 —— 也就是说，界面上的「还没有群简介」只在 Core 明确答了 `null` 时出现。
 */
export function roomProfileFromView(
  view: RoomProfileView | null | undefined,
): NonNullable<Room['profile']> | null {
  if (!view) return null
  return {
    description: view.description,
    version: view.version,
    // 线协议是 `updated_at`，视图模型是 `updatedAt`：唯一翻译点就在这一行。
    updatedAt: view.updated_at,
  }
}

/**
 * 把单房读面的权威投影落回房间列表：**只改目标那一间**。
 *
 * 为什么是纯函数：`refreshRoom` 每次只拉一间房，而列表里其它房间的 `profile` 必须
 * 原样保留（契约 I6 只说列表端点不带这一格，不是说客户端要把已读到的抹掉）。
 * 抽出来之后这条「不清空别人」的规则可以被直测，而不是靠读 store 的 `map` 猜。
 */
export function applyRoomProfile(
  rooms: readonly Room[],
  roomId: string,
  view: RoomProfileView | null,
): Room[] {
  const profile = roomProfileFromView(view)
  return rooms.map((room) => (room.id === roomId ? { ...room, profile } : room))
}

/**
 * 该不该给「编辑群简介」入口：**三个条件同时成立**。
 *
 * - `kind === 'group'`：`direct` / `human_direct` 没有这一格（契约 §3.2 第 2 条）；
 * - `isOwner`：只有当前 active owner 能写，非群主只读（§3.2 第 3 条）；
 * - `supported`：传输层真的实现了 `putRoomProfile`（能力探测与 `uiPolicy` 同规，
 *   不按 `mode` 判 —— 演示通道也活在 `mode === 'core'` 里）。
 */
export function canEditRoomProfile(input: {
  kind?: Room['kind']
  isOwner: boolean
  supported: boolean
}): boolean {
  return input.supported && input.isOwner && input.kind === 'group'
}

/** 从任意错误对象里鸭子类型取稳定分类（与 `contractErrors` 同一套提取法）。 */
function codeOf(error: unknown): string {
  return error && typeof error === 'object' && 'code' in error
    ? String((error as { code?: unknown }).code ?? '')
    : ''
}

/**
 * 这次失败是不是 `base_version` 过期（409 `profile_version_conflict`）。
 *
 * 只有这一种失败需要「先权威重读最新版本，再让用户决定」；其余分类（403 / 400 /
 * 网络）只给固定人话，绝不自动改写任何东西。
 */
export function isProfileVersionConflict(error: unknown): boolean {
  return codeOf(error) === 'profile_version_conflict'
}
