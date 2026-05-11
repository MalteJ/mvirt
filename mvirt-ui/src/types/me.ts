/** Account + Membership types — ADR-0004. */

export interface Account {
  id: string
  /** `user` | `service_account`. */
  kind: string
  email?: string
  displayName?: string
  createdAt: string
  updatedAt: string
}

export interface Membership {
  id: string
  accountId: string
  /** `platform` | `org` | `project`. */
  scope: string
  /** Set only for `org` / `project` scopes. */
  scopeSlug?: string
  /** `platform-admin` | `org-admin` | `project-admin`. */
  role: string
  createdByAccount: string
  createdAt: string
}

export interface Me {
  account: Account
  memberships: Membership[]
  isPlatformAdmin: boolean
}
