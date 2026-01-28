import { get, post, del } from '../client'
import type {
  Volume,
  Template,
  ImportJob,
  PoolStats,
  CreateVolumeRequest,
  ImportTemplateRequest,
} from '@/types'

export async function listVolumes(projectId: string): Promise<Volume[]> {
  const response = await get<{ volumes: Volume[] }>(`/projects/${projectId}/volumes`)
  return response.volumes
}

export async function getVolume(id: string): Promise<Volume> {
  return get<Volume>(`/volumes/${id}`)
}

export async function createVolume(projectId: string, request: CreateVolumeRequest): Promise<Volume> {
  return post<Volume>(`/projects/${projectId}/volumes`, request)
}

export async function deleteVolume(id: string): Promise<void> {
  await del<void>(`/volumes/${id}`)
}

export async function resizeVolume(id: string, sizeBytes: number): Promise<Volume> {
  return post<Volume>(`/volumes/${id}/resize`, { sizeBytes })
}

export async function createSnapshot(volumeId: string, name: string): Promise<Volume> {
  return post<Volume>(`/volumes/${volumeId}/snapshots`, { name })
}

export async function rollbackSnapshot(volumeId: string, snapshotId: string): Promise<Volume> {
  return post<Volume>(`/volumes/${volumeId}/snapshots/${snapshotId}/rollback`)
}

export async function listTemplates(projectId: string): Promise<Template[]> {
  const response = await get<{ templates: Template[] }>(`/projects/${projectId}/templates`)
  return response.templates
}

export async function importTemplate(projectId: string, request: ImportTemplateRequest): Promise<ImportJob> {
  return post<ImportJob>(`/projects/${projectId}/templates/import`, request)
}

export async function getImportJob(id: string): Promise<ImportJob> {
  return get<ImportJob>(`/import-jobs/${id}`)
}

export async function getPoolStats(): Promise<PoolStats> {
  return get<PoolStats>('/pool')
}
