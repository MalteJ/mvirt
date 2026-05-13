import { LogsList } from './LogsList'

export function LogsPage() {
  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">Logs</h2>
        <p className="text-muted-foreground">View and filter system logs</p>
      </div>
      <LogsList />
    </div>
  )
}
