import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  getMe,
  grantOrgMember,
  grantProjectMember,
  inviteAccountByEmail,
  listAccounts,
  listOrgMembers,
  listProjectMembers,
  revokeOrgMember,
  revokeProjectMember,
} from '@/api/endpoints'

export const meKeys = {
  all: ['me'] as const,
  accounts: ['accounts'] as const,
  orgMembers: (slug: string) => ['orgMembers', slug] as const,
  projectMembers: (slug: string) => ['projectMembers', slug] as const,
}

/** Current user — Account + memberships. Returns `undefined` until the
 *  request resolves; the underlying query swallows 401 (server returns
 *  it when auth is disabled in dev) and yields `null` so callers can
 *  fall through to a dev-mode override. */
export function useMe() {
  return useQuery({
    queryKey: meKeys.all,
    queryFn: async () => {
      try {
        return await getMe()
      } catch (err) {
        // 401 is expected when auth is off (dev mode without a token).
        // Surface as null instead of an error so the localStorage stub
        // can take over for the platform-admin gate.
        const status = (err as { code?: number }).code
        if (status === 401) return null
        throw err
      }
    },
    staleTime: 60_000,
  })
}

export function useAccounts() {
  return useQuery({
    queryKey: meKeys.accounts,
    queryFn: listAccounts,
  })
}

export function useInviteAccount() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      email,
      displayName,
    }: {
      email: string
      displayName?: string
    }) => inviteAccountByEmail(email, displayName),
    onSuccess: () => qc.invalidateQueries({ queryKey: meKeys.accounts }),
  })
}

export function useOrgMembers(orgSlug: string | undefined) {
  return useQuery({
    queryKey: meKeys.orgMembers(orgSlug ?? ''),
    queryFn: () => listOrgMembers(orgSlug!),
    enabled: !!orgSlug,
  })
}

export function useGrantOrgMember(orgSlug: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ accountId, role }: { accountId: string; role: string }) =>
      grantOrgMember(orgSlug, accountId, role),
    onSuccess: () => qc.invalidateQueries({ queryKey: meKeys.orgMembers(orgSlug) }),
  })
}

export function useRevokeOrgMember(orgSlug: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (membershipId: string) => revokeOrgMember(orgSlug, membershipId),
    onSuccess: () => qc.invalidateQueries({ queryKey: meKeys.orgMembers(orgSlug) }),
  })
}

export function useProjectMembers(projectSlug: string | undefined) {
  return useQuery({
    queryKey: meKeys.projectMembers(projectSlug ?? ''),
    queryFn: () => listProjectMembers(projectSlug!),
    enabled: !!projectSlug,
  })
}

export function useGrantProjectMember(projectSlug: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ accountId, role }: { accountId: string; role: string }) =>
      grantProjectMember(projectSlug, accountId, role),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: meKeys.projectMembers(projectSlug) }),
  })
}

export function useRevokeProjectMember(projectSlug: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (membershipId: string) =>
      revokeProjectMember(projectSlug, membershipId),
    onSuccess: () =>
      qc.invalidateQueries({ queryKey: meKeys.projectMembers(projectSlug) }),
  })
}
