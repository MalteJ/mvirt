import { useParams, Link } from 'react-router-dom'
import { FolderKanban, Server, ArrowRight, ShieldCheck } from 'lucide-react'
import { useOrg, useProjectsInOrg, useClustersInOrg } from '@/hooks/queries'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'

export function OrgDashboardPage() {
  const { orgSlug } = useParams<{ orgSlug: string }>()
  const { data: org, isLoading } = useOrg(orgSlug)
  const { data: projects } = useProjectsInOrg(orgSlug)
  const { data: clusters } = useClustersInOrg(orgSlug)

  const totalNodes =
    clusters?.reduce((acc, c) => acc + c.nodeIds.length, 0) ?? 0

  if (!orgSlug) return null
  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-32">
        <p className="text-muted-foreground">Loading...</p>
      </div>
    )
  }
  if (!org) {
    return <p className="text-muted-foreground">Organization not found.</p>
  }

  return (
    <div className="space-y-6">
      <div>
        <div className="text-xs uppercase tracking-wider text-muted-foreground">
          Organization
        </div>
        <h1 className="text-2xl font-bold tracking-tight">
          {org.name}
          <span className="ml-2 font-mono text-base font-normal text-muted-foreground">
            {org.slug}
          </span>
        </h1>
        {org.contact.legalName && (
          <p className="text-sm text-muted-foreground">
            {org.contact.legalName}
          </p>
        )}
      </div>

      <div className="grid gap-4 md:grid-cols-3">
        <StatCard
          to={`/orgs/${orgSlug}/projects`}
          icon={<FolderKanban className="h-5 w-5" />}
          label="Projects"
          value={projects?.length ?? 0}
          hint={
            projects && projects.length === 0
              ? 'No projects yet — create one to host workloads.'
              : undefined
          }
        />
        <StatCard
          to={`/orgs/${orgSlug}/clusters`}
          icon={<Server className="h-5 w-5" />}
          label="Clusters"
          value={clusters?.length ?? 0}
          hint={
            clusters && clusters.length === 0
              ? 'No clusters yet — onboard your first node here.'
              : `${totalNodes} ${totalNodes === 1 ? 'node' : 'nodes'} total`
          }
        />
        <StatCard
          to={`/orgs/${orgSlug}/settings`}
          icon={<ShieldCheck className="h-5 w-5" />}
          label="Settings"
          value={null}
          hint="Contact details, billing info, defaults"
        />
      </div>
    </div>
  )
}

interface StatCardProps {
  to: string
  icon: React.ReactNode
  label: string
  value: number | null
  hint?: string
}

function StatCard({ to, icon, label, value, hint }: StatCardProps) {
  return (
    <Link to={to} className="block">
      <Card className="transition-all hover:border-purple/40 hover:shadow-glow-purple">
        <CardHeader className="pb-2">
          <CardTitle className="flex items-center justify-between text-sm font-medium text-muted-foreground">
            <span className="flex items-center gap-2">
              {icon}
              {label}
            </span>
            <ArrowRight className="h-4 w-4 opacity-0 transition-opacity group-hover:opacity-100" />
          </CardTitle>
        </CardHeader>
        <CardContent>
          {value !== null && <div className="text-3xl font-bold">{value}</div>}
          {hint && <p className="mt-1 text-xs text-muted-foreground">{hint}</p>}
        </CardContent>
      </Card>
    </Link>
  )
}
