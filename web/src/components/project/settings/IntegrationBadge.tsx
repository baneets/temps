import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import type { EnvVarIntegrationInfo } from '@/lib/resolved-env-vars'
import { resolvePluginIcon } from '@/lib/pluginIcons'
import {
  Database,
  HardDrive,
  Leaf,
  Server,
  type LucideIcon,
} from 'lucide-react'

/**
 * Map backend service_type values to a Lucide icon. Any unknown type falls
 * through to resolvePluginIcon (which can interpret a kebab-case name) and
 * finally to Puzzle.
 */
function iconForServiceType(serviceType: string): LucideIcon {
  const normalized = serviceType.toLowerCase()
  switch (normalized) {
    case 'postgres':
    case 'postgresql':
    case 'mysql':
    case 'mariadb':
      return Database
    case 'mongodb':
    case 'mongo':
      return Leaf
    case 'redis':
      return Server
    case 'rustfs':
    case 'minio':
    case 's3':
      return HardDrive
    default:
      return resolvePluginIcon(normalized)
  }
}

interface IntegrationBadgeProps {
  service: EnvVarIntegrationInfo
  /** When true, render a muted "overridden" treatment. */
  overridden?: boolean
  className?: string
}

/**
 * Small green (or muted when overridden) square showing the icon of the
 * integration that produced an environment variable — mirrors the Vercel
 * "variable from integration" affordance.
 */
export function IntegrationBadge({
  service,
  overridden = false,
  className,
}: IntegrationBadgeProps) {
  const Icon = iconForServiceType(service.service_type)
  const tooltipLabel = overridden
    ? `Overridden — ${service.service_name} would provide this variable`
    : `From ${service.service_name}`

  return (
    <TooltipProvider delayDuration={150}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            aria-label={tooltipLabel}
            className={cn(
              'inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-[4px] border',
              overridden
                ? 'border-border bg-muted text-muted-foreground'
                : 'border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400',
              className,
            )}
          >
            <Icon className="h-3 w-3" strokeWidth={2.25} />
          </span>
        </TooltipTrigger>
        <TooltipContent side="top" className="text-xs">
          <div className="space-y-0.5">
            <div className="font-medium">{service.service_name}</div>
            <div className="text-muted-foreground">
              {overridden ? 'Shadowed by manual value' : service.service_type}
            </div>
          </div>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
