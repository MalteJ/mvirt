import { NavLink, Link, useLocation } from 'react-router-dom'
import {
  Cog,
  CreditCard,
  FolderKanban,
  HardDrive,
  Flame,
  LayoutDashboard,
  Network,
  ScrollText,
  Server,
  Settings,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { useApiHealth } from '@/hooks/queries'
import { useIsPlatformAdmin } from '@/hooks/useAuth'

const projectNav = [
  { name: 'Virtual Machines', path: '/vms', icon: Server },
  { name: 'Storage', path: '/storage', icon: HardDrive },
  { name: 'Network', path: '/network', icon: Network },
  { name: 'Firewall', path: '/firewall', icon: Flame },
  { name: 'Logs', path: '/logs', icon: ScrollText },
]

const orgNav = [
  { name: 'Dashboard', path: 'dashboard', icon: LayoutDashboard, end: true },
  { name: 'Projects', path: 'projects', icon: FolderKanban, end: false },
  { name: 'Clusters', path: 'clusters', icon: Server, end: false },
  { name: 'Billing', path: 'billing', icon: CreditCard, end: false },
  { name: 'Settings', path: 'settings', icon: Settings, end: false },
]

const linkClass = (isActive: boolean) =>
  cn(
    'flex items-center rounded-md px-3 py-2 text-sm font-medium transition-all duration-200 border',
    isActive
      ? 'bg-purple/20 text-purple-light border-purple/30'
      : 'text-foreground/80 border-transparent hover:bg-secondary hover:text-foreground',
  )

export function Sidebar() {
  // Sidebar lives outside the inner <Routes> tree that defines :projectSlug
  // / :orgSlug, so useParams() returns nothing here — pull the active scope
  // out of `pathname` directly.
  const { pathname } = useLocation()
  const projectSlug = pathname.match(/^\/projects\/([^/]+)/)?.[1]
  const orgSlug = pathname.match(/^\/orgs\/([^/]+)/)?.[1]
  const apiHealth = useApiHealth()
  const isAdmin = useIsPlatformAdmin()

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
      <Link
        to="/"
        className="group flex h-14 items-center border-b border-border px-4 hover:bg-secondary/50 transition-colors"
      >
        <div className="logo-box-shimmer mr-3 flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-br from-purple to-blue text-white text-lg font-bold shadow-glow-purple">
          m
        </div>
        <span className="logo-shimmer text-lg font-semibold">mvirt</span>
      </Link>
      <nav className="flex-1 space-y-1 p-2">
        {projectSlug &&
          projectNav.map((item) => (
            <NavLink
              key={item.name}
              to={`/projects/${projectSlug}${item.path}`}
              className={({ isActive }) => linkClass(isActive)}
            >
              <item.icon className="mr-3 h-4 w-4" />
              {item.name}
            </NavLink>
          ))}
        {!projectSlug &&
          orgSlug &&
          orgNav.map((item) => (
            <NavLink
              key={item.name}
              to={`/orgs/${orgSlug}/${item.path}`}
              end={item.end}
              className={({ isActive }) => linkClass(isActive)}
            >
              <item.icon className="mr-3 h-4 w-4" />
              {item.name}
            </NavLink>
          ))}
      </nav>
      {isAdmin && (
        <div className="border-t border-border p-2">
          <NavLink
            to="/cluster"
            end
            className={({ isActive }) => linkClass(isActive)}
          >
            <Cog className="mr-3 h-4 w-4" />
            mvirt Admin
          </NavLink>
        </div>
      )}
      <div className="border-t border-border p-4">
        <div className="flex items-center gap-2 text-xs text-foreground/60">
          <div className={cn('h-2 w-2 rounded-full', dotClass)} />
          <span>{statusLabel}</span>
        </div>
      </div>
    </div>
  )
}
