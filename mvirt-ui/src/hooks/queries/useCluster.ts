import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { getClusterInfo, getNodes, getNode, setNodeMaintenance, removeNode } from '@/api/endpoints'

export function useClusterInfo() {
  return useQuery({
    queryKey: ['cluster'],
    queryFn: getClusterInfo,
  })
}

export function useNodes() {
  return useQuery({
    queryKey: ['nodes'],
    queryFn: getNodes,
  })
}

export function useNode(id: string) {
  return useQuery({
    queryKey: ['nodes', id],
    queryFn: () => getNode(id),
    enabled: !!id,
  })
}

export function useSetNodeMaintenance() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, maintenance }: { id: string; maintenance: boolean }) =>
      setNodeMaintenance(id, maintenance),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['nodes'] })
    },
  })
}

export function useRemoveNode() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: removeNode,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['nodes'] })
      queryClient.invalidateQueries({ queryKey: ['cluster'] })
    },
  })
}
