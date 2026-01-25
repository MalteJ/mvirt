export enum NodeState {
  ONLINE = 'ONLINE',
  OFFLINE = 'OFFLINE',
  MAINTENANCE = 'MAINTENANCE',
  JOINING = 'JOINING',
}

export enum NodeRole {
  LEADER = 'LEADER',
  FOLLOWER = 'FOLLOWER',
  CANDIDATE = 'CANDIDATE',
}

export interface Node {
  id: string
  name: string
  address: string
  state: NodeState
  role: NodeRole
  version: string
  cpuCount: number
  memoryTotalBytes: number
  memoryUsedBytes: number
  vmCount: number
  uptime: number
  lastSeen: string
}

export interface ClusterInfo {
  id: string
  name: string
  nodeCount: number
  leaderNodeId: string
  term: number
  createdAt: string
}
