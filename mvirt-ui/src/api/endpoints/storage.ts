import { get, post, del } from '../client'
import type {
  Volume,
  Template,
  ImportJob,
  PoolStats,
  CreateVolumeRequest,
  ImportTemplateRequest,
} from '@/types'

export async function listVolumes(): Promise<Volume[]> {
  const response = await get<{ volumes: Volume[] }>('/storage/volumes')
  return response.volumes
}

export async function getVolume(id: string): Promise<Volume> {
  return get<Volume>(`/storage/volumes/${id}`)
}

export async function createVolume(request: CreateVolumeRequest): Promise<Volume> {
  return post<Volume>('/storage/volumes', request)
}

export async function deleteVolume(id: string): Promise<void> {
  await del<void>(`/storage/volumes/${id}`)
}

export async function resizeVolume(id: string, sizeBytes: number): Promise<Volume> {
  return post<Volume>(`/storage/volumes/${id}/resize`, { sizeBytes })
}

export async function createSnapshot(volumeId: string, name: string): Promise<Volume> {
  return post<Volume>(`/storage/volumes/${volumeId}/snapshots`, { name })
}

export async function rollbackSnapshot(volumeId: string, snapshotId: string): Promise<Volume> {
  return post<Volume>(`/storage/volumes/${volumeId}/snapshots/${snapshotId}/rollback`)
}

export async function listTemplates(): Promise<Template[]> {
  const response = await get<{ templates: Template[] }>('/storage/templates')
  return response.templates
}

export async function importTemplate(request: ImportTemplateRequest): Promise<ImportJob> {
  return post<ImportJob>('/storage/templates/import', request)
}

export async function getImportJob(id: string): Promise<ImportJob> {
  return get<ImportJob>(`/storage/import-jobs/${id}`)
}

export async function getPoolStats(): Promise<PoolStats> {
  return get<PoolStats>('/storage/pool')
}
