/** Project — the slug is the primary key (the "namespace name" per ADR-0004). */
export interface Project {
  slug: string
  orgSlug: string
  name: string
  description?: string
  createdAt: string
  updatedAt: string
}

export interface ProjectListResponse {
  projects: Project[]
}

/** The Org under which the Project is created comes from the URL, not the body. */
export interface CreateProjectRequest {
  slug: string
  name: string
  description?: string
}
