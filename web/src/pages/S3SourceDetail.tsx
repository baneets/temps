'use client'

import {
  deleteBackupScheduleMutation,
  disableBackupScheduleMutation,
  enableBackupScheduleMutation,
  getS3SourceOptions,
  listBackupSchedulesOptions,
  listSourceBackupsOptions,
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
import { EmptyState } from '@/components/ui/empty-state'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { listSourceBackupsWithScan, testS3SourceConnection } from '@/lib/s3-sources'
import { runScheduleNow } from '@/lib/schedule-runs'
import { cn } from '@/lib/utils'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { format } from 'date-fns'
import {
  ArrowLeft,
  CalendarDays,
  Database,
  DatabaseBackup,
  Loader2,
  MoreHorizontal,
  Pencil,
  Play,
  Plug,
  Plus,
  ScanSearch,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { toast } from 'sonner'

/** Format a wall-clock timeout from seconds into a human-readable string.
 * Mirrors the helper in `BackupDetail.tsx`; if this gets copied a third
 * time it should move to `lib/utils.ts`. */
function formatTimeoutSecs(secs: number): string {
  if (secs <= 0) return '—'
  const hours = Math.floor(secs / 3600)
  const minutes = Math.floor((secs % 3600) / 60)
  if (hours > 0 && minutes > 0) return `${hours}h ${minutes}m`
  if (hours > 0) return `${hours}h`
  return `${minutes}m`
}


export function S3SourceDetail() {
  const { id } = useParams<{ id: string }>()
  const sourceId = id ? parseInt(id) : undefined
  const { setBreadcrumbs } = useBreadcrumbs()
  const navigate = useNavigate()

  const [scheduleToDelete, setScheduleToDelete] =
    useState<BackupScheduleResponse | null>(null)

  const { data: source, isLoading: isLoadingSource } = useQuery({
    ...getS3SourceOptions({
      path: { id: sourceId! },
    }),
    enabled: !!sourceId,
  })

  const { data: backupIndex, isLoading: isLoadingBackups } = useQuery({
    ...listSourceBackupsOptions({
      path: { id: sourceId! },
    }),
    enabled: !!sourceId,
  })

  const {
    data: schedules = [],
    refetch: refetchSchedules,
    isLoading: isLoadingSchedules,
  } = useQuery({
    ...listBackupSchedulesOptions(),
  })

  const scheduledForSource = useMemo(
    () => schedules.filter((s) => s.s3_source_id === sourceId),
    [schedules, sourceId],
  )

  // Probes the S3 source's credentials, region, and bucket reachability
  // via the existing POST /backups/s3-sources/{id}/test endpoint.
  // Surfaces the server's `{ ok, message }` verbatim in a toast so operators
  // can confirm a source works before relying on it for backups.
  const testConnectionMutation = useMutation({
    mutationFn: () => {
      if (!sourceId) {
        return Promise.reject(new Error('S3 source id is unknown'))
      }
      return testS3SourceConnection(sourceId)
    },
    onSuccess: (result) => {
      if (result.ok) {
        toast.success('S3 connection succeeded', { description: result.message })
      } else {
        toast.error('S3 connection failed', { description: result.message })
      }
    },
    onError: (err: unknown) => {
      const message = err instanceof Error ? err.message : 'Unknown error'
      toast.error('S3 connection test failed', { description: message })
    },
  })

  const deleteMutation = useMutation({
    ...deleteBackupScheduleMutation(),
    meta: { errorTitle: 'Failed to delete backup schedule' },
    onSuccess: () => {
      refetchSchedules()
      toast.success('Backup schedule deleted successfully')
    },
    onSettled: () => {
      setScheduleToDelete(null)
    },
  })

  const queryClient = useQueryClient()

  // "Discover orphan backups" — explicit opt-in S3 scan. Slow on some
  // endpoints (5-30 s on OVH); only triggered by user action.
  const discoverOrphansMutation = useMutation({
    mutationFn: () => {
      if (!sourceId) {
        return Promise.reject(new Error('S3 source id is unknown'))
      }
      return listSourceBackupsWithScan(sourceId)
    },
    onSuccess: (result) => {
      const scanned = result.backups.filter(
        (b) => (b as { source?: string }).source === 's3_scan',
      ).length
      toast.success(
        scanned > 0
          ? `Found ${scanned} additional backup${scanned === 1 ? '' : 's'} in S3`
          : 'No additional backups found in S3',
      )
      // Refresh the normal listing so newly discovered entries appear.
      void queryClient.invalidateQueries({
        queryKey: ['backups', 's3-source', sourceId],
      })
    },
    onError: (err: unknown) => {
      const message = err instanceof Error ? err.message : 'Unknown error'
      toast.error('S3 scan failed', { description: message })
    },
  })

  const disableMutation = useMutation({
    ...disableBackupScheduleMutation(),
    meta: { errorTitle: 'Failed to disable backup schedule' },
    onSuccess: () => {
      refetchSchedules()
      toast.success('Backup schedule disabled')
    },
  })

  const enableMutation = useMutation({
    ...enableBackupScheduleMutation(),
    meta: { errorTitle: 'Failed to enable backup schedule' },
    onSuccess: () => {
      refetchSchedules()
      toast.success('Backup schedule enabled')
    },
  })

  const runNowMutation = useMutation({
    mutationFn: (scheduleId: number) => runScheduleNow(scheduleId),
    onSuccess: (_data, scheduleId) => {
      toast.success('Backup run enqueued')
      void navigate(`/backups/schedules/${scheduleId}`)
    },
    onError: (err: unknown) => {
      const message = err instanceof Error ? err.message : 'Unknown error'
      toast.error('Failed to start backup run', { description: message })
    },
  })

  useEffect(() => {
    setBreadcrumbs([
      { label: 'Backups', href: '/backups' },
      { label: source?.name || 'S3 Source Details' },
    ])
  }, [setBreadcrumbs, source?.name])

  usePageTitle(source?.name || 'S3 Source Details')

  const sortedBackups = useMemo(
    () =>
      [...(backupIndex?.backups || [])].sort(
        (a, b) =>
          new Date(b.created_at).getTime() - new Date(a.created_at).getTime(),
      ),
    [backupIndex],
  )

  const handleToggleSchedule = (schedule: BackupScheduleResponse) => {
    if (schedule.enabled) {
      disableMutation.mutate({ path: { id: schedule.id } })
    } else {
      enableMutation.mutate({ path: { id: schedule.id } })
    }
  }

  const confirmDeleteSchedule = () => {
    if (!scheduleToDelete) return
    deleteMutation.mutate({ path: { id: scheduleToDelete.id } })
  }

  if (isLoadingSource) {
    return (
      <div className="flex items-center justify-center py-6">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
      </div>
    )
  }

  if (!source) {
    return (
      <div className="flex flex-col items-center justify-center py-6">
        <h2 className="text-lg font-semibold">S3 Source Not Found</h2>
        <p className="text-sm text-muted-foreground">
          The requested S3 source could not be found.
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

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="sm" asChild>
            <Link to="/backups">
              <ArrowLeft className="mr-2 h-4 w-4" />
              Back
            </Link>
          </Button>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => testConnectionMutation.mutate()}
          disabled={testConnectionMutation.isPending || !sourceId}
        >
          {testConnectionMutation.isPending ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Plug className="mr-2 h-4 w-4" />
          )}
          Test connection
        </Button>
      </div>

      <div className="grid gap-6">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Database className="h-5 w-5" />
              {source.name}
            </CardTitle>
            <CardDescription>S3 Storage Configuration</CardDescription>
          </CardHeader>
          <CardContent>
            <dl className="grid gap-4">
              <div>
                <dt className="text-sm font-medium text-muted-foreground">
                  Bucket Name
                </dt>
                <dd className="text-sm">{source.bucket_name}</dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-muted-foreground">
                  Region
                </dt>
                <dd className="text-sm">{source.region}</dd>
              </div>
              {source.endpoint && (
                <div>
                  <dt className="text-sm font-medium text-muted-foreground">
                    Endpoint URL
                  </dt>
                  <dd className="text-sm">{source.endpoint}</dd>
                </div>
              )}
              <div>
                <dt className="text-sm font-medium text-muted-foreground">
                  Force Path Style
                </dt>
                <dd className="text-sm">
                  <Badge
                    variant={source.force_path_style ? 'default' : 'secondary'}
                  >
                    {source.force_path_style ? 'Enabled' : 'Disabled'}
                  </Badge>
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-muted-foreground">
                  Access Key ID
                </dt>
                <dd className="text-sm font-mono">
                  &bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;
                </dd>
              </div>
              <div>
                <dt className="text-sm font-medium text-muted-foreground">
                  Secret Key
                </dt>
                <dd className="text-sm font-mono">
                  &bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;&bull;
                </dd>
              </div>
            </dl>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-start justify-between gap-3 space-y-0">
            <div>
              <CardTitle className="flex items-center gap-2">
                <CalendarDays className="h-5 w-5" />
                Backup Schedules
              </CardTitle>
              <CardDescription>
                Scheduled backups writing to this S3 source
              </CardDescription>
            </div>
            <Button size="sm" asChild>
              <Link to={`/backups/s3-sources/${sourceId}/schedules/new`}>
                <Plus className="mr-2 h-4 w-4" />
                New Schedule
              </Link>
            </Button>
          </CardHeader>
          <CardContent>
            {isLoadingSchedules ? (
              <div className="flex items-center justify-center py-6">
                <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
              </div>
            ) : scheduledForSource.length === 0 ? (
              <EmptyState
                icon={CalendarDays}
                title="No schedules for this source"
                description="Create a schedule to back up automatically to this S3 target"
                action={
                  <Button asChild>
                    <Link
                      to={`/backups/s3-sources/${sourceId}/schedules/new`}
                    >
                      <Plus className="mr-2 h-4 w-4" />
                      New Schedule
                    </Link>
                  </Button>
                }
              />
            ) : (
              <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Name</TableHead>
                      <TableHead>Type</TableHead>
                      <TableHead>Schedule</TableHead>
                      <TableHead>Status</TableHead>
                      <TableHead className="hidden md:table-cell">
                        Retention
                      </TableHead>
                      <TableHead className="hidden md:table-cell">
                        Timeout
                      </TableHead>
                      <TableHead className="hidden md:table-cell">
                        Last Run
                      </TableHead>
                      <TableHead className="hidden md:table-cell">
                        Next Run
                      </TableHead>
                      <TableHead className="w-[80px]">Actions</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {scheduledForSource.map((schedule) => (
                      <TableRow
                        key={schedule.id}
                        className={cn(
                          'transition-colors hover:bg-accent/50',
                          !schedule.enabled && 'text-muted-foreground',
                        )}
                      >
                        <TableCell>
                          {/* Anchor — supports ⌘-click / middle-click /
                              right-click "Open in new tab". The whole-row
                              click fallback got replaced because <a>
                              inside <tr> with onClick eats modifier-key
                              navigation. */}
                          <Link
                            to={`/backups/schedules/${schedule.id}`}
                            className="flex items-center gap-3 hover:underline"
                          >
                            <DatabaseBackup
                              className={cn(
                                'h-4 w-4',
                                !schedule.enabled &&
                                  'text-muted-foreground/60',
                              )}
                            />
                            <div>
                              <div className="font-medium">
                                {schedule.name}
                              </div>
                              {schedule.description && (
                                <div
                                  className={cn(
                                    'text-sm',
                                    !schedule.enabled
                                      ? 'text-muted-foreground/60'
                                      : 'text-muted-foreground',
                                  )}
                                >
                                  {schedule.description}
                                </div>
                              )}
                            </div>
                          </Link>
                        </TableCell>
                        <TableCell>
                          <Badge variant="outline">
                            {schedule.backup_type}
                          </Badge>
                        </TableCell>
                        <TableCell className="font-mono text-xs">
                          {schedule.schedule_expression}
                        </TableCell>
                        <TableCell>
                          <Badge
                            variant={
                              schedule.enabled ? 'default' : 'secondary'
                            }
                          >
                            {schedule.enabled ? 'Enabled' : 'Disabled'}
                          </Badge>
                        </TableCell>
                        <TableCell className="hidden md:table-cell">
                          {schedule.retention_period} days
                        </TableCell>
                        <TableCell className="hidden md:table-cell text-muted-foreground">
                          {schedule.max_runtime_secs
                            ? formatTimeoutSecs(schedule.max_runtime_secs)
                            : 'engine default'}
                        </TableCell>
                        <TableCell className="hidden md:table-cell">
                          {schedule.last_run
                            ? format(
                                new Date(schedule.last_run),
                                'MMM d, yyyy HH:mm',
                              )
                            : '-'}
                        </TableCell>
                        <TableCell className="hidden md:table-cell">
                          {schedule.next_run
                            ? format(
                                new Date(schedule.next_run),
                                'MMM d, yyyy HH:mm',
                              )
                            : '-'}
                        </TableCell>
                        <TableCell
                          onClick={(e) => e.stopPropagation()}
                        >
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button variant="ghost" size="icon">
                                <MoreHorizontal className="h-4 w-4" />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="end">
                              <DropdownMenuItem
                                onClick={() =>
                                  runNowMutation.mutate(schedule.id)
                                }
                                disabled={
                                  !schedule.enabled ||
                                  runNowMutation.isPending
                                }
                              >
                                <Play className="mr-2 h-4 w-4" />
                                Run now
                              </DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem asChild>
                                <Link
                                  to={`/backups/s3-sources/${sourceId}/schedules/${schedule.id}/edit`}
                                >
                                  <Pencil className="mr-2 h-4 w-4" />
                                  Edit
                                </Link>
                              </DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem
                                onClick={() =>
                                  handleToggleSchedule(schedule)
                                }
                                disabled={
                                  disableMutation.isPending ||
                                  enableMutation.isPending
                                }
                              >
                                {schedule.enabled ? 'Disable' : 'Enable'}
                              </DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem
                                onClick={() => setScheduleToDelete(schedule)}
                                className="text-destructive"
                                disabled={deleteMutation.isPending}
                              >
                                Delete
                              </DropdownMenuItem>
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-start justify-between gap-3 space-y-0">
            <div>
              <CardTitle>Recent Backups</CardTitle>
              <CardDescription>
                Backups that have been written to this S3 source
              </CardDescription>
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={() => discoverOrphansMutation.mutate()}
              disabled={discoverOrphansMutation.isPending || !sourceId}
              title="Scan S3 for backups not tracked in the database — useful after a Temps restore from a different instance"
            >
              {discoverOrphansMutation.isPending ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <ScanSearch className="mr-2 h-4 w-4" />
              )}
              <span className="hidden sm:inline">Discover orphan backups</span>
              <span className="sm:hidden">Scan S3</span>
            </Button>
          </CardHeader>
          <CardContent>
            {isLoadingBackups ? (
              <div className="flex items-center justify-center py-6">
                <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
              </div>
            ) : sortedBackups.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No backups found for this S3 source.
              </p>
            ) : (
              <div className="space-y-2">
                {sortedBackups.map((backup) => (
                  <Link
                    key={backup.backup_id}
                    to={`/backups/s3-sources/${id}/backups/${backup.backup_id}`}
                    className="block"
                  >
                    <div className="flex items-center justify-between p-4 border rounded-lg hover:bg-muted/50 transition-colors">
                      <div className="flex items-center gap-4">
                        <DatabaseBackup className="h-4 w-4" />
                        <div className="text-sm">
                          {format(new Date(backup.created_at), 'PPP p')}
                        </div>
                      </div>
                    </div>
                  </Link>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      <AlertDialog
        open={scheduleToDelete !== null}
        onOpenChange={(open) => {
          if (!open && !deleteMutation.isPending) {
            setScheduleToDelete(null)
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete backup schedule?</AlertDialogTitle>
            <AlertDialogDescription>
              This will permanently delete the schedule
              {scheduleToDelete ? (
                <>
                  {' '}
                  <span className="font-medium text-foreground">
                    {scheduleToDelete.name}
                  </span>
                </>
              ) : null}
              . Existing backups created by this schedule will not be removed,
              but no new backups will run on this schedule. This action cannot
              be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteMutation.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault()
                confirmDeleteSchedule()
              }}
              disabled={deleteMutation.isPending}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {deleteMutation.isPending ? 'Deleting...' : 'Delete schedule'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
