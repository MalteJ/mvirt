import { get, post, del } from '../client'
import type {
  SecurityGroup,
  CreateSecurityGroupRequest,
  SecurityGroupRule,
  CreateSecurityGroupRuleRequest,
} from '@/types'

export async function listSecurityGroups(projectId: string): Promise<SecurityGroup[]> {
  const response = await get<{ securityGroups: SecurityGroup[] }>(`/projects/${projectId}/security-groups`)
  return response.securityGroups
}

export async function getSecurityGroup(id: string): Promise<SecurityGroup> {
  return get<SecurityGroup>(`/security-groups/${id}`)
}

export async function createSecurityGroup(projectId: string, request: CreateSecurityGroupRequest): Promise<SecurityGroup> {
  return post<SecurityGroup>(`/projects/${projectId}/security-groups`, request)
}

export async function deleteSecurityGroup(id: string): Promise<void> {
  await del<void>(`/security-groups/${id}`)
}

export async function createSecurityGroupRule(request: CreateSecurityGroupRuleRequest): Promise<SecurityGroupRule> {
  return post<SecurityGroupRule>(`/security-groups/${request.securityGroupId}/rules`, request)
}

export async function deleteSecurityGroupRule(securityGroupId: string, ruleId: string): Promise<void> {
  await del<void>(`/security-groups/${securityGroupId}/rules/${ruleId}`)
}
