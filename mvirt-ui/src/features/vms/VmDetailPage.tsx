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
  const { data: vm, isLoading } = useVm(id!)
  const startVm = useStartVm()
  const stopVm = useStopVm()

  if (isLoading || !vm) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    )
  }

  const canStart = vm.state === VmState.STOPPED
  const canStop = vm.state === VmState.RUNNING

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('/vms')}>
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
                <CardTitle className="text-sm font-medium">NICs</CardTitle>
                <Network className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{vm.config.nics.length}</div>
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
                {vm.config.bootDisk && (
                  <div>
                    <dt className="text-muted-foreground">Boot Disk</dt>
                    <dd className="font-mono text-xs">{truncateId(vm.config.bootDisk, 16)}</dd>
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
              <CardTitle>Disks</CardTitle>
            </CardHeader>
            <CardContent>
              {vm.config.disks.length === 0 ? (
                <p className="text-muted-foreground text-sm">No disks attached</p>
              ) : (
                <ul className="space-y-2">
                  {vm.config.disks.map((disk, i) => (
                    <li key={i} className="flex items-center justify-between text-sm">
                      <span className="font-mono">{disk.path}</span>
                      <span className="text-muted-foreground">
                        {disk.readonly ? 'Read-only' : 'Read-write'}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Network Interfaces</CardTitle>
            </CardHeader>
            <CardContent>
              {vm.config.nics.length === 0 ? (
                <p className="text-muted-foreground text-sm">No NICs attached</p>
              ) : (
                <ul className="space-y-2">
                  {vm.config.nics.map((nic, i) => (
                    <li key={i} className="flex items-center justify-between text-sm">
                      <span className="font-mono">{nic.macAddress}</span>
                      <span className="text-muted-foreground">
                        Network: {truncateId(nic.networkId)}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  )
}
