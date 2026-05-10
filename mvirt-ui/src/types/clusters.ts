/** Cluster — a named, explicitly-listed group of Nodes within an Org.
 *  Slug is the primary key (platform-unique, immutable). See ADR-0005. */
export interface Cluster {
  slug: string
  orgSlug: string
  name: string
  description?: string
  location?: string
  nodeIds: string[]
  createdAt: string
  updatedAt: string
}

export interface ClusterListResponse {
  clusters: Cluster[]
}

export interface CreateClusterRequest {
  slug: string
  name: string
  description?: string
  location?: string
}

export interface UpdateClusterRequest {
  name?: string
  /** undefined = leave alone; null = clear; string = set. */
  description?: string | null
  location?: string | null
}

// =============================================================================
// Onboarding tokens (ADR-0006)
// =============================================================================

export interface OnboardingToken {
  id: string
  clusterSlug: string
  description?: string
  expiresAt: string
  usedAt?: string
  usedByNodeId?: string
  createdByAccount: string
  createdAt: string
}

export interface OnboardingTokenListResponse {
  tokens: OnboardingToken[]
}

export interface CreateOnboardingTokenRequest {
  ttlSeconds?: number
  description?: string
}

/** Returned once at create time — `token` is the only field that's not
 *  also available via the list endpoint. Copy it into node config; the
 *  cplane keeps only its hash. */
export interface CreateOnboardingTokenResponse {
  id: string
  token: string
  clusterSlug: string
  expiresAt: string
  description?: string
}
