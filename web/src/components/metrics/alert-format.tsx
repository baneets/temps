// Shared formatting helpers for OTel metric alert rules.
//
// Keeps the list rows, the form header, and any future surfaces rendering the
// firing-state badge + one-line rule summary identically.

import { Badge } from '@/components/ui/badge'
import { aggregationLabel } from './metric-format'

/** Comparator tokens the backend accepts. */
export const COMPARATORS: { value: string; label: string; symbol: string }[] = [
  { value: 'gt', label: 'Greater than (>)', symbol: '>' },
  { value: 'gte', label: 'Greater or equal (≥)', symbol: '≥' },
  { value: 'lt', label: 'Less than (<)', symbol: '<' },
  { value: 'lte', label: 'Less or equal (≤)', symbol: '≤' },
]

export const SEVERITIES: { value: string; label: string }[] = [
  { value: 'info', label: 'Info' },
  { value: 'warning', label: 'Warning' },
  { value: 'critical', label: 'Critical' },
]

export function comparatorSymbol(comparator: string): string {
  return COMPARATORS.find((c) => c.value === comparator)?.symbol ?? comparator
}

/**
 * Compact one-line description of a rule for the list row, e.g.
 * `avg http.server.duration > 500 · warning`.
 */
export function alertSummary(rule: {
  metric_name: string
  aggregation: string
  comparator: string
  threshold: number
  severity: string
}): string {
  const agg = aggregationLabel(rule.aggregation)
  return `${agg} ${rule.metric_name} ${comparatorSymbol(rule.comparator)} ${rule.threshold} · ${rule.severity}`
}

/** Firing-state badge derived from `rule.last_state` (ok|firing|unknown). */
export function AlertStateBadge({ state }: { state: string }) {
  if (state === 'firing') {
    return (
      <Badge variant="destructive" className="shrink-0">
        Firing
      </Badge>
    )
  }
  if (state === 'ok') {
    return (
      <Badge variant="success" className="shrink-0">
        OK
      </Badge>
    )
  }
  return (
    <Badge variant="secondary" className="shrink-0">
      Unknown
    </Badge>
  )
}
