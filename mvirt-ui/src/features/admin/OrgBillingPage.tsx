import { useParams } from 'react-router-dom'
import { CreditCard } from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'

export function OrgBillingPage() {
  const { orgSlug } = useParams<{ orgSlug: string }>()

  // Billing model isn't designed yet — stub the page so the sidebar
  // entry doesn't 404. Real implementation lands with the billing ADR.
  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold tracking-tight">Billing</h2>
        <p className="text-sm text-muted-foreground">
          Usage, invoices, payment methods for {orgSlug}.
        </p>
      </div>
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <CreditCard className="h-4 w-4" />
            Coming soon
          </CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">
            The billing model isn't designed yet. Contact + VAT info already
            live under{' '}
            <a
              href={`/orgs/${orgSlug}/settings`}
              className="text-purple-light hover:underline"
            >
              Settings
            </a>
            .
          </p>
        </CardContent>
      </Card>
    </div>
  )
}
