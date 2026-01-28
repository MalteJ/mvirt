import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listVolumes,
  getVolume,
  createVolume,
  deleteVolume,
  resizeVolume,
  createSnapshot,
  listTemplates,
  importTemplate,
  getImportJob,
  getPoolStats,
} from '@/api/endpoints'
import type { CreateVolumeRequest, ImportTemplateRequest } from '@/types'

export const storageKeys = {
  all: ['storage'] as const,
  volumes: () => [...storageKeys.all, 'volumes'] as const,
  volumeList: (projectId: string) => [...storageKeys.volumes(), 'list', projectId] as const,
  volume: (id: string) => [...storageKeys.volumes(), id] as const,
  templates: (projectId: string) => [...storageKeys.all, 'templates', projectId] as const,
  importJobs: () => [...storageKeys.all, 'import-jobs'] as const,
  importJob: (id: string) => [...storageKeys.importJobs(), id] as const,
  pool: () => [...storageKeys.all, 'pool'] as const,
}

export function useVolumes(projectId: string) {
  return useQuery({
    queryKey: storageKeys.volumeList(projectId),
    queryFn: () => listVolumes(projectId),
  })
}

export function useVolume(id: string) {
  return useQuery({
    queryKey: storageKeys.volume(id),
    queryFn: () => getVolume(id),
    enabled: !!id,
  })
}

export function useCreateVolume(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateVolumeRequest) => createVolume(projectId, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: storageKeys.volumes() })
    },
  })
}

export function useDeleteVolume() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteVolume(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: storageKeys.volumes() })
    },
  })
}

export function useResizeVolume() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, sizeBytes }: { id: string; sizeBytes: number }) =>
      resizeVolume(id, sizeBytes),
    onSuccess: (volume) => {
      queryClient.invalidateQueries({ queryKey: storageKeys.volumes() })
      queryClient.setQueryData(storageKeys.volume(volume.id), volume)
    },
  })
}

export function useCreateSnapshot() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ volumeId, name }: { volumeId: string; name: string }) =>
      createSnapshot(volumeId, name),
    onSuccess: (volume) => {
      queryClient.setQueryData(storageKeys.volume(volume.id), volume)
    },
  })
}

export function useTemplates(projectId: string) {
  return useQuery({
    queryKey: storageKeys.templates(projectId),
    queryFn: () => listTemplates(projectId),
  })
}

export function useImportTemplate(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: ImportTemplateRequest) => importTemplate(projectId, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: storageKeys.importJobs() })
    },
  })
}

export function useImportJob(id: string) {
  return useQuery({
    queryKey: storageKeys.importJob(id),
    queryFn: () => getImportJob(id),
    enabled: !!id,
    refetchInterval: 1000,
  })
}

export function usePoolStats() {
  return useQuery({
    queryKey: storageKeys.pool(),
    queryFn: getPoolStats,
  })
}
