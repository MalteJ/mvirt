import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listNetworks,
  createNetwork,
  deleteNetwork,
  listNics,
  createNic,
  deleteNic,
  attachNic,
  detachNic,
} from '@/api/endpoints'
import type { CreateNetworkRequest, CreateNicRequest } from '@/types'

export const networkKeys = {
  all: ['network'] as const,
  networks: () => [...networkKeys.all, 'networks'] as const,
  networkList: (projectId: string) => [...networkKeys.networks(), 'list', projectId] as const,
  network: (id: string) => [...networkKeys.networks(), id] as const,
  nics: () => [...networkKeys.all, 'nics'] as const,
  nicList: (projectId: string) => [...networkKeys.nics(), 'list', projectId] as const,
  nic: (id: string) => [...networkKeys.nics(), id] as const,
}

export function useNetworks(projectId: string) {
  return useQuery({
    queryKey: networkKeys.networkList(projectId),
    queryFn: () => listNetworks(projectId),
  })
}

export function useCreateNetwork(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateNetworkRequest) => createNetwork(projectId, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: networkKeys.networks() })
    },
  })
}

export function useDeleteNetwork() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteNetwork(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: networkKeys.networks() })
    },
  })
}

export function useNics(projectId: string) {
  return useQuery({
    queryKey: networkKeys.nicList(projectId),
    queryFn: () => listNics(projectId),
  })
}

export function useCreateNic(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateNicRequest) => createNic(projectId, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: networkKeys.nics() })
    },
  })
}

export function useDeleteNic() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteNic(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: networkKeys.nics() })
    },
  })
}

export function useAttachNic() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ nicId, vmId }: { nicId: string; vmId: string }) =>
      attachNic(nicId, vmId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: networkKeys.nics() })
    },
  })
}

export function useDetachNic() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (nicId: string) => detachNic(nicId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: networkKeys.nics() })
    },
  })
}
