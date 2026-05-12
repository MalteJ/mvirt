import { get, post, del } from '../client'
import type {
  ApiKey,
  ApiKeyListResponse,
  CreateApiKeyRequest,
  CreateServiceAccountRequest,
  ServiceAccount,
  ServiceAccountListResponse,
} from '@/types'

export async function listServiceAccounts(
  projectSlug: string,
): Promise<ServiceAccount[]> {
  const r = await get<ServiceAccountListResponse>(
    `/projects/${projectSlug}/service-accounts`,
  )
  return r.serviceAccounts
}

export async function createServiceAccount(
  projectSlug: string,
  req: CreateServiceAccountRequest,
): Promise<ServiceAccount> {
  return post<ServiceAccount>(
    `/projects/${projectSlug}/service-accounts`,
    req,
  )
}

export async function deleteServiceAccount(
  projectSlug: string,
  id: string,
): Promise<void> {
  await del<void>(`/projects/${projectSlug}/service-accounts/${id}`)
}

export async function listApiKeys(
  projectSlug: string,
  saId: string,
): Promise<ApiKey[]> {
  const r = await get<ApiKeyListResponse>(
    `/projects/${projectSlug}/service-accounts/${saId}/api-keys`,
  )
  return r.apiKeys
}

export async function createApiKey(
  projectSlug: string,
  saId: string,
  req: CreateApiKeyRequest,
): Promise<ApiKey> {
  return post<ApiKey>(
    `/projects/${projectSlug}/service-accounts/${saId}/api-keys`,
    req,
  )
}

export async function revokeApiKey(
  projectSlug: string,
  saId: string,
  keyId: string,
): Promise<void> {
  await del<void>(
    `/projects/${projectSlug}/service-accounts/${saId}/api-keys/${keyId}`,
  )
}
