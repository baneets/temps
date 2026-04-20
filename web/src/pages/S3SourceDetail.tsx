'use client'

import {
  createBackupScheduleMutation,
  deleteBackupScheduleMutation,
  disableBackupScheduleMutation,
  enableBackupScheduleMutation,
  getS3SourceOptions,
  listBackupSchedulesOptions,
  listSourceBackupsOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { BackupScheduleResponse } from '@/api/client/types.gen'
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
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
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
import { cn } from '@/lib/utils'
import { useMutation, useQuery } from '@tanstack/react-query'
import { format } from 'date-fns'
import {
  ArrowLeft,
  CalendarDays,
  Database,
  DatabaseBackup,
  MoreHorizontal,
  Plus,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { toast } from 'sonner'

interface ScheduleOption {
  label: string
  value: string
  description: string
  customizable?: boolean
}

const scheduleOptions: ScheduleOption[] = [
  {
    label: 'Every 12 hours',
    value: '0 0 */12 * * *',
    description: 'Runs at 00:00 and 12:00',
  },
  {
    label: 'Daily',
    value: '0 0 0 * * *',
    description: 'Runs every day at midnight',
  },
  {
    label: 'Weekly',
    value: '0 0 0 * * 0',
    description: 'Runs every Sunday at midnight',
  },
  {
    label: 'Monthly',
    value: '0 0 0 1 * *',
    description: 'Runs on the first day of every month at midnight',
  },
  {
    label: 'Custom',
    value: 'custom',
    description: 'Specify a custom cron expression',
    customizable: true,
  },
]

interface NewScheduleForm {
  name: string
  description?: string
  backup_type: string
  retention_period: number
  enabled: boolean
}

export function S3SourceDetail() {
  const { id } = useParams<{ id: string }>()
  const sourceId = id ? parseInt(id) : undefined
  const { setBreadcrumbs } = useBreadcrumbs()

  const [isCreateDialogOpen, setIsCreateDialogOpen] = useState(false)
  const [newSchedule, setNewSchedule] = useState<Partial<NewScheduleForm>>({
    backup_type: 'scheduled',
    retention_period: 7,
    enabled: true,
  })
  const [selectedSchedule, setSelectedSchedule] = useState<string>(
    scheduleOptions[1].value,
  )
  const [customCron, setCustomCron] = useState('')

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

  const createMutation = useMutation({
    ...createBackupScheduleMutation(),
    meta: { errorTitle: 'Failed to create backup schedule' },
    onSuccess: () => {
      refetchSchedules()
      setNewSchedule({
        backup_type: 'scheduled',
        retention_period: 7,
        enabled: true,
      })
      setSelectedSchedule(scheduleOptions[1].value)
      setCustomCron('')
      setIsCreateDialogOpen(false)
      toast.success('Backup schedule created successfully')
    },
  })

  const deleteMutation = useMutation({
    ...deleteBackupScheduleMutation(),
    meta: { errorTitle: 'Failed to delete backup schedule' },
    onSuccess: () => {
      refetchSchedules()
      toast.success('Backup schedule deleted successfully')
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

  const handleScheduleChange = (value: string) => {
    setSelectedSchedule(value)
  }

  const handleCreateSchedule = () => {
    if (!newSchedule.name) {
      toast.error('Schedule name is required')
      return
    }
    if (!sourceId) return

    const schedule_expression =
      selectedSchedule === 'custom' ? customCron : selectedSchedule

    if (!schedule_expression) {
      toast.error('Please select a schedule or enter a custom cron expression')
      return
    }

    createMutation.mutate({
      body: {
        name: newSchedule.name,
        description: newSchedule.description,
        backup_type: newSchedule.backup_type || 'scheduled',
        schedule_expression,
        retention_period: newSchedule.retention_period || 7,
        s3_source_id: sourceId,
        enabled: newSchedule.enabled ?? true,
        tags: [],
      },
    })
  }

  const handleToggleSchedule = (schedule: BackupScheduleResponse) => {
    if (schedule.enabled) {
      disableMutation.mutate({ path: { id: schedule.id } })
    } else {
      enableMutation.mutate({ path: { id: schedule.id } })
    }
  }

  const handleDeleteSchedule = (scheduleId: number) => {
    deleteMutation.mutate({ path: { id: scheduleId } })
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
            <Dialog
              open={isCreateDialogOpen}
              onOpenChange={setIsCreateDialogOpen}
            >
              <DialogTrigger asChild>
                <Button size="sm">
                  <Plus className="mr-2 h-4 w-4" />
                  New Schedule
                </Button>
              </DialogTrigger>
              <DialogContent className="max-h-screen flex flex-col">
                <DialogHeader>
                  <DialogTitle>Create Backup Schedule</DialogTitle>
                </DialogHeader>
                <div className="grid gap-4 py-4 flex-1 overflow-y-auto">
                  <div className="grid gap-2">
                    <Label htmlFor="name">Schedule Name</Label>
                    <Input
                      id="name"
                      placeholder="Daily Backup"
                      value={newSchedule.name || ''}
                      onChange={(e) =>
                        setNewSchedule({
                          ...newSchedule,
                          name: e.target.value,
                        })
                      }
                    />
                  </div>
                  <div className="grid gap-2">
                    <Label htmlFor="description">
                      Description (Optional)
                    </Label>
                    <Input
                      id="description"
                      placeholder="Daily backup at midnight"
                      value={newSchedule.description || ''}
                      onChange={(e) =>
                        setNewSchedule({
                          ...newSchedule,
                          description: e.target.value,
                        })
                      }
                    />
                  </div>
                  <div className="grid gap-2">
                    <Label htmlFor="type">Backup Type</Label>
                    <Select
                      value={newSchedule.backup_type}
                      onValueChange={(value) =>
                        setNewSchedule({
                          ...newSchedule,
                          backup_type: value,
                        })
                      }
                    >
                      <SelectTrigger>
                        <SelectValue placeholder="Select type" />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="manual">Manual</SelectItem>
                        <SelectItem value="scheduled">Scheduled</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  {newSchedule.backup_type === 'scheduled' && (
                    <div className="grid gap-2">
                      <Label>Schedule</Label>
                      <RadioGroup
                        value={selectedSchedule}
                        onValueChange={handleScheduleChange}
                        className="gap-4"
                      >
                        {scheduleOptions.map((option) => (
                          <div
                            key={option.value}
                            className="flex items-start space-x-3 space-y-0"
                          >
                            <RadioGroupItem
                              value={option.value}
                              id={option.value}
                            />
                            <div className="grid gap-1.5 leading-none">
                              <Label
                                htmlFor={option.value}
                                className="text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70"
                              >
                                {option.label}
                              </Label>
                              <p className="text-sm text-muted-foreground">
                                {option.description}
                              </p>
                            </div>
                          </div>
                        ))}
                      </RadioGroup>
                      {selectedSchedule === 'custom' && (
                        <div className="mt-4">
                          <Label htmlFor="customCron">
                            Custom Cron Expression
                          </Label>
                          <Input
                            id="customCron"
                            placeholder="0 0 * * *"
                            value={customCron}
                            onChange={(e) => setCustomCron(e.target.value)}
                          />
                          <p className="text-xs text-muted-foreground mt-1">
                            Format: minute hour day month weekday
                          </p>
                        </div>
                      )}
                    </div>
                  )}
                  <div className="grid gap-2">
                    <Label htmlFor="retention">
                      Retention Period (days)
                    </Label>
                    <Input
                      id="retention"
                      type="number"
                      min={1}
                      value={newSchedule.retention_period || 7}
                      onChange={(e) =>
                        setNewSchedule({
                          ...newSchedule,
                          retention_period: parseInt(e.target.value),
                        })
                      }
                    />
                  </div>
                </div>
                <DialogFooter className="shrink-0">
                  <Button
                    onClick={handleCreateSchedule}
                    disabled={createMutation.isPending}
                  >
                    {createMutation.isPending
                      ? 'Creating...'
                      : 'Create Schedule'}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
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
                  <Button onClick={() => setIsCreateDialogOpen(true)}>
                    <Plus className="mr-2 h-4 w-4" />
                    New Schedule
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
                          !schedule.enabled && 'text-muted-foreground',
                        )}
                      >
                        <TableCell>
                          <div className="flex items-center gap-3">
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
                          </div>
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
                        <TableCell>
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button variant="ghost" size="icon">
                                <MoreHorizontal className="h-4 w-4" />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="end">
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
                                onClick={() =>
                                  handleDeleteSchedule(schedule.id)
                                }
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
          <CardHeader>
            <CardTitle>Recent Backups</CardTitle>
            <CardDescription>
              Backups that have been written to this S3 source
            </CardDescription>
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
    </div>
  )
}
