import { useState } from 'react'
import { useParams, useNavigate } from 'react-router-dom'
import { useProjectId } from '@/hooks/useProjectId'
import { ColumnDef } from '@tanstack/react-table'
import {
  ArrowLeft,
  Plus,
  Trash2,
  Shield,
  ArrowDownToLine,
  ArrowUpFromLine,
} from 'lucide-react'
import {
  useSecurityGroup,
  useCreateSecurityGroupRule,
  useDeleteSecurityGroupRule,
} from '@/hooks/queries'
import { DataTable } from '@/components/data-display/DataTable'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { SecurityGroupRule, RuleDirection, RuleProtocol } from '@/types'

const protocolLabels: Record<RuleProtocol, string> = {
  [RuleProtocol.ALL]: 'All Traffic',
  [RuleProtocol.TCP]: 'TCP',
  [RuleProtocol.UDP]: 'UDP',
  [RuleProtocol.ICMP]: 'ICMP',
  [RuleProtocol.ICMPV6]: 'ICMPv6',
}

export function SecurityGroupDetailPage() {
  const { id } = useParams<{ id: string }>()
  const projectId = useProjectId()
  const navigate = useNavigate()

  // All hooks must be called before any early returns
  const { data: securityGroup, isLoading, error } = useSecurityGroup(id || '')
  const createRule = useCreateSecurityGroupRule()
  const deleteRule = useDeleteSecurityGroupRule()

  const [ruleDialogOpen, setRuleDialogOpen] = useState(false)
  const [ruleDirection, setRuleDirection] = useState<RuleDirection>(RuleDirection.INGRESS)
  const [ruleProtocol, setRuleProtocol] = useState<RuleProtocol>(RuleProtocol.TCP)
  const [rulePortStart, setRulePortStart] = useState('')
  const [rulePortEnd, setRulePortEnd] = useState('')
  const [ruleCidr, setRuleCidr] = useState('')
  const [ruleDescription, setRuleDescription] = useState('')

  const showPorts = ruleProtocol === RuleProtocol.TCP ||
                    ruleProtocol === RuleProtocol.UDP ||
                    ruleProtocol === RuleProtocol.ALL

  const handleCreateRule = () => {
    if (!id) return
    createRule.mutate(
      {
        securityGroupId: id,
        direction: ruleDirection,
        protocol: ruleProtocol,
        portStart: showPorts && rulePortStart ? parseInt(rulePortStart) : undefined,
        portEnd: showPorts && rulePortEnd ? parseInt(rulePortEnd) : undefined,
        cidr: ruleCidr.trim() || undefined,
        description: ruleDescription.trim() || undefined,
      },
      {
        onSuccess: () => {
          setRuleDialogOpen(false)
          setRuleDirection(RuleDirection.INGRESS)
          setRuleProtocol(RuleProtocol.TCP)
          setRulePortStart('')
          setRulePortEnd('')
          setRuleCidr('')
          setRuleDescription('')
        },
      }
    )
  }

  // Now we can do early returns after all hooks are called
  if (!id) {
    return (
      <div className="flex items-center justify-center h-64">
        <p className="text-muted-foreground">No security group ID provided</p>
      </div>
    )
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <p className="text-muted-foreground">Loading...</p>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-4">
        <p className="text-destructive">Failed to load security group</p>
        <p className="text-sm text-muted-foreground">{String(error)}</p>
        <Button variant="outline" onClick={() => navigate(`/p/${projectId}/firewall`)}>
          Back to Firewall
        </Button>
      </div>
    )
  }

  if (!securityGroup) {
    return (
      <div className="flex items-center justify-center h-64">
        <p className="text-muted-foreground">Security group not found</p>
      </div>
    )
  }

  const rules = securityGroup.rules || []
  const ingressCount = rules.filter(r => r.direction === RuleDirection.INGRESS).length
  const egressCount = rules.filter(r => r.direction === RuleDirection.EGRESS).length

  const columns: ColumnDef<SecurityGroupRule>[] = [
    {
      accessorKey: 'direction',
      header: 'Direction',
      cell: ({ row }) => (
        <Badge variant={row.original.direction === RuleDirection.INGRESS ? 'running' : 'starting'}>
          <span className="flex items-center gap-1">
            {row.original.direction === RuleDirection.INGRESS ? (
              <ArrowDownToLine className="h-3 w-3" />
            ) : (
              <ArrowUpFromLine className="h-3 w-3" />
            )}
            {row.original.direction}
          </span>
        </Badge>
      ),
    },
    {
      accessorKey: 'protocol',
      header: 'Protocol',
      cell: ({ row }) => protocolLabels[row.original.protocol] || row.original.protocol,
    },
    {
      accessorKey: 'ports',
      header: 'Ports',
      cell: ({ row }) => {
        const { portStart, portEnd, protocol } = row.original
        if (protocol === RuleProtocol.ICMP || protocol === RuleProtocol.ICMPV6) {
          return <span className="text-muted-foreground">-</span>
        }
        if (!portStart && !portEnd) {
          return <span className="font-mono text-sm">All</span>
        }
        if (portStart === portEnd) {
          return <span className="font-mono text-sm">{portStart}</span>
        }
        return <span className="font-mono text-sm">{portStart}-{portEnd}</span>
      },
    },
    {
      accessorKey: 'cidr',
      header: 'Source/Destination',
      cell: ({ row }) => (
        <span className="font-mono text-sm">
          {row.original.cidr || '0.0.0.0/0'}
        </span>
      ),
    },
    {
      accessorKey: 'description',
      header: 'Description',
      cell: ({ row }) => (
        <span className="text-muted-foreground">
          {row.original.description || '-'}
        </span>
      ),
    },
    {
      id: 'actions',
      cell: ({ row }) => (
        <Button
          variant="ghost"
          size="icon"
          onClick={() => {
            if (confirm('Delete this rule?')) {
              deleteRule.mutate({
                securityGroupId: id,
                ruleId: row.original.id,
              })
            }
          }}
        >
          <Trash2 className="h-4 w-4 text-destructive" />
        </Button>
      ),
    },
  ]

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate(`/p/${projectId}/firewall`)}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-purple/20">
          <Shield className="h-5 w-5 text-purple" />
        </div>
        <div className="flex-1">
          <h2 className="text-2xl font-bold tracking-tight">{securityGroup.name}</h2>
          <p className="text-muted-foreground">
            {securityGroup.description || 'No description'}
          </p>
        </div>
        <div className="flex items-center gap-4 text-sm text-muted-foreground">
          <span>{ingressCount} inbound</span>
          <span>{egressCount} outbound</span>
          <span>{securityGroup.nicCount} NICs</span>
        </div>
      </div>

      <div className="flex items-center justify-between">
        <h3 className="text-lg font-semibold">Firewall Rules</h3>
        <Dialog open={ruleDialogOpen} onOpenChange={setRuleDialogOpen}>
          <DialogTrigger asChild>
            <Button>
              <Plus className="mr-2 h-4 w-4" />
              Add Rule
            </Button>
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Add Firewall Rule</DialogTitle>
              <DialogDescription>
                Add a rule to {securityGroup.name}
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-4 py-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="grid gap-2">
                  <Label>Direction</Label>
                  <Select
                    value={ruleDirection}
                    onValueChange={(v) => setRuleDirection(v as RuleDirection)}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={RuleDirection.INGRESS}>
                        <span className="flex items-center gap-2">
                          <ArrowDownToLine className="h-4 w-4" />
                          Ingress (Inbound)
                        </span>
                      </SelectItem>
                      <SelectItem value={RuleDirection.EGRESS}>
                        <span className="flex items-center gap-2">
                          <ArrowUpFromLine className="h-4 w-4" />
                          Egress (Outbound)
                        </span>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-2">
                  <Label>Protocol</Label>
                  <Select
                    value={ruleProtocol}
                    onValueChange={(v) => setRuleProtocol(v as RuleProtocol)}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={RuleProtocol.ALL}>All Traffic</SelectItem>
                      <SelectItem value={RuleProtocol.TCP}>TCP</SelectItem>
                      <SelectItem value={RuleProtocol.UDP}>UDP</SelectItem>
                      <SelectItem value={RuleProtocol.ICMP}>ICMP</SelectItem>
                      <SelectItem value={RuleProtocol.ICMPV6}>ICMPv6</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>

              {showPorts && (
                <div className="grid grid-cols-2 gap-4">
                  <div className="grid gap-2">
                    <Label>Port Start</Label>
                    <Input
                      type="number"
                      min="1"
                      max="65535"
                      placeholder="22"
                      value={rulePortStart}
                      onChange={(e) => setRulePortStart(e.target.value)}
                    />
                  </div>
                  <div className="grid gap-2">
                    <Label>Port End</Label>
                    <Input
                      type="number"
                      min="1"
                      max="65535"
                      placeholder="22"
                      value={rulePortEnd}
                      onChange={(e) => setRulePortEnd(e.target.value)}
                    />
                  </div>
                </div>
              )}

              <div className="grid gap-2">
                <Label>CIDR (optional)</Label>
                <Input
                  placeholder="0.0.0.0/0 or 10.0.0.0/8"
                  className="font-mono"
                  value={ruleCidr}
                  onChange={(e) => setRuleCidr(e.target.value)}
                />
                <p className="text-xs text-muted-foreground">
                  Leave empty to allow from/to any address
                </p>
              </div>

              <div className="grid gap-2">
                <Label>Description (optional)</Label>
                <Input
                  placeholder="Allow SSH from internal network"
                  value={ruleDescription}
                  onChange={(e) => setRuleDescription(e.target.value)}
                />
              </div>
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => setRuleDialogOpen(false)}>
                Cancel
              </Button>
              <Button
                onClick={handleCreateRule}
                disabled={createRule.isPending}
              >
                {createRule.isPending ? 'Adding...' : 'Add Rule'}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>

      {rules.length > 0 ? (
        <DataTable
          columns={columns}
          data={rules}
          searchColumn="description"
          searchPlaceholder="Filter rules..."
        />
      ) : (
        <div className="flex flex-col items-center justify-center h-48 border border-dashed border-border rounded-lg">
          <Shield className="h-10 w-10 text-muted-foreground mb-3" />
          <p className="text-muted-foreground mb-1">No rules defined</p>
          <p className="text-xs text-muted-foreground">
            All inbound traffic is denied by default
          </p>
        </div>
      )}
    </div>
  )
}
