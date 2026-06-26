import { EnvironmentResponse, ProjectResponse } from '@/api/client'
import {
  getEnvironmentsOptions,
  listMetricNamesOptions,
  queryMetricsOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { EmptyState } from '@/components/ui/empty-state'
import { ThresholdLineChart } from '@/components/charts/threshold-line-chart'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useQuery } from '@tanstack/react-query'
import { format } from 'date-fns'
import {
  Gauge,
  LineChart as LineChartIcon,
  Plus,
  RefreshCw,
  Search,
  Tag,
  X,
} from 'lucide-react'
import { useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'

interface MetricsExplorerProps {
  project: ProjectResponse
}

// ── Time ranges ────────────────────────────────────────────────────────────

type TimeRange = '1h' | '6h' | '24h' | '7d' | '30d'

const TIME_RANGES: { value: TimeRange; label: string }[] = [
  { value: '1h', label: 'Last hour' },
  { value: '6h', label: 'Last 6 hours' },
  { value: '24h', label: 'Last 24 hours' },
  { value: '7d', label: 'Last 7 days' },
  { value: '30d', label: 'Last 30 days' },
]

/**
 * Map a time range to a sensible default ClickHouse-side bucket interval.
 *
 * The backend (`query_metrics`) parses these through a strict allowlist
 * (`translate_bucket_interval`), so only the canonical `"N unit"` strings are
 * accepted — keep this map in lockstep with that allowlist.
 */
const RANGE_BUCKET: Record<TimeRange, string> = {
  '1h': '1 minute',
  '6h': '5 minutes',
  '24h': '15 minutes',
  '7d': '1 hour',
  '30d': '6 hours',
}

function timeRangeToFrom(range: TimeRange): Date {
  const now = Date.now()
  const map: Record<TimeRange, number> = {
    '1h': 60 * 60 * 1000,
    '6h': 6 * 60 * 60 * 1000,
    '24h': 24 * 60 * 60 * 1000,
    '7d': 7 * 24 * 60 * 60 * 1000,
    '30d': 30 * 24 * 60 * 60 * 1000,
  }
  return new Date(now - map[range])
}

// ── Aggregation selector ─────────────────────────────────────────────────────
//
// These mirror the FROZEN store-neutral contract from Phase C
// (`MetricAggregation` in temps-otel/src/types.rs). The backend accepts the
// string form via the `aggregation` query param. NOTE: the `aggregation`
// param is NOT yet present in the generated `QueryMetricsData` type — it only
// lands after the SDK is regenerated against the Phase C backend (see the
// REGEN TODO near the query below). Until then the selector is presentational
// and the chart falls back to the average series the current SDK returns.

type AggKind =
  | 'avg'
  | 'sum'
  | 'min'
  | 'max'
  | 'count'
  | 'rate'
  | 'p50'
  | 'p90'
  | 'p95'
  | 'p99'

const AGGREGATIONS: { value: AggKind; label: string }[] = [
  { value: 'avg', label: 'Average' },
  { value: 'sum', label: 'Sum' },
  { value: 'min', label: 'Min' },
  { value: 'max', label: 'Max' },
  { value: 'count', label: 'Count' },
  { value: 'rate', label: 'Rate / sec' },
  { value: 'p50', label: 'p50' },
  { value: 'p90', label: 'p90' },
  { value: 'p95', label: 'p95' },
  { value: 'p99', label: 'p99' },
]

// ── Label filters ────────────────────────────────────────────────────────────

interface LabelFilter {
  key: string
  value: string
}

/** Serialize label filters to a compact, regen-ready URL string: `k=v,k2=v2`. */
function serializeLabelFilters(filters: LabelFilter[]): string {
  return filters
    .filter((f) => f.key.trim().length > 0)
    .map((f) => `${f.key.trim()}=${f.value.trim()}`)
    .join(',')
}

function parseLabelFilters(raw: string | null): LabelFilter[] {
  if (!raw) return []
  return raw
    .split(',')
    .map((pair) => pair.trim())
    .filter(Boolean)
    .map((pair) => {
      const idx = pair.indexOf('=')
      if (idx === -1) return { key: pair, value: '' }
      return { key: pair.slice(0, idx), value: pair.slice(idx + 1) }
    })
}

// ── Page ─────────────────────────────────────────────────────────────────────

/**
 * OTel metrics explorer — mirrors the TracesList pattern (metric-name
 * selector + filter builder + time range) but renders a time-bucketed line
 * chart instead of a row list.
 *
 * Backend wiring: bound to the generated SDK (`listMetricNames`,
 * `queryMetrics`). The richer Phase C query surface (per-series `group_by`,
 * typed `aggregation`, `metric_type`, `label_filters`, and the richer
 * `OtelMetricBucket` response with `value`/`quantiles`/`series_key`) is
 * SCAFFOLDED here but commented as a REGEN TODO because the generated SDK in
 * this worktree predates the Phase C handler changes. Project rule: always use
 * the generated SDK — never hand-roll a fetch — so the new params stay
 * commented until the SDK is regenerated against a running server:
 *
 *   1. Start the server (see start-temps skill).
 *   2. Mint a local admin API key:  bunx @temps-sdk/cli api-key create ...
 *   3. cd web && bun run openapi-ts        # regenerates src/api/client
 *   4. Revoke the temporary key.
 *
 * After regen the renamed types (OtelMetricsResponse / OtelMetricBucket /
 * OtelMetricNamesResponse) and the new query params become available and the
 * TODO blocks below can be un-commented.
 */
export default function MetricsExplorer({ project }: MetricsExplorerProps) {
  usePageTitle(`Metrics · ${project.name}`)
  const [searchParams, setSearchParams] = useSearchParams()

  // ── URL-backed filter state ──
  const metricName = searchParams.get('metric') ?? ''
  const serviceName = searchParams.get('service') ?? ''
  const timeRange = ((): TimeRange => {
    const r = searchParams.get('range') as TimeRange | null
    return r && TIME_RANGES.some((t) => t.value === r) ? r : '24h'
  })()
  const environmentId = searchParams.get('environment_id')
    ? Number(searchParams.get('environment_id'))
    : null
  const aggregation = ((): AggKind => {
    const a = searchParams.get('agg') as AggKind | null
    return a && AGGREGATIONS.some((x) => x.value === a) ? a : 'avg'
  })()
  const labelFilters = useMemo(
    () => parseLabelFilters(searchParams.get('labels')),
    [searchParams],
  )

  const [nameSearch, setNameSearch] = useState('')

  const patchParams = (mutate: (p: URLSearchParams) => void) => {
    const params = new URLSearchParams(searchParams)
    mutate(params)
    setSearchParams(params, { replace: true })
  }

  const setMetricName = (v: string) =>
    patchParams((p) => (v ? p.set('metric', v) : p.delete('metric')))
  const setServiceName = (v: string) =>
    patchParams((p) => (v ? p.set('service', v) : p.delete('service')))
  const setTimeRange = (v: TimeRange) =>
    patchParams((p) => (v === '24h' ? p.delete('range') : p.set('range', v)))
  const setEnvironmentId = (v: number | null) =>
    patchParams((p) =>
      v == null ? p.delete('environment_id') : p.set('environment_id', String(v)),
    )
  const setAggregation = (v: AggKind) =>
    patchParams((p) => (v === 'avg' ? p.delete('agg') : p.set('agg', v)))
  const setLabelFilters = (next: LabelFilter[]) =>
    patchParams((p) => {
      const s = serializeLabelFilters(next)
      s ? p.set('labels', s) : p.delete('labels')
    })

  // ── Environments (for the env selector, mirrors TracesList) ──
  const { data: environments } = useQuery({
    ...getEnvironmentsOptions({ path: { project_id: project.id } }),
    enabled: !!project.id,
  })

  // ── Metric-name catalogue ──
  const namesQuery = useQuery({
    ...listMetricNamesOptions({ path: { project_id: project.id } }),
    enabled: !!project.id,
  })

  const allNames = namesQuery.data?.names ?? []
  const filteredNames = useMemo(() => {
    const q = nameSearch.trim().toLowerCase()
    if (!q) return allNames
    return allNames.filter((n) => n.toLowerCase().includes(q))
  }, [allNames, nameSearch])

  // ── Time-bucketed series ──
  const fromDate = useMemo(() => timeRangeToFrom(timeRange), [timeRange])
  const selectedEnv = environments?.find((e) => e.id === environmentId) ?? null

  const metricsQuery = useQuery({
    ...queryMetricsOptions({
      query: {
        project_id: project.id,
        metric_name: metricName || undefined,
        service_name: serviceName || undefined,
        environment: selectedEnv?.name || undefined,
        start_time: fromDate.toISOString(),
        end_time: new Date().toISOString(),
        bucket_interval: RANGE_BUCKET[timeRange],
        limit: 1000,
        // ── REGEN TODO (Phase C query surface) ──
        // The following params exist on the backend handler
        // (`MetricQueryParams` in query_handler.rs) but are NOT yet on the
        // generated `QueryMetricsData` type in this worktree. Un-comment
        // after `bun run openapi-ts` regenerates the SDK against a running
        // Phase C server:
        //
        //   metric_type: undefined,        // 'gauge' | 'sum' | 'histogram' | ...
        //   aggregation,                   // AggKind string ('avg' | 'p95' | 'rate' | ...)
        //   label_filters: serializeLabelFilters(labelFilters) || undefined,
        //   group_by: undefined,           // comma-separated label keys → per-series buckets
        //
        // and the response narrows from `MetricsResponse`/`MetricBucket` to
        // the richer `OtelMetricsResponse`/`OtelMetricBucket`
        // (value / quantiles / histogram_summary / series_key).
      },
    }),
    // Only fetch once a concrete metric is chosen — querying every metric at
    // once would be expensive and meaningless to chart.
    enabled: !!project.id && metricName.length > 0,
  })

  const buckets = metricsQuery.data?.data ?? []

  // Map the generated `MetricBucket` rows into the recharts point shape.
  //
  // REGEN NOTE: with the current (pre-Phase-C) SDK the only series available is
  // `avg_value`. After regen, `OtelMetricBucket.value` carries the requested
  // aggregation (avg/sum/p95/rate/…) and `series_key` enables multi-series
  // rendering. Until then we always plot the average and the aggregation
  // selector is advisory.
  const chartData = useMemo(
    () =>
      buckets.map((b) => ({
        bucket: b.bucket,
        // `value` is the regen-ready aggregation field; the current SDK only
        // ships avg_value, so fall back to it. `(b as { value?: number })`
        // keeps TS happy before regen widens the type.
        value: (b as { value?: number }).value ?? b.avg_value,
        avg_value: b.avg_value,
        min_value: b.min_value,
        max_value: b.max_value,
        count: b.count,
      })),
    [buckets],
  )

  const aggLabel =
    AGGREGATIONS.find((a) => a.value === aggregation)?.label ?? 'Average'

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-4">
      {/* Header */}
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-col gap-1">
          <div className="flex items-center gap-2">
            <Gauge className="size-5 text-muted-foreground" />
            <h1 className="text-lg font-semibold tracking-tight">Metrics</h1>
          </div>
          <p className="text-sm text-muted-foreground">
            Explore OpenTelemetry metrics for {project.name}.
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            namesQuery.refetch()
            if (metricName) metricsQuery.refetch()
          }}
          className="gap-1.5 self-start"
        >
          <RefreshCw
            className={
              metricsQuery.isFetching || namesQuery.isFetching
                ? 'size-3.5 animate-spin'
                : 'size-3.5'
            }
          />
          Refresh
        </Button>
      </div>

      {/* Filter bar */}
      <div className="flex flex-col gap-3 rounded-lg border border-border bg-card p-3">
        <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-end">
          {/* Metric-name selector with inline search */}
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <label className="text-xs font-medium text-muted-foreground">
              Metric
            </label>
            <Select value={metricName} onValueChange={setMetricName}>
              <SelectTrigger className="w-full font-mono sm:w-[280px]">
                <SelectValue placeholder="Select a metric…" />
              </SelectTrigger>
              <SelectContent>
                <div className="sticky top-0 z-10 bg-popover p-1.5">
                  <div className="relative">
                    <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                    <Input
                      value={nameSearch}
                      onChange={(e) => setNameSearch(e.target.value)}
                      onKeyDown={(e) => e.stopPropagation()}
                      placeholder="Filter metrics…"
                      className="h-8 pl-7 font-mono text-xs"
                    />
                  </div>
                </div>
                {namesQuery.isPending ? (
                  <div className="p-2">
                    <Skeleton className="h-5 w-full" />
                  </div>
                ) : filteredNames.length === 0 ? (
                  <div className="p-3 text-center text-xs text-muted-foreground">
                    {allNames.length === 0
                      ? 'No metrics ingested yet.'
                      : 'No metrics match your search.'}
                  </div>
                ) : (
                  filteredNames.map((n) => (
                    <SelectItem key={n} value={n} className="font-mono text-xs">
                      {n}
                    </SelectItem>
                  ))
                )}
              </SelectContent>
            </Select>
          </div>

          {/* Aggregation */}
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium text-muted-foreground">
              Aggregation
            </label>
            <Select
              value={aggregation}
              onValueChange={(v) => setAggregation(v as AggKind)}
            >
              <SelectTrigger className="w-full font-mono sm:w-[140px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {AGGREGATIONS.map((a) => (
                  <SelectItem key={a.value} value={a.value}>
                    {a.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {/* Environment */}
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium text-muted-foreground">
              Environment
            </label>
            <Select
              value={environmentId != null ? String(environmentId) : 'all'}
              onValueChange={(v) =>
                setEnvironmentId(v === 'all' ? null : Number(v))
              }
            >
              <SelectTrigger className="w-full sm:w-[160px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All environments</SelectItem>
                {(environments ?? []).map((env: EnvironmentResponse) => (
                  <SelectItem key={env.id} value={String(env.id)}>
                    {env.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {/* Time range */}
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium text-muted-foreground">
              Range
            </label>
            <Select
              value={timeRange}
              onValueChange={(v) => setTimeRange(v as TimeRange)}
            >
              <SelectTrigger className="w-full sm:w-[160px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {TIME_RANGES.map((r) => (
                  <SelectItem key={r.value} value={r.value}>
                    {r.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>

        {/* Service-name filter + label-filter builder */}
        <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center">
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={serviceName}
              onChange={(e) => setServiceName(e.target.value)}
              placeholder="Filter by service name…"
              className="pl-8 font-mono text-xs"
            />
          </div>
        </div>

        {/* Label filters (regen-ready, sent once SDK exposes label_filters) */}
        <LabelFilterBuilder value={labelFilters} onChange={setLabelFilters} />
      </div>

      {/* Chart / states */}
      <div className="rounded-lg border border-border bg-card p-4">
        <MetricChart
          metricName={metricName}
          aggLabel={aggLabel}
          isPending={metricsQuery.isPending && metricName.length > 0}
          isError={metricsQuery.isError}
          errorMessage={
            metricsQuery.error instanceof Error
              ? metricsQuery.error.message
              : 'Failed to load metric series.'
          }
          chartData={chartData}
        />
      </div>
    </div>
  )
}

// ── Label filter builder ─────────────────────────────────────────────────────

function LabelFilterBuilder({
  value,
  onChange,
}: {
  value: LabelFilter[]
  onChange: (next: LabelFilter[]) => void
}) {
  const add = () => onChange([...value, { key: '', value: '' }])
  const update = (i: number, patch: Partial<LabelFilter>) =>
    onChange(value.map((f, idx) => (idx === i ? { ...f, ...patch } : f)))
  const remove = (i: number) => onChange(value.filter((_, idx) => idx !== i))

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Tag className="size-3.5" />
          Label filters
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={add}
          className="h-7 gap-1 px-2 text-xs"
        >
          <Plus className="size-3" />
          Add
        </Button>
      </div>
      {value.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No label filters. Add key/value pairs to narrow the series by
          attribute (e.g. <span className="font-mono">http.method = GET</span>).
        </p>
      ) : (
        <div className="flex flex-col gap-1.5">
          {value.map((f, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <Input
                value={f.key}
                onChange={(e) => update(i, { key: e.target.value })}
                placeholder="label.key"
                className="h-8 font-mono text-xs sm:w-[200px]"
              />
              <span className="text-muted-foreground">=</span>
              <Input
                value={f.value}
                onChange={(e) => update(i, { value: e.target.value })}
                placeholder="value"
                className="h-8 flex-1 font-mono text-xs"
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                onClick={() => remove(i)}
                className="h-8 w-8 shrink-0"
                aria-label="Remove label filter"
              >
                <X className="size-3.5" />
              </Button>
            </div>
          ))}
          <p className="text-[11px] text-muted-foreground">
            Label filters are applied server-side once the SDK is regenerated
            against the Phase C backend (the <span className="font-mono">label_filters</span>{' '}
            query param). They are URL-persisted now so the view stays shareable.
          </p>
        </div>
      )}
    </div>
  )
}

// ── Chart + states ───────────────────────────────────────────────────────────

interface ChartPoint {
  bucket: string
  value: number
  avg_value: number
  min_value: number
  max_value: number
  count: number
}

function MetricChart({
  metricName,
  aggLabel,
  isPending,
  isError,
  errorMessage,
  chartData,
}: {
  metricName: string
  aggLabel: string
  isPending: boolean
  isError: boolean
  errorMessage: string
  chartData: ChartPoint[]
}) {
  if (!metricName) {
    return (
      <EmptyState
        icon={LineChartIcon}
        title="Pick a metric to chart"
        description="Choose a metric name from the selector above to render its time series. Series are bucketed server-side over the selected range."
      />
    )
  }

  if (isPending) {
    return <Skeleton className="h-[300px] w-full" />
  }

  if (isError) {
    return (
      <div className="flex h-[300px] flex-col items-center justify-center gap-2 text-center">
        <p className="text-sm font-medium text-rose-500">
          Failed to load metric series
        </p>
        <p className="text-xs text-muted-foreground">{errorMessage}</p>
      </div>
    )
  }

  if (chartData.length === 0) {
    return (
      <EmptyState
        icon={LineChartIcon}
        title="No data in range"
        description="This metric has no samples in the selected time range. Try widening the range or clearing filters."
      />
    )
  }

  // Format bucket timestamps for the X axis. Buckets arrive as ISO strings
  // from the backend; render a compact HH:mm (or date for wide ranges).
  const xData = chartData.map((p) => ({
    ...p,
    label: formatBucketLabel(p.bucket),
  }))

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <Badge variant="secondary" className="font-mono text-xs">
          {metricName}
        </Badge>
        <span className="text-xs text-muted-foreground">{aggLabel}</span>
      </div>
      <ThresholdLineChart
        data={xData}
        xKey="label"
        series={{ dataKey: 'value', label: aggLabel, tone: 'primary' }}
        height={320}
        tooltipValueFormatter={(v) => formatMetricValue(v)}
        yTickFormatter={(v) => formatMetricValue(v)}
      />
    </div>
  )
}

function formatBucketLabel(bucket: string): string {
  const d = new Date(bucket)
  if (Number.isNaN(d.getTime())) return bucket
  return format(d, 'MMM d HH:mm')
}

function formatMetricValue(v: number): string {
  if (!Number.isFinite(v)) return '—'
  const abs = Math.abs(v)
  if (abs >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`
  if (abs >= 1_000) return `${(v / 1_000).toFixed(2)}k`
  if (abs >= 1) return v.toFixed(2)
  return v.toPrecision(3)
}
