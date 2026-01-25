import { Server, HardDrive, Network, Activity } from 'lucide-react'
import { useVms, useVolumes, useNetworks, useSystemInfo, useLogs } from '@/hooks/queries'
import { StatCard } from '@/components/data-display/StatCard'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { formatBytes } from '@/lib/utils'
import { VmState, LogLevel } from '@/types'

const levelVariants: Record<LogLevel, 'default' | 'secondary' | 'destructive' | 'outline'> = {
  [LogLevel.DEBUG]: 'outline',
  [LogLevel.INFO]: 'secondary',
  [LogLevel.WARN]: 'default',
  [LogLevel.ERROR]: 'destructive',
  [LogLevel.AUDIT]: 'secondary',
}

export function DashboardPage() {
  const { data: vms } = useVms()
  const { data: volumes } = useVolumes()
  const { data: networks } = useNetworks()
  const { data: systemInfo } = useSystemInfo()
  const { data: recentLogs } = useLogs({ limit: 10 })

  const runningVms = vms?.filter((vm) => vm.state === VmState.RUNNING).length ?? 0
  const totalVms = vms?.length ?? 0

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight text-gradient">Dashboard</h2>
        <p className="text-muted-foreground">
          Overview of your virtual infrastructure
        </p>
      </div>

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <StatCard
          title="Virtual Machines"
          value={`${runningVms}/${totalVms}`}
          icon={<Server className="h-6 w-6" />}
          description={`${runningVms} running`}
          color="purple"
        />
        <StatCard
          title="Volumes"
          value={volumes?.length ?? 0}
          icon={<HardDrive className="h-6 w-6" />}
          color="cyan"
        />
        <StatCard
          title="Networks"
          value={networks?.length ?? 0}
          icon={<Network className="h-6 w-6" />}
          color="blue"
        />
        <StatCard
          title="Memory Used"
          value={systemInfo ? formatBytes(systemInfo.memoryUsedBytes) : '-'}
          icon={<Activity className="h-6 w-6" />}
          description={systemInfo ? `of ${formatBytes(systemInfo.memoryTotalBytes)}` : undefined}
          color="green"
        />
      </div>

      <div className="grid gap-4 md:grid-cols-2">
        <Card className="border-border bg-card/50 backdrop-blur-sm">
          <CardHeader>
            <CardTitle className="text-purple-light">System Info</CardTitle>
          </CardHeader>
          <CardContent>
            {systemInfo ? (
              <dl className="space-y-3 text-sm">
                <div className="flex justify-between">
                  <dt className="text-muted-foreground">Hostname</dt>
                  <dd className="font-mono text-cyan">{systemInfo.hostname}</dd>
                </div>
                <div className="flex justify-between">
                  <dt className="text-muted-foreground">Version</dt>
                  <dd className="font-mono text-purple-light">{systemInfo.version}</dd>
                </div>
                <div className="flex justify-between">
                  <dt className="text-muted-foreground">CPUs</dt>
                  <dd className="text-foreground">{systemInfo.cpuCount}</dd>
                </div>
                <div className="flex justify-between">
                  <dt className="text-muted-foreground">Uptime</dt>
                  <dd className="text-state-running">{Math.floor(systemInfo.uptime / 3600)}h {Math.floor((systemInfo.uptime % 3600) / 60)}m</dd>
                </div>
              </dl>
            ) : (
              <p className="text-muted-foreground">Loading...</p>
            )}
          </CardContent>
        </Card>

        <Card className="border-border bg-card/50 backdrop-blur-sm">
          <CardHeader>
            <CardTitle className="text-cyan">Recent Activity</CardTitle>
          </CardHeader>
          <CardContent>
            {recentLogs && recentLogs.length > 0 ? (
              <ul className="space-y-3">
                {recentLogs.slice(0, 5).map((log) => (
                  <li key={log.id} className="flex items-start gap-2 text-sm">
                    <Badge variant={levelVariants[log.level]} className="shrink-0 text-xs">
                      {log.level}
                    </Badge>
                    <span className="text-muted-foreground truncate">
                      {log.message}
                    </span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-muted-foreground text-sm">No recent activity</p>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
