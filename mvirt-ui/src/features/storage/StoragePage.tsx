import { useState, useEffect } from 'react'
import { ColumnDef } from '@tanstack/react-table'
import { MoreHorizontal, Plus, Trash2, Copy, Download, CheckCircle2, AlertCircle, Loader2 } from 'lucide-react'
import { useVolumes, useTemplates, useDeleteVolume, usePoolStats, useImportTemplate, useImportJob, useCreateVolume } from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { StatCard } from '@/components/data-display/StatCard'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Card, CardContent } from '@/components/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
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
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useProjectId } from '@/hooks/useProjectId'
import { truncateId, formatBytes } from '@/lib/utils'
import { Volume, Template, ImportJobState } from '@/types'

export function StoragePage() {
  const projectId = useProjectId()
  const { data: volumes, isLoading: loadingVolumes } = useVolumes(projectId)
  const { data: templates, isLoading: loadingTemplates } = useTemplates(projectId)
  const { data: poolStats } = usePoolStats()
  const deleteVolume = useDeleteVolume()
  const createVolume = useCreateVolume(projectId)
  const importTemplate = useImportTemplate(projectId)
  const [activeTab, setActiveTab] = useState('volumes')

  // Import dialog state
  const [importDialogOpen, setImportDialogOpen] = useState(false)
  const [templateName, setTemplateName] = useState('')
  const [templateUrl, setTemplateUrl] = useState('')
  const [importJobId, setImportJobId] = useState('')

  const { data: importJob } = useImportJob(importJobId)

  const importDone = importJob?.state === ImportJobState.COMPLETED
  const importFailed = importJob?.state === ImportJobState.FAILED
  const importRunning = !!importJobId && !importDone && !importFailed

  // Switch to templates tab when import completes
  useEffect(() => {
    if (importDone) {
      setActiveTab('templates')
    }
  }, [importDone])

  const handleImport = () => {
    if (!templateName.trim() || !templateUrl.trim()) return
    importTemplate.mutate(
      { name: templateName.trim(), url: templateUrl.trim() },
      {
        onSuccess: (job) => {
          setImportJobId(job.id)
        },
      }
    )
  }

  const handleCloseImportDialog = () => {
    setImportDialogOpen(false)
    setTemplateName('')
    setTemplateUrl('')
    setImportJobId('')
  }

  const importProgress = importJob && importJob.totalBytes > 0
    ? Math.round((importJob.bytesWritten / importJob.totalBytes) * 100)
    : 0

  // Create volume dialog state
  const [volumeDialogOpen, setVolumeDialogOpen] = useState(false)
  const [volumeName, setVolumeName] = useState('')
  const [volumeSize, setVolumeSize] = useState('10')
  const [volumeSizeUnit, setVolumeSizeUnit] = useState('GB')
  const [volumeTemplateId, setVolumeTemplateId] = useState('')

  const handleCreateVolume = () => {
    if (!volumeName.trim()) return
    const multiplier = volumeSizeUnit === 'TB' ? 1024 * 1024 * 1024 * 1024
      : volumeSizeUnit === 'GB' ? 1024 * 1024 * 1024
      : 1024 * 1024
    createVolume.mutate(
      {
        name: volumeName.trim(),
        sizeBytes: parseInt(volumeSize) * multiplier,
        templateId: volumeTemplateId || undefined,
      },
      {
        onSuccess: () => {
          setVolumeDialogOpen(false)
          setVolumeName('')
          setVolumeSize('10')
          setVolumeSizeUnit('GB')
          setVolumeTemplateId('')
        },
      }
    )
  }

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
          <Dialog open={importDialogOpen} onOpenChange={(open) => {
            if (!open && !importRunning) handleCloseImportDialog()
            else setImportDialogOpen(open)
          }}>
            <DialogTrigger asChild>
              <Button variant="outline">
                <Download className="mr-2 h-4 w-4" />
                Import Template
              </Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Import Template</DialogTitle>
                <DialogDescription>
                  Import a disk image from a URL to use as a VM template.
                </DialogDescription>
              </DialogHeader>

              {!importJobId ? (
                <>
                  <div className="grid gap-4 py-4">
                    <div className="grid gap-2">
                      <Label htmlFor="template-name">Template Name</Label>
                      <Input
                        id="template-name"
                        placeholder="ubuntu-24.04"
                        value={templateName}
                        onChange={(e) => setTemplateName(e.target.value)}
                      />
                    </div>
                    <div className="grid gap-2">
                      <Label htmlFor="template-url">Image URL</Label>
                      <Input
                        id="template-url"
                        placeholder="https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img"
                        className="font-mono text-xs"
                        value={templateUrl}
                        onChange={(e) => setTemplateUrl(e.target.value)}
                      />
                      <p className="text-xs text-muted-foreground">
                        Supports raw and qcow2 disk images
                      </p>
                    </div>
                  </div>
                  <DialogFooter>
                    <Button variant="outline" onClick={handleCloseImportDialog}>
                      Cancel
                    </Button>
                    <Button
                      onClick={handleImport}
                      disabled={!templateName.trim() || !templateUrl.trim() || importTemplate.isPending}
                    >
                      {importTemplate.isPending ? 'Starting...' : 'Import'}
                    </Button>
                  </DialogFooter>
                </>
              ) : (
                <div className="py-4 space-y-4">
                  <div className="flex items-center gap-3">
                    {importFailed ? (
                      <AlertCircle className="h-5 w-5 text-destructive shrink-0" />
                    ) : importDone ? (
                      <CheckCircle2 className="h-5 w-5 text-state-running shrink-0" />
                    ) : (
                      <Loader2 className="h-5 w-5 animate-spin text-purple shrink-0" />
                    )}
                    <div className="flex-1 min-w-0">
                      <div className="font-medium">
                        {importFailed ? 'Import failed' : importDone ? 'Import complete' : 'Importing...'}
                      </div>
                      <div className="text-sm text-muted-foreground font-mono truncate">
                        {importJob?.templateName}
                      </div>
                    </div>
                  </div>

                  {!importFailed && (
                    <div className="space-y-1.5">
                      <div className="flex justify-between text-xs text-muted-foreground">
                        <span>{formatBytes(importJob?.bytesWritten ?? 0)}</span>
                        <span>{importJob?.totalBytes ? formatBytes(importJob.totalBytes) : '...'}</span>
                      </div>
                      <div className="h-2 rounded-full bg-secondary overflow-hidden">
                        <div
                          className="h-full rounded-full bg-purple transition-all duration-300"
                          style={{ width: `${importDone ? 100 : importProgress}%` }}
                        />
                      </div>
                    </div>
                  )}

                  {importFailed && importJob?.error && (
                    <p className="text-sm text-destructive">{importJob.error}</p>
                  )}

                  <DialogFooter>
                    <Button
                      onClick={handleCloseImportDialog}
                      disabled={importRunning}
                      variant={importDone ? 'default' : 'outline'}
                    >
                      {importDone ? 'Done' : 'Close'}
                    </Button>
                  </DialogFooter>
                </div>
              )}
            </DialogContent>
          </Dialog>

          <Dialog open={volumeDialogOpen} onOpenChange={setVolumeDialogOpen}>
            <DialogTrigger asChild>
              <Button>
                <Plus className="mr-2 h-4 w-4" />
                Create Volume
              </Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Create Volume</DialogTitle>
                <DialogDescription>
                  Create a new storage volume for your VMs.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="volume-name">Name</Label>
                  <Input
                    id="volume-name"
                    placeholder="my-volume"
                    value={volumeName}
                    onChange={(e) => setVolumeName(e.target.value)}
                  />
                </div>
                <div className="grid gap-2">
                  <Label>Size</Label>
                  <div className="flex gap-2">
                    <Input
                      type="number"
                      min="1"
                      value={volumeSize}
                      onChange={(e) => setVolumeSize(e.target.value)}
                      className="font-mono"
                    />
                    <Select value={volumeSizeUnit} onValueChange={setVolumeSizeUnit}>
                      <SelectTrigger className="w-24">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="MB">MB</SelectItem>
                        <SelectItem value="GB">GB</SelectItem>
                        <SelectItem value="TB">TB</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                </div>
                <div className="grid gap-2">
                  <Label>Template (optional)</Label>
                  <Select
                    value={volumeTemplateId || 'none'}
                    onValueChange={(v) => setVolumeTemplateId(v === 'none' ? '' : v)}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="No template" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="none">None (empty volume)</SelectItem>
                      {templates?.map((t) => (
                        <SelectItem key={t.id} value={t.id}>
                          {t.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground">
                    Clone from an existing template to pre-populate the volume
                  </p>
                </div>
              </div>
              <DialogFooter>
                <Button variant="outline" onClick={() => setVolumeDialogOpen(false)}>
                  Cancel
                </Button>
                <Button
                  onClick={handleCreateVolume}
                  disabled={!volumeName.trim() || !volumeSize || createVolume.isPending}
                >
                  {createVolume.isPending ? 'Creating...' : 'Create'}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
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
