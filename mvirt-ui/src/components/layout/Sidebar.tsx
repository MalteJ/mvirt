import { NavLink, Link, useLocation } from 'react-router-dom'
import {
  Bot,
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
  Users,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { useApiHealth } from '@/hooks/queries'
import { useIsPlatformAdmin } from '@/hooks/useAuth'
import { useSidebar } from '@/hooks/useSidebar'

const projectNav = [
  { name: 'Virtual Machines', path: '/vms', icon: Server },
  { name: 'Storage', path: '/storage', icon: HardDrive },
  { name: 'Network', path: '/network', icon: Network },
  { name: 'Firewall', path: '/firewall', icon: Flame },
  { name: 'Members', path: '/members', icon: Users },
  { name: 'Service Accounts', path: '/service-accounts', icon: Bot },
  { name: 'Logs', path: '/logs', icon: ScrollText },
]

const orgNav = [
  { name: 'Dashboard', path: 'dashboard', icon: LayoutDashboard, end: true },
  { name: 'Projects', path: 'projects', icon: FolderKanban, end: false },
  { name: 'Clusters', path: 'clusters', icon: Server, end: false },
  { name: 'Members', path: 'members', icon: Users, end: false },
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

interface SidebarProps {
  /**
   * When true, the sidebar is rendered inside a Sheet (mobile drawer). The
   * outer wrapper drops its fixed width and right border because the Sheet
   * owns those; click handlers also close the drawer on navigation.
   */
  variant?: 'inline' | 'sheet'
}

/**
 * The persistent nav rail. Defaults to the inline (desktop) variant; the
 * mobile drawer in Layout renders it with `variant="sheet"`.
 */
export function Sidebar({ variant = 'inline' }: SidebarProps) {
  const { pathname } = useLocation()
  const projectSlug = pathname.match(/^\/projects\/([^/]+)/)?.[1]
  const orgSlug = pathname.match(/^\/orgs\/([^/]+)/)?.[1]
  const apiHealth = useApiHealth()
  const isAdmin = useIsPlatformAdmin()
  const closeDrawer = useSidebar((s) => s.setOpen)

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

  const handleNavigate = () => {
    if (variant === 'sheet') closeDrawer(false)
  }

  const isSheet = variant === 'sheet'

  return (
    <div
      className={cn(
        'relative z-10 flex h-full flex-col bg-card/80 backdrop-blur-xl',
        !isSheet && 'w-64 border-r border-border',
      )}
    >
      <Link
        to="/"
        onClick={handleNavigate}
        className="group flex h-14 items-center border-b border-border px-4 hover:bg-secondary/50 transition-colors"
      >
        <div className="logo-box-shimmer mr-3 flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-br from-purple to-blue text-white text-lg font-bold shadow-glow-purple">
          m
        </div>
        <span className="logo-shimmer text-lg font-semibold">mvirt</span>
      </Link>
      <nav className="flex-1 space-y-1 overflow-y-auto p-2">
        {projectSlug &&
          projectNav.map((item) => (
            <NavLink
              key={item.name}
              to={`/projects/${projectSlug}${item.path}`}
              onClick={handleNavigate}
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
              onClick={handleNavigate}
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
            onClick={handleNavigate}
            className={({ isActive }) => linkClass(isActive)}
          >
            <Cog className="mr-3 h-4 w-4" />
            mvirt Admin
          </NavLink>
        </div>
      )}
      <div className="border-t border-border p-4 pb-[max(1rem,env(safe-area-inset-bottom))]">
        <div className="flex items-center gap-2 text-xs text-foreground/60">
          <div className={cn('h-2 w-2 rounded-full', dotClass)} />
          <span>{statusLabel}</span>
        </div>
      </div>
    </div>
  )
}
