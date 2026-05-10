import { get, post, patch, del } from '../client'
import type {
  Org,
  OrgListResponse,
  CreateOrgRequest,
  UpdateOrgRequest,
  Project,
  ProjectListResponse,
  CreateProjectRequest,
} from '@/types'

export async function listOrgs(): Promise<Org[]> {
  const response = await get<OrgListResponse>('/orgs')
  return response.orgs
}

export async function getOrg(slug: string): Promise<Org> {
  return get<Org>(`/orgs/${slug}`)
}

export async function createOrg(request: CreateOrgRequest): Promise<Org> {
  return post<Org>('/orgs', request)
}

export async function updateOrg(slug: string, request: UpdateOrgRequest): Promise<Org> {
  return patch<Org>(`/orgs/${slug}`, request)
}

export async function deleteOrg(slug: string): Promise<void> {
  await del<void>(`/orgs/${slug}`)
}

export async function listProjectsInOrg(orgSlug: string): Promise<Project[]> {
  const response = await get<ProjectListResponse>(`/orgs/${orgSlug}/projects`)
  return response.projects
}

export async function createProjectInOrg(
  orgSlug: string,
  request: CreateProjectRequest,
): Promise<Project> {
  return post<Project>(`/orgs/${orgSlug}/projects`, request)
}
