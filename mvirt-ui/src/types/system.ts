export interface SystemInfo {
  version: string
  hostname: string
  cpuCount: number
  memoryTotalBytes: number
  memoryUsedBytes: number
  uptime: number
}

export interface ClusterNode {
  id: string
  hostname: string
  address: string
  isLeader: boolean
  state: 'online' | 'offline' | 'maintenance'
}
