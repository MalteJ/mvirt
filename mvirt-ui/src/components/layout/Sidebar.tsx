import { NavLink, Link } from 'react-router-dom'
import {
  Server,
  HardDrive,
  Network,
  ScrollText,
  Boxes,
} from 'lucide-react'
import { cn } from '@/lib/utils'

const navigation = [
  { name: 'Virtual Machines', href: '/vms', icon: Server },
  { name: 'Storage', href: '/storage', icon: HardDrive },
  { name: 'Network', href: '/network', icon: Network },
  { name: 'Logs', href: '/logs', icon: ScrollText },
  { name: 'Cluster', href: '/cluster', icon: Boxes },
]

export function Sidebar() {
  return (
    <div className="relative z-10 flex w-64 flex-col border-r border-border bg-card/80 backdrop-blur-xl">
      <Link to="/dashboard" className="group flex h-14 items-center border-b border-border px-4 hover:bg-secondary/50 transition-colors">
        <div className="logo-box-shimmer mr-3 flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-br from-purple to-blue text-white text-lg font-bold shadow-glow-purple">
          m
        </div>
        <span className="logo-shimmer text-lg font-semibold">mvirt</span>
      </Link>
      <nav className="flex-1 space-y-1 p-2">
        {navigation.map((item) => (
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
      </nav>
      <div className="border-t border-border p-4">
        <div className="flex items-center gap-2 text-xs text-foreground/60">
          <div className="h-2 w-2 rounded-full bg-state-running animate-pulse" />
          <span>Connected to localhost</span>
        </div>
      </div>
    </div>
  )
}
