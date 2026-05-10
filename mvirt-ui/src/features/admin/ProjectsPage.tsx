import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, FolderOpen } from 'lucide-react'
import {
  useProjects,
  useDeleteProject,
  useOrgs,
  useCreateProjectInOrg,
} from '@/hooks/queries'
import { useProject } from '@/hooks/useProject'
import { useOrg } from '@/hooks/useOrg'
import { DataTable } from '@/components/data-display/DataTable'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { formatDate } from '@/lib/utils'
import { Project } from '@/types'

// Convert a free-text name to a kebab-case slug suitable for the URL
// identifier. Per ADR-0004, slugs are lowercase letters/digits/hyphens, no
// leading/trailing hyphen, ≤63 chars.
function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 63)
}

export function ProjectsPage() {
  const navigate = useNavigate()
  const { data: allProjects, isLoading } = useProjects()
  const { data: orgs } = useOrgs()
  const { currentProject, setCurrentProject } = useProject()
  const { currentOrg } = useOrg()
  const deleteProject = useDeleteProject()

  const [dialogOpen, setDialogOpen] = useState(false)
  // The create dialog defaults to the active Org from the switcher; user can
  // still override per-create if they want.
  const [orgSlug, setOrgSlug] = useState<string>('')
  const [slug, setSlug] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [slugTouched, setSlugTouched] = useState(false)

  const createProject = useCreateProjectInOrg(orgSlug)

  // Page is scoped to the active Org from the global switcher.
  // Falls back to all projects only if no Org is active (shouldn't normally happen).
  const projects = currentOrg
    ? allProjects?.filter((p) => p.orgSlug === currentOrg.slug)
    : allProjects

  // Pre-fill the create dialog with the active Org.
  useEffect(() => {
    if (!orgSlug && currentOrg) {
      setOrgSlug(currentOrg.slug)
    } else if (!orgSlug && orgs && orgs.length > 0) {
      setOrgSlug(orgs[0].slug)
    }
  }, [orgSlug, currentOrg, orgs])

  // Auto-generate slug from name unless the user has manually edited it.
  useEffect(() => {
    if (!slugTouched) {
      setSlug(slugify(name))
    }
  }, [name, slugTouched])

  const handleCreate = () => {
    if (!slug.trim() || !name.trim() || !orgSlug) return
    createProject.mutate(
      {
        slug: slug.trim(),
        name: name.trim(),
        description: description.trim() || undefined,
      },
      {
        onSuccess: (project) => {
          setDialogOpen(false)
          setSlug('')
          setName('')
          setDescription('')
          setSlugTouched(false)
          // Switch to the new project
          setCurrentProject(project)
          navigate(`/projects/${project.slug}/vms`)
        },
      }
    )
  }

  const columns: ColumnDef<Project>[] = [
    {
      accessorKey: 'name',
      header: 'Name',
      cell: ({ row }) => (
        <div className="flex items-center gap-2">
          <div>
            <div className="font-medium">{row.original.name}</div>
            <div className="text-xs text-muted-foreground font-mono">
              {row.original.slug}
            </div>
          </div>
          {currentProject?.slug === row.original.slug && (
            <Badge variant="running">Active</Badge>
          )}
        </div>
      ),
    },
    {
      id: 'org',
      header: 'Org',
      cell: ({ row }) => {
        const org = orgs?.find((o) => o.slug === row.original.orgSlug)
        return (
          <span className="text-sm font-mono text-muted-foreground">
            {org?.slug ?? row.original.orgSlug}
          </span>
        )
      },
    },
    {
      accessorKey: 'description',
      header: 'Description',
      cell: ({ row }) => (
        <span className="text-muted-foreground">
          {row.original.description || '-'}
        </span>
      ),
    },
    {
      accessorKey: 'createdAt',
      header: 'Created',
      cell: ({ row }) => (
        <span className="text-sm text-muted-foreground">
          {formatDate(row.original.createdAt)}
        </span>
      ),
    },
    {
      id: 'actions',
      cell: ({ row }) => (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon">
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              onClick={() => {
                setCurrentProject(row.original)
                navigate(`/projects/${row.original.slug}/vms`)
              }}
              disabled={currentProject?.slug === row.original.slug}
            >
              <FolderOpen className="mr-2 h-4 w-4" />
              Set as Active
            </DropdownMenuItem>
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => {
                if (
                  confirm(
                    `Delete project "${row.original.name}"? This will also delete all resources in this project.`,
                  )
                ) {
                  deleteProject.mutate(row.original.slug)
                }
              }}
              disabled={currentProject?.slug === row.original.slug}
            >
              <Trash2 className="mr-2 h-4 w-4" />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ]

  const isSlugValid = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/.test(slug) && slug.length > 0

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">
            Projects
            {currentOrg && (
              <span className="ml-2 font-mono text-base font-normal text-muted-foreground">
                in {currentOrg.slug}
              </span>
            )}
          </h2>
          <p className="text-muted-foreground">
            {currentOrg
              ? `Projects scoped to ${currentOrg.name}. Switch Org in the header to view projects elsewhere.`
              : 'Pick an Org from the header switcher to scope this view.'}
          </p>
        </div>
        <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
          <DialogTrigger asChild>
            <Button disabled={!orgs || orgs.length === 0}>
              <Plus className="mr-2 h-4 w-4" />
              Create Project
            </Button>
          </DialogTrigger>
          <DialogContent>
            <form
              onSubmit={(e) => {
                e.preventDefault()
                handleCreate()
              }}
            >
              <DialogHeader>
                <DialogTitle>Create Project</DialogTitle>
                <DialogDescription>
                  Projects help organize VMs, containers, networks, and storage.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="org">Organization</Label>
                  <Select value={orgSlug} onValueChange={setOrgSlug}>
                    <SelectTrigger id="org">
                      <SelectValue placeholder="Select an Org" />
                    </SelectTrigger>
                    <SelectContent>
                      {orgs?.map((o) => (
                        <SelectItem key={o.slug} value={o.slug}>
                          {o.name}{' '}
                          <span className="text-muted-foreground">({o.slug})</span>
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="name">Name</Label>
                  <Input
                    id="name"
                    placeholder="My Project"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="slug">Project Slug</Label>
                  <Input
                    id="slug"
                    placeholder="my-project"
                    className="font-mono"
                    value={slug}
                    onChange={(e) => {
                      setSlug(
                        e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, ''),
                      )
                      setSlugTouched(true)
                    }}
                  />
                  <p className="text-xs text-muted-foreground">
                    URL identifier — kebab-case, platform-wide unique, immutable.
                  </p>
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="description">Description (optional)</Label>
                  <Input
                    id="description"
                    placeholder="Project description"
                    value={description}
                    onChange={(e) => setDescription(e.target.value)}
                  />
                </div>
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setDialogOpen(false)}
                >
                  Cancel
                </Button>
                <Button
                  type="submit"
                  disabled={
                    !name.trim() ||
                    !isSlugValid ||
                    !orgSlug ||
                    createProject.isPending
                  }
                >
                  {createProject.isPending ? 'Creating...' : 'Create'}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        </Dialog>
      </div>

      {isLoading ? (
        <div className="flex items-center justify-center h-32">
          <p className="text-muted-foreground">Loading...</p>
        </div>
      ) : (
        <DataTable
          columns={columns}
          data={projects || []}
          searchColumn="name"
          searchPlaceholder="Filter projects..."
        />
      )}
    </div>
  )
}
