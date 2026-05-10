/** API client for the Cluster entity (ADR-0005) + onboarding tokens (ADR-0006).
 *  Lives alongside the legacy `cluster.ts` which still handles control-plane /
 *  Node mgmt. */
import { get, post, patch, del } from '../client'
import type {
  Cluster,
  ClusterListResponse,
  CreateClusterRequest,
  UpdateClusterRequest,
  OnboardingToken,
  OnboardingTokenListResponse,
  CreateOnboardingTokenRequest,
  CreateOnboardingTokenResponse,
} from '@/types'

// =============================================================================
// Cluster CRUD
// =============================================================================

export async function listClustersInOrg(orgSlug: string): Promise<Cluster[]> {
  const response = await get<ClusterListResponse>(`/orgs/${orgSlug}/clusters`)
  return response.clusters
}

export async function listClusters(): Promise<Cluster[]> {
  const response = await get<ClusterListResponse>(`/clusters`)
  return response.clusters
}

export async function getCluster(slug: string): Promise<Cluster> {
  return get<Cluster>(`/clusters/${slug}`)
}

export async function createClusterInOrg(
  orgSlug: string,
  request: CreateClusterRequest,
): Promise<Cluster> {
  return post<Cluster>(`/orgs/${orgSlug}/clusters`, request)
}

export async function updateCluster(
  slug: string,
  request: UpdateClusterRequest,
): Promise<Cluster> {
  return patch<Cluster>(`/clusters/${slug}`, request)
}

export async function deleteCluster(slug: string): Promise<void> {
  await del<void>(`/clusters/${slug}`)
}

export async function addNodeToCluster(
  slug: string,
  nodeId: string,
): Promise<Cluster> {
  return post<Cluster>(`/clusters/${slug}/nodes/${nodeId}`, {})
}

export async function removeNodeFromCluster(
  slug: string,
  nodeId: string,
): Promise<Cluster> {
  return del<Cluster>(`/clusters/${slug}/nodes/${nodeId}`)
}

// =============================================================================
// Onboarding tokens
// =============================================================================

export async function listOnboardingTokens(
  clusterSlug: string,
): Promise<OnboardingToken[]> {
  const response = await get<OnboardingTokenListResponse>(
    `/clusters/${clusterSlug}/onboarding-tokens`,
  )
  return response.tokens
}

export async function createOnboardingToken(
  clusterSlug: string,
  request: CreateOnboardingTokenRequest,
): Promise<CreateOnboardingTokenResponse> {
  return post<CreateOnboardingTokenResponse>(
    `/clusters/${clusterSlug}/onboarding-tokens`,
    request,
  )
}

export async function deleteOnboardingToken(
  clusterSlug: string,
  id: string,
): Promise<void> {
  await del<void>(`/clusters/${clusterSlug}/onboarding-tokens/${id}`)
}

// =============================================================================
// Node revoke
// =============================================================================

export type NodeRevocationReason = 'compromise' | 'decommission' | 'other'

export async function revokeNode(
  nodeId: string,
  reason: NodeRevocationReason,
): Promise<void> {
  await post<void>(`/nodes/${nodeId}/revoke`, { reason })
}
