import { useState } from 'react'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, Link, Unlink } from 'lucide-react'
import { useNetworks, useNics, useDeleteNetwork, useDeleteNic, useCreateNetwork, useCreateNic } from '@/hooks/queries'
import { useProjectId } from '@/hooks/useProjectId'
import { DataTable } from '@/components/data-display/DataTable'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
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
import { truncateId } from '@/lib/utils'
import { Network, Nic, NicState } from '@/types'

export function NetworkPage() {
  const projectId = useProjectId()
  const { data: networks, isLoading: loadingNetworks } = useNetworks(projectId)
  const { data: nics, isLoading: loadingNics } = useNics(projectId)
  const deleteNetwork = useDeleteNetwork()
  const deleteNic = useDeleteNic()
  const createNetwork = useCreateNetwork(projectId)
  const createNic = useCreateNic(projectId)
  const [activeTab, setActiveTab] = useState('networks')

  // Create Network Dialog state
  const [networkDialogOpen, setNetworkDialogOpen] = useState(false)
  const [networkName, setNetworkName] = useState('')
  const [ipv4Subnet, setIpv4Subnet] = useState('')
  const [ipv6Prefix, setIpv6Prefix] = useState('')

  // Create NIC Dialog state
  const [nicDialogOpen, setNicDialogOpen] = useState(false)
  const [nicName, setNicName] = useState('')
  const [nicNetworkId, setNicNetworkId] = useState('')
  const [nicMacAddress, setNicMacAddress] = useState('')

  const handleCreateNetwork = () => {
    if (!networkName.trim()) return
    createNetwork.mutate(
      {
        name: networkName.trim(),
        ipv4Subnet: ipv4Subnet.trim() || undefined,
        ipv6Prefix: ipv6Prefix.trim() || undefined,
      },
      {
        onSuccess: () => {
          setNetworkDialogOpen(false)
          setNetworkName('')
          setIpv4Subnet('')
          setIpv6Prefix('')
        },
      }
    )
  }

  const handleCreateNic = () => {
    if (!nicName.trim() || !nicNetworkId) return
    createNic.mutate(
      {
        name: nicName.trim(),
        networkId: nicNetworkId,
        macAddress: nicMacAddress.trim() || undefined,
      },
      {
        onSuccess: () => {
          setNicDialogOpen(false)
          setNicName('')
          setNicNetworkId('')
          setNicMacAddress('')
          setActiveTab('nics')
        },
      }
    )
  }

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
          <Dialog open={nicDialogOpen} onOpenChange={setNicDialogOpen}>
            <DialogTrigger asChild>
              <Button variant="outline">
                <Plus className="mr-2 h-4 w-4" />
                Create NIC
              </Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Create NIC</DialogTitle>
                <DialogDescription>
                  Create a new network interface card.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="nic-name">Name</Label>
                  <Input
                    id="nic-name"
                    placeholder="my-nic"
                    value={nicName}
                    onChange={(e) => setNicName(e.target.value)}
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="nic-network">Network</Label>
                  <Select value={nicNetworkId} onValueChange={setNicNetworkId}>
                    <SelectTrigger>
                      <SelectValue placeholder="Select a network" />
                    </SelectTrigger>
                    <SelectContent>
                      {networks?.map((net) => (
                        <SelectItem key={net.id} value={net.id}>
                          {net.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="nic-mac">MAC Address (optional)</Label>
                  <Input
                    id="nic-mac"
                    placeholder="Auto-generated if empty"
                    className="font-mono"
                    value={nicMacAddress}
                    onChange={(e) => setNicMacAddress(e.target.value)}
                  />
                </div>
              </div>
              <DialogFooter>
                <Button variant="outline" onClick={() => setNicDialogOpen(false)}>
                  Cancel
                </Button>
                <Button
                  onClick={handleCreateNic}
                  disabled={!nicName.trim() || !nicNetworkId || createNic.isPending}
                >
                  {createNic.isPending ? 'Creating...' : 'Create'}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>

          <Dialog open={networkDialogOpen} onOpenChange={setNetworkDialogOpen}>
            <DialogTrigger asChild>
              <Button>
                <Plus className="mr-2 h-4 w-4" />
                Create Network
              </Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Create Network</DialogTitle>
                <DialogDescription>
                  Create a new virtual network for your VMs and containers.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="network-name">Name</Label>
                  <Input
                    id="network-name"
                    placeholder="my-network"
                    value={networkName}
                    onChange={(e) => setNetworkName(e.target.value)}
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="ipv4-subnet">IPv4 Subnet (optional)</Label>
                  <Input
                    id="ipv4-subnet"
                    placeholder="10.0.0.0/24"
                    className="font-mono"
                    value={ipv4Subnet}
                    onChange={(e) => setIpv4Subnet(e.target.value)}
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="ipv6-prefix">IPv6 Prefix (optional)</Label>
                  <Input
                    id="ipv6-prefix"
                    placeholder="2001:db8::/64"
                    className="font-mono"
                    value={ipv6Prefix}
                    onChange={(e) => setIpv6Prefix(e.target.value)}
                  />
                </div>
              </div>
              <DialogFooter>
                <Button variant="outline" onClick={() => setNetworkDialogOpen(false)}>
                  Cancel
                </Button>
                <Button
                  onClick={handleCreateNetwork}
                  disabled={!networkName.trim() || createNetwork.isPending}
                >
                  {createNetwork.isPending ? 'Creating...' : 'Create'}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
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
