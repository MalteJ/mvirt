import { ReactNode } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { FolderKanban, Sparkles } from 'lucide-react'
import { Sidebar } from './Sidebar'
import { Header } from './Header'
import { Button } from '@/components/ui/button'
import { useProjects } from '@/hooks/queries'

interface LayoutProps {
  children: ReactNode
}

function EmptyProjectsState() {
  const navigate = useNavigate()

  return (
    <div className="flex flex-1 items-center justify-center">
      <div className="text-center max-w-md mx-auto px-6">
        <div className="relative mb-8">
          <div className="absolute inset-0 flex items-center justify-center">
            <div className="h-32 w-32 rounded-full bg-purple/20 blur-2xl" />
          </div>
          <div className="relative flex items-center justify-center">
            <div className="flex h-24 w-24 items-center justify-center rounded-2xl bg-gradient-to-br from-purple to-blue shadow-glow-purple">
              <FolderKanban className="h-12 w-12 text-white" />
            </div>
          </div>
        </div>
        <h2 className="text-2xl font-bold tracking-tight mb-2">
          It's quiet... too quiet
        </h2>
        <p className="text-muted-foreground mb-8">
          No VMs humming, no containers running, no packets flowing.
          Time to bring this place to life.
        </p>
        <Button
          size="lg"
          onClick={() => navigate('/projects')}
          className="gap-2"
        >
          <Sparkles className="h-4 w-4" />
          Let's Build Something
        </Button>
      </div>
    </div>
  )
}

export function Layout({ children }: LayoutProps) {
  const location = useLocation()
  const { data: projects, isLoading } = useProjects()

  const hasProjects = projects && projects.length > 0
  const isProjectsPage = location.pathname === '/projects'

  // Show empty state if no projects exist (except on the projects page itself)
  const showEmptyState = !isLoading && !hasProjects && !isProjectsPage

  return (
    <div className="flex h-screen flex-col bg-background">
      {/* Animated gradient background */}
      <div className="bg-gradient-animated" />

      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <div className="relative z-10 flex flex-1 flex-col overflow-hidden">
          <Header />
          <main className="main-content flex-1 overflow-auto p-6">
            {showEmptyState ? <EmptyProjectsState /> : children}
          </main>
        </div>
      </div>

      {/* Pride stripe */}
      <div className="h-1 w-full flex shrink-0">
        <div className="flex-1 bg-[#e40303]" />
        <div className="flex-1 bg-[#ff8c00]" />
        <div className="flex-1 bg-[#ffed00]" />
        <div className="flex-1 bg-[#008026]" />
        <div className="flex-1 bg-[#24408e]" />
        <div className="flex-1 bg-[#732982]" />
      </div>
    </div>
  )
}
