export interface Snapshot {
  id: string
  name: string
  createdAt: string
  usedBytes: number
}

export interface Volume {
  id: string
  projectId: string
  name: string
  path: string
  volsizeBytes: number
  usedBytes: number
  compressionRatio: number
  snapshots: Snapshot[]
}

export interface Template {
  id: string
  name: string
  sizeBytes: number
  cloneCount: number
}

export enum ImportJobState {
  PENDING = 'PENDING',
  RUNNING = 'RUNNING',
  COMPLETED = 'COMPLETED',
  FAILED = 'FAILED',
}

export interface ImportJob {
  id: string
  templateName: string
  state: ImportJobState
  bytesWritten: number
  totalBytes: number
  error?: string
}

export interface PoolStats {
  name: string
  totalBytes: number
  usedBytes: number
  freeBytes: number
}

export interface CreateVolumeRequest {
  name: string
  projectId: string
  sizeBytes: number
  templateId?: string
}

export interface ImportTemplateRequest {
  name: string
  url: string
}
