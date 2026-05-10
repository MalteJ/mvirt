import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  listOrgs,
  getOrg,
  createOrg,
  updateOrg,
  deleteOrg,
  listProjectsInOrg,
  createProjectInOrg,
} from '@/api/endpoints'
import type { CreateOrgRequest, CreateProjectRequest, UpdateOrgRequest } from '@/types'

export const orgKeys = {
  all: ['orgs'] as const,
  lists: () => [...orgKeys.all, 'list'] as const,
  detail: (slug: string) => [...orgKeys.all, 'detail', slug] as const,
  projects: (slug: string) => [...orgKeys.all, 'projects', slug] as const,
}

export function useOrgs() {
  return useQuery({
    queryKey: orgKeys.lists(),
    queryFn: listOrgs,
  })
}

export function useOrg(slug: string | undefined) {
  return useQuery({
    queryKey: orgKeys.detail(slug ?? ''),
    queryFn: () => getOrg(slug!),
    enabled: !!slug,
  })
}

export function useProjectsInOrg(slug: string | undefined) {
  return useQuery({
    queryKey: orgKeys.projects(slug ?? ''),
    queryFn: () => listProjectsInOrg(slug!),
    enabled: !!slug,
  })
}

export function useCreateOrg() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateOrgRequest) => createOrg(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: orgKeys.all })
    },
  })
}

export function useUpdateOrg(slug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: UpdateOrgRequest) => updateOrg(slug, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: orgKeys.all })
    },
  })
}

export function useDeleteOrg() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (slug: string) => deleteOrg(slug),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: orgKeys.all })
    },
  })
}

/**
 * Create a Project under the named Org. Org slug comes from the URL of the
 * caller (e.g. an OrgsPage or the active Org from the switcher).
 */
export function useCreateProjectInOrg(orgSlug: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateProjectRequest) => createProjectInOrg(orgSlug, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: orgKeys.projects(orgSlug) })
      queryClient.invalidateQueries({ queryKey: ['projects'] })
    },
  })
}
