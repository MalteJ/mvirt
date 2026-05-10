import { useParams } from 'react-router-dom'

/**
 * Get the active project slug from the URL. cplane keys all project-scoped
 * routes (`/v1/projects/{slug}/...`) by slug now (per ADR-0004) — no
 * UUID-resolution step needed.
 *
 * The hook is named `useProjectId` for legacy-call-site compatibility; the
 * value it returns is the slug. Rename pending across consumer pages.
 */
export function useProjectId(): string {
  const { projectSlug } = useParams<{ projectSlug: string }>()
  if (!projectSlug) {
    throw new Error(
      'useProjectId must be used within a project-scoped route (/projects/:projectSlug/*)',
    )
  }
  return projectSlug
}
