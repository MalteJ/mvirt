import { useParams, useNavigate } from 'react-router-dom'
import { ArrowLeft, Play, Square, Box, Network, Server } from 'lucide-react'
import { usePod, useStartPod, useStopPod } from '@/hooks/queries'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { StateIndicator } from '@/components/data-display/StateIndicator'
import { truncateId } from '@/lib/utils'
import { PodState } from '@/types'

export function PodDetailPage() {
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()
  const { data: pod, isLoading } = usePod(id!)
  const startPod = useStartPod()
  const stopPod = useStopPod()

  if (isLoading || !pod) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    )
  }

  const canStart = pod.state === PodState.STOPPED || pod.state === PodState.CREATED
  const canStop = pod.state === PodState.RUNNING

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('/containers')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex-1">
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-bold tracking-tight">{pod.name}</h2>
            <StateIndicator state={pod.state} />
          </div>
          <p className="text-sm text-muted-foreground font-mono">{pod.id}</p>
        </div>
        <div className="flex gap-2">
          {canStart && (
            <Button onClick={() => startPod.mutate(pod.id)}>
              <Play className="mr-2 h-4 w-4" />
              Start
            </Button>
          )}
          {canStop && (
            <Button variant="secondary" onClick={() => stopPod.mutate(pod.id)}>
              <Square className="mr-2 h-4 w-4" />
              Stop
            </Button>
          )}
        </div>
      </div>

      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="containers">
            <Box className="mr-2 h-4 w-4" />
            Containers ({pod.containers.length})
          </TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="space-y-4">
          <div className="grid gap-4 md:grid-cols-3">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">Containers</CardTitle>
                <Box className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{pod.containers.length}</div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">IP Address</CardTitle>
                <Network className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold font-mono">
                  {pod.ipAddress || '—'}
                </div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">VM</CardTitle>
                <Server className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-lg font-mono">
                  {pod.vmId ? truncateId(pod.vmId) : '—'}
                </div>
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
                  <dd className="font-mono">{pod.id}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Created</dt>
                  <dd>{new Date(pod.createdAt).toLocaleString()}</dd>
                </div>
                {pod.startedAt && (
                  <div>
                    <dt className="text-muted-foreground">Started</dt>
                    <dd>{new Date(pod.startedAt).toLocaleString()}</dd>
                  </div>
                )}
                {pod.errorMessage && (
                  <div className="col-span-2">
                    <dt className="text-muted-foreground">Error</dt>
                    <dd className="text-destructive">{pod.errorMessage}</dd>
                  </div>
                )}
              </dl>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="containers" className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle>Containers</CardTitle>
            </CardHeader>
            <CardContent>
              {pod.containers.length === 0 ? (
                <p className="text-muted-foreground text-sm">No containers</p>
              ) : (
                <div className="space-y-3">
                  {pod.containers.map((container) => (
                    <div
                      key={container.id}
                      className="flex items-center justify-between rounded-lg border border-border p-4"
                    >
                      <div className="space-y-1">
                        <div className="flex items-center gap-2">
                          <span className="font-medium">{container.name}</span>
                          <StateIndicator state={container.state} />
                        </div>
                        <div className="text-sm text-muted-foreground font-mono">
                          {container.image}
                        </div>
                        <div className="text-xs text-muted-foreground font-mono">
                          {truncateId(container.id)}
                        </div>
                      </div>
                      <div className="text-right text-sm">
                        {container.exitCode !== undefined && container.exitCode !== null && (
                          <div className="text-muted-foreground">
                            Exit code: <span className="font-mono">{container.exitCode}</span>
                          </div>
                        )}
                        {container.errorMessage && (
                          <div className="text-destructive text-xs">
                            {container.errorMessage}
                          </div>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  )
}
