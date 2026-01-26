export enum VmState {
  STOPPED = 'STOPPED',
  STARTING = 'STARTING',
  RUNNING = 'RUNNING',
  STOPPING = 'STOPPING',
}

export interface DiskConfig {
  path: string
  readonly: boolean
}

export interface NicConfig {
  macAddress: string
  networkId: string
}

export interface VmConfig {
  vcpus: number
  memoryMb: number
  kernelPath?: string
  initrdPath?: string
  cmdline?: string
  bootDisk?: string
  disks: DiskConfig[]
  nics: NicConfig[]
  userData?: string
}

export interface Vm {
  id: string
  projectId: string
  name: string
  state: VmState
  config: VmConfig
  createdAt: string
  startedAt?: string
}

export interface CreateVmRequest {
  name: string
  projectId: string
  config: VmConfig
}

export interface VmListResponse {
  vms: Vm[]
}
