import { ReactNode } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { Building2, FolderKanban, Sparkles } from 'lucide-react'
import { Sidebar } from './Sidebar'
import { Header } from './Header'
import { Button } from '@/components/ui/button'
import { useOrgs, useProjects } from '@/hooks/queries'

interface LayoutProps {
  children: ReactNode
}

function EmptyProjectsState({ hasOrgs }: { hasOrgs: boolean }) {
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
              {hasOrgs ? (
                <FolderKanban className="h-12 w-12 text-white" />
              ) : (
                <Building2 className="h-12 w-12 text-white" />
              )}
            </div>
          </div>
        </div>
        <h2 className="text-2xl font-bold tracking-tight mb-2">
          It's quiet... too quiet
        </h2>
        <p className="text-muted-foreground mb-8">
          {hasOrgs
            ? 'No VMs humming, no containers running, no packets flowing. Time to bring this place to life.'
            : 'No organization yet. Create one to get started — every project lives under an Org.'}
        </p>
        <Button
          size="lg"
          onClick={() => navigate(hasOrgs ? '/projects' : '/orgs')}
          className="gap-2"
        >
          <Sparkles className="h-4 w-4" />
          {hasOrgs ? "Let's Build Something" : 'Create your first Org'}
        </Button>
      </div>
    </div>
  )
}

export function Layout({ children }: LayoutProps) {
  const location = useLocation()
  const { data: projects, isLoading: projectsLoading } = useProjects()
  const { data: orgs, isLoading: orgsLoading } = useOrgs()

  const hasProjects = projects && projects.length > 0
  const hasOrgs = !!orgs && orgs.length > 0
  // The admin pages (Orgs, Projects, Cluster) and Org-create flow must remain
  // reachable even when nothing else exists — otherwise the empty-state
  // overlay traps the user on a CTA that points at the very page it's
  // hiding.
  const isAdminPage =
    location.pathname === '/orgs' ||
    location.pathname.startsWith('/orgs/') ||
    location.pathname === '/projects' ||
    location.pathname === '/cluster' ||
    location.pathname.startsWith('/cluster/')

  const showEmptyState =
    !projectsLoading && !orgsLoading && !hasProjects && !isAdminPage

  return (
    <div className="flex h-screen flex-col bg-background">
      {/* Animated gradient background */}
      <div className="bg-gradient-animated" />

      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <div className="relative z-10 flex flex-1 flex-col overflow-hidden">
          <Header />
          <main className="main-content flex-1 overflow-auto p-6">
            {showEmptyState ? <EmptyProjectsState hasOrgs={hasOrgs} /> : children}
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
