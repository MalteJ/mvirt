import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, FolderOpen } from 'lucide-react'
import { useProjects, useCreateProject, useDeleteProject } from '@/hooks/queries'
import { useProject } from '@/hooks/useProject'
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
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { formatDate } from '@/lib/utils'
import { Project } from '@/types'

// Convert name to a valid project ID (lowercase alphanumeric)
function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '')
    .slice(0, 32)
}

export function ProjectsPage() {
  const navigate = useNavigate()
  const { data: projects, isLoading } = useProjects()
  const { currentProject, setCurrentProject } = useProject()
  const createProject = useCreateProject()
  const deleteProject = useDeleteProject()

  const [dialogOpen, setDialogOpen] = useState(false)
  const [id, setId] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [idTouched, setIdTouched] = useState(false)

  // Auto-generate ID from name unless user has manually edited it
  useEffect(() => {
    if (!idTouched) {
      setId(slugify(name))
    }
  }, [name, idTouched])

  const handleCreate = () => {
    if (!id.trim() || !name.trim()) return
    createProject.mutate(
      {
        id: id.trim(),
        name: name.trim(),
        description: description.trim() || undefined,
      },
      {
        onSuccess: (project) => {
          setDialogOpen(false)
          setId('')
          setName('')
          setDescription('')
          setIdTouched(false)
          // Switch to the new project
          setCurrentProject(project)
          navigate(`/p/${project.id}/vms`)
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
              {row.original.id}
            </div>
          </div>
          {currentProject?.id === row.original.id && (
            <Badge variant="running">Active</Badge>
          )}
        </div>
      ),
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
                navigate(`/p/${row.original.id}/vms`)
              }}
              disabled={currentProject?.id === row.original.id}
            >
              <FolderOpen className="mr-2 h-4 w-4" />
              Set as Active
            </DropdownMenuItem>
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => {
                if (confirm(`Delete project "${row.original.name}"? This will also delete all resources in this project.`)) {
                  deleteProject.mutate(row.original.id)
                }
              }}
              disabled={currentProject?.id === row.original.id}
            >
              <Trash2 className="mr-2 h-4 w-4" />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ]

  const isIdValid = /^[a-z0-9]+$/.test(id) && id.length > 0

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Projects</h2>
          <p className="text-muted-foreground">
            Manage projects to organize your resources
          </p>
        </div>
        <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
          <DialogTrigger asChild>
            <Button>
              <Plus className="mr-2 h-4 w-4" />
              Create Project
            </Button>
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Create Project</DialogTitle>
              <DialogDescription>
                Projects help organize VMs, containers, networks, and storage.
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-4 py-4">
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
                <Label htmlFor="id">Project ID</Label>
                <Input
                  id="id"
                  placeholder="myproject"
                  className="font-mono"
                  value={id}
                  onChange={(e) => {
                    setId(e.target.value.toLowerCase().replace(/[^a-z0-9]/g, ''))
                    setIdTouched(true)
                  }}
                />
                <p className="text-xs text-muted-foreground">
                  Unique identifier for URLs. Only lowercase letters and numbers.
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
              <Button variant="outline" onClick={() => setDialogOpen(false)}>
                Cancel
              </Button>
              <Button
                onClick={handleCreate}
                disabled={!name.trim() || !isIdValid || createProject.isPending}
              >
                {createProject.isPending ? 'Creating...' : 'Create'}
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
          data={projects || []}
          searchColumn="name"
          searchPlaceholder="Filter projects..."
        />
      )}
    </div>
  )
}
