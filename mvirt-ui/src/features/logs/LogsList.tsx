import { useMemo, useState } from 'react'
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

const levelVariants: Record<
  string,
  'default' | 'secondary' | 'destructive' | 'outline' | 'running'
> = {
  Debug: 'outline',
  Info: 'secondary',
  Notice: 'secondary',
  Warn: 'default',
  Error: 'destructive',
  Critical: 'destructive',
  Alert: 'destructive',
  Emergency: 'destructive',
  Audit: 'running',
}

interface LogsListProps {
  /// If set, the backend pre-filters logs to this object_id (e.g. a node id).
  /// Without it, the list shows everything mvirt-log holds.
  objectId?: string
  /// Optional fixed component values to populate the filter dropdown.
  /// Defaults to the standard component set.
  components?: string[]
  /// How many entries to fetch from the backend at a time.
  limit?: number
}

export function LogsList({
  objectId,
  components = ['all', 'api', 'vmm', 'zfs', 'ebpf', 'net', 'shipper'],
  limit = 200,
}: LogsListProps) {
  const [levelFilter, setLevelFilter] = useState<string>('all')
  const [componentFilter, setComponentFilter] = useState<string>('all')

  const { data: logs, isLoading, refetch } = useLogs({ objectId, limit })

  // Level + component filters are applied client-side: the backend's
  // QueryRequest schema only supports object_id and limit today, and adding
  // more would mean teaching mvirt-log's redb storage about secondary
  // indexes. Filtering a 200-entry list in JS is fast.
  const filtered = useMemo(() => {
    let rows: LogEntry[] = logs ?? []
    if (levelFilter !== 'all') {
      rows = rows.filter((r) => r.level === levelFilter)
    }
    if (componentFilter !== 'all') {
      rows = rows.filter((r) => r.component === componentFilter)
    }
    return rows
  }, [logs, levelFilter, componentFilter])

  const columns: ColumnDef<LogEntry>[] = [
    {
      accessorKey: 'timestamp',
      header: 'Time',
      cell: ({ row }) => {
        const date = new Date(row.original.timestamp)
        const hms = date.toLocaleTimeString(undefined, {
          hour: '2-digit',
          minute: '2-digit',
          second: '2-digit',
          hour12: false,
        })
        const ms = date.getMilliseconds().toString().padStart(3, '0')
        return (
          <span className="font-mono text-xs text-muted-foreground">
            {hms}.{ms}
          </span>
        )
      },
    },
    {
      accessorKey: 'level',
      header: 'Level',
      cell: ({ row }) => (
        <Badge variant={levelVariants[row.original.level] ?? 'secondary'}>
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

  return (
    <div className="space-y-4">
      <div className="flex gap-4 items-center">
        <div className="w-40">
          <Select value={levelFilter} onValueChange={setLevelFilter}>
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
        <Button variant="outline" size="sm" onClick={() => refetch()}>
          <RefreshCw className="mr-2 h-4 w-4" />
          Refresh
        </Button>
      </div>

      {isLoading ? (
        <div className="flex items-center justify-center h-32">
          <p className="text-muted-foreground">Loading...</p>
        </div>
      ) : (
        <DataTable
          columns={columns}
          data={filtered}
          compact
          defaultSorting={[{ id: 'timestamp', desc: true }]}
        />
      )}
    </div>
  )
}
