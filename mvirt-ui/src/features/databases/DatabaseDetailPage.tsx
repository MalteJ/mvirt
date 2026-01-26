import { useParams, useNavigate } from 'react-router-dom'
import { ArrowLeft, Play, Square, Database as DatabaseIcon, Network, HardDrive, Users } from 'lucide-react'
import { useDatabase, useStartDatabase, useStopDatabase } from '@/hooks/queries'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { StateIndicator } from '@/components/data-display/StateIndicator'
import { DatabaseState, DatabaseType } from '@/types'

const typeLabels: Record<DatabaseType, string> = {
  [DatabaseType.POSTGRESQL]: 'PostgreSQL',
  [DatabaseType.REDIS]: 'Redis',
}

const typeColors: Record<DatabaseType, string> = {
  [DatabaseType.POSTGRESQL]: 'bg-blue-500/20 text-blue-400 border-blue-500/30',
  [DatabaseType.REDIS]: 'bg-red-500/20 text-red-400 border-red-500/30',
}

export function DatabaseDetailPage() {
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()
  const { data: db, isLoading } = useDatabase(id!)
  const startDatabase = useStartDatabase()
  const stopDatabase = useStopDatabase()

  if (isLoading || !db) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    )
  }

  const canStart = db.state === DatabaseState.STOPPED
  const canStop = db.state === DatabaseState.RUNNING

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('/databases')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex-1">
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-bold tracking-tight">{db.name}</h2>
            <Badge variant="outline" className={typeColors[db.type]}>
              {typeLabels[db.type]} {db.version}
            </Badge>
            <StateIndicator state={db.state} />
          </div>
          <p className="text-sm text-muted-foreground font-mono">{db.id}</p>
        </div>
        <div className="flex gap-2">
          {canStart && (
            <Button onClick={() => startDatabase.mutate(db.id)}>
              <Play className="mr-2 h-4 w-4" />
              Start
            </Button>
          )}
          {canStop && (
            <Button variant="secondary" onClick={() => stopDatabase.mutate(db.id)}>
              <Square className="mr-2 h-4 w-4" />
              Stop
            </Button>
          )}
        </div>
      </div>

      <div className="grid gap-4 md:grid-cols-4">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">Endpoint</CardTitle>
            <Network className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-lg font-bold font-mono">
              {db.host && db.port ? `${db.host}:${db.port}` : '—'}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">Storage</CardTitle>
            <HardDrive className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {db.usedStorageGb} / {db.storageSizeGb} GB
            </div>
            <div className="mt-2 h-1 bg-muted rounded-full overflow-hidden">
              <div
                className="h-full bg-purple"
                style={{ width: `${(db.usedStorageGb / db.storageSizeGb) * 100}%` }}
              />
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">Connections</CardTitle>
            <Users className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {db.connections} / {db.maxConnections}
            </div>
            <div className="mt-2 h-1 bg-muted rounded-full overflow-hidden">
              <div
                className="h-full bg-purple"
                style={{ width: `${(db.connections / db.maxConnections) * 100}%` }}
              />
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">Username</CardTitle>
            <DatabaseIcon className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-lg font-bold font-mono">{db.username}</div>
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
              <dd className="font-mono">{db.id}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Type</dt>
              <dd>{typeLabels[db.type]} {db.version}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Created</dt>
              <dd>{new Date(db.createdAt).toLocaleString()}</dd>
            </div>
            {db.startedAt && (
              <div>
                <dt className="text-muted-foreground">Started</dt>
                <dd>{new Date(db.startedAt).toLocaleString()}</dd>
              </div>
            )}
            {db.errorMessage && (
              <div className="col-span-2">
                <dt className="text-muted-foreground">Error</dt>
                <dd className="text-destructive">{db.errorMessage}</dd>
              </div>
            )}
          </dl>
        </CardContent>
      </Card>

      {db.state === DatabaseState.RUNNING && (
        <Card>
          <CardHeader>
            <CardTitle>Connection String</CardTitle>
          </CardHeader>
          <CardContent>
            <code className="block p-3 rounded-md bg-muted font-mono text-sm break-all">
              {db.type === DatabaseType.POSTGRESQL &&
                `postgresql://${db.username}:<password>@${db.host}:${db.port}/postgres`}
              {db.type === DatabaseType.REDIS &&
                `redis://${db.username}:<password>@${db.host}:${db.port}`}
            </code>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
