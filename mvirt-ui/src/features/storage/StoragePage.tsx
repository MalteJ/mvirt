import { useState } from 'react'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, Copy, Download } from 'lucide-react'
import { useVolumes, useTemplates, useDeleteVolume, usePoolStats } from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { StatCard } from '@/components/data-display/StatCard'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { truncateId, formatBytes } from '@/lib/utils'
import { Volume, Template } from '@/types'

export function StoragePage() {
  const { data: volumes, isLoading: loadingVolumes } = useVolumes()
  const { data: templates, isLoading: loadingTemplates } = useTemplates()
  const { data: poolStats } = usePoolStats()
  const deleteVolume = useDeleteVolume()
  const [activeTab, setActiveTab] = useState('volumes')

  const volumeColumns: ColumnDef<Volume>[] = [
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
      accessorKey: 'volsizeBytes',
      header: 'Size',
      cell: ({ row }) => formatBytes(row.original.volsizeBytes),
    },
    {
      accessorKey: 'usedBytes',
      header: 'Used',
      cell: ({ row }) => formatBytes(row.original.usedBytes),
    },
    {
      accessorKey: 'compressionRatio',
      header: 'Compression',
      cell: ({ row }) => `${row.original.compressionRatio.toFixed(2)}x`,
    },
    {
      accessorKey: 'snapshots',
      header: 'Snapshots',
      cell: ({ row }) => row.original.snapshots.length,
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
            <DropdownMenuItem>
              <Copy className="mr-2 h-4 w-4" />
              Clone
            </DropdownMenuItem>
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => {
                if (confirm(`Delete volume "${row.original.name}"?`)) {
                  deleteVolume.mutate(row.original.id)
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

  const templateColumns: ColumnDef<Template>[] = [
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
      accessorKey: 'sizeBytes',
      header: 'Size',
      cell: ({ row }) => formatBytes(row.original.sizeBytes),
    },
    {
      accessorKey: 'cloneCount',
      header: 'Clones',
      cell: ({ row }) => row.original.cloneCount,
    },
    {
      id: 'actions',
      cell: () => (
        <Button variant="ghost" size="sm">
          <Copy className="mr-2 h-4 w-4" />
          Create Volume
        </Button>
      ),
    },
  ]

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Storage</h2>
          <p className="text-muted-foreground">
            Manage volumes and templates
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">
            <Download className="mr-2 h-4 w-4" />
            Import Template
          </Button>
          <Button>
            <Plus className="mr-2 h-4 w-4" />
            Create Volume
          </Button>
        </div>
      </div>

      {poolStats && (
        <div className="grid gap-4 md:grid-cols-3">
          <StatCard
            title="Total Capacity"
            value={formatBytes(poolStats.totalBytes)}
          />
          <StatCard
            title="Used"
            value={formatBytes(poolStats.usedBytes)}
            description={`${((poolStats.usedBytes / poolStats.totalBytes) * 100).toFixed(1)}%`}
          />
          <StatCard
            title="Free"
            value={formatBytes(poolStats.freeBytes)}
          />
        </div>
      )}

      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList>
          <TabsTrigger value="volumes">Volumes ({volumes?.length ?? 0})</TabsTrigger>
          <TabsTrigger value="templates">Templates ({templates?.length ?? 0})</TabsTrigger>
        </TabsList>

        <TabsContent value="volumes">
          {loadingVolumes ? (
            <Card>
              <CardContent className="flex items-center justify-center h-32">
                <p className="text-muted-foreground">Loading...</p>
              </CardContent>
            </Card>
          ) : (
            <DataTable
              columns={volumeColumns}
              data={volumes || []}
              searchColumn="name"
              searchPlaceholder="Filter volumes..."
            />
          )}
        </TabsContent>

        <TabsContent value="templates">
          {loadingTemplates ? (
            <Card>
              <CardContent className="flex items-center justify-center h-32">
                <p className="text-muted-foreground">Loading...</p>
              </CardContent>
            </Card>
          ) : (
            <DataTable
              columns={templateColumns}
              data={templates || []}
              searchColumn="name"
              searchPlaceholder="Filter templates..."
            />
          )}
        </TabsContent>
      </Tabs>
    </div>
  )
}
