import { useState, useEffect } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, FolderOpen } from 'lucide-react'
import {
  useProjects,
  useDeleteProject,
  useOrgs,
  useCreateProjectInOrg,
} from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
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
  // Org context comes from the URL: `/orgs/:orgSlug/projects`. ScopeSync
  // (App.tsx) syncs the zustand store to this URL on every route change,
  // so reading currentOrg here always reflects the URL.
  const { orgSlug: orgSlugFromUrl } = useParams<{ orgSlug: string }>()
  const { data: allProjects, isLoading } = useProjects()
  const { data: orgs } = useOrgs()
  const deleteProject = useDeleteProject()

  // Resolve the URL Org from the loaded list. Used for filtering + the
  // page header. Independent of the zustand store — the URL is truth here.
  const scopedOrg = orgs?.find((o) => o.slug === orgSlugFromUrl) ?? null

  const [dialogOpen, setDialogOpen] = useState(false)
  const [orgSlug, setOrgSlug] = useState<string>('')
  const [slug, setSlug] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [slugTouched, setSlugTouched] = useState(false)

  const createProject = useCreateProjectInOrg(orgSlug)

  const projects = scopedOrg
    ? allProjects?.filter((p) => p.orgSlug === scopedOrg.slug)
    : allProjects

  // Pre-fill the create dialog with the URL-scoped Org.
  useEffect(() => {
    if (!orgSlug && scopedOrg) {
      setOrgSlug(scopedOrg.slug)
    } else if (!orgSlug && orgs && orgs.length > 0) {
      setOrgSlug(orgs[0].slug)
    }
  }, [orgSlug, scopedOrg, orgs])

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
          // ScopeSync will set currentProject from the URL.
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
        <div>
          <div className="font-medium">{row.original.name}</div>
          <div className="text-xs text-muted-foreground font-mono">
            {row.original.slug}
          </div>
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
              onClick={() => navigate(`/projects/${row.original.slug}/vms`)}
            >
              <FolderOpen className="mr-2 h-4 w-4" />
              Open
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
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h2 className="text-lg font-semibold tracking-tight">Projects</h2>
          <p className="text-sm text-muted-foreground">
            K8s-style namespaces — every VM, network, and volume lives in one.
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
