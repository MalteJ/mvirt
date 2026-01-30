import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { ArrowLeft } from 'lucide-react'
import { useCreateVm, useVolumes, useNics, useTemplates } from '@/hooks/queries'
import { useProjectId } from '@/hooks/useProjectId'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { formatBytes } from '@/lib/utils'

export function CreateVmPage() {
  const navigate = useNavigate()
  const projectId = useProjectId()
  const createVm = useCreateVm(projectId)
  const { data: volumes } = useVolumes(projectId)
  const { data: nics } = useNics(projectId)
  const { data: templates } = useTemplates(projectId)

  const [name, setName] = useState('')
  const [vcpus, setVcpus] = useState('2')
  const [memoryMb, setMemoryMb] = useState('2048')
  const [volumeId, setVolumeId] = useState('')
  const [nicId, setNicId] = useState('')
  const [image, setImage] = useState('')

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!name || !volumeId || !nicId || !image) return
    createVm.mutate(
      {
        name,
        config: {
          vcpus: parseInt(vcpus),
          memoryMb: parseInt(memoryMb),
          volumeId,
          nicId,
          image,
        },
      },
      {
        onSuccess: (vm) => {
          navigate(`../vms/${vm.id}`)
        },
      }
    )
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('../vms')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Create Virtual Machine</h2>
          <p className="text-muted-foreground">Configure your new VM</p>
        </div>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>VM Configuration</CardTitle>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="space-y-6">
            <div className="space-y-2">
              <Label htmlFor="name">Name</Label>
              <Input
                id="name"
                placeholder="my-vm"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label>vCPUs</Label>
                <Select value={vcpus} onValueChange={setVcpus}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="1">1 vCPU</SelectItem>
                    <SelectItem value="2">2 vCPUs</SelectItem>
                    <SelectItem value="4">4 vCPUs</SelectItem>
                    <SelectItem value="8">8 vCPUs</SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div className="space-y-2">
                <Label>Memory</Label>
                <Select value={memoryMb} onValueChange={setMemoryMb}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="512">512 MB</SelectItem>
                    <SelectItem value="1024">1 GB</SelectItem>
                    <SelectItem value="2048">2 GB</SelectItem>
                    <SelectItem value="4096">4 GB</SelectItem>
                    <SelectItem value="8192">8 GB</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>

            <div className="space-y-2">
              <Label>Volume</Label>
              <Select value={volumeId || 'none'} onValueChange={(v) => setVolumeId(v === 'none' ? '' : v)}>
                <SelectTrigger>
                  <SelectValue placeholder="Select a volume..." />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">Select a volume</SelectItem>
                  {volumes?.map((vol) => (
                    <SelectItem key={vol.id} value={vol.id}>
                      {vol.name} ({formatBytes(vol.sizeBytes)})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-2">
              <Label>NIC</Label>
              <Select value={nicId || 'none'} onValueChange={(v) => setNicId(v === 'none' ? '' : v)}>
                <SelectTrigger>
                  <SelectValue placeholder="Select a NIC..." />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">Select a NIC</SelectItem>
                  {nics?.map((nic) => (
                    <SelectItem key={nic.id} value={nic.id}>
                      {nic.name || nic.macAddress}{nic.ipv4Address ? ` (${nic.ipv4Address})` : ''}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-2">
              <Label>Image</Label>
              <Select value={image || 'none'} onValueChange={(v) => setImage(v === 'none' ? '' : v)}>
                <SelectTrigger>
                  <SelectValue placeholder="Select an image..." />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">Select an image</SelectItem>
                  {templates?.map((t) => (
                    <SelectItem key={t.id} value={t.name}>
                      {t.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                Template name used as the boot image identifier
              </p>
            </div>

            <div className="flex gap-2 pt-4">
              <Button
                type="submit"
                disabled={createVm.isPending || !name || !volumeId || !nicId || !image}
              >
                {createVm.isPending ? 'Creating...' : 'Create VM'}
              </Button>
              <Button type="button" variant="outline" onClick={() => navigate('../vms')}>
                Cancel
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
