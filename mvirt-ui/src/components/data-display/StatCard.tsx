import { ReactNode } from 'react'
import { Card, CardContent } from '@/components/ui/card'
import { cn } from '@/lib/utils'

interface StatCardProps {
  title: string
  value: string | number
  icon?: ReactNode
  description?: string
  className?: string
  color?: 'purple' | 'cyan' | 'green' | 'blue' | 'rust'
}

const colorClasses = {
  purple: 'from-purple/20 to-pink/10 border-purple/30',
  cyan: 'from-cyan/20 to-blue/10 border-cyan/30',
  green: 'from-state-running/20 to-cyan/10 border-state-running/30',
  blue: 'from-blue/20 to-purple/10 border-blue/30',
  rust: 'from-rust/20 to-destructive/10 border-rust/30',
}

const iconColorClasses = {
  purple: 'text-purple-light',
  cyan: 'text-cyan',
  green: 'text-state-running',
  blue: 'text-blue',
  rust: 'text-rust',
}

export function StatCard({ title, value, icon, description, className, color = 'purple' }: StatCardProps) {
  return (
    <Card className={cn(
      'bg-gradient-to-br border backdrop-blur-sm',
      colorClasses[color],
      className
    )}>
      <CardContent className="p-4">
        <div className="flex items-center justify-between">
          <div>
            <p className="text-sm font-medium text-muted-foreground">{title}</p>
            <p className="text-2xl font-bold">{value}</p>
            {description && (
              <p className="text-xs text-muted-foreground mt-1">{description}</p>
            )}
          </div>
          {icon && (
            <div className={cn('opacity-80', iconColorClasses[color])}>{icon}</div>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
