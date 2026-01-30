import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listSecurityGroups,
  getSecurityGroup,
  createSecurityGroup,
  deleteSecurityGroup,
  createSecurityGroupRule,
  deleteSecurityGroupRule,
} from '@/api/endpoints'
import type { CreateSecurityGroupRequest, CreateSecurityGroupRuleRequest } from '@/types'
import { RuleDirection, RuleProtocol } from '@/types'

export const firewallKeys = {
  all: ['firewall'] as const,
  securityGroups: () => [...firewallKeys.all, 'security-groups'] as const,
  securityGroupList: (projectId: string) => [...firewallKeys.securityGroups(), 'list', projectId] as const,
  securityGroup: (id: string) => [...firewallKeys.securityGroups(), id] as const,
}

export function useSecurityGroups(projectId: string) {
  return useQuery({
    queryKey: firewallKeys.securityGroupList(projectId),
    queryFn: () => listSecurityGroups(projectId),
  })
}

export function useSecurityGroup(id: string) {
  return useQuery({
    queryKey: firewallKeys.securityGroup(id),
    queryFn: () => getSecurityGroup(id),
    enabled: !!id,
  })
}

export function useCreateSecurityGroup(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (request: CreateSecurityGroupRequest) => {
      const sg = await createSecurityGroup(projectId, request)
      // Auto-create egress allow-all rules (IPv4 + IPv6)
      await Promise.all([
        createSecurityGroupRule({
          securityGroupId: sg.id,
          direction: RuleDirection.EGRESS,
          protocol: RuleProtocol.ALL,
          cidr: '0.0.0.0/0',
        }),
        createSecurityGroupRule({
          securityGroupId: sg.id,
          direction: RuleDirection.EGRESS,
          protocol: RuleProtocol.ALL,
          cidr: '::/0',
        }),
      ])
      return sg
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroups() })
    },
  })
}

export function useDeleteSecurityGroup() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteSecurityGroup(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroups() })
    },
  })
}

export function useCreateSecurityGroupRule(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateSecurityGroupRuleRequest) => createSecurityGroupRule(request),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroup(variables.securityGroupId) })
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroupList(projectId) })
    },
  })
}

export function useDeleteSecurityGroupRule(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ securityGroupId, ruleId }: { securityGroupId: string; ruleId: string }) =>
      deleteSecurityGroupRule(securityGroupId, ruleId),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroup(variables.securityGroupId) })
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroupList(projectId) })
    },
  })
}
