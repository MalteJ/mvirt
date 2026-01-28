export interface Network {
  id: string
  projectId: string
  name: string
  ipv4Subnet?: string
  ipv6Prefix?: string
  nicCount: number
}

export enum NicState {
  DETACHED = 'DETACHED',
  ATTACHED = 'ATTACHED',
}

export interface Nic {
  id: string
  projectId: string
  name: string
  macAddress: string
  networkId: string
  vmId?: string
  state: NicState
  ipv4Address?: string
  ipv6Address?: string
}

export interface CreateNetworkRequest {
  name: string
  ipv4Subnet?: string
  ipv6Prefix?: string
}

export interface CreateNicRequest {
  name: string
  networkId: string
  macAddress?: string
}
