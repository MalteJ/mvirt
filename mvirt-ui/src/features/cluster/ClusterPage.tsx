import { ColumnDef } from '@tanstack/react-table'
import { useNavigate } from 'react-router-dom'
import { Server, Crown, Shield, MoreHorizontal, Wrench } from 'lucide-react'
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
import { truncateId } from '@/lib/utils'
import { Node, NodeStatus } from '@/types'

function formatMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`
  return `${mb} MB`
}

function timeAgo(iso: string): string {
  const seconds = Math.floor((Date.now() - new Date(iso).getTime()) / 1000)
  if (seconds < 60) return `${seconds}s ago`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  return `${Math.floor(seconds / 3600)}h ago`
}

export function ClusterPage() {
  const navigate = useNavigate()
  const { data: nodes, isLoading } = useNodes()
  const { data: clusterInfo } = useClusterInfo()

  const onlineNodes = nodes?.filter((n) => n.status === NodeStatus.ONLINE).length ?? 0
  const totalNodes = nodes?.length ?? 0
  const totalCpus = nodes?.reduce((sum, n) => sum + n.resources.cpu_cores, 0) ?? 0
  const totalMemoryMb = nodes?.reduce((sum, n) => sum + n.resources.memory_mb, 0) ?? 0

  const leaderPeer = clusterInfo?.peers?.find(p => p.is_leader)

  const columns: ColumnDef<Node>[] = [
    {
      accessorKey: 'name',
      header: 'Node',
      cell: ({ row }) => {
        return (
          <div className="flex items-center gap-3">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-secondary">
              <Server className="h-4 w-4 text-muted-foreground" />
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
      accessorKey: 'status',
      header: 'Status',
      cell: ({ row }) => (
        <Badge variant={row.original.status === NodeStatus.ONLINE ? 'running' : 'error'}>
          {row.original.status}
        </Badge>
      ),
    },
    {
      accessorKey: 'resources',
      header: 'Resources',
      cell: ({ row }) => {
        const r = row.original.resources
        return (
          <div className="text-sm">
            <span className="text-muted-foreground">{r.cpu_cores} CPUs</span>
            <span className="text-muted-foreground mx-1">|</span>
            <span className="text-muted-foreground">{formatMb(r.memory_mb)}</span>
          </div>
        )
      },
    },
    {
      accessorKey: 'last_heartbeat',
      header: 'Last Heartbeat',
      cell: ({ row }) => (
        <span className="text-sm text-muted-foreground">
          {timeAgo(row.original.last_heartbeat)}
        </span>
      ),
    },
    {
      id: 'actions',
      cell: () => (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon">
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem>
              <Wrench className="mr-2 h-4 w-4" />
              Enter Maintenance
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
          title="Total CPUs"
          value={totalCpus}
          color="cyan"
        />
        <StatCard
          title="Total Memory"
          value={formatMb(totalMemoryMb)}
          color="blue"
        />
        <StatCard
          title="Raft Term"
          value={clusterInfo?.current_term ?? '-'}
          color="green"
        />
      </div>

      {clusterInfo && (
        <Card className="border-border bg-card/50 backdrop-blur-sm">
          <CardHeader>
            <CardTitle className="text-sm font-medium">Control Plane</CardTitle>
          </CardHeader>
          <CardContent>
            <dl className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
              <div>
                <dt className="text-muted-foreground">Cluster ID</dt>
                <dd className="font-mono text-xs">{clusterInfo.cluster_id}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Leader</dt>
                <dd className="flex items-center gap-1.5">
                  <Crown className="h-3.5 w-3.5 text-purple-light" />
                  <span className="font-medium">{leaderPeer?.name ?? `peer-${clusterInfo.leader_id}`}</span>
                </dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Term</dt>
                <dd className="font-mono">{clusterInfo.current_term}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Peers</dt>
                <dd className="flex gap-1.5 flex-wrap">
                  {clusterInfo.peers.map(p => (
                    <Badge key={p.id} variant={p.is_leader ? 'running' : 'default'}>
                      {p.is_leader ? <Crown className="mr-1 h-3 w-3" /> : <Shield className="mr-1 h-3 w-3" />}
                      {p.name}
                    </Badge>
                  ))}
                </dd>
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
          onRowClick={(node) => navigate(`/cluster/${node.id}`)}
        />
      )}
    </div>
  )
}
