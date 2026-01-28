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

export const firewallKeys = {
  all: ['firewall'] as const,
  securityGroups: () => [...firewallKeys.all, 'security-groups'] as const,
  securityGroupList: () => [...firewallKeys.securityGroups(), 'list'] as const,
  securityGroup: (id: string) => [...firewallKeys.securityGroups(), id] as const,
}

export function useSecurityGroups() {
  return useQuery({
    queryKey: firewallKeys.securityGroupList(),
    queryFn: listSecurityGroups,
  })
}

export function useSecurityGroup(id: string) {
  return useQuery({
    queryKey: firewallKeys.securityGroup(id),
    queryFn: () => getSecurityGroup(id),
    enabled: !!id,
  })
}

export function useCreateSecurityGroup() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateSecurityGroupRequest) => createSecurityGroup(request),
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

export function useCreateSecurityGroupRule() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateSecurityGroupRuleRequest) => createSecurityGroupRule(request),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroup(variables.securityGroupId) })
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroupList() })
    },
  })
}

export function useDeleteSecurityGroupRule() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ securityGroupId, ruleId }: { securityGroupId: string; ruleId: string }) =>
      deleteSecurityGroupRule(securityGroupId, ruleId),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroup(variables.securityGroupId) })
      queryClient.invalidateQueries({ queryKey: firewallKeys.securityGroupList() })
    },
  })
}
