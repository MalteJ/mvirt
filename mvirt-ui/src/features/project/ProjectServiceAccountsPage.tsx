import { useState } from 'react'
import { useParams } from 'react-router-dom'
import {
  AlertTriangle,
  Bot,
  ChevronDown,
  ChevronRight,
  Copy,
  KeyRound,
  Plus,
  Trash2,
} from 'lucide-react'
import {
  useApiKeys,
  useCreateApiKey,
  useCreateServiceAccount,
  useDeleteServiceAccount,
  useRevokeApiKey,
  useServiceAccounts,
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
import type { ApiKey, ServiceAccount } from '@/types'

export function ProjectServiceAccountsPage() {
  const { projectSlug } = useParams<{ projectSlug: string }>()
  const { data: accounts, isLoading } = useServiceAccounts(projectSlug)
  const createSa = useCreateServiceAccount(projectSlug ?? '')

  const [createOpen, setCreateOpen] = useState(false)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [expanded, setExpanded] = useState<string | null>(null)

  const handleCreate = (e: React.FormEvent) => {
    e.preventDefault()
    const trimmed = name.trim()
    if (!trimmed) return
    createSa.mutate(
      {
        name: trimmed,
        description: description.trim() || undefined,
      },
      {
        onSuccess: () => {
          setCreateOpen(false)
          setName('')
          setDescription('')
        },
      },
    )
  }

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h2 className="text-lg font-semibold tracking-tight">
            Service Accounts
          </h2>
          <p className="text-sm text-muted-foreground">
            Machine identities scoped to this project. Each gets project-admin
            and can be authenticated via static API keys.
          </p>
        </div>
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <Button onClick={() => setCreateOpen(true)}>
            <Plus className="mr-2 h-4 w-4" />
            New Service Account
          </Button>
          <DialogContent>
            <form onSubmit={handleCreate}>
              <DialogHeader>
                <DialogTitle>Create Service Account</DialogTitle>
                <DialogDescription>
                  Names are unique within this project. The account inherits
                  project-admin automatically — no extra membership grant
                  needed.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="sa-name">Name</Label>
                  <Input
                    id="sa-name"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="github-actions-ci"
                    autoFocus
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="sa-desc">Description (optional)</Label>
                  <Input
                    id="sa-desc"
                    value={description}
                    onChange={(e) => setDescription(e.target.value)}
                    placeholder="Used by the prod-deploy workflow"
                  />
                </div>
                {createSa.error && (
                  <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                    {String(createSa.error)}
                  </div>
                )}
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setCreateOpen(false)}
                >
                  Cancel
                </Button>
                <Button
                  type="submit"
                  disabled={!name.trim() || createSa.isPending}
                >
                  {createSa.isPending ? 'Creating…' : 'Create'}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        </Dialog>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Bot className="h-4 w-4" />
            {accounts?.length ?? 0}{' '}
            {accounts?.length === 1 ? 'account' : 'accounts'}
          </CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : !accounts || accounts.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No service accounts yet. Create one to mint API keys for
              automation.
            </p>
          ) : (
            <div className="space-y-2">
              {accounts.map((sa) => (
                <ServiceAccountRow
                  key={sa.id}
                  projectSlug={projectSlug ?? ''}
                  account={sa}
                  expanded={expanded === sa.id}
                  onToggle={() =>
                    setExpanded((prev) => (prev === sa.id ? null : sa.id))
                  }
                />
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

function ServiceAccountRow({
  projectSlug,
  account,
  expanded,
  onToggle,
}: {
  projectSlug: string
  account: ServiceAccount
  expanded: boolean
  onToggle: () => void
}) {
  const deleteSa = useDeleteServiceAccount(projectSlug)

  return (
    <div className="rounded-md border bg-card/50">
      <div className="flex items-center gap-3 px-3 py-2">
        <button
          onClick={onToggle}
          className="text-muted-foreground transition-colors hover:text-foreground"
          aria-label={expanded ? 'Collapse' : 'Expand'}
        >
          {expanded ? (
            <ChevronDown className="h-4 w-4" />
          ) : (
            <ChevronRight className="h-4 w-4" />
          )}
        </button>
        <div className="flex-1">
          <div className="font-medium">{account.name}</div>
          {account.description && (
            <div className="text-xs text-muted-foreground">
              {account.description}
            </div>
          )}
        </div>
        <span className="text-xs text-muted-foreground">
          {formatDate(account.createdAt)}
        </span>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            if (
              confirm(
                `Delete ServiceAccount "${account.name}"? All of its API keys will be revoked.`,
              )
            ) {
              deleteSa.mutate(account.id)
            }
          }}
        >
          <Trash2 className="h-3 w-3" />
        </Button>
      </div>
      {expanded && (
        <div className="border-t px-4 py-3">
          <ApiKeysSection projectSlug={projectSlug} saId={account.id} />
        </div>
      )}
    </div>
  )
}

function ApiKeysSection({
  projectSlug,
  saId,
}: {
  projectSlug: string
  saId: string
}) {
  const { data: keys, isLoading } = useApiKeys(projectSlug, saId)
  const create = useCreateApiKey(projectSlug, saId)
  const revoke = useRevokeApiKey(projectSlug, saId)

  const [createOpen, setCreateOpen] = useState(false)
  const [description, setDescription] = useState('')
  const [expiresAt, setExpiresAt] = useState('')
  const [issuedSecret, setIssuedSecret] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  const handleCreate = (e: React.FormEvent) => {
    e.preventDefault()
    create.mutate(
      {
        description: description.trim() || undefined,
        expiresAt: expiresAt.trim() || undefined,
      },
      {
        onSuccess: (k) => {
          if (k.secret) setIssuedSecret(k.secret)
          setCreateOpen(false)
          setDescription('')
          setExpiresAt('')
        },
      },
    )
  }

  const dismissSecret = () => {
    setIssuedSecret(null)
    setCopied(false)
  }

  const copy = async () => {
    if (!issuedSecret) return
    try {
      await navigator.clipboard.writeText(issuedSecret)
      setCopied(true)
    } catch {
      /* clipboard might be unavailable on http; ignore */
    }
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div className="text-xs uppercase tracking-wide text-muted-foreground">
          API keys
        </div>
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <Button size="sm" variant="outline" onClick={() => setCreateOpen(true)}>
            <Plus className="mr-1 h-3 w-3" />
            New key
          </Button>
          <DialogContent>
            <form onSubmit={handleCreate}>
              <DialogHeader>
                <DialogTitle>New static API key</DialogTitle>
                <DialogDescription>
                  The secret is shown exactly once. Copy it now — only its
                  hash is stored on the server.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="key-desc">Description (optional)</Label>
                  <Input
                    id="key-desc"
                    value={description}
                    onChange={(e) => setDescription(e.target.value)}
                    placeholder="laptop-2026"
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor="key-exp">Expires at (RFC3339, optional)</Label>
                  <Input
                    id="key-exp"
                    value={expiresAt}
                    onChange={(e) => setExpiresAt(e.target.value)}
                    placeholder="2027-01-01T00:00:00Z"
                  />
                  <p className="text-xs text-muted-foreground">
                    Leave blank for a non-expiring key. No org-wide default —
                    rotation is your responsibility.
                  </p>
                </div>
                {create.error && (
                  <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                    {String(create.error)}
                  </div>
                )}
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setCreateOpen(false)}
                >
                  Cancel
                </Button>
                <Button type="submit" disabled={create.isPending}>
                  {create.isPending ? 'Creating…' : 'Create'}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        </Dialog>
      </div>

      {issuedSecret && (
        <Dialog open onOpenChange={dismissSecret}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <AlertTriangle className="h-4 w-4 text-yellow-500" />
                Copy your API key now
              </DialogTitle>
              <DialogDescription>
                This is the only time the secret is shown. Once you close this
                dialog, only the hash remains on the server.
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-3 py-2">
              <div className="rounded-md border bg-muted/30 px-3 py-2 font-mono text-xs break-all">
                {issuedSecret}
              </div>
              <Button onClick={copy} variant="secondary" className="w-full">
                <Copy className="mr-2 h-3 w-3" />
                {copied ? 'Copied!' : 'Copy to clipboard'}
              </Button>
            </div>
            <DialogFooter>
              <Button onClick={dismissSecret}>I've copied it</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}

      {isLoading ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : !keys || keys.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No API keys yet. Create one to authenticate this account.
        </p>
      ) : (
        <table className="w-full text-xs">
          <thead className="border-b text-left text-muted-foreground">
            <tr>
              <th className="py-1 font-medium">Prefix</th>
              <th className="py-1 font-medium">Description</th>
              <th className="py-1 font-medium">Expires</th>
              <th className="py-1 font-medium">Created</th>
              <th className="py-1 font-medium">Status</th>
              <th className="py-1"></th>
            </tr>
          </thead>
          <tbody>
            {keys.map((k) => (
              <ApiKeyRow key={k.id} apiKey={k} onRevoke={() => revoke.mutate(k.id)} />
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

function ApiKeyRow({
  apiKey,
  onRevoke,
}: {
  apiKey: ApiKey
  onRevoke: () => void
}) {
  const revoked = !!apiKey.revokedAt
  const expired =
    !!apiKey.expiresAt && new Date(apiKey.expiresAt) < new Date()
  const status = revoked
    ? { label: 'revoked', cls: 'text-muted-foreground' }
    : expired
      ? { label: 'expired', cls: 'text-yellow-600' }
      : { label: 'active', cls: 'text-state-running' }

  return (
    <tr className="border-b last:border-0">
      <td className="py-2 font-mono">
        <KeyRound className="mr-1 inline h-3 w-3" />
        {apiKey.displayPrefix}…
      </td>
      <td className="py-2">{apiKey.description ?? '—'}</td>
      <td className="py-2 text-muted-foreground">
        {apiKey.expiresAt ? formatDate(apiKey.expiresAt) : 'never'}
      </td>
      <td className="py-2 text-muted-foreground">
        {formatDate(apiKey.createdAt)}
      </td>
      <td className={`py-2 ${status.cls}`}>{status.label}</td>
      <td className="py-2 text-right">
        {!revoked && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              if (
                confirm(
                  `Revoke API key starting with "${apiKey.displayPrefix}"? This cannot be undone.`,
                )
              ) {
                onRevoke()
              }
            }}
          >
            <Trash2 className="h-3 w-3" />
          </Button>
        )}
      </td>
    </tr>
  )
}
