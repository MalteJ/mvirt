import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useProjectId } from '@/hooks/useProjectId'
import { ColumnDef } from '@tanstack/react-table'
import {
  MoreHorizontal,
  Plus,
  Trash2,
  Shield,
  ArrowDownToLine,
  ArrowUpFromLine,
} from 'lucide-react'
import {
  useSecurityGroups,
  useCreateSecurityGroup,
  useDeleteSecurityGroup,
} from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
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
import { truncateId } from '@/lib/utils'
import { SecurityGroup, RuleDirection } from '@/types'

export function FirewallPage() {
  const projectId = useProjectId()
  const navigate = useNavigate()
  const { data: securityGroups, isLoading, error } = useSecurityGroups()
  const createSecurityGroup = useCreateSecurityGroup()
  const deleteSecurityGroup = useDeleteSecurityGroup()

  // Create Security Group dialog
  const [sgDialogOpen, setSgDialogOpen] = useState(false)
  const [sgName, setSgName] = useState('')
  const [sgDescription, setSgDescription] = useState('')

  const handleCreateSecurityGroup = () => {
    if (!sgName.trim()) return
    createSecurityGroup.mutate(
      {
        name: sgName.trim(),
        description: sgDescription.trim() || undefined,
      },
      {
        onSuccess: () => {
          setSgDialogOpen(false)
          setSgName('')
          setSgDescription('')
        },
      }
    )
  }

  const columns: ColumnDef<SecurityGroup>[] = [
    {
      accessorKey: 'name',
      header: 'Name',
      cell: ({ row }) => (
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-purple/20">
            <Shield className="h-4 w-4 text-purple" />
          </div>
          <div>
            <div className="font-medium">{row.original.name}</div>
            <div className="text-xs text-muted-foreground font-mono">
              {truncateId(row.original.id)}
            </div>
          </div>
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
      accessorKey: 'rules',
      header: 'Rules',
      cell: ({ row }) => {
        const rules = row.original.rules || []
        return (
          <div className="flex gap-2">
            <Badge variant="outline" className="gap-1">
              <ArrowDownToLine className="h-3 w-3" />
              {rules.filter(r => r.direction === RuleDirection.INGRESS).length}
            </Badge>
            <Badge variant="outline" className="gap-1">
              <ArrowUpFromLine className="h-3 w-3" />
              {rules.filter(r => r.direction === RuleDirection.EGRESS).length}
            </Badge>
          </div>
        )
      },
    },
    {
      accessorKey: 'nicCount',
      header: 'NICs',
      cell: ({ row }) => row.original.nicCount,
    },
    {
      id: 'actions',
      cell: ({ row }) => (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" onClick={(e) => e.stopPropagation()}>
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              className="text-destructive"
              onClick={(e) => {
                e.stopPropagation()
                if (confirm(`Delete security group "${row.original.name}"?`)) {
                  deleteSecurityGroup.mutate(row.original.id)
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

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <p className="text-muted-foreground">Loading...</p>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-2">
        <p className="text-destructive">Failed to load security groups</p>
        <p className="text-sm text-muted-foreground">{error.message}</p>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Firewall</h2>
          <p className="text-muted-foreground">
            Manage security groups and firewall rules
          </p>
        </div>
        <Dialog open={sgDialogOpen} onOpenChange={setSgDialogOpen}>
          <DialogTrigger asChild>
            <Button>
              <Plus className="mr-2 h-4 w-4" />
              Create Security Group
            </Button>
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Create Security Group</DialogTitle>
              <DialogDescription>
                Security groups act as a virtual firewall for your NICs.
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-4 py-4">
              <div className="grid gap-2">
                <Label htmlFor="sg-name">Name</Label>
                <Input
                  id="sg-name"
                  placeholder="web-servers"
                  value={sgName}
                  onChange={(e) => setSgName(e.target.value)}
                />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="sg-description">Description (optional)</Label>
                <Input
                  id="sg-description"
                  placeholder="Allow HTTP/HTTPS traffic"
                  value={sgDescription}
                  onChange={(e) => setSgDescription(e.target.value)}
                />
              </div>
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => setSgDialogOpen(false)}>
                Cancel
              </Button>
              <Button
                onClick={handleCreateSecurityGroup}
                disabled={!sgName.trim() || createSecurityGroup.isPending}
              >
                {createSecurityGroup.isPending ? 'Creating...' : 'Create'}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>

      {securityGroups && securityGroups.length > 0 ? (
        <DataTable
          columns={columns}
          data={securityGroups}
          searchColumn="name"
          searchPlaceholder="Filter security groups..."
          onRowClick={(sg) => navigate(`/p/${projectId}/firewall/${sg.id}`)}
        />
      ) : (
        <div className="flex flex-col items-center justify-center h-48 border border-dashed border-border rounded-lg">
          <Shield className="h-10 w-10 text-muted-foreground mb-3" />
          <p className="text-muted-foreground mb-1">No security groups</p>
          <p className="text-xs text-muted-foreground">
            Create a security group to define firewall rules for your NICs
          </p>
        </div>
      )}
    </div>
  )
}
