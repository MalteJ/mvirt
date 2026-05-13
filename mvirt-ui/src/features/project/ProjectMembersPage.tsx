import { useState } from 'react'
import { useParams } from 'react-router-dom'
import { Plus, Trash2, Users } from 'lucide-react'
import {
  useAccounts,
  useGrantProjectMember,
  useInviteAccount,
  useProjectMembers,
  useRevokeProjectMember,
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

export function ProjectMembersPage() {
  const { projectSlug } = useParams<{ projectSlug: string }>()
  const { data: members, isLoading } = useProjectMembers(projectSlug)
  const { data: accounts } = useAccounts()
  const grant = useGrantProjectMember(projectSlug ?? '')
  const revoke = useRevokeProjectMember(projectSlug ?? '')
  const invite = useInviteAccount()

  const [open, setOpen] = useState(false)
  const [email, setEmail] = useState('')

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
      { accountId, role: 'project-admin' },
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
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h2 className="text-lg font-semibold tracking-tight">Members</h2>
          <p className="text-sm text-muted-foreground">
            Accounts with project-admin in this Project. Org-admins +
            platform-admins have implicit access — they don't need to appear
            here.
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
                <DialogTitle>Add Project Member</DialogTitle>
                <DialogDescription>
                  Enter the user's email. Existing accounts get the grant
                  immediately; new addresses pre-create an invite that links
                  to the OIDC identity on first login.
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
                    project-admin{' '}
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
            {members?.length ?? 0}{' '}
            {members?.length === 1 ? 'member' : 'members'}
          </CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <p className="text-sm text-muted-foreground">Loading...</p>
          ) : !members || members.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No project-level members yet. Add one with the button above.
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
