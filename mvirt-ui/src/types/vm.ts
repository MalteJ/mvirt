export enum VmState {
  STOPPED = 'STOPPED',
  STARTING = 'STARTING',
  RUNNING = 'RUNNING',
  STOPPING = 'STOPPING',
}

export interface VmConfig {
  vcpus: number
  memoryMb: number
  volumeId: string
  nicId: string
  image: string
}

export interface Vm {
  id: string
  projectId: string
  name: string
  state: VmState
  config: VmConfig
  createdAt: string
  startedAt?: string
  nodeId?: string
  ipAddress?: string
}

export interface CreateVmConfig {
  vcpus: number
  memoryMb: number
  volumeId: string
  nicId: string
  image: string
}

export interface CreateVmRequest {
  name: string
  config: CreateVmConfig
  nodeSelector?: string
}

export interface VmListResponse {
  vms: Vm[]
}
