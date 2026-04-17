import {
  getProjectsOptions,
  getTimeBucketStatsOptions,
  getProjectsHealthOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { ProjectResponse } from '@/api/client/types.gen'
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  ChartConfig,
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  ChartLegend,
  ChartLegendContent,
} from '@/components/ui/chart'
import { Skeleton } from '@/components/ui/skeleton'
import { useQuery } from '@tanstack/react-query'
import { format, subHours } from 'date-fns'
import { useMemo, useState } from 'react'
import type { DateRange } from 'react-day-picker'
import { DateRangePicker } from '@/components/ui/date-range-picker'
import { Line, LineChart, XAxis, YAxis, CartesianGrid } from 'recharts'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Activity,
  CheckCircle2,
  AlertTriangle,
  XCircle,
  HelpCircle,
  TrendingUp,
  TrendingDown,
  Minus,
} from 'lucide-react'
import { cn } from '@/lib/utils'

// ── Chart configs ────────────────────────────────────────────────────

const requestsChartConfig = {
  requests: {
    label: 'Requests',
    color: 'var(--chart-1)',
  },
  errors: {
    label: 'Errors',
    color: 'hsl(0 84% 60%)',
  },
} satisfies ChartConfig

const responseTimeChartConfig = {
  response_time: {
    label: 'Avg Response Time (ms)',
    color: 'var(--chart-4)',
  },
} satisfies ChartConfig

// ── Types ────────────────────────────────────────────────────────────

type TimeRange = '1h' | '6h' | '24h' | 'custom'

interface ResolvedRange {
  start: Date
  end: Date
  /** Short, user-facing label like "last 1h" or "Apr 10–16". */
  label: string
  /** TimescaleDB bucket interval matching the window size. */
  bucketInterval: string
}

const PRESET_LABEL: Record<Exclude<TimeRange, 'custom'>, string> = {
  '1h': 'last 1h',
  '6h': 'last 6h',
  '24h': 'last 24h',
}

function pickBucketInterval(hours: number): string {
  if (hours <= 1) return '5 minutes'
  if (hours <= 6) return '30 minutes'
  if (hours <= 48) return '1 hour'
  if (hours <= 24 * 14) return '6 hours'
  return '1 day'
}

function resolveRange(
  timeRange: TimeRange,
  custom: DateRange | undefined,
  now: Date
): ResolvedRange {
  if (timeRange === 'custom' && custom?.from && custom?.to) {
    const hours = (custom.to.getTime() - custom.from.getTime()) / 3_600_000
    const sameDay =
      custom.from.toDateString() === custom.to.toDateString()
    const label = sameDay
      ? `${format(custom.from, 'MMM d, HH:mm')}–${format(custom.to, 'HH:mm')}`
      : `${format(custom.from, 'MMM d')}–${format(custom.to, 'MMM d')}`
    return {
      start: custom.from,
      end: custom.to,
      label,
      bucketInterval: pickBucketInterval(Math.max(hours, 0.25)),
    }
  }
  const preset: Exclude<TimeRange, 'custom'> =
    timeRange === 'custom' ? '24h' : timeRange
  const hours = preset === '1h' ? 1 : preset === '6h' ? 6 : 24
  return {
    start: subHours(now, hours),
    end: now,
    label: PRESET_LABEL[preset],
    bucketInterval: pickBucketInterval(hours),
  }
}

// ── Health status helpers ────────────────────────────────────────────

function getStatusConfig(status: string) {
  switch (status) {
    case 'healthy':
      return {
        icon: CheckCircle2,
        color: 'text-green-600',
        bg: 'bg-green-500/10',
        border: 'border-green-500/20',
        label: 'Healthy',
      }
    case 'degraded':
      return {
        icon: AlertTriangle,
        color: 'text-yellow-600',
        bg: 'bg-yellow-500/10',
        border: 'border-yellow-500/20',
        label: 'Degraded',
      }
    case 'down':
      return {
        icon: XCircle,
        color: 'text-red-600',
        bg: 'bg-red-500/10',
        border: 'border-red-500/20',
        label: 'Down',
      }
    default:
      return {
        icon: HelpCircle,
        color: 'text-muted-foreground',
        bg: 'bg-muted/50',
        border: 'border-muted',
        label: 'Unknown',
      }
  }
}

