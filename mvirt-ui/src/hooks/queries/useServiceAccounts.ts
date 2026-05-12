import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  createApiKey,
  createServiceAccount,
  deleteServiceAccount,
  listApiKeys,
  listServiceAccounts,
  revokeApiKey,
} from '@/api/endpoints'
import type {
  ApiKey,
  CreateApiKeyRequest,
  CreateServiceAccountRequest,
} from '@/types'

export const serviceAccountKeys = {
  all: ['service-accounts'] as const,
  list: (projectSlug: string) =>
    [...serviceAccountKeys.all, 'list', projectSlug] as const,
  keys: (projectSlug: string, saId: string) =>
    [...serviceAccountKeys.all, 'keys', projectSlug, saId] as const,
}

export function useServiceAccounts(projectSlug: string | undefined) {
  return useQuery({
    queryKey: serviceAccountKeys.list(projectSlug ?? ''),
    queryFn: () => listServiceAccounts(projectSlug!),
    enabled: !!projectSlug,
  })
}

export function useCreateServiceAccount(projectSlug: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (req: CreateServiceAccountRequest) =>
      createServiceAccount(projectSlug, req),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: serviceAccountKeys.list(projectSlug) })
    },
  })
}

export function useDeleteServiceAccount(projectSlug: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteServiceAccount(projectSlug, id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: serviceAccountKeys.list(projectSlug) })
    },
  })
}

export function useApiKeys(
  projectSlug: string | undefined,
  saId: string | undefined,
) {
  return useQuery({
    queryKey: serviceAccountKeys.keys(projectSlug ?? '', saId ?? ''),
    queryFn: () => listApiKeys(projectSlug!, saId!),
    enabled: !!projectSlug && !!saId,
  })
}

/**
 * Mint a new API key. The mutation result includes the plaintext `secret`
 * — show it to the user once, then drop it from app state. Subsequent
 * GETs return the metadata only.
 */
export function useCreateApiKey(projectSlug: string, saId: string) {
  const qc = useQueryClient()
  return useMutation<ApiKey, Error, CreateApiKeyRequest>({
    mutationFn: (req) => createApiKey(projectSlug, saId, req),
    onSuccess: () => {
      qc.invalidateQueries({
        queryKey: serviceAccountKeys.keys(projectSlug, saId),
      })
    },
  })
}

export function useRevokeApiKey(projectSlug: string, saId: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (keyId: string) => revokeApiKey(projectSlug, saId, keyId),
    onSuccess: () => {
      qc.invalidateQueries({
        queryKey: serviceAccountKeys.keys(projectSlug, saId),
      })
    },
  })
}
