import { useState } from 'react'
import { useParams, Link } from 'react-router-dom'
import {
  ArrowLeft,
  Check,
  Copy,
  KeyRound,
  Plus,
  Server,
  Trash2,
  CheckCircle2,
  AlertTriangle,
} from 'lucide-react'
import {
  useCluster,
  useCreateOnboardingToken,
  useDeleteOnboardingToken,
  useOnboardingTokens,
  useRemoveNodeFromCluster,
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
import { formatDate } from '@/lib/utils'
import type { CreateOnboardingTokenResponse } from '@/types'

export function ClusterDetailPage() {
  const { slug } = useParams<{ slug: string }>()
  const { data: cluster, isLoading } = useCluster(slug)
  const { data: tokens } = useOnboardingTokens(slug)
  const createToken = useCreateOnboardingToken(slug ?? '')
  const deleteToken = useDeleteOnboardingToken(slug ?? '')
  const removeNode = useRemoveNodeFromCluster(slug ?? '')

  const [tokenDialogOpen, setTokenDialogOpen] = useState(false)
  const [newTokenDescription, setNewTokenDescription] = useState('')
  const [newTokenTtl, setNewTokenTtl] = useState('3600')
  const [revealedToken, setRevealedToken] = useState<
    CreateOnboardingTokenResponse | null
  >(null)
  // Must live with the other hooks (above the early returns) — Rules of Hooks
  // forbid conditional hook calls, so anything below `if (isLoading) return`
  // changes call order between renders and crashes the component.
  const [tokenCopied, setTokenCopied] = useState(false)

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
          to="/dashboard"
          className="inline-flex items-center text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back
        </Link>
        <p>Cluster not found.</p>
      </div>
    )
  }

  const handleCreateToken = () => {
    const ttl = parseInt(newTokenTtl, 10)
    createToken.mutate(
      {
        ttlSeconds: Number.isFinite(ttl) && ttl > 0 ? ttl : undefined,
        description: newTokenDescription.trim() || undefined,
      },
      {
        onSuccess: (resp) => {
          setRevealedToken(resp)
          setTokenDialogOpen(false)
          setNewTokenDescription('')
          setNewTokenTtl('3600')
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

      {(removeNode.error || deleteToken.error) && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {String(removeNode.error ?? deleteToken.error)}
        </div>
      )}

      {/* Nodes card */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Server className="h-4 w-4" />
            Nodes ({cluster.nodeIds.length})
          </CardTitle>
        </CardHeader>
        <CardContent>
          {cluster.nodeIds.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No nodes yet. Issue an onboarding token below and run mvirt-node
              with it to add one.
            </p>
          ) : (
            <ul className="space-y-2">
              {cluster.nodeIds.map((id) => (
                <li
                  key={id}
                  className="flex items-center justify-between rounded border px-3 py-2"
                >
                  <span className="font-mono text-sm">{id}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      if (
                        confirm(`Remove node ${id} from cluster ${cluster.slug}?`)
                      ) {
                        removeNode.mutate(id)
                      }
                    }}
                  >
                    <Trash2 className="h-3 w-3" />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      {/* Onboarding tokens card */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle className="flex items-center gap-2">
              <KeyRound className="h-4 w-4" />
              Onboarding Tokens
            </CardTitle>
            <Dialog open={tokenDialogOpen} onOpenChange={setTokenDialogOpen}>
              <Button size="sm" onClick={() => setTokenDialogOpen(true)}>
                <Plus className="mr-1 h-3 w-3" />
                New Token
              </Button>
              <DialogContent>
                <form
                  onSubmit={(e) => {
                    e.preventDefault()
                    handleCreateToken()
                  }}
                >
                  <DialogHeader>
                    <DialogTitle>Issue Onboarding Token</DialogTitle>
                    <DialogDescription>
                      The token is single-use, bound to this Cluster, and
                      revealed only once. Copy it into the node config.
                    </DialogDescription>
                  </DialogHeader>
                  <div className="grid gap-4 py-4">
                    <div className="grid gap-2">
                      <Label htmlFor="desc">Description (optional)</Label>
                      <Input
                        id="desc"
                        placeholder="rack-3 node 5"
                        value={newTokenDescription}
                        onChange={(e) => setNewTokenDescription(e.target.value)}
                      />
                    </div>
                    <div className="grid gap-2">
                      <Label htmlFor="ttl">Lifetime (seconds)</Label>
                      <Input
                        id="ttl"
                        type="number"
                        min={60}
                        max={604800}
                        value={newTokenTtl}
                        onChange={(e) => setNewTokenTtl(e.target.value)}
                      />
                      <p className="text-xs text-muted-foreground">
                        60s … 7d. Default 1h.
                      </p>
                    </div>
                  </div>
                  {createToken.error && (
                    <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                      {String(createToken.error)}
                    </div>
                  )}
                  <DialogFooter>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => setTokenDialogOpen(false)}
                    >
                      Cancel
                    </Button>
                    <Button type="submit" disabled={createToken.isPending}>
                      {createToken.isPending ? 'Creating...' : 'Issue'}
                    </Button>
                  </DialogFooter>
                </form>
              </DialogContent>
            </Dialog>
          </div>
        </CardHeader>
        <CardContent>
          {tokens && tokens.length > 0 ? (
            <ul className="space-y-2">
              {tokens.map((t) => (
                <li
                  key={t.id}
                  className="flex items-center justify-between rounded border px-3 py-2"
                >
                  <div className="flex-1">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-sm">{t.id}</span>
                      {t.usedAt ? (
                        <span className="inline-flex items-center text-xs text-green-700 dark:text-green-400">
                          <CheckCircle2 className="mr-1 h-3 w-3" />
                          Redeemed
                        </span>
                      ) : new Date(t.expiresAt).getTime() < Date.now() ? (
                        <span className="inline-flex items-center text-xs text-amber-700 dark:text-amber-400">
                          <AlertTriangle className="mr-1 h-3 w-3" />
                          Expired
                        </span>
                      ) : (
                        <span className="text-xs text-muted-foreground">
                          Pending
                        </span>
                      )}
                    </div>
                    {t.description && (
                      <div className="text-xs text-muted-foreground">
                        {t.description}
                      </div>
                    )}
                    <div className="text-xs text-muted-foreground">
                      Expires {formatDate(t.expiresAt)}
                      {t.usedByNodeId && (
                        <>
                          {' '}
                          ·{' '}
                          <span className="font-mono">{t.usedByNodeId}</span>
                        </>
                      )}
                    </div>
                  </div>
                  {!t.usedAt && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        if (confirm(`Delete onboarding token ${t.id}?`)) {
                          deleteToken.mutate(t.id)
                        }
                      }}
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">No tokens issued.</p>
          )}
        </CardContent>
      </Card>

      {/* One-time-show modal for a freshly-issued token. */}
      <Dialog
        open={!!revealedToken}
        onOpenChange={(open) => !open && setRevealedToken(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Token issued</DialogTitle>
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
                className={tokenCopied ? 'text-green-600 dark:text-green-400' : ''}
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
