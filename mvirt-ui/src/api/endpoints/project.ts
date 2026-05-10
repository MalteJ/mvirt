import { get, del } from '../client'
import type { Project, ProjectListResponse } from '@/types'

export async function listProjects(): Promise<Project[]> {
  const response = await get<ProjectListResponse>('/projects')
  return response.projects
}

export async function getProject(id: string): Promise<Project> {
  return get<Project>(`/projects/${id}`)
}

export async function deleteProject(id: string): Promise<void> {
  await del<void>(`/projects/${id}`)
}

// Project creation is org-scoped — see api/endpoints/org.ts → createProjectInOrg.
