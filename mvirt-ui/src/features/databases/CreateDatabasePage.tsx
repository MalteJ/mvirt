import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { ArrowLeft } from 'lucide-react'
import { useCreateDatabase, useNetworks } from '@/hooks/queries'
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
import { DatabaseType } from '@/types'

const databaseTypes = [
  { value: DatabaseType.POSTGRESQL, label: 'PostgreSQL', versions: ['16.1', '15.5', '14.10'] },
  { value: DatabaseType.REDIS, label: 'Redis', versions: ['7.2', '7.0', '6.2'] },
]

export function CreateDatabasePage() {
  const navigate = useNavigate()
  const createDatabase = useCreateDatabase()
  const { data: networks } = useNetworks()

  const [name, setName] = useState('')
  const [dbType, setDbType] = useState<DatabaseType | ''>('')
  const [version, setVersion] = useState('')
  const [networkId, setNetworkId] = useState('')
  const [storageSizeGb, setStorageSizeGb] = useState('20')
  const [username, setUsername] = useState('admin')
  const [password, setPassword] = useState('')

  const selectedTypeConfig = databaseTypes.find((t) => t.value === dbType)

  const handleTypeChange = (value: string) => {
    setDbType(value as DatabaseType)
    setVersion('')
  }

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!dbType) return

    createDatabase.mutate(
      {
        name,
        type: dbType,
        version,
        networkId,
        storageSizeGb: parseInt(storageSizeGb),
        username,
        password,
      },
      {
        onSuccess: (db) => {
          navigate(`/databases/${db.id}`)
        },
      }
    )
  }

  const isValid = name && dbType && version && networkId && storageSizeGb && username && password

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('/databases')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Create Database</h2>
          <p className="text-muted-foreground">Deploy a new managed database instance</p>
        </div>
      </div>

      <form onSubmit={handleSubmit} className="space-y-6">
        <Card>
          <CardHeader>
            <CardTitle>Database Configuration</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="name">Database Name</Label>
              <Input
                id="name"
                placeholder="my-database"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label htmlFor="type">Database Type</Label>
                <Select value={dbType} onValueChange={handleTypeChange}>
                  <SelectTrigger>
                    <SelectValue placeholder="Select type..." />
                  </SelectTrigger>
                  <SelectContent>
                    {databaseTypes.map((type) => (
                      <SelectItem key={type.value} value={type.value}>
                        {type.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>

              <div className="space-y-2">
                <Label htmlFor="version">Version</Label>
                <Select
                  value={version}
                  onValueChange={setVersion}
                  disabled={!selectedTypeConfig}
                >
                  <SelectTrigger>
                    <SelectValue placeholder="Select version..." />
                  </SelectTrigger>
                  <SelectContent>
                    {selectedTypeConfig?.versions.map((v) => (
                      <SelectItem key={v} value={v}>
                        {v}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
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

            <div className="space-y-2">
              <Label htmlFor="storage">Storage Size (GB)</Label>
              <Select value={storageSizeGb} onValueChange={setStorageSizeGb}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="10">10 GB</SelectItem>
                  <SelectItem value="20">20 GB</SelectItem>
                  <SelectItem value="50">50 GB</SelectItem>
                  <SelectItem value="100">100 GB</SelectItem>
                  <SelectItem value="200">200 GB</SelectItem>
                  <SelectItem value="500">500 GB</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Credentials</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label htmlFor="username">Username</Label>
                <Input
                  id="username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  required
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="password">Password</Label>
                <Input
                  id="password"
                  type="password"
                  placeholder="Enter password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  required
                />
              </div>
            </div>
          </CardContent>
        </Card>

        <div className="flex gap-2">
          <Button type="submit" disabled={createDatabase.isPending || !isValid}>
            {createDatabase.isPending ? 'Creating...' : 'Create Database'}
          </Button>
          <Button type="button" variant="outline" onClick={() => navigate('/databases')}>
            Cancel
          </Button>
        </div>
      </form>
    </div>
  )
}
