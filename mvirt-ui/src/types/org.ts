export interface Org {
  id: string
  slug: string
  name: string
  defaultStaticKeyTtlDays: number
  disallowStaticKeys: boolean
  createdAt: string
  updatedAt: string
}

export interface OrgListResponse {
  orgs: Org[]
}

export interface CreateOrgRequest {
  slug: string
  name: string
  defaultStaticKeyTtlDays?: number
  disallowStaticKeys?: boolean
}

export interface UpdateOrgRequest {
  name?: string
  defaultStaticKeyTtlDays?: number
  disallowStaticKeys?: boolean
}
