import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { ArrowLeft, Plus, X } from 'lucide-react'
import { useCreatePod, useNetworks } from '@/hooks/queries'
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
import type { ContainerSpec } from '@/types'

interface ContainerFormData {
  name: string
  image: string
  command: string
}

export function CreatePodPage() {
  const navigate = useNavigate()
  const projectId = useProjectId()
  const createPod = useCreatePod()
  const { data: networks } = useNetworks(projectId)

  const [name, setName] = useState('')
  const [networkId, setNetworkId] = useState('')
  const [containers, setContainers] = useState<ContainerFormData[]>([
    { name: '', image: '', command: '' },
  ])

  const addContainer = () => {
    setContainers([...containers, { name: '', image: '', command: '' }])
  }

  const removeContainer = (index: number) => {
    if (containers.length > 1) {
      setContainers(containers.filter((_, i) => i !== index))
    }
  }

  const updateContainer = (index: number, field: keyof ContainerFormData, value: string) => {
    const updated = [...containers]
    updated[index] = { ...updated[index], [field]: value }
    setContainers(updated)
  }

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()

    const containerSpecs: ContainerSpec[] = containers.map((c) => ({
      name: c.name,
      image: c.image,
      command: c.command || undefined,
    }))

    createPod.mutate(
      {
        projectId,
        name,
        networkId,
        containers: containerSpecs,
      },
      {
        onSuccess: (pod) => {
          navigate(`/containers/${pod.id}`)
        },
      }
    )
  }

  const isValid = name && networkId && containers.every((c) => c.name && c.image)

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('/containers')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Create Pod</h2>
          <p className="text-muted-foreground">Configure your new isolated pod</p>
        </div>
      </div>

      <form onSubmit={handleSubmit} className="space-y-6">
        <Card>
          <CardHeader>
            <CardTitle>Pod Configuration</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="name">Pod Name</Label>
              <Input
                id="name"
                placeholder="my-pod"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="network">Network</Label>
              <Select value={networkId} onValueChange={setNetworkId}>
                <SelectTrigger>
                  <SelectValue placeholder="Select a network..." />
                </SelectTrigger>
                <SelectContent>
                  {networks?.map((network) => (
                    <SelectItem key={network.id} value={network.id}>
                      {network.name}
                      {network.ipv4Subnet && (
                        <span className="ml-2 text-muted-foreground">
                          ({network.ipv4Subnet})
                        </span>
                      )}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0">
            <CardTitle>Containers</CardTitle>
            <Button type="button" variant="outline" size="sm" onClick={addContainer}>
              <Plus className="mr-2 h-4 w-4" />
              Add Container
            </Button>
          </CardHeader>
          <CardContent className="space-y-4">
            {containers.map((container, index) => (
              <div
                key={index}
                className="rounded-lg border border-border p-4 space-y-4"
              >
                <div className="flex items-center justify-between">
                  <div className="text-sm font-medium text-muted-foreground">
                    Container {index + 1}
                  </div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className={`h-6 w-6 ${containers.length <= 1 ? 'invisible' : ''}`}
                    onClick={() => removeContainer(index)}
                  >
                    <X className="h-4 w-4" />
                  </Button>
                </div>

                <div className="grid grid-cols-2 gap-4">
                  <div className="space-y-2">
                    <Label htmlFor={`container-${index}-name`}>Name</Label>
                    <Input
                      id={`container-${index}-name`}
                      placeholder="nginx"
                      value={container.name}
                      onChange={(e) => updateContainer(index, 'name', e.target.value)}
                      required
                    />
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor={`container-${index}-image`}>Image</Label>
                    <Input
                      id={`container-${index}-image`}
                      placeholder="nginx:latest"
                      value={container.image}
                      onChange={(e) => updateContainer(index, 'image', e.target.value)}
                      required
                    />
                  </div>
                </div>

                <div className="space-y-2">
                  <Label htmlFor={`container-${index}-command`}>Command (optional)</Label>
                  <Input
                    id={`container-${index}-command`}
                    placeholder="/bin/sh -c 'echo hello'"
                    value={container.command}
                    onChange={(e) => updateContainer(index, 'command', e.target.value)}
                  />
                </div>
              </div>
            ))}
          </CardContent>
        </Card>

        <div className="flex gap-2">
          <Button type="submit" disabled={createPod.isPending || !isValid}>
            {createPod.isPending ? 'Creating...' : 'Create Pod'}
          </Button>
          <Button type="button" variant="outline" onClick={() => navigate('/containers')}>
            Cancel
          </Button>
        </div>
      </form>
    </div>
  )
}
