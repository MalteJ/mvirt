import { get, post, del } from '../client'
import type { Account, Me, Membership } from '@/types'

export async function getMe(): Promise<Me> {
  return get<Me>('/me')
}

/// One-shot called after the OIDC redirect lands. cplane fetches the IdP's
/// UserInfo endpoint with the bearer to backfill display_name / email on
/// the Account row — done here so the hot path (`/v1/me` and other
/// authenticated calls) doesn't touch the IdP on every request.
export async function postSignin(): Promise<Me> {
  return post<Me>('/auth/signin', {})
}

export async function listAccounts(): Promise<Account[]> {
  return get<Account[]>('/accounts')
}

export async function inviteAccountByEmail(
  email: string,
  displayName?: string,
): Promise<Account> {
  return post<Account>('/accounts', { email, displayName })
}

export interface OrgMemberListResponse {
  memberships: Membership[]
}

export async function listOrgMembers(orgSlug: string): Promise<Membership[]> {
  const r = await get<OrgMemberListResponse>(`/orgs/${orgSlug}/members`)
  return r.memberships
}

export async function grantOrgMember(
  orgSlug: string,
  accountId: string,
  role: string,
): Promise<Membership> {
  return post<Membership>(`/orgs/${orgSlug}/members`, { accountId, role })
}

export async function revokeOrgMember(
  orgSlug: string,
  membershipId: string,
): Promise<void> {
  await del<void>(`/orgs/${orgSlug}/members/${membershipId}`)
}

export async function listProjectMembers(
  projectSlug: string,
): Promise<Membership[]> {
  const r = await get<OrgMemberListResponse>(
    `/projects/${projectSlug}/members`,
  )
  return r.memberships
}

export async function grantProjectMember(
  projectSlug: string,
  accountId: string,
  role: string,
): Promise<Membership> {
  return post<Membership>(`/projects/${projectSlug}/members`, {
    accountId,
    role,
  })
}

export async function revokeProjectMember(
  projectSlug: string,
  membershipId: string,
): Promise<void> {
  await del<void>(`/projects/${projectSlug}/members/${membershipId}`)
}
