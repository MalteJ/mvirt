import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { listProjects, deleteProject } from '@/api/endpoints'

export const projectKeys = {
  all: ['projects'] as const,
  lists: () => [...projectKeys.all, 'list'] as const,
  list: () => [...projectKeys.lists()] as const,
}

/** List every Project the caller may see (filtered by membership server-side). */
export function useProjects() {
  return useQuery({
    queryKey: projectKeys.list(),
    queryFn: listProjects,
  })
}

export function useDeleteProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteProject(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: projectKeys.all })
    },
  })
}

// Project creation is org-scoped: import `useCreateProjectInOrg` from `useOrgs`
// and pass the parent Org's slug.
