import { useEffect, useMemo, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { ArrowLeft, Save } from 'lucide-react'
import { useOrg as useOrgQuery, useUpdateOrg } from '@/hooks/queries'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import type { OrgContact } from '@/types'

const emptyContact: OrgContact = {}

/**
 * Org-level settings: display name, contact / billing details, security
 * policy knobs (static-key TTL, disallow-static-keys). Lives at
 * `/orgs/:orgSlug/settings`. The slug itself is immutable per ADR-0004,
 * so it's read-only here.
 */
export function OrgSettingsPage() {
  const { orgSlug } = useParams<{ orgSlug: string }>()
  const navigate = useNavigate()
  const { data: org, isLoading } = useOrgQuery(orgSlug)
  const updateOrg = useUpdateOrg(orgSlug ?? '')

  const [name, setName] = useState('')
  const [contact, setContact] = useState<OrgContact>(emptyContact)
  const [ttlDays, setTtlDays] = useState<string>('90')
  const [disallowStaticKeys, setDisallowStaticKeys] = useState(false)

  // Reset form when the loaded Org changes.
  useEffect(() => {
    if (org) {
      setName(org.name)
      setContact(org.contact ?? emptyContact)
      setTtlDays(String(org.defaultStaticKeyTtlDays))
      setDisallowStaticKeys(org.disallowStaticKeys)
    }
  }, [org])

  const dirty = useMemo(() => {
    if (!org) return false
    if (name !== org.name) return true
    if (Number(ttlDays) !== org.defaultStaticKeyTtlDays) return true
    if (disallowStaticKeys !== org.disallowStaticKeys) return true
    const cur = org.contact ?? emptyContact
    return (
      contact.legalName !== cur.legalName ||
      contact.streetAddress !== cur.streetAddress ||
      contact.postalCode !== cur.postalCode ||
      contact.city !== cur.city ||
      contact.country !== cur.country ||
      contact.technicalContactEmail !== cur.technicalContactEmail ||
      contact.billingContactEmail !== cur.billingContactEmail ||
      contact.vatId !== cur.vatId
    )
  }, [org, name, contact, ttlDays, disallowStaticKeys])

  const setContactField = <K extends keyof OrgContact>(key: K, value: string) => {
    // Empty string → drop the field so the server stores `null` for it
    // (clearer than persisting empty strings).
    setContact((prev) => ({ ...prev, [key]: value.trim() === '' ? undefined : value }))
  }

  const handleSave = (e: React.FormEvent) => {
    e.preventDefault()
    if (!org || !dirty) return
    const ttl = Number(ttlDays)
    updateOrg.mutate({
      name: name !== org.name ? name : undefined,
      defaultStaticKeyTtlDays: ttl !== org.defaultStaticKeyTtlDays ? ttl : undefined,
      disallowStaticKeys:
        disallowStaticKeys !== org.disallowStaticKeys ? disallowStaticKeys : undefined,
      contact,
    })
  }

  if (isLoading) {
    return <div className="text-muted-foreground">Loading…</div>
  }
  if (!org) {
    return <div className="text-destructive">Org "{orgSlug}" not found.</div>
  }

  return (
    <form onSubmit={handleSave} className="space-y-8 max-w-2xl">
      <div className="flex items-center gap-3">
        <Button variant="ghost" size="icon" type="button" onClick={() => navigate('/orgs')}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div>
          <h2 className="text-2xl font-bold tracking-tight">{org.name}</h2>
          <p className="text-sm text-muted-foreground font-mono">{org.slug}</p>
        </div>
      </div>

      <section className="space-y-4">
        <div>
          <h3 className="text-sm font-medium uppercase tracking-wide text-muted-foreground">
            General
          </h3>
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <div className="grid gap-2 sm:col-span-2">
            <Label htmlFor="name">Display name</Label>
            <Input id="name" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="grid gap-2">
            <Label>Slug</Label>
            <Input value={org.slug} disabled className="font-mono" />
            <p className="text-xs text-muted-foreground">Slug is immutable.</p>
          </div>
        </div>
      </section>

      <section className="space-y-4">
        <div>
          <h3 className="text-sm font-medium uppercase tracking-wide text-muted-foreground">
            Contact &amp; Billing
          </h3>
          <p className="text-xs text-muted-foreground">
            Used on invoices and operational notifications. All fields optional.
          </p>
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <div className="grid gap-2 sm:col-span-2">
            <Label htmlFor="legalName">Company name (Firmenname)</Label>
            <Input
              id="legalName"
              value={contact.legalName ?? ''}
              onChange={(e) => setContactField('legalName', e.target.value)}
            />
          </div>
          <div className="grid gap-2 sm:col-span-2">
            <Label htmlFor="streetAddress">Street &amp; number (Straße/Hausnummer)</Label>
            <Input
              id="streetAddress"
              value={contact.streetAddress ?? ''}
              onChange={(e) => setContactField('streetAddress', e.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="postalCode">Postal code (PLZ)</Label>
            <Input
              id="postalCode"
              value={contact.postalCode ?? ''}
              onChange={(e) => setContactField('postalCode', e.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="city">City (Ort)</Label>
            <Input
              id="city"
              value={contact.city ?? ''}
              onChange={(e) => setContactField('city', e.target.value)}
            />
          </div>
          <div className="grid gap-2 sm:col-span-2">
            <Label htmlFor="country">Country</Label>
            <Input
              id="country"
              placeholder="DE"
              value={contact.country ?? ''}
              onChange={(e) => setContactField('country', e.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="techEmail">Technical contact email</Label>
            <Input
              id="techEmail"
              type="email"
              value={contact.technicalContactEmail ?? ''}
              onChange={(e) =>
                setContactField('technicalContactEmail', e.target.value)
              }
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="billEmail">Billing contact email</Label>
            <Input
              id="billEmail"
              type="email"
              value={contact.billingContactEmail ?? ''}
              onChange={(e) => setContactField('billingContactEmail', e.target.value)}
            />
          </div>
          <div className="grid gap-2 sm:col-span-2">
            <Label htmlFor="vatId">VAT ID (USt-IdNr.)</Label>
            <Input
              id="vatId"
              placeholder="DE123456789"
              className="font-mono"
              value={contact.vatId ?? ''}
              onChange={(e) => setContactField('vatId', e.target.value)}
            />
          </div>
        </div>
      </section>

      <section className="space-y-4">
        <div>
          <h3 className="text-sm font-medium uppercase tracking-wide text-muted-foreground">
            Security
          </h3>
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <div className="grid gap-2">
            <Label htmlFor="ttl">Default static-key TTL (days)</Label>
            <Input
              id="ttl"
              type="number"
              min="1"
              value={ttlDays}
              onChange={(e) => setTtlDays(e.target.value)}
              disabled={disallowStaticKeys}
            />
          </div>
          <div className="grid gap-2">
            <Label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={disallowStaticKeys}
                onChange={(e) => setDisallowStaticKeys(e.target.checked)}
                className="h-4 w-4 rounded border-border bg-secondary"
              />
              Disallow static API keys for ServiceAccounts
            </Label>
            <p className="text-xs text-muted-foreground">
              Stricter setups force FederatedTrust / SignedJwt only.
            </p>
          </div>
        </div>
      </section>

      {updateOrg.error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {String(updateOrg.error)}
        </div>
      )}

      <div className="flex items-center gap-3">
        <Button type="submit" disabled={!dirty || updateOrg.isPending} className="gap-2">
          <Save className="h-4 w-4" />
          {updateOrg.isPending ? 'Saving…' : 'Save changes'}
        </Button>
        {updateOrg.isSuccess && !dirty && (
          <span className="text-xs text-state-running">Saved.</span>
        )}
      </div>
    </form>
  )
}
