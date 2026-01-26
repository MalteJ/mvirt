import { useNavigate } from 'react-router-dom'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Play, Square, Trash2, Plus } from 'lucide-react'
import { usePods, useStartPod, useStopPod, useDeletePod } from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { StateIndicator } from '@/components/data-display/StateIndicator'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { truncateId } from '@/lib/utils'
import { Pod, PodState } from '@/types'

export function ContainersPage() {
  const navigate = useNavigate()
  const { data: pods, isLoading } = usePods()
  const startPod = useStartPod()
  const stopPod = useStopPod()
  const deletePod = useDeletePod()

  const columns: ColumnDef<Pod>[] = [
    {
      accessorKey: 'name',
      header: 'Name',
      cell: ({ row }) => (
        <div>
          <div className="font-medium">{row.original.name}</div>
          <div className="text-xs text-muted-foreground font-mono">
            {truncateId(row.original.id)}
          </div>
        </div>
      ),
    },
    {
      accessorKey: 'state',
      header: 'State',
      cell: ({ row }) => <StateIndicator state={row.original.state} />,
    },
    {
      accessorKey: 'containers',
      header: 'Containers',
      cell: ({ row }) => row.original.containers.length,
    },
    {
      accessorKey: 'ipAddress',
      header: 'IP Address',
      cell: ({ row }) => (
        <span className="font-mono text-sm">
          {row.original.ipAddress || '—'}
        </span>
      ),
    },
    {
      id: 'actions',
      cell: ({ row }) => {
        const pod = row.original
        const canStart = pod.state === PodState.STOPPED || pod.state === PodState.CREATED
        const canStop = pod.state === PodState.RUNNING

        return (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" onClick={(e) => e.stopPropagation()}>
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {canStart && (
                <DropdownMenuItem
                  onClick={(e) => {
                    e.stopPropagation()
                    startPod.mutate(pod.id)
                  }}
                >
                  <Play className="mr-2 h-4 w-4" />
                  Start
                </DropdownMenuItem>
              )}
              {canStop && (
                <DropdownMenuItem
                  onClick={(e) => {
                    e.stopPropagation()
                    stopPod.mutate(pod.id)
                  }}
                >
                  <Square className="mr-2 h-4 w-4" />
                  Stop
                </DropdownMenuItem>
              )}
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive"
                onClick={(e) => {
                  e.stopPropagation()
                  if (confirm(`Delete Pod "${pod.name}"?`)) {
                    deletePod.mutate(pod.id)
                  }
                }}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        )
      },
    },
  ]

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Containers</h2>
          <p className="text-muted-foreground">
            Manage your isolated pods
          </p>
        </div>
        <Button onClick={() => navigate('/containers/new')}>
          <Plus className="mr-2 h-4 w-4" />
          Create Pod
        </Button>
      </div>
      <DataTable
        columns={columns}
        data={pods || []}
        searchColumn="name"
        searchPlaceholder="Filter pods..."
        onRowClick={(pod) => navigate(`/containers/${pod.id}`)}
      />
    </div>
  )
}
