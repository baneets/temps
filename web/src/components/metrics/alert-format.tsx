// Shared formatting helpers for OTel metric alert rules.
//
// Keeps the list rows, the form header, and any future surfaces rendering the
// firing-state badge + one-line rule summary identically.

import type { OtelMetricAlertRuleResponse } from '@/api/client'
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
 *
 * Reads the detector from the typed `detection_config` discriminated union.
 * Today only `static` rules exist; the fallback covers future kinds
 * (anomaly/forecast/outlier/auto_watch) without breaking the row.
 */
export function alertSummary(rule: OtelMetricAlertRuleResponse): string {
  const agg = aggregationLabel(rule.aggregation)
  const cfg = rule.detection_config
  if (cfg.kind === 'static') {
    return `${agg} ${rule.metric_name} ${comparatorSymbol(cfg.comparator)} ${cfg.threshold} · ${rule.severity}`
  }
  return `${agg} ${rule.metric_name} · ${cfg.kind} · ${rule.severity}`
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