// ── Project Health Card ─────────────────────────────────────────────

function ProjectHealthCard({
  project,
  health,
}: {
  project: ProjectResponse
  health?: {
    status: string
    total_requests: number
    total_errors: number
    error_rate: number
    avg_response_time_ms: number
  }
}) {
  const status = health?.status ?? 'unknown'
  const config = getStatusConfig(status)
  const StatusIcon = config.icon

  return (
    <Card className={cn('transition-colors', config.border)}>
      <CardContent className="p-4">
        <div className="flex items-start justify-between mb-3">
          <div className="min-w-0">
            <p className="font-medium text-sm truncate">{project.name}</p>
            {project.preset && (
              <p className="text-xs text-muted-foreground">{project.preset}</p>
            )}
          </div>
          <div
            className={cn(
              'flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium',
              config.bg,
              config.color
            )}
          >
            <StatusIcon className="h-3 w-3" />
            {config.label}
          </div>
        </div>

        {health && health.status !== 'unknown' ? (
          <div className="grid grid-cols-3 gap-3">
            <div>
              <p className="text-xs text-muted-foreground">Requests</p>
              <p className="text-sm font-semibold tabular-nums">
                {health.total_requests.toLocaleString()}
              </p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground">Error Rate</p>
              <p
                className={cn(
                  'text-sm font-semibold tabular-nums',
                  health.error_rate > 5
                    ? 'text-red-600'
                    : health.error_rate > 1
                      ? 'text-yellow-600'
                      : ''
                )}
              >
                {health.error_rate.toFixed(1)}%
              </p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground">Avg Response</p>
              <p className="text-sm font-semibold tabular-nums">
                {health.avg_response_time_ms.toFixed(0)}ms
              </p>
            </div>
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">No traffic data</p>
        )}
      </CardContent>
    </Card>
  )
}

// ── Summary Stat Card ───────────────────────────────────────────────

function SummaryStatCard({
  label,
  value,
  subValue,
  trend,
}: {
  label: string
  value: string
  subValue?: string
  trend?: 'up' | 'down' | 'neutral'
}) {
  const TrendIcon =
    trend === 'up'
      ? TrendingUp
      : trend === 'down'
        ? TrendingDown
        : Minus

  return (
    <Card>
      <CardContent className="p-4">
        <p className="text-xs text-muted-foreground mb-1">{label}</p>
        <div className="flex items-baseline gap-2">
          <p className="text-2xl font-bold tabular-nums">{value}</p>
          {trend && (
            <TrendIcon
              className={cn(
                'h-4 w-4',
                trend === 'up' && 'text-red-500',
                trend === 'down' && 'text-green-500',
                trend === 'neutral' && 'text-muted-foreground'
              )}
            />
          )}
        </div>
        {subValue && (
          <p className="text-xs text-muted-foreground mt-0.5">{subValue}</p>
        )}
      </CardContent>
    </Card>
  )
}

// ── Simplified Trend Charts ─────────────────────────────────────────

