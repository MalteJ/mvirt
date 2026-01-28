export enum RuleDirection {
  INGRESS = 'INGRESS',
  EGRESS = 'EGRESS',
}

export enum RuleProtocol {
  ALL = 'ALL',
  TCP = 'TCP',
  UDP = 'UDP',
  ICMP = 'ICMP',
  ICMPV6 = 'ICMPV6',
}

export interface SecurityGroupRule {
  id: string
  securityGroupId: string
  direction: RuleDirection
  protocol: RuleProtocol
  portStart?: number
  portEnd?: number
  cidr?: string
  description?: string
  createdAt: string
}

export interface SecurityGroup {
  id: string
  name: string
  description?: string
  rules: SecurityGroupRule[]
  nicCount: number
  createdAt: string
  updatedAt: string
}

export interface CreateSecurityGroupRequest {
  name: string
  description?: string
}

export interface CreateSecurityGroupRuleRequest {
  securityGroupId: string
  direction: RuleDirection
  protocol: RuleProtocol
  portStart?: number
  portEnd?: number
  cidr?: string
  description?: string
}
