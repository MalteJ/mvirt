import { useParams } from 'react-router-dom'

/**
 * Get the current project ID from the URL.
 * Use this in project-scoped pages to get the projectId.
 */
export function useProjectId(): string {
  const { projectId } = useParams<{ projectId: string }>()
  if (!projectId) {
    throw new Error('useProjectId must be used within a project-scoped route (/p/:projectId/*)')
  }
  return projectId
}
