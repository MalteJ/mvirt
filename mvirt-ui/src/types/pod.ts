export enum PodState {
  CREATED = 'CREATED',
  STARTING = 'STARTING',
  RUNNING = 'RUNNING',
  STOPPING = 'STOPPING',
  STOPPED = 'STOPPED',
  FAILED = 'FAILED',
}

export enum ContainerState {
  CREATING = 'CREATING',
  CREATED = 'CREATED',
  RUNNING = 'RUNNING',
  STOPPED = 'STOPPED',
  FAILED = 'FAILED',
}

export interface Container {
  id: string
  name: string
  state: ContainerState
  image: string
  exitCode?: number
  errorMessage?: string
}

export interface ContainerSpec {
  name: string
  image: string
  command?: string
  args?: string[]
  env?: Record<string, string>
  workingDir?: string
}

export interface Pod {
  id: string
  name: string
  state: PodState
  networkId: string
  vmId?: string
  containers: Container[]
  ipAddress?: string
  createdAt: string
  startedAt?: string
  errorMessage?: string
}

export interface CreatePodRequest {
  name: string
  networkId: string
  containers: ContainerSpec[]
}

export interface PodListResponse {
  pods: Pod[]
}
