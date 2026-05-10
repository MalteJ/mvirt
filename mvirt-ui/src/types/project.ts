export interface Project {
  id: string
  orgId: string
  slug: string
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
