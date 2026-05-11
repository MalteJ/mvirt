import { NavLink, Outlet, useParams, Navigate } from 'react-router-dom'
import { FolderKanban, Server, Settings } from 'lucide-react'
import { useOrg } from '@/hooks/queries'
import { cn } from '@/lib/utils'

/// Wraps the org-scoped admin routes (Projects / Clusters / Settings / ...)
/// with a shared header (org name + slug) and a tab navbar. Each tab is its
/// own URL so deep links keep working.
export function OrgLayout() {
  const { orgSlug } = useParams<{ orgSlug: string }>()
  const { data: org, isLoading } = useOrg(orgSlug)

  if (!orgSlug) return <Navigate to="/orgs" replace />

  const tabs = [
    { to: 'projects', label: 'Projects', icon: FolderKanban },
    { to: 'clusters', label: 'Clusters', icon: Server },
    { to: 'settings', label: 'Settings', icon: Settings },
  ]

  return (
    <div className="space-y-6">
      <div>
        <div className="text-xs uppercase tracking-wider text-muted-foreground">
          Organization
        </div>
        <h1 className="text-2xl font-bold tracking-tight">
          {isLoading ? '...' : (org?.name ?? orgSlug)}
          <span className="ml-2 font-mono text-base font-normal text-muted-foreground">
            {orgSlug}
          </span>
        </h1>
      </div>

      <nav className="flex gap-1 border-b border-border">
        {tabs.map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            className={({ isActive }) =>
              cn(
                'inline-flex items-center gap-2 border-b-2 px-3 py-2 text-sm font-medium transition-colors',
                isActive
                  ? 'border-purple text-foreground'
                  : 'border-transparent text-muted-foreground hover:text-foreground',
              )
            }
          >
            <t.icon className="h-4 w-4" />
            {t.label}
          </NavLink>
        ))}
      </nav>

      <Outlet />
    </div>
  )
}
