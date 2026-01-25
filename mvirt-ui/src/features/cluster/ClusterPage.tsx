import { ColumnDef } from '@tanstack/react-table'
import { Server, Crown, Shield, Wrench, MoreHorizontal } from 'lucide-react'
import { useNodes, useClusterInfo } from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { StatCard } from '@/components/data-display/StatCard'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { truncateId, formatBytes } from '@/lib/utils'
import { Node, NodeState, NodeRole } from '@/types'

const stateVariants: Record<NodeState, 'running' | 'starting' | 'stopped' | 'error'> = {
  [NodeState.ONLINE]: 'running',
  [NodeState.JOINING]: 'starting',
  [NodeState.MAINTENANCE]: 'stopped',
  [NodeState.OFFLINE]: 'error',
}

const stateLabels: Record<NodeState, string> = {
  [NodeState.ONLINE]: 'Online',
  [NodeState.JOINING]: 'Joining',
  [NodeState.MAINTENANCE]: 'Maintenance',
  [NodeState.OFFLINE]: 'Offline',
}

const roleIcons: Record<NodeRole, typeof Crown> = {
  [NodeRole.LEADER]: Crown,
  [NodeRole.FOLLOWER]: Shield,
  [NodeRole.CANDIDATE]: Server,
}

export function ClusterPage() {
  const { data: nodes, isLoading } = useNodes()
  const { data: clusterInfo } = useClusterInfo()

  const onlineNodes = nodes?.filter((n) => n.state === NodeState.ONLINE).length ?? 0
  const totalNodes = nodes?.length ?? 0
  const totalVms = nodes?.reduce((sum, n) => sum + n.vmCount, 0) ?? 0
  const totalMemory = nodes?.reduce((sum, n) => sum + n.memoryTotalBytes, 0) ?? 0
  const usedMemory = nodes?.reduce((sum, n) => sum + n.memoryUsedBytes, 0) ?? 0

  const columns: ColumnDef<Node>[] = [
    {
      accessorKey: 'name',
      header: 'Node',
      cell: ({ row }) => {
        const RoleIcon = roleIcons[row.original.role]
        return (
          <div className="flex items-center gap-3">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-secondary">
              <RoleIcon className="h-4 w-4 text-muted-foreground" />
            </div>
            <div>
              <div className="font-medium">{row.original.name}</div>
              <div className="text-xs text-muted-foreground font-mono">
                {truncateId(row.original.id)}
              </div>
            </div>
          </div>
        )
      },
    },
    {
      accessorKey: 'address',
      header: 'Address',
      cell: ({ row }) => (
        <span className="font-mono text-sm">{row.original.address}</span>
      ),
    },
    {
      accessorKey: 'state',
      header: 'Status',
      cell: ({ row }) => (
        <Badge variant={stateVariants[row.original.state]}>
          {stateLabels[row.original.state]}
        </Badge>
      ),
    },
    {
      accessorKey: 'role',
      header: 'Role',
      cell: ({ row }) => (
        <span className={row.original.role === NodeRole.LEADER ? 'text-purple-light font-medium' : ''}>
          {row.original.role}
        </span>
      ),
    },
    {
      accessorKey: 'vmCount',
      header: 'VMs',
    },
    {
      accessorKey: 'resources',
      header: 'Resources',
      cell: ({ row }) => {
        const memPercent = Math.round(
          (row.original.memoryUsedBytes / row.original.memoryTotalBytes) * 100
        )
        return (
          <div className="text-sm">
            <div className="flex items-center gap-2">
              <span className="text-muted-foreground">{row.original.cpuCount} CPUs</span>
              <span className="text-muted-foreground">|</span>
              <span className="text-muted-foreground">
                {formatBytes(row.original.memoryUsedBytes)} / {formatBytes(row.original.memoryTotalBytes)}
              </span>
              <span className="text-xs text-muted-foreground">({memPercent}%)</span>
            </div>
          </div>
        )
      },
    },
    {
      accessorKey: 'version',
      header: 'Version',
      cell: ({ row }) => (
        <span className="font-mono text-xs">{row.original.version}</span>
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
            <DropdownMenuItem>
              <Wrench className="mr-2 h-4 w-4" />
              {row.original.state === NodeState.MAINTENANCE
                ? 'Exit Maintenance'
                : 'Enter Maintenance'}
            </DropdownMenuItem>
            <DropdownMenuItem className="text-destructive">
              Remove from Cluster
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ]

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">Cluster</h2>
        <p className="text-muted-foreground">
          Manage cluster nodes and view cluster health
        </p>
      </div>

      <div className="grid gap-4 md:grid-cols-4">
        <StatCard
          title="Nodes"
          value={`${onlineNodes}/${totalNodes}`}
          icon={<Server className="h-6 w-6" />}
          description={`${onlineNodes} online`}
          color="purple"
        />
        <StatCard
          title="Total VMs"
          value={totalVms}
          color="cyan"
        />
        <StatCard
          title="Cluster Memory"
          value={formatBytes(usedMemory)}
          description={`of ${formatBytes(totalMemory)}`}
          color="blue"
        />
        <StatCard
          title="Raft Term"
          value={clusterInfo?.term ?? '-'}
          color="green"
        />
      </div>

      {clusterInfo && (
        <Card className="border-border bg-card/50 backdrop-blur-sm">
          <CardHeader>
            <CardTitle className="text-sm font-medium">Cluster Info</CardTitle>
          </CardHeader>
          <CardContent>
            <dl className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
              <div>
                <dt className="text-muted-foreground">Cluster Name</dt>
                <dd className="font-medium">{clusterInfo.name}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Cluster ID</dt>
                <dd className="font-mono text-xs">{truncateId(clusterInfo.id)}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Leader</dt>
                <dd className="font-mono text-xs">{truncateId(clusterInfo.leaderNodeId)}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Created</dt>
                <dd>{new Date(clusterInfo.createdAt).toLocaleDateString()}</dd>
              </div>
            </dl>
          </CardContent>
        </Card>
      )}

      {isLoading ? (
        <Card>
          <CardContent className="flex items-center justify-center h-32">
            <p className="text-muted-foreground">Loading...</p>
          </CardContent>
        </Card>
      ) : (
        <DataTable
          columns={columns}
          data={nodes || []}
          searchColumn="name"
          searchPlaceholder="Filter nodes..."
        />
      )}
    </div>
  )
}
