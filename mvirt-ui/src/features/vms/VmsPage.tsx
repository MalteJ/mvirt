import { useNavigate } from 'react-router-dom'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Play, Square, Trash2, Plus } from 'lucide-react'
import { useVms, useStartVm, useStopVm, useDeleteVm } from '@/hooks/queries'
import { useProjectId } from '@/hooks/useProjectId'
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
import { Vm, VmState } from '@/types'

export function VmsPage() {
  const navigate = useNavigate()
  const projectId = useProjectId()
  const { data: vms, isLoading } = useVms(projectId)
  const startVm = useStartVm()
  const stopVm = useStopVm()
  const deleteVm = useDeleteVm()

  const columns: ColumnDef<Vm>[] = [
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
      accessorKey: 'config.vcpus',
      header: 'vCPUs',
      cell: ({ row }) => row.original.config.vcpus,
    },
    {
      accessorKey: 'config.memoryMb',
      header: 'Memory',
      cell: ({ row }) => `${row.original.config.memoryMb} MB`,
    },
    {
      id: 'actions',
      cell: ({ row }) => {
        const vm = row.original
        const canStart = vm.state === VmState.STOPPED
        const canStop = vm.state === VmState.RUNNING

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
                    startVm.mutate(vm.id)
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
                    stopVm.mutate(vm.id)
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
                  if (confirm(`Delete VM "${vm.name}"?`)) {
                    deleteVm.mutate(vm.id)
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
          <h2 className="text-2xl font-bold tracking-tight">Virtual Machines</h2>
          <p className="text-muted-foreground">
            Manage your virtual machines
          </p>
        </div>
        <Button onClick={() => navigate('new')}>
          <Plus className="mr-2 h-4 w-4" />
          Create VM
        </Button>
      </div>
      <DataTable
        columns={columns}
        data={vms || []}
        searchColumn="name"
        searchPlaceholder="Filter VMs..."
        onRowClick={(vm) => navigate(vm.id)}
      />
    </div>
  )
}
