import { useEffect } from 'react'
import { useNavigate, Link } from 'react-router-dom'
import { Bell, Moon, Sun, LogOut, User, Check, AlertTriangle, Info, AlertCircle, CheckCircle, ChevronDown, FolderKanban, Settings } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useTheme } from '@/hooks/useTheme'
import { useAuth } from '@/hooks/useAuth'
import { useProject } from '@/hooks/useProject'
import { useNotifications, useMarkAllNotificationsRead, useProjects } from '@/hooks/queries'
import { NotificationType } from '@/types'
import { cn } from '@/lib/utils'

const notificationIcons: Record<NotificationType, typeof Info> = {
  [NotificationType.INFO]: Info,
  [NotificationType.WARNING]: AlertTriangle,
  [NotificationType.ERROR]: AlertCircle,
  [NotificationType.SUCCESS]: CheckCircle,
}

const notificationColors: Record<NotificationType, string> = {
  [NotificationType.INFO]: 'text-blue-400',
  [NotificationType.WARNING]: 'text-yellow-400',
  [NotificationType.ERROR]: 'text-red-400',
  [NotificationType.SUCCESS]: 'text-green-400',
}

export function Header() {
  const navigate = useNavigate()
  const { theme, toggleTheme } = useTheme()
  const { user, logout } = useAuth()
  const { currentProject, setCurrentProject } = useProject()
  const { data: projects } = useProjects()
  const { data: notifications } = useNotifications()
  const markAllRead = useMarkAllNotificationsRead()

  // Auto-select first project if none selected
  useEffect(() => {
    if (!currentProject && projects && projects.length > 0) {
      setCurrentProject(projects[0])
    }
  }, [currentProject, projects, setCurrentProject])

  const unreadCount = notifications?.filter((n) => !n.read).length ?? 0

  const handleLogout = () => {
    logout()
    navigate('/login')
  }

  const formatTime = (dateStr: string) => {
    const date = new Date(dateStr)
    const now = new Date()
    const diff = now.getTime() - date.getTime()
    const minutes = Math.floor(diff / 60000)
    const hours = Math.floor(diff / 3600000)
    const days = Math.floor(diff / 86400000)

    if (minutes < 60) return `${minutes}m ago`
    if (hours < 24) return `${hours}h ago`
    return `${days}d ago`
  }

  return (
    <header className="flex h-14 items-center justify-between border-b border-border bg-card/80 backdrop-blur-xl px-6">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" className="gap-2 hover:bg-purple/20 hover:text-purple-light">
            <FolderKanban className="h-4 w-4" />
            <span className="font-medium">{currentProject?.name ?? 'Select Project'}</span>
            <ChevronDown className="h-4 w-4 opacity-50" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
            Projects
          </div>
          <DropdownMenuSeparator />
          {projects?.map((project) => (
            <DropdownMenuItem
              key={project.id}
              onClick={() => setCurrentProject(project)}
              className={cn(
                currentProject?.id === project.id && 'bg-purple/20 text-purple-light'
              )}
            >
              <FolderKanban className="mr-2 h-4 w-4" />
              <div className="flex-1">
                <div>{project.name}</div>
                {project.description && (
                  <div className="text-xs text-muted-foreground">{project.description}</div>
                )}
              </div>
              {currentProject?.id === project.id && (
                <Check className="h-4 w-4 ml-2" />
              )}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem asChild>
            <Link to="/projects" className="cursor-pointer">
              <Settings className="mr-2 h-4 w-4" />
              Manage Projects
            </Link>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <div className="flex items-center gap-2">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" className="h-10 w-10 hover:bg-purple/20 hover:text-purple-light relative">
              <Bell className="h-5 w-5" />
              {unreadCount > 0 && (
                <span className="absolute -top-0.5 -right-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-destructive text-[10px] font-medium text-white">
                  {unreadCount}
                </span>
              )}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-80">
            <div className="flex items-center justify-between px-3 py-2">
              <span className="text-sm font-medium">Notifications</span>
              {unreadCount > 0 && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-auto py-1 px-2 text-xs"
                  onClick={() => markAllRead.mutate()}
                >
                  <Check className="mr-1 h-3 w-3" />
                  Mark all read
                </Button>
              )}
            </div>
            <DropdownMenuSeparator />
            {notifications && notifications.length > 0 ? (
              <div className="max-h-80 overflow-y-auto">
                {notifications.map((notification) => {
                  const Icon = notificationIcons[notification.type]
                  return (
                    <div
                      key={notification.id}
                      className={cn(
                        'px-3 py-2 cursor-pointer hover:bg-secondary/50',
                        !notification.read && 'bg-secondary/30'
                      )}
                    >
                      <div className="flex gap-3">
                        <Icon className={cn('h-5 w-5 shrink-0 mt-0.5', notificationColors[notification.type])} />
                        <div className="flex-1 min-w-0">
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-sm font-medium truncate">{notification.title}</span>
                            <span className="text-xs text-muted-foreground shrink-0">
                              {formatTime(notification.createdAt)}
                            </span>
                          </div>
                          <p className="text-xs text-muted-foreground mt-0.5 line-clamp-2">
                            {notification.message}
                          </p>
                        </div>
                      </div>
                    </div>
                  )
                })}
              </div>
            ) : (
              <div className="px-3 py-6 text-center text-sm text-muted-foreground">
                No notifications
              </div>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          variant="ghost"
          size="icon"
          className="h-10 w-10 hover:bg-purple/20 hover:text-purple-light"
          onClick={toggleTheme}
          title={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
        >
          {theme === 'dark' ? (
            <Sun className="h-5 w-5" />
          ) : (
            <Moon className="h-5 w-5" />
          )}
        </Button>

        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" className="h-10 w-10 hover:bg-purple/20 hover:text-purple-light">
              <User className="h-5 w-5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-56">
            <div className="px-2 py-1.5">
              <p className="text-sm font-medium">{user?.name}</p>
              <p className="text-xs text-muted-foreground">{user?.email}</p>
            </div>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={handleLogout} className="text-destructive">
              <LogOut className="mr-2 h-4 w-4" />
              Sign out
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </header>
  )
}
