import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { VmState } from '@/types'

interface StateIndicatorProps {
  state: VmState
  showLabel?: boolean
  className?: string
}

const stateConfig: Record<VmState, { variant: 'running' | 'starting' | 'stopped' | 'error'; label: string; pulse: boolean }> = {
  [VmState.RUNNING]: { variant: 'running', label: 'Running', pulse: false },
  [VmState.STARTING]: { variant: 'starting', label: 'Starting', pulse: true },
  [VmState.STOPPING]: { variant: 'starting', label: 'Stopping', pulse: true },
  [VmState.STOPPED]: { variant: 'stopped', label: 'Stopped', pulse: false },
}

export function StateIndicator({ state, showLabel = true, className }: StateIndicatorProps) {
  const config = stateConfig[state]

  return (
    <Badge
      variant={config.variant}
      className={cn(
        config.pulse && 'animate-pulse-state',
        className
      )}
    >
      <span className={cn(
        'mr-1.5 h-2 w-2 rounded-full',
        config.variant === 'running' && 'bg-state-running',
        config.variant === 'starting' && 'bg-state-starting',
        config.variant === 'stopped' && 'bg-state-stopped',
        config.variant === 'error' && 'bg-state-error'
      )} />
      {showLabel && config.label}
    </Badge>
  )
}
