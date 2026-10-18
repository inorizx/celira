import assert from 'node:assert/strict'
import test from 'node:test'
import { agentProfilePath, buildAgentProfilePutRequest } from '../src/api/profile.ts'

test('Agent profile GET path encodes the Core resource id', () => {
  assert.equal(agentProfilePath('agent/1'), '/v1/agents/agent%2F1/profile')
})

test('Agent profile PUT preserves version/null fields and idempotency key', () => {
    const request = buildAgentProfilePutRequest('agent-1', {
      baseVersion: 2,
      shortDescription: null,
      publicDescription: '新介绍',
    }, 'fixed-key')
    assert.equal(request.path, '/v1/agents/agent-1/profile')
    assert.equal(request.method, 'PUT')
    assert.equal(request.idempotencyKey, 'fixed-key')
    assert.deepEqual(request.body, {
      base_version: 2,
      short_description: null,
      public_description: '新介绍',
    })
})
