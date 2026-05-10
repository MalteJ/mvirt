import { useState } from 'react'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2 } from 'lucide-react'
import { useOrgs, useCreateOrg, useDeleteOrg } from '@/hooks/queries'
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
import type { Org } from '@/types'

function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 63)
}

export function OrgsPage() {
  const { data: orgs, isLoading } = useOrgs()
  const createOrg = useCreateOrg()
  const deleteOrg = useDeleteOrg()

  const [dialogOpen, setDialogOpen] = useState(false)
  const [slug, setSlug] = useState('')
  const [name, setName] = useState('')
  const [slugTouched, setSlugTouched] = useState(false)

  // Auto-derive slug from name unless user has edited it.
  const handleNameChange = (v: string) => {
    setName(v)
    if (!slugTouched) {
      setSlug(slugify(v))
    }
  }

  const handleCreate = () => {
    if (!slug.trim() || !name.trim()) return
    createOrg.mutate(
      { slug: slug.trim(), name: name.trim() },
      {
        onSuccess: () => {
          setDialogOpen(false)
          setSlug('')
          setName('')
          setSlugTouched(false)
        },
        onError: () => {
          // Surfaced inline below the form via createOrg.error.
        },
      },
    )
  }

  const isSlugValid = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/.test(slug) && slug.length > 0

  const columns: ColumnDef<Org>[] = [
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
      accessorKey: 'defaultStaticKeyTtlDays',
      header: 'Static Key TTL',
      cell: ({ row }) =>
        row.original.disallowStaticKeys ? (
          <span className="text-muted-foreground">disallowed</span>
        ) : (
          <span className="text-sm font-mono">
            {row.original.defaultStaticKeyTtlDays} d
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
              className="text-destructive"
              onClick={() => {
                if (
                  confirm(
                    `Delete Org "${row.original.name}"? Rejected if any Projects still belong to it.`,
                  )
                ) {
                  deleteOrg.mutate(row.original.slug)
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
          <h2 className="text-2xl font-bold tracking-tight">Organizations</h2>
          <p className="text-muted-foreground">
            Tenancy containers above Project. Every Project belongs to exactly one
            Org.
          </p>
        </div>
        <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
          <DialogTrigger asChild>
            <Button>
              <Plus className="mr-2 h-4 w-4" />
              Create Org
            </Button>
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Create Organization</DialogTitle>
              <DialogDescription>
                The Org slug becomes the URL identifier and is immutable.
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-4 py-4">
              <div className="grid gap-2">
                <Label htmlFor="name">Name</Label>
                <Input
                  id="name"
                  placeholder="Acme Corp"
                  value={name}
                  onChange={(e) => handleNameChange(e.target.value)}
                />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="slug">Slug</Label>
                <Input
                  id="slug"
                  placeholder="acme"
                  className="font-mono"
                  value={slug}
                  onChange={(e) => {
                    setSlug(e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, ''))
                    setSlugTouched(true)
                  }}
                />
                <p className="text-xs text-muted-foreground">
                  Kebab-case, platform-wide unique, immutable.
                </p>
              </div>
              {createOrg.error && (
                <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                  {String(createOrg.error)}
                </div>
              )}
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => setDialogOpen(false)}>
                Cancel
              </Button>
              <Button
                onClick={handleCreate}
                disabled={!name.trim() || !isSlugValid || createOrg.isPending}
              >
                {createOrg.isPending ? 'Creating...' : 'Create'}
              </Button>
            </DialogFooter>
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
          data={orgs || []}
          searchColumn="name"
          searchPlaceholder="Filter Orgs..."
        />
      )}
    </div>
  )
}
