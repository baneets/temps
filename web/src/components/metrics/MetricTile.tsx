import { ProjectResponse } from '@/api/client'
import { queryMetricsOptions } from '@/api/client/@tanstack/react-query.gen'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { ThresholdLineChart } from '@/components/charts/threshold-line-chart'
import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import {
  aggregationLabel,
  formatBucketLabel,
  formatMetricValue,
  histogramQuantile,
  percentileFromAgg,
} from './metric-format'

interface MetricTileProps {
  project: ProjectResponse
  metricName: string
  /** Aggregation token (avg|sum|min|max|count|rate|pNN). */
  aggregation: string
  /** Optional display title; falls back to the metric name. */
  title?: string | null
  /** ISO start bound — memoize upstream so the query key stays stable. */
  fromIso: string
  /** ISO end bound — memoize upstream so the query key stays stable. */
  toIso: string
  /** Canonical ClickHouse bucket interval (e.g. "15 minutes"). */
  bucketInterval: string
  /** Chart body height in px. */
  height?: number
}

/**
 * A single dashboard metric tile: fetches a time-bucketed series for one metric
 * + aggregation and renders it as a compact line chart. Reuses the exact query
 * shape and histogram-percentile recomputation from MetricsExplorer so saved
 * dashboards render identically to the ad-hoc explorer.
 *
 * Time bounds are passed in as already-memoized ISO strings — callers MUST NOT
 * pass an inline `new Date()` (that bug was fixed in MetricsExplorer: a fresh
 * Date every render changes the query key and spins React Query into an
 * infinite refetch loop).
 */
export function MetricTile({
  project,
  metricName,
  aggregation,
  title,
  fromIso,
  toIso,
  bucketInterval,
  height = 160,
}: MetricTileProps) {
  const isPercentile = aggregation.startsWith('p')

  const q = useQuery({
    ...queryMetricsOptions({
      query: {
        project_id: project.id,
        metric_name: metricName,
        start_time: fromIso,
        end_time: toIso,
        bucket_interval: bucketInterval,
        // AggKind strings map directly to the backend's MetricAggregation::parse.
        aggregation,
        limit: 500,
      },
    }),
    enabled: !!project.id && metricName.length > 0,
  })

  const buckets = q.data?.data ?? []
  const isHistogram = buckets.some((b) => b.histogram_summary)

  const data = useMemo(
    () =>
      buckets.map((b) => {
        const hs = b.histogram_summary
        const value =
          isPercentile && hs && hs.bounds.length > 0
            ? histogramQuantile(
                hs.bounds,
                hs.bucket_counts,
                percentileFromAgg(aggregation),
              )
            : (b.value ?? b.avg_value)
        return { label: formatBucketLabel(b.bucket), value }
      }),
    [buckets, isPercentile, aggregation],
  )

  const latest = data.length ? data[data.length - 1].value : null
  const displayTitle = title?.trim() ? title : metricName

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-card p-3">
      <div className="flex items-center justify-between gap-2">
        <span
          className="truncate font-mono text-xs font-medium"
          title={metricName}
        >
          {displayTitle}
        </span>
        <div className="flex shrink-0 items-center gap-1">
          {isHistogram && (
            <Badge variant="outline" className="text-[10px]">
              histogram
            </Badge>
          )}
          <Badge variant="secondary" className="text-[10px]">
            {aggregationLabel(aggregation)}
          </Badge>
        </div>
      </div>

      {q.isPending ? (
        <Skeleton className="w-full" style={{ height }} />
      ) : q.isError ? (
        <div
          className="flex items-center justify-center text-center text-xs text-rose-500"
          style={{ height }}
        >
          Failed to load metric
        </div>
      ) : data.length === 0 ? (
        <div
          className="flex items-center justify-center text-center text-xs text-muted-foreground"
          style={{ height }}
        >
          No data in range
        </div>
      ) : (
        <ThresholdLineChart
          data={data}
          xKey="label"
          series={{
            dataKey: 'value',
            label: aggregationLabel(aggregation),
            tone: 'primary',
          }}
          height={height}
          tooltipValueFormatter={(v) => formatMetricValue(v)}
          yTickFormatter={(v) => formatMetricValue(v)}
        />
      )}

      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span className="text-[10px] uppercase tracking-wide">latest</span>
        <span className="font-mono tabular-nums">
          {latest != null ? formatMetricValue(latest) : '—'}
        </span>
      </div>
    </div>
  )
}
