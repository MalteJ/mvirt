import { useNavigate } from 'react-router-dom'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Play, Square, Trash2, Plus } from 'lucide-react'
import { useDatabases, useStartDatabase, useStopDatabase, useDeleteDatabase } from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { StateIndicator } from '@/components/data-display/StateIndicator'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { truncateId } from '@/lib/utils'
import { Database, DatabaseState, DatabaseType } from '@/types'

const typeLabels: Record<DatabaseType, string> = {
  [DatabaseType.POSTGRESQL]: 'PostgreSQL',
  [DatabaseType.REDIS]: 'Redis',
}

const typeColors: Record<DatabaseType, string> = {
  [DatabaseType.POSTGRESQL]: 'bg-blue-500/20 text-blue-400 border-blue-500/30',
  [DatabaseType.REDIS]: 'bg-red-500/20 text-red-400 border-red-500/30',
}

export function DatabasesPage() {
  const navigate = useNavigate()
  const { data: databases, isLoading } = useDatabases()
  const startDatabase = useStartDatabase()
  const stopDatabase = useStopDatabase()
  const deleteDatabase = useDeleteDatabase()

  const columns: ColumnDef<Database>[] = [
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
      accessorKey: 'type',
      header: 'Type',
      cell: ({ row }) => (
        <Badge variant="outline" className={typeColors[row.original.type]}>
          {typeLabels[row.original.type]}
        </Badge>
      ),
    },
    {
      accessorKey: 'state',
      header: 'State',
      cell: ({ row }) => <StateIndicator state={row.original.state} />,
    },
    {
      accessorKey: 'version',
      header: 'Version',
      cell: ({ row }) => (
        <span className="font-mono text-sm">{row.original.version}</span>
      ),
    },
    {
      accessorKey: 'host',
      header: 'Endpoint',
      cell: ({ row }) => (
        <span className="font-mono text-sm">
          {row.original.host && row.original.port
            ? `${row.original.host}:${row.original.port}`
            : '—'}
        </span>
      ),
    },
    {
      accessorKey: 'connections',
      header: 'Connections',
      cell: ({ row }) => (
        <span className="text-sm">
          {row.original.connections} / {row.original.maxConnections}
        </span>
      ),
    },
    {
      id: 'actions',
      cell: ({ row }) => {
        const db = row.original
        const canStart = db.state === DatabaseState.STOPPED
        const canStop = db.state === DatabaseState.RUNNING

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
                    startDatabase.mutate(db.id)
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
                    stopDatabase.mutate(db.id)
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
                  if (confirm(`Delete Database "${db.name}"?`)) {
                    deleteDatabase.mutate(db.id)
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
          <h2 className="text-2xl font-bold tracking-tight">Databases</h2>
          <p className="text-muted-foreground">
            Manage your managed database instances
          </p>
        </div>
        <Button onClick={() => navigate('/databases/new')}>
          <Plus className="mr-2 h-4 w-4" />
          Create Database
        </Button>
      </div>
      <DataTable
        columns={columns}
        data={databases || []}
        searchColumn="name"
        searchPlaceholder="Filter databases..."
        onRowClick={(db) => navigate(`/databases/${db.id}`)}
      />
    </div>
  )
}
