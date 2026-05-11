import { useState } from 'react'
import { useNavigate, useParams, Link } from 'react-router-dom'
import {
  ArrowLeft,
  Check,
  Copy,
  Plus,
  Server,
  Trash2,
} from 'lucide-react'
import {
  useCluster,
  useClusterNodes,
  useCreateOnboardingToken,
  useRevokeNode,
} from '@/hooks/queries'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { NodeStatus, type CreateOnboardingTokenResponse, type Node } from '@/types'

function timeAgo(iso?: string): string {
  if (!iso) return '—'
  const t = new Date(iso).getTime()
  if (!Number.isFinite(t)) return '—'
  const delta = Date.now() - t
  if (delta < 0) return 'just now'
  const s = Math.floor(delta / 1000)
  if (s < 60) return `${s}s ago`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  const d = Math.floor(h / 24)
  return `${d}d ago`
}

function StatusBadge({ node }: { node: Node }) {
  const map: Record<string, { label: string; color: string }> = {
    [NodeStatus.ONLINE]: {
      label: 'online',
      color:
        'bg-green-500/15 text-green-700 dark:text-green-300 border-green-500/30',
    },
    [NodeStatus.OFFLINE]: {
      label: 'offline',
      color:
        'bg-amber-500/15 text-amber-700 dark:text-amber-300 border-amber-500/30',
    },
    [NodeStatus.ONBOARDING]: {
      label: 'onboarding',
      color:
        'bg-blue-500/15 text-blue-700 dark:text-blue-300 border-blue-500/30',
    },
    [NodeStatus.REVOKED]: {
      label: 'revoked',
      color:
        'bg-red-500/15 text-red-700 dark:text-red-300 border-red-500/30',
    },
    [NodeStatus.UNKNOWN]: {
      label: 'unknown',
      color:
        'bg-muted text-muted-foreground border-border',
    },
  }
  const entry = map[node.status] ?? map[NodeStatus.UNKNOWN]
  return (
    <span
      className={`inline-flex items-center rounded-md border px-2 py-0.5 text-xs font-medium ${entry.color}`}
    >
      {entry.label}
    </span>
  )
}

