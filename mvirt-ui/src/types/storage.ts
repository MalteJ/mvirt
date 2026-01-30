export interface Snapshot {
  id: string
  name: string
  createdAt: string
  usedBytes: number
}

export interface Volume {
  id: string
  projectId: string
  nodeId: string
  name: string
  sizeBytes: number
  usedBytes: number
  compressionRatio: number
  snapshots: Snapshot[]
  templateId?: string
  createdAt: string
}

export interface Template {
  id: string
  nodeId: string
  name: string
  sizeBytes: number
  cloneCount: number
  createdAt: string
}

export enum ImportJobState {
  PENDING = 'PENDING',
  RUNNING = 'RUNNING',
  COMPLETED = 'COMPLETED',
  FAILED = 'FAILED',
}

export interface ImportJob {
  id: string
  nodeId: string
  templateName: string
  url: string
  state: ImportJobState
  bytesWritten: number
  totalBytes: number
  error?: string
  createdAt: string
}

export interface PoolStats {
  totalBytes: number
  usedBytes: number
  availableBytes: number
  compressionRatio: number
}

export interface CreateVolumeRequest {
  nodeId: string
  name: string
  sizeBytes: number
  templateId?: string
}

export interface ImportTemplateRequest {
  nodeId: string
  name: string
  url: string
}
