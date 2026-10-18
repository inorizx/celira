import assert from 'node:assert/strict'
import test from 'node:test'
import {
  buildRoomPresentationPutRequest,
  memberPresentationsPath,
  roomPresentationErrorMessage,
  roomPresentationPath,
} from '../src/api/presentation.ts'

test('Room Presentation summary and detail paths encode resource ids', () => {
  assert.equal(memberPresentationsPath('room/1'), '/v1/rooms/room%2F1/member-presentations')
  assert.equal(
    roomPresentationPath('room/1', 'agent/2'),
    '/v1/rooms/room%2F1/members/agent%2F2/presentation',
  )
})

test('Room Presentation PUT preserves base version, nullable fields, and idempotency key', () => {
  const request = buildRoomPresentationPutRequest(
    'room-1',
    'agent-1',
    {
      baseVersion: 3,
      roomDisplayName: '群内助手',
      roomRoleName: null,
      roomRoleDescription: '负责整理上下文',
    },
    'fixed-room-key',
  )
  assert.equal(request.path, '/v1/rooms/room-1/members/agent-1/presentation')
  assert.equal(request.method, 'PUT')
  assert.equal(request.idempotencyKey, 'fixed-room-key')
  assert.deepEqual(request.body, {
    base_version: 3,
    room_display_name: '群内助手',
    room_role_name: null,
    room_role_description: '负责整理上下文',
  })
})

test('Room Presentation errors map stale without exposing raw response details', () => {
  assert.equal(
    roomPresentationErrorMessage({ code: 'room_presentation_stale' }),
    '群内成员介绍已被其他设备更新，请刷新后再保存',
  )
  assert.equal(
    roomPresentationErrorMessage({ code: 'room_presentation_invalid' }),
    '介绍内容不符合长度或正文规则，请修改后重试',
  )
  assert.equal(roomPresentationErrorMessage({ code: 'unknown_error', message: 'secret' }), '群成员介绍加载或保存失败，请检查连接后重试')
})