function TrendCharts({
  projectId,
  range,
  hideBots,
}: {
  projectId?: number
  range: ResolvedRange
  hideBots: boolean
}) {
  const { data, isLoading } = useQuery({
    ...getTimeBucketStatsOptions({
      query: {
        start_time: range.start.toISOString(),
        end_time: range.end.toISOString(),
        bucket_interval: range.bucketInterval,
        project_id: projectId,
        // When no project is selected, match the summary cards which only
        // aggregate project-routed requests (project_id IS NOT NULL).
        has_project: projectId === undefined ? true : undefined,
        is_bot: hideBots ? false : undefined,
      },
    }),
    refetchInterval: 30000,
  })

  const chartData = useMemo(() => {
    if (!data?.stats) return []
    return data.stats.map((s) => ({
      time: format(new Date(s.bucket), 'HH:mm'),
      requests: s.request_count,
      errors: s.error_count,
      response_time: Math.round(s.avg_response_time_ms * 10) / 10,
    }))
  }, [data])

  const totals = useMemo(() => {
    if (!data?.stats) return { requests: 0, errors: 0, avgResponse: 0 }
    const requests = data.stats.reduce((s, b) => s + b.request_count, 0)
    const errors = data.stats.reduce((s, b) => s + b.error_count, 0)
    const withTime = data.stats.filter((b) => b.avg_response_time_ms > 0)
    const avgResponse =
      withTime.length > 0
        ? withTime.reduce((s, b) => s + b.avg_response_time_ms, 0) /
          withTime.length
        : 0
    return { requests, errors, avgResponse }
  }, [data])

  if (isLoading) {
    return (
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <Skeleton className="h-[300px] w-full" />
        <Skeleton className="h-[300px] w-full" />
      </div>
    )
  }

  if (!chartData.length) {
    return (
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <Card>
          <CardContent className="flex items-center justify-center h-[300px] text-sm text-muted-foreground">
            No request data for this period
          </CardContent>
        </Card>
        <Card>
          <CardContent className="flex items-center justify-center h-[300px] text-sm text-muted-foreground">
            No response time data for this period
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
      {/* Requests & Errors */}
      <Card>
        <CardHeader className="pb-2">
          <div className="flex items-center justify-between">
            <CardTitle className="text-base">Requests & Errors</CardTitle>
            <div className="flex items-center gap-3 text-sm">
              <span>
                <span className="font-semibold">
                  {totals.requests.toLocaleString()}
                </span>{' '}
                <span className="text-muted-foreground">total</span>
              </span>
              {totals.errors > 0 && (
                <span>
                  <span className="font-semibold text-destructive">
                    {totals.errors.toLocaleString()}
                  </span>{' '}
                  <span className="text-muted-foreground">errors</span>
                </span>
              )}
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <ChartContainer
            config={requestsChartConfig}
            className="h-[240px] w-full"
          >
            <LineChart
              data={chartData}
              margin={{ left: 12, right: 12, top: 8, bottom: 0 }}
            >
              <CartesianGrid
                strokeDasharray="3 3"
                vertical={false}
                className="stroke-muted/30"
              />
              <XAxis
                dataKey="time"
                tickLine={false}
                axisLine={false}
                tickMargin={8}
                minTickGap={40}
                tick={{ fontSize: 11 }}
              />
              <YAxis
                tickLine={false}
                axisLine={false}
                tickMargin={8}
                tick={{ fontSize: 11 }}
                width={50}
                tickFormatter={(v) => v.toLocaleString()}
              />
              <ChartTooltip content={<ChartTooltipContent />} />
              <ChartLegend content={<ChartLegendContent />} />
              <Line
                dataKey="requests"
                type="monotone"
                stroke="var(--color-requests)"
                strokeWidth={2}
                dot={false}
                activeDot={{ r: 4 }}
              />
              <Line
                dataKey="errors"
                type="monotone"
                stroke="var(--color-errors)"
                strokeWidth={2}
                dot={false}
                activeDot={{ r: 4 }}
              />
            </LineChart>
          </ChartContainer>
        </CardContent>
      </Card>

      {/* Response Time */}
      <Card>
        <CardHeader className="pb-2">
          <div className="flex items-center justify-between">
            <CardTitle className="text-base">Response Time</CardTitle>
            <span className="text-sm">
              <span className="font-semibold">
                {totals.avgResponse.toFixed(0)}
              </span>{' '}
              <span className="text-muted-foreground">ms avg</span>
            </span>
          </div>
        </CardHeader>
        <CardContent>
          <ChartContainer
            config={responseTimeChartConfig}
            className="h-[240px] w-full"
          >
            <LineChart
              data={chartData}
              margin={{ left: 12, right: 12, top: 8, bottom: 0 }}
            >
              <CartesianGrid
                strokeDasharray="3 3"
                vertical={false}
                className="stroke-muted/30"
              />
              <XAxis
                dataKey="time"
                tickLine={false}
                axisLine={false}
                tickMargin={8}
                minTickGap={40}
                tick={{ fontSize: 11 }}
              />
              <YAxis
                tickLine={false}
                axisLine={false}
                tickMargin={8}
                tick={{ fontSize: 11 }}
                width={50}
                tickFormatter={(v) => `${v}ms`}
              />
              <ChartTooltip
                content={
                  <ChartTooltipContent
                    formatter={(value) => [
                      `${Number(value).toFixed(1)} ms`,
                      'Response Time',
                    ]}
                  />
                }
              />
              <Line
                dataKey="response_time"
                type="monotone"
                stroke="var(--color-response_time)"
                strokeWidth={2}
                dot={false}
                activeDot={{ r: 4 }}
              />
            </LineChart>
          </ChartContainer>
        </CardContent>
      </Card>
    </div>
  )
}

// ── Main page ────────────────────────────────────────────────────────

export function ResourceMonitoring() {
  const [timeRange, setTimeRange] = useState<TimeRange>('24h')
  const [customRange, setCustomRange] = useState<DateRange | undefined>(() => {
    const end = new Date()
    const start = subHours(end, 24)
    return { from: start, to: end }
  })
  const [selectedProjectId, setSelectedProjectId] = useState<string>('all')
  const [hideBots, setHideBots] = useState<boolean>(true)

  // For preset ranges (1h/6h/24h), "now" should tick forward with the
  // refetch interval so the window stays live. We snap it to the nearest
  // 30s so identical ticks produce stable query keys instead of a new
  // one every render.
  const now = useMemo(
    () => {
      const d = new Date()
      d.setSeconds(Math.floor(d.getSeconds() / 30) * 30, 0)
      return d
    },
    // Re-evaluate whenever the user switches preset/custom and whenever
    // react-query refetches (30s interval). A ref to timeRange is enough
    // — we intentionally do not depend on a ticking clock here.
    [timeRange]
  )

  const range = useMemo(
    () => resolveRange(timeRange, customRange, now),
    [timeRange, customRange, now]
  )

  const { data: projectsData, isLoading: projectsLoading } = useQuery({
    ...getProjectsOptions({ query: { page: 1, per_page: 100 } }),
  })

  const projects = projectsData?.projects ?? []
  const projectIds = useMemo(
    () => projects.map((p) => p.id),
    [projects]
  )

  // Fetch health summary for all projects, over the selected window
  const { data: healthData } = useQuery({
    ...getProjectsHealthOptions({
      query: {
        project_ids: projectIds.join(','),
        start_time: range.start.toISOString(),
        end_time: range.end.toISOString(),
        is_bot: hideBots ? false : undefined,
      },
    }),
    enabled: projectIds.length > 0,
    refetchInterval: timeRange === 'custom' ? false : 30000,
  })

  const healthMap = healthData?.projects ?? {}

  // Compute overall summary stats
  const summary = useMemo(() => {
    const healthEntries = Object.values(healthMap)
    if (healthEntries.length === 0) {
      return {
        totalRequests: 0,
        totalErrors: 0,
        avgResponseTime: 0,
        errorRate: 0,
        healthyCount: 0,
        degradedCount: 0,
        downCount: 0,
      }
    }

    const totalRequests = healthEntries.reduce(
      (sum, h) => sum + h.total_requests,
      0
    )
    const totalErrors = healthEntries.reduce(
      (sum, h) => sum + h.total_errors,
      0
    )
    const errorRate =
      totalRequests > 0 ? (totalErrors / totalRequests) * 100 : 0
    const withTime = healthEntries.filter((h) => h.avg_response_time_ms > 0)
    const avgResponseTime =
      withTime.length > 0
        ? withTime.reduce((sum, h) => sum + h.avg_response_time_ms, 0) /
          withTime.length
        : 0

    return {
      totalRequests,
      totalErrors,
      avgResponseTime,
      errorRate,
      healthyCount: healthEntries.filter((h) => h.status === 'healthy').length,
      degradedCount: healthEntries.filter((h) => h.status === 'degraded')
        .length,
      downCount: healthEntries.filter((h) => h.status === 'down').length,
    }
  }, [healthMap])

  // Determine overall status for trend indicator
  const errorTrend: 'up' | 'down' | 'neutral' =
    summary.errorRate > 5
      ? 'up'
      : summary.errorRate > 0
        ? 'neutral'
        : 'down'

  return (
    <div className="space-y-6">
      {/* Header + Controls */}
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h3 className="text-lg font-medium">Health Overview</h3>
          <p className="text-sm text-muted-foreground">
            Is everything working? At a glance.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Select
            value={selectedProjectId}
            onValueChange={setSelectedProjectId}
          >
            <SelectTrigger className="w-[180px]">
              <SelectValue placeholder="All projects" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All projects</SelectItem>
              {projects.map((project) => (
                <SelectItem key={project.id} value={project.id.toString()}>
                  {project.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            variant={hideBots ? 'default' : 'outline'}
            size="sm"
            className="h-8 px-3 text-xs"
            onClick={() => setHideBots((v) => !v)}
            title={hideBots ? 'Bots are hidden' : 'Bots are included'}
          >
            {hideBots ? 'Hide bots' : 'Show bots'}
          </Button>
          <div className="flex items-center gap-1 rounded-md border p-0.5">
            {(['1h', '6h', '24h'] as const).map((preset) => (
              <Button
                key={preset}
                variant={timeRange === preset ? 'default' : 'ghost'}
                size="sm"
                className="h-7 px-3 text-xs"
                onClick={() => setTimeRange(preset)}
              >
                {preset}
              </Button>
            ))}
            <Button
              variant={timeRange === 'custom' ? 'default' : 'ghost'}
              size="sm"
              className="h-7 px-3 text-xs"
              onClick={() => setTimeRange('custom')}
            >
              Custom
            </Button>
          </div>
          {timeRange === 'custom' && (
            <DateRangePicker
              date={customRange}
              onDateChange={setCustomRange}
              showTime
              className="w-[300px]"
            />
          )}
        </div>
      </div>

      {projectsLoading ? (
        <div className="space-y-6">
          <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
            <Skeleton className="h-[90px] w-full" />
            <Skeleton className="h-[90px] w-full" />
            <Skeleton className="h-[90px] w-full" />
            <Skeleton className="h-[90px] w-full" />
          </div>
          <Skeleton className="h-[300px] w-full" />
        </div>
      ) : projects.length === 0 ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-16">
            <Activity className="h-10 w-10 text-muted-foreground mb-4" />
            <p className="text-sm text-muted-foreground">
              No projects found. Create a project to start monitoring.
            </p>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-6">
          {/* ── Summary Stats ── */}
          <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
            <SummaryStatCard
              label="Total Requests"
              value={summary.totalRequests.toLocaleString()}
              subValue={range.label}
            />
            <SummaryStatCard
              label="Error Rate"
              value={`${summary.errorRate.toFixed(1)}%`}
              subValue={`${summary.totalErrors.toLocaleString()} errors`}
              trend={errorTrend}
            />
            <SummaryStatCard
              label="Avg Response Time"
              value={`${summary.avgResponseTime.toFixed(0)}ms`}
              trend={
                summary.avgResponseTime > 500
                  ? 'up'
                  : summary.avgResponseTime > 0
                    ? 'neutral'
                    : 'down'
              }
            />
            <SummaryStatCard
              label="Projects Status"
              value={`${summary.healthyCount}/${projects.length}`}
              subValue={
                summary.downCount > 0
                  ? `${summary.downCount} down`
                  : summary.degradedCount > 0
                    ? `${summary.degradedCount} degraded`
                    : 'all healthy'
              }
            />
          </div>

          {/* ── Project Health Cards ── */}
          <div>
            <h4 className="text-sm font-medium mb-3">Project Status</h4>
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              {projects
                .filter(
                  (p) =>
                    selectedProjectId === 'all' ||
                    p.id.toString() === selectedProjectId
                )
                .map((project) => (
                  <ProjectHealthCard
                    key={project.id}
                    project={project}
                    health={healthMap[project.id.toString()]}
                  />
                ))}
            </div>
          </div>

          {/* ── Trend Charts ── */}
          <TrendCharts
            projectId={
              selectedProjectId !== 'all'
                ? parseInt(selectedProjectId, 10)
                : undefined
            }
            range={range}
            hideBots={hideBots}
          />
        </div>
      )}
    </div>
  )
}
