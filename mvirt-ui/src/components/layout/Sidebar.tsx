import { NavLink, Link, useLocation } from 'react-router-dom'
import {
  Server,
  Container,
  HardDrive,
  Network,
  Flame,
  ScrollText,
  Boxes,
  Building2,
  FolderKanban,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { useApiHealth } from '@/hooks/queries'
import { useOrg } from '@/hooks/useOrg'

const navigation = [
  { name: 'Virtual Machines', path: '/vms', icon: Server },
  { name: 'Containers', path: '/containers', icon: Container },
  { name: 'Storage', path: '/storage', icon: HardDrive },
  { name: 'Network', path: '/network', icon: Network },
  { name: 'Firewall', path: '/firewall', icon: Flame },
  { name: 'Logs', path: '/logs', icon: ScrollText },
]

const adminBaseNav = [
  { name: 'Organizations', href: '/orgs', icon: Building2 },
  { name: 'Cluster', href: '/cluster', icon: Boxes },
]

export function Sidebar() {
  // Project-scoped nav (VMs, Containers, Storage, …) is shown only when the
  // user is actually inside a project route (`/projects/:projectSlug/*`).
  // On Org-scope admin pages (`/projects`, `/orgs`, `/cluster`) those links
  // would either point at a previously-active project that the user has
  // navigated away from, or have no meaningful target at all — hide them.
  //
  // Sidebar is rendered by `Layout`, which sits outside the inner `<Routes>`
  // that defines `:projectSlug`, so `useParams` returns nothing here. Pull
  // the slug out of `pathname` instead.
  const { pathname } = useLocation()
  const projectMatch = pathname.match(/^\/projects\/([^/]+)/)
  const projectSlug = projectMatch?.[1]
  const { currentOrg } = useOrg()
  const apiHealth = useApiHealth()

  // The Projects entry points at the active Org's project list when one is
  // known; otherwise it falls back to the Org list (where the user can pick).
  const adminNavigation = [
    adminBaseNav[0],
    {
      name: 'Projects',
      href: currentOrg ? `/orgs/${currentOrg.slug}/projects` : '/orgs',
      icon: FolderKanban,
    },
    adminBaseNav[1],
  ]

  const apiStatus: 'connected' | 'connecting' | 'disconnected' =
    apiHealth.isSuccess ? 'connected'
    : apiHealth.isError ? 'disconnected'
    : 'connecting'

  const dotClass =
    apiStatus === 'connected' ? 'bg-state-running animate-pulse'
    : apiStatus === 'disconnected' ? 'bg-state-error'
    : 'bg-state-starting animate-pulse'

  const statusLabel =
    apiStatus === 'connected' ? 'Connected'
    : apiStatus === 'disconnected' ? 'Disconnected'
    : 'Connecting…'

  return (
    <div className="relative z-10 flex w-64 flex-col border-r border-border bg-card/80 backdrop-blur-xl">
      <Link to="/dashboard" className="group flex h-14 items-center border-b border-border px-4 hover:bg-secondary/50 transition-colors">
        <div className="logo-box-shimmer mr-3 flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-br from-purple to-blue text-white text-lg font-bold shadow-glow-purple">
          m
        </div>
        <span className="logo-shimmer text-lg font-semibold">mvirt</span>
      </Link>
      <nav className="flex-1 space-y-1 p-2">
        {projectSlug &&
          navigation.map((item) => (
            <NavLink
              key={item.name}
              to={`/projects/${projectSlug}${item.path}`}
              className={({ isActive }) =>
                cn(
                  'flex items-center rounded-md px-3 py-2 text-sm font-medium transition-all duration-200 border',
                  isActive
                    ? 'bg-purple/20 text-purple-light border-purple/30'
                    : 'text-foreground/80 border-transparent hover:bg-secondary hover:text-foreground',
                )
              }
            >
              <item.icon className="mr-3 h-4 w-4" />
              {item.name}
            </NavLink>
          ))}
      </nav>
      <div className="border-t border-border p-2">
        {adminNavigation.map((item) => (
          <NavLink
            key={item.name}
            to={item.href}
            className={({ isActive }) =>
              cn(
                'flex items-center rounded-md px-3 py-2 text-sm font-medium transition-all duration-200 border',
                isActive
                  ? 'bg-purple/20 text-purple-light border-purple/30'
                  : 'text-foreground/80 border-transparent hover:bg-secondary hover:text-foreground'
              )
            }
          >
            <item.icon className="mr-3 h-4 w-4" />
            {item.name}
          </NavLink>
        ))}
      </div>
      <div className="border-t border-border p-4">
        <div className="flex items-center gap-2 text-xs text-foreground/60">
          <div className={cn('h-2 w-2 rounded-full', dotClass)} />
          <span>{statusLabel}</span>
        </div>
      </div>
    </div>
  )
}
