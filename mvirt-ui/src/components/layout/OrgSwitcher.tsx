import { useEffect } from 'react'
import { Building2, Check, ChevronsUpDown, Plus } from 'lucide-react'
import { useNavigate } from 'react-router-dom'
import { useOrgs } from '@/hooks/queries'
import { useOrg } from '@/hooks/useOrg'
import { useProject } from '@/hooks/useProject'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'

/**
 * Sidebar-mounted Org context indicator + switcher. Shows the active Org and
 * lets the user switch between Orgs they have access to. Switching clears
 * the active Project so the user picks a new one within the new Org.
 */
export function OrgSwitcher() {
  const navigate = useNavigate()
  const { data: orgs, isLoading } = useOrgs()
  const { currentOrg, setCurrentOrg } = useOrg()
  const { setCurrentProject } = useProject()

  // Auto-pick the first Org on first load if none is active and at least one exists.
  useEffect(() => {
    if (!currentOrg && orgs && orgs.length > 0) {
      setCurrentOrg(orgs[0])
    }
    // If the currently-active Org disappears (e.g. another admin deleted it),
    // fall back to the first available.
    if (currentOrg && orgs && !orgs.some((o) => o.id === currentOrg.id)) {
      setCurrentOrg(orgs[0] ?? null)
    }
  }, [currentOrg, orgs, setCurrentOrg])

  const handleSelect = (orgId: string) => {
    const next = orgs?.find((o) => o.id === orgId)
    if (!next || next.id === currentOrg?.id) return
    setCurrentOrg(next)
    setCurrentProject(null as unknown as never)
    navigate('/projects')
  }

  if (isLoading) {
    return (
      <div className="px-3 py-2 text-xs text-muted-foreground">Loading Orgs…</div>
    )
  }

  if (!orgs || orgs.length === 0) {
    return (
      <button
        type="button"
        onClick={() => navigate('/orgs')}
        className="m-2 flex items-center gap-2 rounded-md border border-dashed border-border px-3 py-2 text-sm text-muted-foreground hover:bg-secondary"
      >
        <Plus className="h-4 w-4" />
        Create your first Org
      </button>
    )
  }

  return (
    <div className="m-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            className={cn(
              'w-full justify-between gap-2 px-3 py-2 text-left font-normal',
            )}
          >
            <div className="flex items-center gap-2 overflow-hidden">
              <Building2 className="h-4 w-4 shrink-0 text-purple-light" />
              <div className="overflow-hidden">
                <div className="truncate text-sm font-medium">
                  {currentOrg?.name ?? 'Select Org'}
                </div>
                {currentOrg && (
                  <div className="truncate font-mono text-xs text-muted-foreground">
                    {currentOrg.slug}
                  </div>
                )}
              </div>
            </div>
            <ChevronsUpDown className="h-4 w-4 shrink-0 text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          {orgs.map((org) => (
            <DropdownMenuItem
              key={org.id}
              onSelect={() => handleSelect(org.id)}
              className="flex items-center justify-between"
            >
              <div className="flex items-center gap-2">
                <Building2 className="h-4 w-4 text-muted-foreground" />
                <div>
                  <div className="text-sm">{org.name}</div>
                  <div className="font-mono text-xs text-muted-foreground">
                    {org.slug}
                  </div>
                </div>
              </div>
              {org.id === currentOrg?.id && (
                <Check className="h-4 w-4 text-purple-light" />
              )}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => navigate('/orgs')}>
            <Plus className="mr-2 h-4 w-4" />
            Manage Orgs
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}
