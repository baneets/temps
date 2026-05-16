'use client'

import {
  getBackupScheduleOptions,
  getS3SourceOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { listScheduleRunJobsOptions } from '@/lib/schedule-runs'
import { formatBytes } from '@/lib/utils'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { format } from 'date-fns'
import {
  ArrowLeft,
  DatabaseBackup,
  Loader2,
  RefreshCw,
} from 'lucide-react'
import { useEffect } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'

// ── Local helpers ────────────────────────────────────────────────────────────

function stateBadgeVariant(
  state: string,
): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (state) {
    case 'completed':
      return 'default'
    case 'failed':
      return 'destructive'
    case 'running':
    case 'pending':
      return 'secondary'
    default:
      return 'outline'
  }
}

/** Duration from ms: "1h 5m", "32m 10s", "45s", or "—" for invalid. */
function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '—'
  const seconds = Math.floor(ms / 1000)
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  const remSeconds = seconds % 60
  if (minutes < 60)
    return remSeconds ? `${minutes}m ${remSeconds}s` : `${minutes}m`
  const hours = Math.floor(minutes / 60)
  const remMinutes = minutes % 60
  return remMinutes ? `${hours}h ${remMinutes}m` : `${hours}h`
}

// ── Component ────────────────────────────────────────────────────────────────

