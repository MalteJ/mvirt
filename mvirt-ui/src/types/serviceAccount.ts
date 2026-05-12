export interface ServiceAccount {
  id: string
  projectSlug: string
  name: string
  description?: string
  createdAt: string
  updatedAt: string
}

export interface ServiceAccountListResponse {
  serviceAccounts: ServiceAccount[]
}

export interface CreateServiceAccountRequest {
  name: string
  description?: string
}

export interface ApiKey {
  id: string
  accountId: string
  displayPrefix: string
  description?: string
  expiresAt?: string
  lastUsedAt?: string
  revokedAt?: string
  createdAt: string
  /** One-time plaintext returned only on creation. */
  secret?: string
}

export interface ApiKeyListResponse {
  apiKeys: ApiKey[]
}

export interface CreateApiKeyRequest {
  description?: string
  /** RFC3339 timestamp. Omit for "never expires". */
  expiresAt?: string
}
