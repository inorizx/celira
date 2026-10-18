/**
 * 只服务演示数据 / 演示管理面的类型。
 *
 * 真实世界也用的「世界事实的形状」住在 `@/api/domain`；这里只剩下 Provider / Binding
 * 一类的演示管理面形状。
 */
export type BindingState = 'connected' | 'offline' | 'pending'

export interface AgentOwnership {
  agentId: string
  ownerId: string
  createdAt: string
}

export interface BackendBinding {
  agentId: string
  backendLabel: string
  state: BindingState
  scopes: string[]
}

export interface BackendCredential {
  id: string
  agentId: string
  label: string
  state: 'active' | 'revoked'
  issuedAt: string
}

export type ProviderStatus = 'connected' | 'unconfigured'

export type ModelCapability = 'text' | 'vision' | 'tools'

export interface Provider {
  id: string
  name: string
  initials: string
  apiAddress: string
  status: ProviderStatus
  keyUrl?: string
}

export interface ProviderModel {
  id: string
  providerId: string
  family: string
  label: string
  capabilities: ModelCapability[]
}
