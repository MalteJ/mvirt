import { useState } from 'react'
import { useParams } from 'react-router-dom'
import { Plus, Trash2, Users } from 'lucide-react'
import {
  useAccounts,
  useGrantOrgMember,
  useInviteAccount,
  useOrgMembers,
  useRevokeOrgMember,
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

export function OrgMembersPage() {
  const { orgSlug } = useParams<{ orgSlug: string }>()
  const { data: members, isLoading } = useOrgMembers(orgSlug)
  const { data: accounts } = useAccounts()
  const grant = useGrantOrgMember(orgSlug ?? '')
  const revoke = useRevokeOrgMember(orgSlug ?? '')
  const invite = useInviteAccount()

  const [open, setOpen] = useState(false)
  const [email, setEmail] = useState('')

  // Single email-driven flow: look up the Account by email, fall back to
  // creating an invite Account, then grant org-admin on it.
  const handleSubmit = async () => {
    const normalized = email.trim().toLowerCase()
    if (!normalized) return
    const existing = accounts?.find(
      (a) => a.email?.toLowerCase() === normalized,
    )
    const accountId = existing
      ? existing.id
      : (await invite.mutateAsync({ email: normalized })).id
    grant.mutate(
      { accountId, role: 'org-admin' },
      {
        onSuccess: () => {
          setOpen(false)
          setEmail('')
        },
      },
    )
  }

  const submitting = invite.isPending || grant.isPending

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-lg font-semibold tracking-tight">Members</h2>
          <p className="text-sm text-muted-foreground">
            Accounts with a role at this Org. Platform-admins implicitly have
            org-admin everywhere — they don't need to appear here.
          </p>
        </div>
        <Dialog open={open} onOpenChange={setOpen}>
          <Button onClick={() => setOpen(true)}>
            <Plus className="mr-2 h-4 w-4" />
            Add Member
          </Button>
          <DialogContent>
            <form
              onSubmit={(e) => {
                e.preventDefault()
                handleSubmit()
              }}
            >
              <DialogHeader>
                <DialogTitle>Add Org Member</DialogTitle>
                <DialogDescription>
                  Enter the user's email. If they already exist in mvirt the
                  membership is granted immediately. If not, an invite-Account
                  is created and gets linked to their OIDC identity on first
                  login — the role grant takes effect right away either way.
                </DialogDescription>
              </DialogHeader>
              <div className="grid gap-4 py-4">
                <div className="grid gap-2">
                  <Label htmlFor="email">Email</Label>
                  <Input
                    id="email"
                    type="email"
                    placeholder="user@example.com"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    autoFocus
                  />
                </div>
                <div className="grid gap-2">
                  <Label>Role</Label>
                  <div className="rounded-md border bg-muted/30 px-3 py-2 text-sm">
                    org-admin{' '}
                    <span className="text-muted-foreground">
                      (only role available today)
                    </span>
                  </div>
                </div>
                {(invite.error || grant.error) && (
                  <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                    {String(invite.error ?? grant.error)}
                  </div>
                )}
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setOpen(false)}
                >
                  Cancel
                </Button>
                <Button
                  type="submit"
                  disabled={!email.trim() || submitting}
                >
                  {submitting ? 'Adding...' : 'Add'}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        </Dialog>
      </div>

      {revoke.error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {String(revoke.error)}
        </div>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Users className="h-4 w-4" />
            {members?.length ?? 0} {members?.length === 1 ? 'member' : 'members'}
          </CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <p className="text-sm text-muted-foreground">Loading...</p>
          ) : !members || members.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No org-level members yet. Add one with the button above.
            </p>
          ) : (
            <table className="w-full text-sm">
              <thead className="border-b text-left text-muted-foreground">
                <tr>
                  <th className="py-2 font-medium">Account</th>
                  <th className="py-2 font-medium">Role</th>
                  <th className="py-2 font-medium">Granted</th>
                  <th className="py-2"></th>
                </tr>
              </thead>
              <tbody>
                {members.map((m) => {
                  const acc = accounts?.find((a) => a.id === m.accountId)
                  return (
                    <tr key={m.id} className="border-b last:border-0">
                      <td className="py-2">
                        <div className="font-medium">
                          {acc?.displayName ?? acc?.email ?? m.accountId}
                        </div>
                        {acc?.email && (
                          <div className="text-xs text-muted-foreground">
                            {acc.email}
                          </div>
                        )}
                      </td>
                      <td className="py-2">
                        <span className="rounded bg-muted px-2 py-0.5 font-mono text-xs">
                          {m.role}
                        </span>
                      </td>
                      <td className="py-2 text-muted-foreground">
                        {formatDate(m.createdAt)}
                      </td>
                      <td className="py-2 text-right">
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => {
                            if (
                              confirm(
                                `Revoke ${m.role} from ${
                                  acc?.email ?? m.accountId
                                }?`,
                              )
                            ) {
                              revoke.mutate(m.id)
                            }
                          }}
                        >
                          <Trash2 className="h-3 w-3" />
                        </Button>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
