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
  network: (id: string) => [...networkKeys.networks(), id] as const,
  nics: () => [...networkKeys.all, 'nics'] as const,
  nic: (id: string) => [...networkKeys.nics(), id] as const,
}

export function useNetworks() {
  return useQuery({
    queryKey: networkKeys.networks(),
    queryFn: listNetworks,
  })
}

export function useCreateNetwork() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateNetworkRequest) => createNetwork(request),
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

export function useNics() {
  return useQuery({
    queryKey: networkKeys.nics(),
    queryFn: listNics,
  })
}

export function useCreateNic() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateNicRequest) => createNic(request),
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
