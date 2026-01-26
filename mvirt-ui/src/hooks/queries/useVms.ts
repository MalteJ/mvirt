import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { listVms, getVm, createVm, deleteVm, startVm, stopVm, killVm } from '@/api/endpoints'
import { useProject } from '@/hooks/useProject'
import type { CreateVmRequest } from '@/types'

export const vmKeys = {
  all: ['vms'] as const,
  lists: () => [...vmKeys.all, 'list'] as const,
  list: (projectId?: string) => [...vmKeys.lists(), projectId] as const,
  details: () => [...vmKeys.all, 'detail'] as const,
  detail: (id: string) => [...vmKeys.details(), id] as const,
}

export function useVms() {
  const { currentProject } = useProject()
  return useQuery({
    queryKey: vmKeys.list(currentProject?.id),
    queryFn: () => listVms(currentProject?.id),
    refetchInterval: 5000,
    enabled: !!currentProject,
  })
}

export function useVm(id: string) {
  return useQuery({
    queryKey: vmKeys.detail(id),
    queryFn: () => getVm(id),
    enabled: !!id,
    refetchInterval: 2000,
  })
}

export function useCreateVm() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateVmRequest) => createVm(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: vmKeys.lists() })
    },
  })
}

export function useDeleteVm() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteVm(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: vmKeys.lists() })
    },
  })
}

export function useStartVm() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => startVm(id),
    onSuccess: (vm) => {
      queryClient.invalidateQueries({ queryKey: vmKeys.lists() })
      queryClient.setQueryData(vmKeys.detail(vm.id), vm)
    },
  })
}

export function useStopVm() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => stopVm(id),
    onSuccess: (vm) => {
      queryClient.invalidateQueries({ queryKey: vmKeys.lists() })
      queryClient.setQueryData(vmKeys.detail(vm.id), vm)
    },
  })
}

export function useKillVm() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => killVm(id),
    onSuccess: (vm) => {
      queryClient.invalidateQueries({ queryKey: vmKeys.lists() })
      queryClient.setQueryData(vmKeys.detail(vm.id), vm)
    },
  })
}
