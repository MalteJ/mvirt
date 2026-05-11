import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listClustersInOrg,
  getCluster,
  createClusterInOrg,
  updateCluster,
  deleteCluster,
  addNodeToCluster,
  removeNodeFromCluster,
  listNodesInCluster,
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
  nodes: (slug: string) => [...clusterKeys.all, 'nodes', slug] as const,
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

/** Poll while the cluster has any nodes mid-onboarding so the status badge
 *  flips to Online without a manual refresh. */
export function useClusterNodes(slug: string | undefined) {
  return useQuery({
    queryKey: clusterKeys.nodes(slug ?? ''),
    queryFn: () => listNodesInCluster(slug!),
    enabled: !!slug,
    refetchInterval: (q) => {
      const pending = q.state.data?.some(
        (n) => n.status === 'onboarding' || n.status === 'offline',
      )
      return pending ? 3000 : false
    },
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
      // Token issuance also creates the placeholder Node row + appends to
      // cluster.node_ids, so the detail page's node list + cluster query
      // both need to refresh.
      queryClient.invalidateQueries({ queryKey: clusterKeys.tokens(clusterSlug) })
      queryClient.invalidateQueries({ queryKey: clusterKeys.nodes(clusterSlug) })
      queryClient.invalidateQueries({ queryKey: clusterKeys.detail(clusterSlug) })
    },
  })
}

export function useDeleteOnboardingToken(clusterSlug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteOnboardingToken(clusterSlug, id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: clusterKeys.tokens(clusterSlug) })
      queryClient.invalidateQueries({ queryKey: clusterKeys.nodes(clusterSlug) })
      queryClient.invalidateQueries({ queryKey: clusterKeys.detail(clusterSlug) })
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