export function ClusterDetailPage() {
  const navigate = useNavigate()
  const { slug } = useParams<{ slug: string }>()
  const { data: cluster, isLoading } = useCluster(slug)
  const { data: nodes } = useClusterNodes(slug)
  const createToken = useCreateOnboardingToken(slug ?? '')
  const revokeNode = useRevokeNode()

  const [addOpen, setAddOpen] = useState(false)
  const [newHostname, setNewHostname] = useState('')
  const [revealedToken, setRevealedToken] =
    useState<CreateOnboardingTokenResponse | null>(null)
  const [tokenCopied, setTokenCopied] = useState(false)

  const handleAdd = () => {
    const host = newHostname.trim()
    if (!host) return
    createToken.mutate(
      { hostname: host },
      {
        onSuccess: (resp) => {
          setRevealedToken(resp)
          setAddOpen(false)
          setNewHostname('')
        },
      },
    )
  }

  const copyToken = async () => {
    if (!revealedToken) return
    await navigator.clipboard.writeText(revealedToken.token)
    setTokenCopied(true)
    setTimeout(() => setTokenCopied(false), 2000)
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-32">
        <p className="text-muted-foreground">Loading...</p>
      </div>
    )
  }
  if (!cluster) {
    return (
      <div className="space-y-4">
        <Link
          to="/"
          className="inline-flex items-center text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back
        </Link>
        <p>Cluster not found.</p>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <Link
            to={`/orgs/${cluster.orgSlug}/clusters`}
            className="inline-flex items-center text-sm text-muted-foreground hover:text-foreground"
          >
            <ArrowLeft className="mr-1 h-4 w-4" />
            Clusters in {cluster.orgSlug}
          </Link>
          <h2 className="mt-1 text-2xl font-bold tracking-tight">
            {cluster.name}{' '}
            <span className="ml-2 font-mono text-base font-normal text-muted-foreground">
              {cluster.slug}
            </span>
          </h2>
          {cluster.description && (
            <p className="text-muted-foreground">{cluster.description}</p>
          )}
          {cluster.location && (
            <p className="mt-1 text-sm text-muted-foreground">
              📍 {cluster.location}
            </p>
          )}
        </div>
      </div>

      {(createToken.error || revokeNode.error) && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {String(createToken.error ?? revokeNode.error)}
        </div>
      )}

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle className="flex items-center gap-2">
              <Server className="h-4 w-4" />
              Nodes ({nodes?.length ?? 0})
            </CardTitle>
            <Dialog open={addOpen} onOpenChange={setAddOpen}>
              <Button size="sm" onClick={() => setAddOpen(true)}>
                <Plus className="mr-1 h-3 w-3" />
                Add Node
              </Button>
              <DialogContent>
                <form
                  onSubmit={(e) => {
                    e.preventDefault()
                    handleAdd()
                  }}
                >
                  <DialogHeader>
                    <DialogTitle>Add Node</DialogTitle>
                    <DialogDescription>
                      Issues a single-use onboarding token bound to this Cluster.
                      The node will appear immediately with status{' '}
                      <span className="font-mono">onboarding</span>; once it
                      redeems the token, the status flips to{' '}
                      <span className="font-mono">online</span>.
                    </DialogDescription>
                  </DialogHeader>
                  <div className="grid gap-4 py-4">
                    <div className="grid gap-2">
                      <Label htmlFor="hostname">Hostname</Label>
                      <Input
                        id="hostname"
                        placeholder="rack3-node5"
                        value={newHostname}
                        onChange={(e) => setNewHostname(e.target.value)}
                        autoFocus
                      />
                      <p className="text-xs text-muted-foreground">
                        Operator-supplied display name. Must be unique within
                        the cluster.
                      </p>
                    </div>
                    {createToken.error && (
                      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                        {String(createToken.error)}
                      </div>
                    )}
                  </div>
                  <DialogFooter>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => setAddOpen(false)}
                    >
                      Cancel
                    </Button>
                    <Button
                      type="submit"
                      disabled={!newHostname.trim() || createToken.isPending}
                    >
                      {createToken.isPending ? 'Issuing...' : 'Issue Token'}
                    </Button>
                  </DialogFooter>
                </form>
              </DialogContent>
            </Dialog>
          </div>
        </CardHeader>
        <CardContent>
          {!nodes || nodes.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No nodes yet. Click <strong>Add Node</strong> to onboard one.
            </p>
          ) : (
            <table className="w-full text-sm">
              <thead className="border-b text-left text-muted-foreground">
                <tr>
                  <th className="py-2 font-medium">Hostname</th>
                  <th className="py-2 font-medium">Status</th>
                  <th className="py-2 font-medium">Last heartbeat</th>
                  <th className="py-2 font-medium">Node id</th>
                  <th className="py-2"></th>
                </tr>
              </thead>
              <tbody>
                {nodes.map((n) => (
                  <tr
                    key={n.id}
                    className="border-b last:border-0 hover:bg-secondary/40 cursor-pointer"
                    onClick={() => navigate(`/cluster/${n.id}`)}
                  >
                    <td className="py-2 font-medium">{n.name}</td>
                    <td className="py-2">
                      <StatusBadge node={n} />
                    </td>
                    <td className="py-2 text-muted-foreground">
                      {n.status === NodeStatus.ONBOARDING
                        ? '—'
                        : timeAgo(n.lastHeartbeat)}
                    </td>
                    <td className="py-2 font-mono text-xs text-muted-foreground">
                      {n.id}
                    </td>
                    <td
                      className="py-2 text-right"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          const what =
                            n.status === NodeStatus.ONBOARDING
                              ? `Cancel onboarding of "${n.name}"? The token will be revoked.`
                              : `Decommission "${n.name}"? Its cert will be revoked and the row removed.`
                          if (confirm(what)) {
                            revokeNode.mutate({
                              nodeId: n.id,
                              reason: 'decommission',
                            })
                          }
                        }}
                      >
                        <Trash2 className="h-3 w-3" />
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      {/* One-time-show modal for the freshly-issued token. */}
      <Dialog
        open={!!revealedToken}
        onOpenChange={(open) => !open && setRevealedToken(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Token issued for {revealedToken?.hostname}</DialogTitle>
            <DialogDescription>
              Copy the token now — this is the only time the bare value is
              shown. Lost tokens can't be recovered, only re-issued.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-2 py-4">
            <Label>Token</Label>
            <div className="flex items-start gap-2">
              <pre className="flex-1 overflow-x-auto rounded border bg-muted/40 p-2 font-mono text-xs">
                {revealedToken?.token ?? ''}
              </pre>
              <Button
                variant="outline"
                size="icon"
                onClick={copyToken}
                aria-label={tokenCopied ? 'Copied' : 'Copy token'}
                className={
                  tokenCopied ? 'text-green-600 dark:text-green-400' : ''
                }
              >
                {tokenCopied ? (
                  <Check className="h-3 w-3" />
                ) : (
                  <Copy className="h-3 w-3" />
                )}
              </Button>
            </div>
            <p className="text-xs text-muted-foreground">
              Use as <code className="font-mono">--onboarding-token</code> when
              starting mvirt-node, or set{' '}
              <code className="font-mono">MVIRT_NODE_ONBOARDING_TOKEN</code>.
              The node will appear in this list with status{' '}
              <span className="font-mono">onboarding</span> until it redeems.
            </p>
          </div>
          <DialogFooter>
            <Button onClick={() => setRevealedToken(null)}>Done</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
