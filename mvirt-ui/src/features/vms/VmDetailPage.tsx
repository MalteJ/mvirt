import { useParams, useNavigate } from 'react-router-dom'
import { ArrowLeft, Play, Square, Terminal, Cpu, HardDrive, Network } from 'lucide-react'
import { useVm, useStartVm, useStopVm } from '@/hooks/queries'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { StateIndicator } from '@/components/data-display/StateIndicator'
import { VmConsole } from './VmConsole'
import { truncateId, formatBytes } from '@/lib/utils'
import { VmState } from '@/types'

export function VmDetailPage() {
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()
  const { data: vm, isLoading, isError, error } = useVm(id!)
  const startVm = useStartVm()
  const stopVm = useStopVm()

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    )
  }

  if (isError || !vm) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-destructive">
          {error ? `Failed to load VM: ${error.message}` : 'VM not found'}
        </div>
      </div>
    )
  }

  const canStart = vm.state === VmState.STOPPED
  const canStop = vm.state === VmState.RUNNING

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('../vms')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex-1">
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-bold tracking-tight">{vm.name}</h2>
            <StateIndicator state={vm.state} />
          </div>
          <p className="text-sm text-muted-foreground font-mono">{vm.id}</p>
        </div>
        <div className="flex gap-2">
          {canStart && (
            <Button onClick={() => startVm.mutate(vm.id)}>
              <Play className="mr-2 h-4 w-4" />
              Start
            </Button>
          )}
          {canStop && (
            <Button variant="secondary" onClick={() => stopVm.mutate(vm.id)}>
              <Square className="mr-2 h-4 w-4" />
              Stop
            </Button>
          )}
        </div>
      </div>

      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="console" disabled={vm.state !== VmState.RUNNING}>
            <Terminal className="mr-2 h-4 w-4" />
            Console
          </TabsTrigger>
          <TabsTrigger value="hardware">Hardware</TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="space-y-4">
          <div className="grid gap-4 md:grid-cols-3">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">vCPUs</CardTitle>
                <Cpu className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{vm.config.vcpus}</div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">Memory</CardTitle>
                <HardDrive className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{formatBytes(vm.config.memoryMb * 1024 * 1024)}</div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">NIC</CardTitle>
                <Network className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="font-mono text-xs">{truncateId(vm.config.nicId)}</div>
              </CardContent>
            </Card>
          </div>

          <Card>
            <CardHeader>
              <CardTitle>Details</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-2 gap-4 text-sm">
                <div>
                  <dt className="text-muted-foreground">ID</dt>
                  <dd className="font-mono">{vm.id}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Created</dt>
                  <dd>{new Date(vm.createdAt).toLocaleString()}</dd>
                </div>
                {vm.startedAt && (
                  <div>
                    <dt className="text-muted-foreground">Started</dt>
                    <dd>{new Date(vm.startedAt).toLocaleString()}</dd>
                  </div>
                )}
                <div>
                  <dt className="text-muted-foreground">Image</dt>
                  <dd>{vm.config.image}</dd>
                </div>
                {vm.nodeId && (
                  <div>
                    <dt className="text-muted-foreground">Node</dt>
                    <dd className="font-mono text-xs">{truncateId(vm.nodeId)}</dd>
                  </div>
                )}
                {vm.ipAddress && (
                  <div>
                    <dt className="text-muted-foreground">IP Address</dt>
                    <dd className="font-mono">{vm.ipAddress}</dd>
                  </div>
                )}
              </dl>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="console">
          {vm.state === VmState.RUNNING && <VmConsole vmId={vm.id} />}
        </TabsContent>

        <TabsContent value="hardware" className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle>Storage</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-2 gap-4 text-sm">
                <div>
                  <dt className="text-muted-foreground">Volume ID</dt>
                  <dd className="font-mono text-xs">{truncateId(vm.config.volumeId)}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Image</dt>
                  <dd>{vm.config.image}</dd>
                </div>
              </dl>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Network</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-2 gap-4 text-sm">
                <div>
                  <dt className="text-muted-foreground">NIC ID</dt>
                  <dd className="font-mono text-xs">{truncateId(vm.config.nicId)}</dd>
                </div>
                {vm.ipAddress && (
                  <div>
                    <dt className="text-muted-foreground">IP Address</dt>
                    <dd className="font-mono">{vm.ipAddress}</dd>
                  </div>
                )}
              </dl>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  )
}
