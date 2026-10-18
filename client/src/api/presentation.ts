// Group Room Presentation 写请求构造纯逻辑（CORE_V1_PARTICIPANT_PRESENTATION.md
// §2.2/§5/§7）。只描述 URL / HTTP 方法 / 请求体 / Idempotency-Key 的固定组合，
// 供 core.ts 传输层与测试共用；请求体字段可显式 `null`（= 空投影）。绝不携带
// 任何凭据或会话密钥。

export interface RoomPresentationWriteInput {
  baseVersion: number
  roomDisplayName: string | null
  roomRoleName: string | null
  roomRoleDescription: string | null
}

export interface RoomPresentationPutRequest {
  path: string
  method: 'PUT'
  idempotencyKey: string
  body: {
    base_version: number
    room_display_name: string | null
    room_role_name: string | null
    room_role_description: string | null
  }
}

/** Core 稳定错误码 → 群内 Presentation 的中文反馈；不暴露原始响应正文。 */
export function roomPresentationErrorMessage(error: unknown): string {
  const code =
    error && typeof error === 'object' && 'code' in error
      ? String((error as { code?: unknown }).code)
      : ''
  switch (code) {
    case 'room_presentation_stale':
      return '群内成员介绍已被其他设备更新，请刷新后再保存'
    case 'room_presentation_invalid':
      return '介绍内容不符合长度或正文规则，请修改后重试'
    case 'room_presentation_not_supported':
      return '这个房间类型不支持群成员介绍'
    case 'idempotency_conflict':
      return '重复请求冲突，请重新保存'
    case 'agent_not_active':
      return '这个 Agent 已失效，当前不能编辑'
    case 'agent_not_found':
      return '没有找到这位 Agent，可能已被撤销'
    case 'member_not_found':
      return '这位成员已不在群内或不再是可编辑目标'
    case 'room_not_found':
      return '房间不存在或当前不是群主，无法编辑'
    default:
      return '群成员介绍加载或保存失败，请检查连接后重试'
  }
}

/** GET/PUT 成员 Presentation 的路径（shared by detail GET & detail PUT）。 */
export function roomPresentationPath(roomId: string, participantId: string): string {
  return `/v1/rooms/${encodeURIComponent(roomId)}/members/${encodeURIComponent(participantId)}/presentation`
}

/** GET /v1/rooms/{room_id}/member-presentations 摘要路径。 */
export function memberPresentationsPath(roomId: string): string {
  return `/v1/rooms/${encodeURIComponent(roomId)}/member-presentations`
}

export function buildRoomPresentationPutRequest(
  roomId: string,
  participantId: string,
  input: RoomPresentationWriteInput,
  idempotencyKey: string,
): RoomPresentationPutRequest {
  return {
    path: roomPresentationPath(roomId, participantId),
    method: 'PUT',
    idempotencyKey,
    body: {
      base_version: input.baseVersion,
      room_display_name: input.roomDisplayName,
      room_role_name: input.roomRoleName,
      room_role_description: input.roomRoleDescription,
    },
  }
}