export function ScheduleRunDetail() {
  const { scheduleId, runId } = useParams<{
    scheduleId: string
    runId: string
  }>()
  const scheduleIdNum = scheduleId ? parseInt(scheduleId) : undefined
  const runIdNum = runId ? parseInt(runId) : undefined
  const navigate = useNavigate()
  const { setBreadcrumbs } = useBreadcrumbs()

  // ── Data ─────────────────────────────────────────────────────────────────

  const { data: schedule, isPending: isSchedulePending } = useQuery({
    ...getBackupScheduleOptions({ path: { id: scheduleIdNum ?? 0 } }),
    enabled: scheduleIdNum !== undefined,
  })

  const { data: s3Source } = useQuery({
    ...getS3SourceOptions({
      path: { id: schedule?.s3_source_id ?? 0 },
    }),
    enabled: schedule?.s3_source_id !== undefined,
  })

  const queryClient = useQueryClient()

  const jobsQueryOptions = listScheduleRunJobsOptions(runIdNum, 1, 200)

  const {
    data: jobs,
    isPending: isJobsPending,
    isError: isJobsError,
    isFetching: isJobsFetching,
    error: jobsError,
  } = useQuery({
    ...jobsQueryOptions,
    // Auto-refresh every 5s while any child is still pending/running. The
    // predicate runs on every poll so the interval stops on its own once
    // the run reaches a terminal state — no manual cleanup needed.
    refetchInterval: (query) => {
      const data = query.state.data
      if (!data) return false
      return data.some((j) => j.state === 'pending' || j.state === 'running')
        ? 5000
        : false
    },
    refetchIntervalInBackground: false,
  })

  // ── Page meta ────────────────────────────────────────────────────────────

  usePageTitle(
    schedule ? `Run #${runIdNum} • ${schedule.name}` : `Run #${runIdNum}`,
  )

  useEffect(() => {
    if (!schedule) return
    setBreadcrumbs([
      { label: 'Backups', href: '/backups' },
      ...(s3Source
        ? [
            {
              label: s3Source.name,
              href: `/backups/s3-sources/${s3Source.id}`,
            },
          ]
        : []),
      {
        label: schedule.name,
        href: `/backups/schedules/${schedule.id}`,
      },
      { label: `Run #${runIdNum}` },
    ])
    return () => setBreadcrumbs([])
  }, [schedule, s3Source, runIdNum, setBreadcrumbs])

  // ── Derived state from jobs ──────────────────────────────────────────────

  const total = jobs?.length ?? 0
  const completed = jobs?.filter((j) => j.state === 'completed').length ?? 0
  const failed = jobs?.filter((j) => j.state === 'failed').length ?? 0
  const running = jobs?.filter((j) => j.state === 'running').length ?? 0
  const pending = jobs?.filter((j) => j.state === 'pending').length ?? 0

  const aggregateState =
    pending > 0 || running > 0
      ? 'running'
      : failed > 0
        ? 'failed'
        : total > 0
          ? 'completed'
          : 'pending'

  // First job's started_at as the run start; max finished_at as run end
  // (matches backend `ScheduleRunSummary` semantics).
  const startedAt = jobs?.length
    ? jobs.reduce(
        (min, j) => (j.started_at < min ? j.started_at : min),
        jobs[0].started_at,
      )
    : undefined

  const finishedAt =
    jobs?.length && jobs.every((j) => j.finished_at)
      ? jobs.reduce<string>((max, j) => {
          const f = j.finished_at!
          return f > max ? f : max
        }, jobs[0].finished_at!)
      : undefined

  const durationMs =
    startedAt && finishedAt
      ? new Date(finishedAt).getTime() - new Date(startedAt).getTime()
      : null

  // ── Render ───────────────────────────────────────────────────────────────

  if (scheduleIdNum === undefined || runIdNum === undefined) {
    return (
      <div className="p-6 text-sm text-destructive">
        Invalid run URL — missing schedule or run id.
      </div>
    )
  }

  return (
    <TooltipProvider>
      <div className="space-y-4 p-4 sm:p-6">
        {/* ── Header ─────────────────────────────────────────────────── */}
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <Button
              variant="ghost"
              size="icon"
              onClick={() => navigate(`/backups/schedules/${scheduleIdNum}`)}
              aria-label="Back to schedule"
            >
              <ArrowLeft className="h-4 w-4" />
            </Button>
            <div>
              <h1 className="flex items-center gap-2 text-2xl font-semibold tracking-tight">
                Run #{runIdNum}
                {jobs && (
                  <Badge variant={stateBadgeVariant(aggregateState)}>
                    {aggregateState}
                  </Badge>
                )}
              </h1>
              {isSchedulePending ? (
                <Skeleton className="mt-1 h-4 w-48" />
              ) : schedule ? (
                <p className="text-sm text-muted-foreground">
                  Scheduler tick for{' '}
                  <Link
                    to={`/backups/schedules/${schedule.id}`}
                    className="hover:underline"
                  >
                    {schedule.name}
                  </Link>
                </p>
              ) : null}
            </div>
          </div>

          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="outline"
                size="icon"
                onClick={() => {
                  void queryClient.invalidateQueries({
                    queryKey: jobsQueryOptions.queryKey,
                  })
                }}
                disabled={isJobsFetching}
                aria-label="Refresh run"
              >
                <RefreshCw
                  className={`h-4 w-4 ${isJobsFetching ? 'animate-spin' : ''}`}
                />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">
              {aggregateState === 'running' || aggregateState === 'pending'
                ? 'Refresh (auto-refreshing every 5s)'
                : 'Refresh'}
            </TooltipContent>
          </Tooltip>
        </div>

        {/* ── Summary stats ──────────────────────────────────────────── */}
        <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
          <SummaryStat
            label="Started"
            value={
              startedAt
                ? format(new Date(startedAt), 'MMM d, yyyy HH:mm:ss')
                : '—'
            }
            mono
          />
          <SummaryStat
            label="Finished"
            value={
              finishedAt
                ? format(new Date(finishedAt), 'MMM d, yyyy HH:mm:ss')
                : aggregateState === 'running'
                  ? 'Running…'
                  : '—'
            }
            mono
          />
          <SummaryStat
            label="Duration"
            value={durationMs !== null ? formatDuration(durationMs) : '—'}
          />
          <SummaryStat
            label="Jobs"
            value={
              failed > 0 ? (
                <span>
                  {completed} / {total}{' '}
                  <span className="text-destructive">({failed} failed)</span>
                </span>
              ) : (
                <span>
                  {completed} / {total}
                </span>
              )
            }
          />
        </div>

        {/* ── Jobs table ─────────────────────────────────────────────── */}
        <Card>
          <CardHeader>
            <CardTitle>Backup jobs</CardTitle>
            <CardDescription>
              One row per backup in this scheduler tick (control plane + each
              external service).
            </CardDescription>
          </CardHeader>
          <CardContent className="px-0">
            {isJobsPending ? (
              <div className="space-y-2 px-6 pb-6">
                {[...Array(4)].map((_, i) => (
                  <Skeleton key={i} className="h-10 w-full" />
                ))}
              </div>
            ) : isJobsError ? (
              <div className="px-6 pb-6 text-sm text-destructive">
                Failed to load jobs:{' '}
                {jobsError instanceof Error ? jobsError.message : 'Unknown error'}
              </div>
            ) : !jobs || jobs.length === 0 ? (
              <div className="flex flex-col items-center justify-center px-6 py-12 text-sm text-muted-foreground">
                <DatabaseBackup className="mb-3 h-8 w-8 opacity-40" />
                No child jobs recorded for this run.
              </div>
            ) : (
              <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Service</TableHead>
                      <TableHead className="hidden sm:table-cell">
                        Engine
                      </TableHead>
                      <TableHead>State</TableHead>
                      <TableHead className="hidden md:table-cell">
                        Started
                      </TableHead>
                      <TableHead className="hidden lg:table-cell">
                        Duration
                      </TableHead>
                      <TableHead className="hidden md:table-cell">
                        Size
                      </TableHead>
                      <TableHead className="hidden xl:table-cell">
                        Error
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {jobs.map((job) => {
                      const jobDurationMs =
                        job.started_at && job.finished_at
                          ? new Date(job.finished_at).getTime() -
                            new Date(job.started_at).getTime()
                          : null

                      const detailUrl = `/backups/s3-sources/${job.s3_source_id}/backups/${job.backup_uuid}`

                      return (
                        <TableRow
                          key={job.backup_id}
                          className="hover:bg-muted/50"
                        >
                          <TableCell className="font-medium">
                            <Link
                              to={detailUrl}
                              className="flex hover:underline"
                            >
                              {job.service_name}
                            </Link>
                          </TableCell>
                          <TableCell className="hidden font-mono text-xs text-muted-foreground sm:table-cell">
                            {job.engine}
                          </TableCell>
                          <TableCell>
                            <Badge variant={stateBadgeVariant(job.state)}>
                              {job.state === 'running' ||
                              job.state === 'pending' ? (
                                <span className="flex items-center gap-1">
                                  <Loader2 className="h-3 w-3 animate-spin" />
                                  {job.state}
                                </span>
                              ) : (
                                job.state
                              )}
                            </Badge>
                          </TableCell>
                          <TableCell className="hidden font-mono text-xs text-muted-foreground md:table-cell">
                            {format(
                              new Date(job.started_at),
                              'MMM d HH:mm:ss',
                            )}
                          </TableCell>
                          <TableCell className="hidden text-sm text-muted-foreground lg:table-cell">
                            {jobDurationMs !== null
                              ? formatDuration(jobDurationMs)
                              : job.state === 'running' ||
                                  job.state === 'pending'
                                ? '…'
                                : '—'}
                          </TableCell>
                          <TableCell className="hidden text-sm md:table-cell">
                            {job.size_bytes != null
                              ? formatBytes(job.size_bytes)
                              : '—'}
                          </TableCell>
                          <TableCell className="hidden max-w-[280px] xl:table-cell">
                            {job.error_message ? (
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <span className="block truncate text-xs text-destructive">
                                    {job.error_message}
                                  </span>
                                </TooltipTrigger>
                                <TooltipContent
                                  side="top"
                                  className="max-w-md whitespace-pre-wrap break-words"
                                >
                                  {job.error_message}
                                </TooltipContent>
                              </Tooltip>
                            ) : (
                              <span className="text-xs text-muted-foreground">
                                —
                              </span>
                            )}
                          </TableCell>
                        </TableRow>
                      )
                    })}
                  </TableBody>
                </Table>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </TooltipProvider>
  )
}

/** Single labelled stat in the summary grid. */
function SummaryStat({
  label,
  value,
  mono = false,
}: {
  label: string
  value: React.ReactNode
  mono?: boolean
}) {
  return (
    <div className="rounded-lg border bg-card px-3 py-2.5">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div
        className={`mt-0.5 text-sm ${mono ? 'font-mono' : 'font-medium'}`}
      >
        {value}
      </div>
    </div>
  )
}
