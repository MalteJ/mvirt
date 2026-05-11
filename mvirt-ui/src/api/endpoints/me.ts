import { get, post, del } from '../client'
import type { Account, Me, Membership } from '@/types'

export async function getMe(): Promise<Me> {
  return get<Me>('/me')
}

export async function listAccounts(): Promise<Account[]> {
  return get<Account[]>('/accounts')
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
