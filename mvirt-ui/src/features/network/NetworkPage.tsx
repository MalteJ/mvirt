import { useState } from 'react'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, Link, Unlink } from 'lucide-react'
import { useNetworks, useNics, useDeleteNetwork, useDeleteNic } from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { truncateId } from '@/lib/utils'
import { Network, Nic, NicState } from '@/types'

export function NetworkPage() {
  const { data: networks, isLoading: loadingNetworks } = useNetworks()
  const { data: nics, isLoading: loadingNics } = useNics()
  const deleteNetwork = useDeleteNetwork()
  const deleteNic = useDeleteNic()
  const [activeTab, setActiveTab] = useState('networks')

  const networkColumns: ColumnDef<Network>[] = [
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
      accessorKey: 'ipv4Subnet',
      header: 'IPv4 Subnet',
      cell: ({ row }) => (
        <span className="font-mono text-sm">
          {row.original.ipv4Subnet || '-'}
        </span>
      ),
    },
    {
      accessorKey: 'ipv6Prefix',
      header: 'IPv6 Prefix',
      cell: ({ row }) => (
        <span className="font-mono text-sm">
          {row.original.ipv6Prefix || '-'}
        </span>
      ),
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
            <Button variant="ghost" size="icon">
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => {
                if (confirm(`Delete network "${row.original.name}"?`)) {
                  deleteNetwork.mutate(row.original.id)
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

  const nicColumns: ColumnDef<Nic>[] = [
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
      accessorKey: 'macAddress',
      header: 'MAC Address',
      cell: ({ row }) => (
        <span className="font-mono text-sm">{row.original.macAddress}</span>
      ),
    },
    {
      accessorKey: 'state',
      header: 'State',
      cell: ({ row }) => (
        <Badge variant={row.original.state === NicState.ATTACHED ? 'running' : 'stopped'}>
          {row.original.state === NicState.ATTACHED ? 'Attached' : 'Detached'}
        </Badge>
      ),
    },
    {
      accessorKey: 'ipv4Address',
      header: 'IPv4',
      cell: ({ row }) => (
        <span className="font-mono text-sm">
          {row.original.ipv4Address || '-'}
        </span>
      ),
    },
    {
      accessorKey: 'vmId',
      header: 'VM',
      cell: ({ row }) => (
        row.original.vmId ? (
          <span className="font-mono text-sm">{truncateId(row.original.vmId)}</span>
        ) : (
          <span className="text-muted-foreground">-</span>
        )
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
            {row.original.state === NicState.ATTACHED ? (
              <DropdownMenuItem>
                <Unlink className="mr-2 h-4 w-4" />
                Detach
              </DropdownMenuItem>
            ) : (
              <DropdownMenuItem>
                <Link className="mr-2 h-4 w-4" />
                Attach to VM
              </DropdownMenuItem>
            )}
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => {
                if (confirm(`Delete NIC "${row.original.name}"?`)) {
                  deleteNic.mutate(row.original.id)
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
          <h2 className="text-2xl font-bold tracking-tight">Network</h2>
          <p className="text-muted-foreground">
            Manage networks and NICs
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">
            <Plus className="mr-2 h-4 w-4" />
            Create NIC
          </Button>
          <Button>
            <Plus className="mr-2 h-4 w-4" />
            Create Network
          </Button>
        </div>
      </div>

      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList>
          <TabsTrigger value="networks">Networks ({networks?.length ?? 0})</TabsTrigger>
          <TabsTrigger value="nics">NICs ({nics?.length ?? 0})</TabsTrigger>
        </TabsList>

        <TabsContent value="networks">
          {loadingNetworks ? (
            <div className="flex items-center justify-center h-32">
              <p className="text-muted-foreground">Loading...</p>
            </div>
          ) : (
            <DataTable
              columns={networkColumns}
              data={networks || []}
              searchColumn="name"
              searchPlaceholder="Filter networks..."
            />
          )}
        </TabsContent>

        <TabsContent value="nics">
          {loadingNics ? (
            <div className="flex items-center justify-center h-32">
              <p className="text-muted-foreground">Loading...</p>
            </div>
          ) : (
            <DataTable
              columns={nicColumns}
              data={nics || []}
              searchColumn="name"
              searchPlaceholder="Filter NICs..."
            />
          )}
        </TabsContent>
      </Tabs>
    </div>
  )
}
