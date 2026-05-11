import { Outlet, useParams, Navigate } from 'react-router-dom'

/// Minimal wrapper around the org-scoped admin routes. Just provides the
/// `<Outlet />` slot the nested routes render into; the actual nav lives in
/// the global Sidebar (which reads /orgs/:slug out of the URL and shows
/// the org-context items). Falls back to /orgs if the slug is missing.
export function OrgLayout() {
  const { orgSlug } = useParams<{ orgSlug: string }>()
  if (!orgSlug) return <Navigate to="/orgs" replace />
  return <Outlet />
}
