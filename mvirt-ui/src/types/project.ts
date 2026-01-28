export interface Project {
  id: string
  name: string
  description?: string
  createdAt: string
}

export interface ProjectListResponse {
  projects: Project[]
}

export interface CreateProjectRequest {
  id: string
  name: string
  description?: string
}
