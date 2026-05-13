import { useEffect, useState } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'
import { Building2, Check, ChevronDown, FolderKanban, Settings } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useOrgs, useProjects } from '@/hooks/queries'
import { useOrg } from '@/hooks/useOrg'
import { useProject } from '@/hooks/useProject'
import { cn } from '@/lib/utils'
import type { Org } from '@/types'

/**
 * Header-mounted Org + Project switcher.
 *
 * Layout: a wide dropdown with two columns —
 *  - left 40%: Org list. Clicking *focuses* an Org (right pane updates) but
 *    doesn't activate it yet.
 *  - right 60%: Projects in the focused Org. Clicking a project activates
 *    both the Org and the Project, then navigates to the project route.
 *
 * The active Org/Project (per their zustand stores) carries a check mark.
 */
export function OrgProjectSwitcher() {
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const { data: orgs } = useOrgs()
  const { data: projects } = useProjects()
  const { currentOrg } = useOrg()
  const { currentProject } = useProject()
  const [open, setOpen] = useState(false)
  const [focusedOrgSlug, setFocusedOrgSlug] = useState<string | null>(null)

  // The label reflects the page context, not just persisted store state.
  // - `/projects/:slug/*` — inside a project; show "{org} / project".
  // - `/orgs` (plain) — Organizations admin list; no Org is scoped here,
  //   show plain "Select Project".
  // - everything else with an active Org — Org-scoped (e.g.
  //   `/orgs/:slug/projects`, `/cluster`); show "{org} / Select Project".
  const inProjectScope = /^\/projects\//.test(pathname)
  const isOrgsAdminList = pathname === '/orgs' || pathname === '/orgs/'

  // When the dropdown opens, focus the active Org by default; otherwise the
  // first Org. Keeps the right pane meaningful even before the user clicks.
  useEffect(() => {
    if (open) {
      setFocusedOrgSlug(currentOrg?.slug ?? orgs?.[0]?.slug ?? null)
    }
  }, [open, currentOrg, orgs])

  // ScopeSync owns currentOrg / currentProject — it derives them from the
  // URL on every route change. Components in here only *read* the stores.

  const focusedOrg = orgs?.find((o) => o.slug === focusedOrgSlug) ?? null
  const focusedProjects =
    projects?.filter((p) => p.orgSlug === focusedOrgSlug) ?? []

  const triggerLabel = (() => {
    if (inProjectScope && currentProject && currentOrg) {
      return (
        <>
          <span className="text-muted-foreground font-mono">
            {currentOrg.slug}
          </span>
          <span className="text-muted-foreground/60 mx-1">/</span>
          <span className="font-medium">{currentProject.name}</span>
        </>
      )
    }
    if (currentOrg && !isOrgsAdminList) {
      return (
        <>
          <span className="text-muted-foreground font-mono">
            {currentOrg.slug}
          </span>
          <span className="text-muted-foreground/60 mx-1">/</span>
          <span className="text-muted-foreground">Select Project</span>
        </>
      )
    }
    return <span className="text-muted-foreground">Select Project</span>
  })()

  const activate = (_org: Org, projectSlug: string) => {
    // Navigate-only — ScopeSync will pick up the new URL and populate
    // currentOrg + currentProject from it. Setting state here in addition
    // races against ScopeSync's effect with the OLD pathname.
    setOpen(false)
    navigate(`/projects/${projectSlug}/vms`)
  }

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          className="min-w-0 max-w-full gap-2 px-2 hover:bg-purple/20 hover:text-purple-light md:px-3"
        >
          <Building2 className="h-4 w-4 shrink-0 text-purple-light" />
          <span className="flex min-w-0 items-center truncate text-sm">
            {triggerLabel}
          </span>
          <ChevronDown className="h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="w-[min(640px,calc(100vw-1rem))] p-0"
      >
        {/* Two-column body — fixed max height, each column scrolls independently. */}
        <div className="flex">
          {/* Left 40% — Orgs */}
          <div className="w-2/5 border-r border-border h-[min(24rem,60vh)] overflow-y-auto">
            <div className="px-3 py-2 text-xs font-medium text-muted-foreground border-b border-border bg-card/40 sticky top-0">
              Organizations
            </div>
            {orgs && orgs.length > 0 ? (
              orgs.map((org) => (
                <button
                  key={org.slug}
                  type="button"
                  // Hover focuses the projects preview on the right; click
                  // navigates into the Org's dashboard (the sidebar then
                  // shows the Org-context nav).
                  onMouseEnter={() => setFocusedOrgSlug(org.slug)}
                  onFocus={() => setFocusedOrgSlug(org.slug)}
                  onClick={() => {
                    setOpen(false)
                    navigate(`/orgs/${org.slug}`)
                  }}
                  className={cn(
                    'flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm transition-colors',
                    focusedOrgSlug === org.slug
                      ? 'bg-secondary'
                      : 'hover:bg-secondary/60',
                  )}
                >
                  <div className="flex items-center gap-2 min-w-0">
                    <Building2 className="h-4 w-4 shrink-0 text-muted-foreground" />
                    <div className="min-w-0">
                      <div className="truncate">{org.name}</div>
                      <div className="truncate font-mono text-xs text-muted-foreground">
                        {org.slug}
                      </div>
                    </div>
                  </div>
                  {currentOrg?.slug === org.slug && (
                    <Check className="h-4 w-4 shrink-0 text-purple-light" />
                  )}
                </button>
              ))
            ) : (
              <div className="px-3 py-4 text-xs text-muted-foreground">No Orgs</div>
            )}
          </div>

          {/* Right 60% — Projects in focused Org */}
          <div className="w-3/5 h-[min(24rem,60vh)] overflow-y-auto">
            <div className="px-3 py-2 text-xs font-medium text-muted-foreground border-b border-border bg-card/40 sticky top-0">
              {focusedOrg ? `Projects in ${focusedOrg.slug}` : 'Projects'}
            </div>
            {focusedOrg && focusedProjects.length > 0 ? (
              focusedProjects.map((project) => (
                <button
                  key={project.slug}
                  type="button"
                  onClick={() => activate(focusedOrg, project.slug)}
                  className={cn(
                    'flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm transition-colors hover:bg-secondary/60',
                    currentProject?.slug === project.slug && 'bg-purple/20',
                  )}
                >
                  <div className="flex items-center gap-2 min-w-0">
                    <FolderKanban className="h-4 w-4 shrink-0 text-muted-foreground" />
                    <div className="min-w-0">
                      <div className="truncate">{project.name}</div>
                      <div className="truncate font-mono text-xs text-muted-foreground">
                        {project.slug}
                      </div>
                    </div>
                  </div>
                  {currentProject?.slug === project.slug && (
                    <Check className="h-4 w-4 shrink-0 text-purple-light" />
                  )}
                </button>
              ))
            ) : (
              <div className="px-3 py-4 text-xs text-muted-foreground">
                {focusedOrg
                  ? 'No projects in this Org yet.'
                  : 'Select an Org on the left.'}
              </div>
            )}
          </div>
        </div>

        {/* Shared bottom action row — workload-context entries only. Cluster
            / Members / Billing live under the Org page's own tab navbar. */}
        <div className="flex border-t border-border bg-card/40">
          <Link
            to="/orgs"
            onClick={() => setOpen(false)}
            className="flex w-2/5 items-center gap-2 border-r border-border px-3 py-2 text-xs text-muted-foreground hover:bg-secondary/60"
          >
            <Settings className="h-3 w-3" />
            Manage Orgs
          </Link>
          <Link
            to={focusedOrg ? `/orgs/${focusedOrg.slug}/projects` : '/orgs'}
            onClick={() => setOpen(false)}
            className="flex w-3/5 items-center gap-2 px-3 py-2 text-xs text-muted-foreground hover:bg-secondary/60"
          >
            <Settings className="h-3 w-3" />
            {focusedOrg
              ? `Manage Projects in ${focusedOrg.slug}`
              : 'Manage Projects'}
          </Link>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
