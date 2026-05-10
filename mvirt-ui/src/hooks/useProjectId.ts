import { useParams } from 'react-router-dom'
import { useProjects } from './queries'

/**
 * Resolve the URL slug to the project's UUID. The cplane API still keys
 * project-scoped routes (`/v1/projects/{project_id}/...`) by UUID, so pages
 * that build those URLs need the UUID — not the slug. Returns an empty
 * string while the projects list is still loading; consumers should pass
 * the result to react-query hooks, which gate themselves on a non-empty
 * key anyway.
 */
export function useProjectId(): string {
  const { projectSlug } = useParams<{ projectSlug: string }>()
  const { data: projects } = useProjects()
  if (!projectSlug) {
    throw new Error(
      'useProjectId must be used within a project-scoped route (/projects/:projectSlug/*)',
    )
  }
  const project = projects?.find((p) => p.slug === projectSlug)
  return project?.id ?? ''
}
