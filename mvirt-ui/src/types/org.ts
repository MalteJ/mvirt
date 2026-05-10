/** Org contact / billing details. All fields optional — omitted means
 *  the server will return them as `undefined`. */
export interface OrgContact {
  legalName?: string
  streetAddress?: string
  postalCode?: string
  city?: string
  country?: string
  technicalContactEmail?: string
  billingContactEmail?: string
  vatId?: string
}

/** Organization — the slug is the primary key. */
export interface Org {
  slug: string
  name: string
  defaultStaticKeyTtlDays: number
  disallowStaticKeys: boolean
  contact: OrgContact
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
  contact?: OrgContact
}
