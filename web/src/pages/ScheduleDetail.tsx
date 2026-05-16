'use client'

import {
  deleteBackupScheduleMutation,
  disableBackupScheduleMutation,
  enableBackupScheduleMutation,
  getBackupScheduleOptions,
  getS3SourceOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { BackupScheduleResponse } from '@/api/client/types.gen'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import {
  listScheduleRunsOptions,
  runScheduleNow,
  type ScheduleRunSummary,
} from '@/lib/schedule-runs'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { format } from 'date-fns'
import {
  ArrowLeft,
  CalendarDays,
  ChevronLeft,
  ChevronRight,
  DatabaseBackup,
  Loader2,
  MoreHorizontal,
  Pencil,
  Play,
  Trash2,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { toast } from 'sonner'

// ── Local helpers ─────────────────────────────────────────────────────────────

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

/** Wall-clock timeout from seconds: "4h", "1h 30m", "20m". */
function formatTimeoutSecs(secs: number): string {
  if (secs <= 0) return '—'
  const hours = Math.floor(secs / 3600)
  const minutes = Math.floor((secs % 3600) / 60)
  if (hours > 0 && minutes > 0) return `${hours}h ${minutes}m`
  if (hours > 0) return `${hours}h`
  return `${minutes}m`
}

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

/**
 * One row in the run-history table. Renders the per-tick summary and lazily
 * fetches the child job list when expanded.
 */
function RunRow({ run }: { run: ScheduleRunSummary }) {
  // Negative `run_id` are synthetic legacy rows (pre-fan-out backups). They
  // have no run-detail page, so render as a plain non-linked row.
  const isLegacy = run.run_id < 0

  const durationMs =
    run.started_at && run.finished_at
      ? new Date(run.finished_at).getTime() - new Date(run.started_at).getTime()
      : null

  const detailUrl = `/backups/schedules/${run.schedule_id}/runs/${run.run_id}`

  return (
    <TableRow className="hover:bg-muted/50">
      <TableCell className="font-mono text-xs">
        {isLegacy ? (
          format(new Date(run.started_at), 'MMM d, yyyy HH:mm:ss')
        ) : (
          <Link to={detailUrl} className="flex hover:underline">
            {format(new Date(run.started_at), 'MMM d, yyyy HH:mm:ss')}
          </Link>
        )}
      </TableCell>

      <TableCell className="hidden text-sm text-muted-foreground sm:table-cell">
        {durationMs !== null
          ? formatDuration(durationMs)
          : run.aggregate_state === 'running' ||
              run.aggregate_state === 'pending'
            ? '…'
            : '—'}
      </TableCell>

      <TableCell>
        <Badge variant={stateBadgeVariant(run.aggregate_state)}>
          {run.aggregate_state}
        </Badge>
      </TableCell>

      <TableCell className="hidden text-sm text-muted-foreground md:table-cell">
        {run.triggered_by}
      </TableCell>

      <TableCell className="text-sm">
        {run.failed_jobs > 0 ? (
          <span className="text-destructive">
            {run.completed_jobs} / {run.total_jobs} (
            <span className="font-medium">{run.failed_jobs} failed</span>)
          </span>
        ) : (
          <span className="text-muted-foreground">
            {run.completed_jobs} / {run.total_jobs}
          </span>
        )}
      </TableCell>
    </TableRow>
  )
}

// ── Component ─────────────────────────────────────────────────────────────────

export function ScheduleDetail() {
  const { id } = useParams<{ id: string }>()
  const scheduleId = id ? parseInt(id) : undefined
  const navigate = useNavigate()
  const { setBreadcrumbs } = useBreadcrumbs()
  const queryClient = useQueryClient()

  const [page, setPage] = useState(1)
  const pageSize = 20

  const [showDeleteDialog, setShowDeleteDialog] = useState(false)

  // ── Data fetching ──────────────────────────────────────────────────────────

  const { data: schedule, isLoading: isLoadingSchedule } = useQuery({
    ...getBackupScheduleOptions({ path: { id: scheduleId! } }),
    enabled: !!scheduleId,
  })

  const { data: s3Source } = useQuery({
    ...getS3SourceOptions({ path: { id: schedule?.s3_source_id! } }),
    enabled: !!schedule?.s3_source_id,
  })

  const {
    data: runsData,
    isLoading: isLoadingRuns,
  } = useQuery({
    ...listScheduleRunsOptions(scheduleId, page, pageSize),
  })

  // ── Mutations ──────────────────────────────────────────────────────────────

  const runNowMutation = useMutation({
    mutationFn: () => runScheduleNow(scheduleId!),
    onSuccess: (data) => {
      toast.success('Run enqueued', {
        description: `Scheduler run #${data.schedule_run_id} fan-out: ${data.jobs.length} job${data.jobs.length === 1 ? '' : 's'} pending.`,
      })
      void queryClient.invalidateQueries({
        queryKey: ['schedule-runs', scheduleId],
      })
      // Navigate to the new run detail page so the user sees per-job progress.
      navigate(`/backups/schedules/${scheduleId}/runs/${data.schedule_run_id}`)
    },
    onError: (err: unknown) => {
      const message = err instanceof Error ? err.message : 'Unknown error'
      toast.error('Failed to enqueue backup', { description: message })
    },
  })

  const disableMutation = useMutation({
    ...disableBackupScheduleMutation(),
    onSuccess: () => {
      toast.success('Schedule disabled')
      void queryClient.invalidateQueries({
        queryKey: getBackupScheduleOptions({ path: { id: scheduleId! } }).queryKey,
      })
    },
    onError: () => toast.error('Failed to disable schedule'),
  })

  const enableMutation = useMutation({
    ...enableBackupScheduleMutation(),
    onSuccess: () => {
      toast.success('Schedule enabled')
      void queryClient.invalidateQueries({
        queryKey: getBackupScheduleOptions({ path: { id: scheduleId! } }).queryKey,
      })
    },
    onError: () => toast.error('Failed to enable schedule'),
  })

  const deleteMutation = useMutation({
    ...deleteBackupScheduleMutation(),
    onSuccess: () => {
      toast.success('Schedule deleted')
      navigate('/backups')
    },
    onError: () => toast.error('Failed to delete schedule'),
    onSettled: () => setShowDeleteDialog(false),
  })

  // ── Breadcrumbs ────────────────────────────────────────────────────────────

  useEffect(() => {
    if (!schedule) return
    setBreadcrumbs([
      { label: 'Backups', href: '/backups' },
      {
        label: s3Source?.name ?? `S3 Source ${schedule.s3_source_id}`,
        href: `/backups/s3-sources/${schedule.s3_source_id}`,
      },
      { label: schedule.name },
    ])
  }, [setBreadcrumbs, schedule, s3Source])

  usePageTitle(schedule?.name ?? 'Schedule Detail')

  // ── Pagination helpers ─────────────────────────────────────────────────────

  const total = runsData?.total ?? 0
  const totalPages = Math.max(1, Math.ceil(total / pageSize))
  const rangeStart = (page - 1) * pageSize + 1
  const rangeEnd = Math.min(page * pageSize, total)

  // ── Render helpers ─────────────────────────────────────────────────────────

  function renderScheduleConfigCard(s: BackupScheduleResponse) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <CalendarDays className="h-5 w-5" />
            Schedule Configuration
          </CardTitle>
          <CardDescription>Current settings for this schedule</CardDescription>
        </CardHeader>
        <CardContent>
          <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div>
              <dt className="text-sm font-medium text-muted-foreground">Cron expression</dt>
              <dd className="font-mono text-sm">{s.schedule_expression}</dd>
            </div>
            <div>
              <dt className="text-sm font-medium text-muted-foreground">Backup type</dt>
              <dd>
                <Badge variant="outline">{s.backup_type}</Badge>
              </dd>
            </div>
            <div>
              <dt className="text-sm font-medium text-muted-foreground">Retention</dt>
              <dd className="text-sm">{s.retention_period} days</dd>
            </div>
            <div>
              <dt className="text-sm font-medium text-muted-foreground">Max runtime</dt>
              <dd className="text-sm text-muted-foreground">
                {s.max_runtime_secs
                  ? formatTimeoutSecs(s.max_runtime_secs)
                  : 'engine default'}
              </dd>
            </div>
            {s.description && (
              <div className="sm:col-span-2">
                <dt className="text-sm font-medium text-muted-foreground">Description</dt>
                <dd className="text-sm">{s.description}</dd>
              </div>
            )}
            <div>
              <dt className="text-sm font-medium text-muted-foreground">Last run</dt>
              <dd className="text-sm">
                {s.last_run
                  ? format(new Date(s.last_run), 'MMM d, yyyy HH:mm')
                  : '—'}
              </dd>
            </div>
            <div>
              <dt className="text-sm font-medium text-muted-foreground">Next run</dt>
              <dd className="text-sm">
                {s.next_run
                  ? format(new Date(s.next_run), 'MMM d, yyyy HH:mm')
                  : '—'}
              </dd>
            </div>
            {s3Source && (
              <div>
                <dt className="text-sm font-medium text-muted-foreground">S3 source</dt>
                <dd>
                  <Link
                    to={`/backups/s3-sources/${s.s3_source_id}`}
                    className="text-sm text-primary hover:underline"
                  >
                    {s3Source.name}
                  </Link>
                </dd>
              </div>
            )}
          </dl>
        </CardContent>
      </Card>
    )
  }

  // ── Loading state ──────────────────────────────────────────────────────────

  if (isLoadingSchedule) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-48 w-full" />
        <Skeleton className="h-96 w-full" />
      </div>
    )
  }

  if (!schedule) {
    return (
      <div className="flex flex-col items-center justify-center py-12">
        <h2 className="text-lg font-semibold">Schedule Not Found</h2>
        <p className="text-sm text-muted-foreground mt-1">
          The requested backup schedule could not be found.
        </p>
        <Button asChild className="mt-4">
          <Link to="/backups">
            <ArrowLeft className="mr-2 h-4 w-4" />
            Back to Backups
          </Link>
        </Button>
      </div>
    )
  }

  // ── Main render ────────────────────────────────────────────────────────────

  return (
    <TooltipProvider>
      <div className="space-y-6">
        {/* ── Header ── */}
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <Button variant="ghost" size="sm" asChild>
              <Link to={`/backups/s3-sources/${schedule.s3_source_id}`}>
                <ArrowLeft className="mr-2 h-4 w-4" />
                Back
              </Link>
            </Button>
            <div>
              <div className="flex items-center gap-2">
                <DatabaseBackup className="h-5 w-5 text-muted-foreground" />
                <h1 className="text-xl font-semibold">{schedule.name}</h1>
                <Badge variant={schedule.enabled ? 'default' : 'secondary'}>
                  {schedule.enabled ? 'Enabled' : 'Disabled'}
                </Badge>
              </div>
            </div>
          </div>

          {/* ── Action row ── */}
          <div className="flex items-center gap-2">
            <Button
              variant="default"
              size="sm"
              disabled={!schedule.enabled || runNowMutation.isPending}
              onClick={() => runNowMutation.mutate()}
              title={
                !schedule.enabled
                  ? 'Enable the schedule before running'
                  : 'Enqueue a backup immediately'
              }
            >
              {runNowMutation.isPending ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Play className="mr-2 h-4 w-4" />
              )}
              Run now
            </Button>

            <Button variant="outline" size="sm" asChild>
              <Link
                to={`/backups/s3-sources/${schedule.s3_source_id}/schedules/${schedule.id}/edit`}
              >
                <Pencil className="mr-2 h-4 w-4" />
                Edit
              </Link>
            </Button>

            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="sm">
                  <MoreHorizontal className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  onClick={() => {
                    if (schedule.enabled) {
                      disableMutation.mutate({ path: { id: schedule.id } })
                    } else {
                      enableMutation.mutate({ path: { id: schedule.id } })
                    }
                  }}
                  disabled={disableMutation.isPending || enableMutation.isPending}
                >
                  {schedule.enabled ? 'Disable' : 'Enable'}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="text-destructive"
                  onClick={() => setShowDeleteDialog(true)}
                >
                  <Trash2 className="mr-2 h-4 w-4" />
                  Delete
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>

        {/* ── Config card ── */}
        {renderScheduleConfigCard(schedule)}

        {/* ── Run history table ── */}
        <Card>
          <CardHeader>
            <CardTitle>Run History</CardTitle>
            <CardDescription>
              Backup runs for this schedule, newest first. Click a row to see
              full details.
            </CardDescription>
          </CardHeader>
          <CardContent className="p-0">
            {isLoadingRuns ? (
              <div className="space-y-3 p-6">
                {[...Array(5)].map((_, i) => (
                  <Skeleton key={i} className="h-10 w-full" />
                ))}
              </div>
            ) : !runsData || runsData.runs.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-12 text-sm text-muted-foreground">
                <DatabaseBackup className="mb-3 h-8 w-8 opacity-40" />
                No runs yet — click &ldquo;Run now&rdquo; to start the first backup.
              </div>
            ) : (
              <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Started</TableHead>
                      <TableHead className="hidden sm:table-cell">Duration</TableHead>
                      <TableHead>State</TableHead>
                      <TableHead className="hidden md:table-cell">Trigger</TableHead>
                      <TableHead>Jobs</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {runsData.runs.map((run) => (
                      <RunRow key={run.run_id} run={run} />
                    ))}
                  </TableBody>
                </Table>
              </div>
            )}

            {/* ── Pagination ── */}
            {runsData && runsData.total > 0 && (
              <div className="flex items-center justify-between border-t px-6 py-4">
                <span className="hidden text-sm text-muted-foreground sm:inline">
                  Showing {rangeStart}–{rangeEnd} of {total}
                </span>
                <span className="text-sm text-muted-foreground sm:hidden">
                  {page} / {totalPages}
                </span>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page <= 1}
                    onClick={() => setPage((p) => Math.max(1, p - 1))}
                  >
                    <ChevronLeft className="h-4 w-4" />
                    <span className="hidden sm:inline">Previous</span>
                  </Button>
                  <span className="text-sm">
                    {page} / {totalPages}
                  </span>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page >= totalPages}
                    onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
                  >
                    <span className="hidden sm:inline">Next</span>
                    <ChevronRight className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {/* ── Delete confirmation dialog ── */}
      <AlertDialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete schedule?</AlertDialogTitle>
            <AlertDialogDescription>
              This will permanently delete &ldquo;{schedule.name}&rdquo;. Existing backup
              records are not deleted, but no new backups will be scheduled.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() =>
                deleteMutation.mutate({ path: { id: schedule.id } })
              }
              disabled={deleteMutation.isPending}
            >
              {deleteMutation.isPending ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : null}
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </TooltipProvider>
  )
}
