import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listClustersInOrg,
  getCluster,
  createClusterInOrg,
  updateCluster,
  deleteCluster,
  addNodeToCluster,
  removeNodeFromCluster,
  listOnboardingTokens,
  createOnboardingToken,
  deleteOnboardingToken,
  revokeNode,
  type NodeRevocationReason,
} from '@/api/endpoints'
import type {
  CreateClusterRequest,
  CreateOnboardingTokenRequest,
  UpdateClusterRequest,
} from '@/types'

export const clusterKeys = {
  all: ['clusters'] as const,
  listsInOrg: (orgSlug: string) => [...clusterKeys.all, 'org', orgSlug] as const,
  detail: (slug: string) => [...clusterKeys.all, 'detail', slug] as const,
  tokens: (slug: string) => [...clusterKeys.all, 'tokens', slug] as const,
}

export function useClustersInOrg(orgSlug: string | undefined) {
  return useQuery({
    queryKey: clusterKeys.listsInOrg(orgSlug ?? ''),
    queryFn: () => listClustersInOrg(orgSlug!),
    enabled: !!orgSlug,
  })
}

export function useCluster(slug: string | undefined) {
  return useQuery({
    queryKey: clusterKeys.detail(slug ?? ''),
    queryFn: () => getCluster(slug!),
    enabled: !!slug,
  })
}

export function useCreateClusterInOrg(orgSlug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateClusterRequest) =>
      createClusterInOrg(orgSlug, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.listsInOrg(orgSlug) })
      queryClient.invalidateQueries({ queryKey: clusterKeys.all })
    },
  })
}

export function useUpdateCluster(slug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: UpdateClusterRequest) => updateCluster(slug, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.all })
    },
  })
}

export function useDeleteCluster() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (slug: string) => deleteCluster(slug),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.all })
    },
  })
}

export function useAddNodeToCluster(slug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (nodeId: string) => addNodeToCluster(slug, nodeId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.detail(slug) })
    },
  })
}

export function useRemoveNodeFromCluster(slug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (nodeId: string) => removeNodeFromCluster(slug, nodeId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.detail(slug) })
    },
  })
}

// =============================================================================
// Onboarding tokens
// =============================================================================

export function useOnboardingTokens(clusterSlug: string | undefined) {
  return useQuery({
    queryKey: clusterKeys.tokens(clusterSlug ?? ''),
    queryFn: () => listOnboardingTokens(clusterSlug!),
    enabled: !!clusterSlug,
  })
}

export function useCreateOnboardingToken(clusterSlug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateOnboardingTokenRequest) =>
      createOnboardingToken(clusterSlug, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.tokens(clusterSlug) })
    },
  })
}

export function useDeleteOnboardingToken(clusterSlug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteOnboardingToken(clusterSlug, id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.tokens(clusterSlug) })
    },
  })
}

export function useRevokeNode() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      nodeId,
      reason,
    }: {
      nodeId: string
      reason: NodeRevocationReason
    }) => revokeNode(nodeId, reason),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.all })
      queryClient.invalidateQueries({ queryKey: ['nodes'] })
    },
  })
}
