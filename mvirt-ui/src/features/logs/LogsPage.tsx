import { useState } from 'react'
import { ColumnDef } from '@tanstack/react-table'
import { RefreshCw } from 'lucide-react'
import { useLogs } from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { truncateId } from '@/lib/utils'
import { LogEntry, LogLevel } from '@/types'

const levelVariants: Record<LogLevel, 'default' | 'secondary' | 'destructive' | 'outline' | 'running'> = {
  [LogLevel.DEBUG]: 'outline',
  [LogLevel.INFO]: 'secondary',
  [LogLevel.WARN]: 'default',
  [LogLevel.ERROR]: 'destructive',
  [LogLevel.AUDIT]: 'running',
}

export function LogsPage() {
  const [levelFilter, setLevelFilter] = useState<LogLevel | 'all'>('all')
  const [componentFilter, setComponentFilter] = useState<string>('all')

  const { data: logs, isLoading, refetch } = useLogs({
    level: levelFilter === 'all' ? undefined : levelFilter,
    component: componentFilter === 'all' ? undefined : componentFilter,
    limit: 100,
  })

  const columns: ColumnDef<LogEntry>[] = [
    {
      accessorKey: 'timestampNs',
      header: 'Time',
      cell: ({ row }) => {
        const date = new Date(row.original.timestampNs / 1_000_000)
        return (
          <span className="font-mono text-xs text-muted-foreground">
            {date.toLocaleTimeString()}
          </span>
        )
      },
    },
    {
      accessorKey: 'level',
      header: 'Level',
      cell: ({ row }) => (
        <Badge variant={levelVariants[row.original.level]}>
          {row.original.level}
        </Badge>
      ),
    },
    {
      accessorKey: 'component',
      header: 'Component',
      cell: ({ row }) => (
        <span className="font-mono text-sm">{row.original.component}</span>
      ),
    },
    {
      accessorKey: 'message',
      header: 'Message',
      cell: ({ row }) => (
        <span className="text-sm">{row.original.message}</span>
      ),
    },
    {
      accessorKey: 'relatedObjectIds',
      header: 'Objects',
      cell: ({ row }) => (
        <div className="flex gap-1 flex-wrap">
          {row.original.relatedObjectIds.slice(0, 2).map((id) => (
            <Badge key={id} variant="outline" className="font-mono text-xs">
              {truncateId(id)}
            </Badge>
          ))}
          {row.original.relatedObjectIds.length > 2 && (
            <Badge variant="outline" className="text-xs">
              +{row.original.relatedObjectIds.length - 2}
            </Badge>
          )}
        </div>
      ),
    },
  ]

  const components = ['all', 'vmm', 'zfs', 'net', 'cli']

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Logs</h2>
          <p className="text-muted-foreground">
            View and filter system logs
          </p>
        </div>
        <Button variant="outline" onClick={() => refetch()}>
          <RefreshCw className="mr-2 h-4 w-4" />
          Refresh
        </Button>
      </div>

      <div className="flex gap-4">
        <div className="w-40">
          <Select value={levelFilter} onValueChange={(v) => setLevelFilter(v as LogLevel | 'all')}>
            <SelectTrigger>
              <SelectValue placeholder="Level" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All Levels</SelectItem>
              {Object.values(LogLevel).map((level) => (
                <SelectItem key={level} value={level}>
                  {level}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="w-40">
          <Select value={componentFilter} onValueChange={setComponentFilter}>
            <SelectTrigger>
              <SelectValue placeholder="Component" />
            </SelectTrigger>
            <SelectContent>
              {components.map((comp) => (
                <SelectItem key={comp} value={comp}>
                  {comp === 'all' ? 'All Components' : comp}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      {isLoading ? (
        <div className="flex items-center justify-center h-64">
          <p className="text-muted-foreground">Loading...</p>
        </div>
      ) : (
        <DataTable columns={columns} data={logs || []} />
      )}
    </div>
  )
}
