export enum DatabaseState {
  CREATING = 'CREATING',
  RUNNING = 'RUNNING',
  STOPPED = 'STOPPED',
  FAILED = 'FAILED',
}

export enum DatabaseType {
  POSTGRESQL = 'POSTGRESQL',
  REDIS = 'REDIS',
}

export interface Database {
  id: string
  name: string
  state: DatabaseState
  type: DatabaseType
  version: string
  networkId: string
  host?: string
  port?: number
  username: string
  storageSizeGb: number
  usedStorageGb: number
  connections: number
  maxConnections: number
  createdAt: string
  startedAt?: string
  errorMessage?: string
}

export interface CreateDatabaseRequest {
  name: string
  type: DatabaseType
  version: string
  networkId: string
  storageSizeGb: number
  username: string
  password: string
}

export interface DatabaseListResponse {
  databases: Database[]
}
