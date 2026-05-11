export enum NodeStatus {
  ONLINE = 'online',
  OFFLINE = 'offline',
  ONBOARDING = 'onboarding',
  REVOKED = 'revoked',
  UNKNOWN = 'unknown',
}

export interface NodeResources {
  cpu_cores: number
  memory_mb: number
  storage_gb: number
  available_cpu_cores: number
  available_memory_mb: number
  available_storage_gb: number
}

/** Wire shape from /v1/nodes (camelCase). */
export interface Node {
  id: string
  name: string
  address: string
  status: NodeStatus
  resources: NodeResources
  labels: Record<string, string>
  lastHeartbeat: string
  createdAt: string
  updatedAt: string
  clusterSlug?: string
  certSerialHex?: string
  certExpiresAt?: string
  hostname?: string
  agentVersion?: string
}

export interface ControlplanePeer {
  id: number
  name: string
  address: string
  state: string
  is_leader: boolean
}

export interface ClusterInfo {
  cluster_id: string
  leader_id: number
  current_term: number
  commit_index: number
  peers: ControlplanePeer[]
}
