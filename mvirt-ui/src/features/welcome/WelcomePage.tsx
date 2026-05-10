import { Link } from 'react-router-dom'
import { Building2, Boxes, FolderKanban, Sparkles } from 'lucide-react'
import { useOrgs, useProjects } from '@/hooks/queries'

/**
 * Top-level welcome screen. The logo + bare `/` route land here — neither
 * scoped to a project nor to a particular Org. Surfaces the primary actions
 * the user might want from a cold start, plus a tiny snapshot of what's in
 * the platform.
 */
export function WelcomePage() {
  const { data: orgs } = useOrgs()
  const { data: projects } = useProjects()

  const orgCount = orgs?.length ?? 0
  const projectCount = projects?.length ?? 0

  return (
    <div className="flex flex-col items-center justify-center py-12">
      <div className="text-center max-w-2xl mx-auto px-6">
        <div className="relative mb-8 inline-flex items-center justify-center">
          <div className="absolute inset-0 flex items-center justify-center">
            <div className="h-40 w-40 rounded-full bg-purple/20 blur-3xl" />
          </div>
          <div className="relative flex h-28 w-28 items-center justify-center rounded-3xl bg-gradient-to-br from-purple to-blue shadow-glow-purple">
            <span className="logo-shimmer text-5xl font-bold text-white">m</span>
          </div>
        </div>

        <h1 className="text-4xl font-bold tracking-tight mb-3">
          Welcome to <span className="logo-shimmer">mvirt</span>
        </h1>
        <p className="text-muted-foreground text-lg mb-10 max-w-xl mx-auto">
          A multi-host hypervisor cluster: Organizations carve up tenancy,
          Projects hold resources, and the cluster runs them.
        </p>

        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3 mb-10">
          <Link
            to="/orgs"
            className="group flex flex-col items-start gap-2 rounded-xl border border-border bg-card/60 px-5 py-4 text-left transition-all hover:border-purple/40 hover:bg-card hover:shadow-glow-purple"
          >
            <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-purple/15 text-purple-light">
              <Building2 className="h-5 w-5" />
            </div>
            <div>
              <div className="font-medium">Organizations</div>
              <div className="text-xs text-muted-foreground">
                {orgCount === 0
                  ? 'Create your first Org'
                  : `${orgCount} ${orgCount === 1 ? 'Org' : 'Orgs'}`}
              </div>
            </div>
          </Link>

          <Link
            to="/orgs"
            className="group flex flex-col items-start gap-2 rounded-xl border border-border bg-card/60 px-5 py-4 text-left transition-all hover:border-purple/40 hover:bg-card hover:shadow-glow-purple"
          >
            <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-purple/15 text-purple-light">
              <FolderKanban className="h-5 w-5" />
            </div>
            <div>
              <div className="font-medium">Projects</div>
              <div className="text-xs text-muted-foreground">
                {projectCount === 0
                  ? 'No projects yet'
                  : `${projectCount} ${projectCount === 1 ? 'Project' : 'Projects'} across all Orgs`}
              </div>
            </div>
          </Link>

          <Link
            to="/cluster"
            className="group flex flex-col items-start gap-2 rounded-xl border border-border bg-card/60 px-5 py-4 text-left transition-all hover:border-purple/40 hover:bg-card hover:shadow-glow-purple"
          >
            <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-purple/15 text-purple-light">
              <Boxes className="h-5 w-5" />
            </div>
            <div>
              <div className="font-medium">Cluster</div>
              <div className="text-xs text-muted-foreground">
                Hypervisor nodes &amp; capacity
              </div>
            </div>
          </Link>
        </div>

        {orgCount === 0 && (
          <Link
            to="/orgs"
            className="inline-flex items-center gap-2 rounded-lg bg-gradient-to-br from-purple to-blue px-5 py-2.5 text-sm font-medium text-white shadow-glow-purple transition-transform hover:scale-[1.02]"
          >
            <Sparkles className="h-4 w-4" />
            Get started — create your first Org
          </Link>
        )}
      </div>
    </div>
  )
}
