import { useState, useEffect } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, FolderOpen, MapPin, Server } from 'lucide-react'
import {
  useClustersInOrg,
  useCreateClusterInOrg,
  useDeleteCluster,
  useOrgs,
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
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { formatDate } from '@/lib/utils'
import type { Cluster } from '@/types'

function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 63)
}

export function ClustersPage() {
  const navigate = useNavigate()
  const { orgSlug: orgSlugFromUrl } = useParams<{ orgSlug: string }>()
  const { data: orgs } = useOrgs()
  const scopedOrg = orgs?.find((o) => o.slug === orgSlugFromUrl) ?? null

  const { data: clusters, isLoading } = useClustersInOrg(scopedOrg?.slug)
  const createCluster = useCreateClusterInOrg(scopedOrg?.slug ?? '')
  const deleteCluster = useDeleteCluster()

  const [dialogOpen, setDialogOpen] = useState(false)
  const [slug, setSlug] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [location, setLocation] = useState('')
  const [slugTouched, setSlugTouched] = useState(false)

  useEffect(() => {
    if (!slugTouched) setSlug(slugify(name))
  }, [name, slugTouched])

  const handleCreate = () => {
    if (!slug.trim() || !name.trim() || !scopedOrg) return
    createCluster.mutate(
      {
        slug: slug.trim(),
        name: name.trim(),
        description: description.trim() || undefined,
        location: location.trim() || undefined,
      },
      {
        onSuccess: (cluster) => {
          setDialogOpen(false)
          setSlug('')
          setName('')
          setDescription('')
          setLocation('')
          setSlugTouched(false)
          navigate(`/clusters/${cluster.slug}`)
        },
      },
    )
  }

  const isSlugValid =
    /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/.test(slug) && slug.length > 0

  const columns: ColumnDef<Cluster>[] = [
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
      accessorKey: 'location',
      header: 'Location',
      cell: ({ row }) =>
        row.original.location ? (
          <span className="inline-flex items-center text-sm text-muted-foreground">
            <MapPin className="mr-1 h-3 w-3" />
            {row.original.location}
          </span>
        ) : (
          <span className="text-muted-foreground">—</span>
        ),
    },
    {
      id: 'nodes',
      header: 'Nodes',
      cell: ({ row }) => (
        <span className="inline-flex items-center text-sm text-muted-foreground">
          <Server className="mr-1 h-3 w-3" />
          {row.original.nodeIds.length}
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
              onClick={() => navigate(`/clusters/${row.original.slug}`)}
            >
              <FolderOpen className="mr-2 h-4 w-4" />
              Open
            </DropdownMenuItem>
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => {
                if (
                  confirm(
                    `Delete cluster "${row.original.name}"? Nodes must be removed first if any are placed here.`,
                  )
                ) {
                  deleteCluster.mutate(row.original.slug)
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

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">
            Clusters
            {scopedOrg && (
              <span className="ml-2 font-mono text-base font-normal text-muted-foreground">
                in {scopedOrg.slug}
              </span>
            )}
          </h2>
          <p className="text-muted-foreground">
            {scopedOrg
              ? `Hardware groups in ${scopedOrg.name}. Resources are placed on a Cluster's nodes.`
              : 'Pick an Org from the header switcher to scope this view.'}
          </p>
        </div>
        <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
          <DialogTrigger asChild>
            <Button disabled={!scopedOrg}>
              <Plus className="mr-2 h-4 w-4" />
              Create Cluster
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
                <DialogTitle>Create Cluster</DialogTitle>
                <DialogDescription>
                  A Cluster is the placement target for resources. Add Nodes via
                  the cluster detail page after creating.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="name">Name</Label>
                  <Input
                    id="name"
                    placeholder="My Cluster"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="slug">Cluster Slug</Label>
                  <Input
                    id="slug"
                    placeholder="west-1"
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
                  <Label htmlFor="location">Location (optional)</Label>
                  <Input
                    id="location"
                    placeholder="frankfurt-rack-3"
                    value={location}
                    onChange={(e) => setLocation(e.target.value)}
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="description">Description (optional)</Label>
                  <Input
                    id="description"
                    placeholder="Cluster description"
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
                    !scopedOrg ||
                    createCluster.isPending
                  }
                >
                  {createCluster.isPending ? 'Creating...' : 'Create'}
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
          data={clusters || []}
          searchColumn="name"
          searchPlaceholder="Filter clusters..."
        />
      )}
    </div>
  )
}
