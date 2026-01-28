import { get, post, del } from '../client'
import type { Project, ProjectListResponse, CreateProjectRequest } from '@/types'

export async function listProjects(): Promise<Project[]> {
  const response = await get<ProjectListResponse>('/projects')
  return response.projects
}

export async function getProject(id: string): Promise<Project> {
  return get<Project>(`/projects/${id}`)
}

export async function createProject(request: CreateProjectRequest): Promise<Project> {
  return post<Project>('/projects', request)
}

export async function deleteProject(id: string): Promise<void> {
  await del<void>(`/projects/${id}`)
}
