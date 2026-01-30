import { useParams, useNavigate } from 'react-router-dom'
import { ArrowLeft, Cpu, HardDrive, Database } from 'lucide-react'
import { useNode } from '@/hooks/queries'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Badge } from '@/components/ui/badge'
import { NodeStatus } from '@/types'

function formatMb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`
  return `${mb} MB`
}

export function NodeDetailPage() {
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()
  const { data: node, isLoading, isError, error } = useNode(id!)

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    )
  }

  if (isError || !node) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-destructive">
          {error ? `Failed to load node: ${error.message}` : 'Node not found'}
        </div>
      </div>
    )
  }

  const r = node.resources
  const labels = Object.entries(node.labels || {})

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('/cluster')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex-1">
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-bold tracking-tight">{node.name}</h2>
            <Badge variant={node.status === NodeStatus.ONLINE ? 'running' : 'error'}>
              {node.status}
            </Badge>
          </div>
          <p className="text-sm text-muted-foreground font-mono">{node.id}</p>
        </div>
      </div>

      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="hardware">Hardware</TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="space-y-4">
          <div className="grid gap-4 md:grid-cols-3">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">CPUs</CardTitle>
                <Cpu className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{r.available_cpu_cores}/{r.cpu_cores}</div>
                <p className="text-xs text-muted-foreground">available / total</p>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">Memory</CardTitle>
                <HardDrive className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{formatMb(r.available_memory_mb)}/{formatMb(r.memory_mb)}</div>
                <p className="text-xs text-muted-foreground">available / total</p>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">Storage</CardTitle>
                <Database className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{r.available_storage_gb}/{r.storage_gb} GB</div>
                <p className="text-xs text-muted-foreground">available / total</p>
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
                  <dd className="font-mono text-xs">{node.id}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Name</dt>
                  <dd className="font-medium">{node.name}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Address</dt>
                  <dd className="font-mono">{node.address}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Status</dt>
                  <dd>
                    <Badge variant={node.status === NodeStatus.ONLINE ? 'running' : 'error'}>
                      {node.status}
                    </Badge>
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Last Heartbeat</dt>
                  <dd>{new Date(node.last_heartbeat).toLocaleString()}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Created</dt>
                  <dd>{new Date(node.created_at).toLocaleString()}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Updated</dt>
                  <dd>{new Date(node.updated_at).toLocaleString()}</dd>
                </div>
              </dl>
            </CardContent>
          </Card>

          {labels.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle>Labels</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="flex flex-wrap gap-2">
                  {labels.map(([key, value]) => (
                    <Badge key={key} variant="secondary">
                      {key}={value}
                    </Badge>
                  ))}
                </div>
              </CardContent>
            </Card>
          )}
        </TabsContent>

        <TabsContent value="hardware" className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle>Compute</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-2 gap-4 text-sm">
                <div>
                  <dt className="text-muted-foreground">Total CPU Cores</dt>
                  <dd className="font-mono">{r.cpu_cores}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Available CPU Cores</dt>
                  <dd className="font-mono">{r.available_cpu_cores}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Total Memory</dt>
                  <dd className="font-mono">{formatMb(r.memory_mb)}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Available Memory</dt>
                  <dd className="font-mono">{formatMb(r.available_memory_mb)}</dd>
                </div>
              </dl>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Storage</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-2 gap-4 text-sm">
                <div>
                  <dt className="text-muted-foreground">Total Storage</dt>
                  <dd className="font-mono">{r.storage_gb} GB</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Available Storage</dt>
                  <dd className="font-mono">{r.available_storage_gb} GB</dd>
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
                  <dt className="text-muted-foreground">Address</dt>
                  <dd className="font-mono">{node.address}</dd>
                </div>
              </dl>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  )
}
