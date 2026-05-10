import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { VmState } from '@/types'

type State = VmState

interface StateIndicatorProps {
  state: State
  showLabel?: boolean
  className?: string
}

interface StateConfig {
  variant: 'running' | 'starting' | 'stopped' | 'error'
  label: string
  pulse: boolean
}

function getStateConfig(state: State): StateConfig {
  switch (state) {
    case VmState.RUNNING:
      return { variant: 'running', label: 'Running', pulse: false }

    case VmState.STARTING:
      return { variant: 'starting', label: 'Starting', pulse: true }

    case VmState.STOPPING:
      return { variant: 'starting', label: 'Stopping', pulse: true }

    case VmState.STOPPED:
      return { variant: 'stopped', label: 'Stopped', pulse: false }

    default:
      return { variant: 'stopped', label: String(state), pulse: false }
  }
}

export function StateIndicator({ state, showLabel = true, className }: StateIndicatorProps) {
  const config = getStateConfig(state)

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
