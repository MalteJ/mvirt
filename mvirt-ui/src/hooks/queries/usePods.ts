import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { listPods, getPod, createPod, deletePod, startPod, stopPod } from '@/api/endpoints'
import { useProject } from '@/hooks/useProject'
import type { CreatePodRequest } from '@/types'

export const podKeys = {
  all: ['pods'] as const,
  lists: () => [...podKeys.all, 'list'] as const,
  list: (projectId?: string) => [...podKeys.lists(), projectId] as const,
  details: () => [...podKeys.all, 'detail'] as const,
  detail: (id: string) => [...podKeys.details(), id] as const,
}

export function usePods() {
  const { currentProject } = useProject()
  return useQuery({
    queryKey: podKeys.list(currentProject?.id),
    queryFn: () => listPods(currentProject?.id),
    refetchInterval: 5000,
    enabled: !!currentProject,
  })
}

export function usePod(id: string) {
  return useQuery({
    queryKey: podKeys.detail(id),
    queryFn: () => getPod(id),
    enabled: !!id,
    refetchInterval: 2000,
  })
}

export function useCreatePod() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreatePodRequest) => createPod(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: podKeys.lists() })
    },
  })
}

export function useDeletePod() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deletePod(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: podKeys.lists() })
    },
  })
}

export function useStartPod() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => startPod(id),
    onSuccess: (pod) => {
      queryClient.invalidateQueries({ queryKey: podKeys.lists() })
      queryClient.setQueryData(podKeys.detail(pod.id), pod)
    },
  })
}

export function useStopPod() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => stopPod(id),
    onSuccess: (pod) => {
      queryClient.invalidateQueries({ queryKey: podKeys.lists() })
      queryClient.setQueryData(podKeys.detail(pod.id), pod)
    },
  })
}
