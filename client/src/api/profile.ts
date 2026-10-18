export interface AgentProfileWriteInput {
  baseVersion: number
  shortDescription: string | null
  publicDescription: string | null
}

export interface AgentProfilePutRequest {
  path: string
  method: 'PUT'
  idempotencyKey: string
  body: {
    base_version: number
    short_description: string | null
    public_description: string | null
  }
}

export function agentProfilePath(agentId: string): string {
  return `/v1/agents/${encodeURIComponent(agentId)}/profile`
}

export function buildAgentProfilePutRequest(
  agentId: string,
  input: AgentProfileWriteInput,
  idempotencyKey: string,
): AgentProfilePutRequest {
  return {
    path: agentProfilePath(agentId),
    method: 'PUT',
    idempotencyKey,
    body: {
      base_version: input.baseVersion,
      short_description: input.shortDescription,
      public_description: input.publicDescription,
    },
  }
}
