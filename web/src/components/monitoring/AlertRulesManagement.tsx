'use client'

import {
  createAlertRuleMutation,
  deleteAlertRuleMutation,
  listAlertRulesOptions,
  updateAlertRuleMutation,
  getProjectsOptions,
} from '@/api/client/@tanstack/react-query.gen'
import {
  AlertRuleResponse,
  CreateAlertRuleRequest,
} from '@/api/client/types.gen'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { EmptyState } from '@/components/ui/empty-state'
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Badge } from '@/components/ui/badge'
import { zodResolver } from '@hookform/resolvers/zod'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { AlertTriangle, EllipsisVertical, Plus, ShieldAlert } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { z } from 'zod'

const TRIGGER_TYPES = [
  { value: 'new_issue', label: 'New Issue', description: 'When a new error group is created' },
  { value: 'regression', label: 'Regression', description: 'When a resolved issue reoccurs' },
  { value: 'frequency', label: 'Frequency', description: 'When error count exceeds threshold in time window' },
  { value: 'new_user', label: 'New User Affected', description: 'When a new user encounters an error' },
  { value: 'user_count', label: 'User Count', description: 'When number of affected users exceeds threshold' },
  { value: 'status_change', label: 'Status Change', description: 'When an error group status changes' },
] as const

const PRIORITIES = [
  { value: 'Low', label: 'Low' },
  { value: 'Normal', label: 'Normal' },
  { value: 'High', label: 'High' },
  { value: 'Critical', label: 'Critical' },
] as const

const alertRuleSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  trigger_type: z.string().min(1, 'Trigger type is required'),
  trigger_config: z.object({
    count: z.number().min(1).optional(),
    window_minutes: z.number().min(1).optional(),
    threshold: z.number().min(1).optional(),
  }).optional(),
  cooldown_minutes: z.number().min(0),
  notification_priority: z.string(),
  environment_filter: z.number().nullable().optional(),
  error_level_filter: z.string().nullable().optional(),
  enabled: z.boolean(),
})

type AlertRuleFormData = z.infer<typeof alertRuleSchema>

function triggerTypeLabel(type: string): string {
  return TRIGGER_TYPES.find((t) => t.value === type)?.label ?? type
}

function priorityVariant(priority: string): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (priority) {
    case 'Critical': return 'destructive'
    case 'High': return 'default'
    case 'Normal': return 'secondary'
    default: return 'outline'
  }
}

function needsConfig(triggerType: string): boolean {
  return ['frequency', 'user_count'].includes(triggerType)
}

function renderTriggerConfig(rule: AlertRuleResponse) {
  const config = (rule.trigger_config ?? {}) as Record<string, unknown>
  switch (rule.trigger_type) {
    case 'frequency':
      return (
        <p>
          Threshold: {String(config.count ?? '—')} events / {String(config.window_minutes ?? '—')} min
        </p>
      )
    case 'user_count':
      return <p>User threshold: {String(config.threshold ?? '—')}</p>
    default:
      return null
  }
}

interface AlertRulesManagementProps {
  projectId?: number
}

export function AlertRulesManagement({ projectId: fixedProjectId }: AlertRulesManagementProps = {}) {
  const queryClient = useQueryClient()
  const [selectedProjectId, setSelectedProjectId] = useState<number | null>(null)
  const [isDialogOpen, setIsDialogOpen] = useState(false)
  const [editingRule, setEditingRule] = useState<AlertRuleResponse | null>(null)

  const { data: projects, isLoading: projectsLoading } = useQuery({
    ...getProjectsOptions(),
    enabled: !fixedProjectId,
  })

  const projectList = projects?.projects
  const projectId = fixedProjectId ?? selectedProjectId ?? projectList?.[0]?.id ?? null
  const showProjectSelector = !fixedProjectId && (projectList?.length ?? 0) > 1

  const { data: rules, isLoading: rulesLoading } = useQuery({
    ...listAlertRulesOptions({
      path: { project_id: projectId! },
    }),
    enabled: !!projectId,
  })

  const form = useForm<AlertRuleFormData>({
    resolver: zodResolver(alertRuleSchema),
    defaultValues: {
      name: '',
      trigger_type: 'new_issue',
      trigger_config: {},
      cooldown_minutes: 60,
      notification_priority: 'High',
      environment_filter: null,
      error_level_filter: null,
      enabled: true,
    },
  })

  const watchedTriggerType = form.watch('trigger_type')

  const createMutation = useMutation({
    ...createAlertRuleMutation(),
    meta: { errorTitle: 'Failed to create alert rule' },
    onSuccess: () => {
      toast.success('Alert rule created')
      closeDialog()
      queryClient.invalidateQueries({ predicate: (query) => (query.queryKey[0] as Record<string, unknown>)?._id === 'listAlertRules' })
    },
  })

  const updateMutation = useMutation({
    ...updateAlertRuleMutation(),
    meta: { errorTitle: 'Failed to update alert rule' },
    onSuccess: () => {
      toast.success('Alert rule updated')
      closeDialog()
      queryClient.invalidateQueries({ predicate: (query) => (query.queryKey[0] as Record<string, unknown>)?._id === 'listAlertRules' })
    },
  })

  const deleteMutation = useMutation({
    ...deleteAlertRuleMutation(),
    meta: { errorTitle: 'Failed to delete alert rule' },
    onSuccess: () => {
      toast.success('Alert rule deleted')
      queryClient.invalidateQueries({ predicate: (query) => (query.queryKey[0] as Record<string, unknown>)?._id === 'listAlertRules' })
    },
  })

  const closeDialog = () => {
    setIsDialogOpen(false)
    setEditingRule(null)
    form.reset({
      name: '',
      trigger_type: 'new_issue',
      trigger_config: {},
      cooldown_minutes: 60,
      notification_priority: 'High',
      environment_filter: null,
      error_level_filter: null,
      enabled: true,
    })
  }

  const openCreate = () => {
    setEditingRule(null)
    form.reset({
      name: '',
      trigger_type: 'new_issue',
      trigger_config: {},
      cooldown_minutes: 60,
      notification_priority: 'High',
      environment_filter: null,
      error_level_filter: null,
      enabled: true,
    })
    setIsDialogOpen(true)
  }

  const openEdit = (rule: AlertRuleResponse) => {
    setEditingRule(rule)
    const config = (rule.trigger_config ?? {}) as Record<string, unknown>
    form.reset({
      name: rule.name,
      trigger_type: rule.trigger_type,
      trigger_config: {
        count: (config.count as number) ?? undefined,
        window_minutes: (config.window_minutes as number) ?? undefined,
        threshold: (config.threshold as number) ?? undefined,
      },
      cooldown_minutes: rule.cooldown_minutes,
      notification_priority: rule.notification_priority,
      environment_filter: rule.environment_filter ?? null,
      error_level_filter: rule.error_level_filter ?? null,
      enabled: rule.enabled,
    })
    setIsDialogOpen(true)
  }

  const onSubmit = async (data: AlertRuleFormData) => {
    if (!projectId) return

    const triggerConfig = needsConfig(data.trigger_type) ? data.trigger_config : undefined

    if (editingRule) {
      await updateMutation.mutateAsync({
        path: { project_id: projectId, rule_id: editingRule.id },
        body: {
          name: data.name,
          trigger_type: data.trigger_type,
          trigger_config: triggerConfig,
          cooldown_minutes: data.cooldown_minutes,
          notification_priority: data.notification_priority,
          environment_filter: data.environment_filter,
          error_level_filter: data.error_level_filter,
          enabled: data.enabled,
        },
      })
    } else {
      await createMutation.mutateAsync({
        path: { project_id: projectId },
        body: {
          name: data.name,
          trigger_type: data.trigger_type,
          trigger_config: triggerConfig,
          cooldown_minutes: data.cooldown_minutes,
          notification_priority: data.notification_priority,
          environment_filter: data.environment_filter,
          error_level_filter: data.error_level_filter,
          enabled: data.enabled,
        } as CreateAlertRuleRequest,
      })
    }
  }

  const handleDelete = async (rule: AlertRuleResponse) => {
    if (!projectId) return
    await deleteMutation.mutateAsync({
      path: { project_id: projectId, rule_id: rule.id },
    })
  }

  const handleToggleEnabled = async (rule: AlertRuleResponse) => {
    if (!projectId) return
    await updateMutation.mutateAsync({
      path: { project_id: projectId, rule_id: rule.id },
      body: { enabled: !rule.enabled },
    })
  }

  const hasRules = useMemo(() => rules && rules.length > 0, [rules])
  const isMutating = createMutation.isPending || updateMutation.isPending

  if (!fixedProjectId && projectsLoading) {
    return (
      <div className="flex items-center justify-center py-6">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
      </div>
    )
  }

  if (!fixedProjectId && !projectList?.length) {
    return (
      <EmptyState
        icon={ShieldAlert}
        title="No projects found"
        description="Create a project first to configure error alert rules."
      />
    )
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h3 className="text-lg font-medium">Error Alert Rules</h3>
          <p className="text-sm text-muted-foreground">
            Configure rules that trigger notifications when errors match certain conditions.
          </p>
        </div>
        <div className="flex items-center gap-2">
          {showProjectSelector && (
            <Select
              value={String(projectId)}
              onValueChange={(v) => setSelectedProjectId(Number(v))}
            >
              <SelectTrigger className="w-[200px]">
                <SelectValue placeholder="Select project" />
              </SelectTrigger>
              <SelectContent>
                {projectList!.map((p) => (
                  <SelectItem key={p.id} value={String(p.id)}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <Button onClick={openCreate} disabled={!projectId}>
            <Plus className="h-4 w-4 mr-2" />
            <span className="hidden sm:inline">Add Rule</span>
          </Button>
        </div>
      </div>

      {rulesLoading ? (
        <div className="flex items-center justify-center py-6">
          <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
        </div>
      ) : !hasRules ? (
        <EmptyState
          icon={AlertTriangle}
          title="No alert rules configured"
          description="Create your first error alert rule to get notified when errors match specific conditions."
          action={
            <Button onClick={openCreate}>
              <Plus className="h-4 w-4 mr-2" />
              Add Rule
            </Button>
          }
        />
      ) : (
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {rules?.map((rule) => (
            <Card key={rule.id}>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <div className="space-y-1 min-w-0 flex-1">
                  <CardTitle className="text-base font-medium leading-none truncate">
                    {rule.name}
                  </CardTitle>
                  <p className="text-xs text-muted-foreground">
                    {triggerTypeLabel(rule.trigger_type)}
                  </p>
                </div>
                <div className="flex items-center gap-1 shrink-0">
                  <Switch
                    checked={rule.enabled}
                    onCheckedChange={() => handleToggleEnabled(rule)}
                    disabled={updateMutation.isPending}
                  />
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="ghost" size="icon" className="h-8 w-8">
                        <EllipsisVertical className="h-4 w-4" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onClick={() => openEdit(rule)}>
                        Edit
                      </DropdownMenuItem>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem
                        className="text-destructive"
                        onClick={() => handleDelete(rule)}
                      >
                        Delete
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              </CardHeader>
              <CardContent className="space-y-3">
                <div className="flex items-center gap-2 flex-wrap">
                  <Badge variant={priorityVariant(rule.notification_priority)}>
                    {rule.notification_priority}
                  </Badge>
                  <Badge variant="outline">
                    {triggerTypeLabel(rule.trigger_type)}
                  </Badge>
                  {rule.error_level_filter && (
                    <Badge variant="secondary">
                      {rule.error_level_filter}
                    </Badge>
                  )}
                </div>
                <div className="space-y-1 text-xs text-muted-foreground">
                  <p>Cooldown: {rule.cooldown_minutes} min</p>
                  {renderTriggerConfig(rule)}
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      <Dialog open={isDialogOpen} onOpenChange={(open) => !open && closeDialog()}>
        <DialogContent className="max-w-lg max-h-[90vh] flex flex-col">
          <DialogHeader>
            <DialogTitle>
              {editingRule ? 'Edit Alert Rule' : 'Create Alert Rule'}
            </DialogTitle>
            <DialogDescription>
              {editingRule
                ? 'Update the alert rule configuration.'
                : 'Configure a new error alert rule.'}
            </DialogDescription>
          </DialogHeader>
          <div className="flex-1 overflow-y-auto">
            <Form {...form}>
              <form
                id="alert-rule-form"
                onSubmit={form.handleSubmit(onSubmit)}
                className="space-y-4"
              >
                <FormField
                  control={form.control}
                  name="name"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Name</FormLabel>
                      <FormControl>
                        <Input placeholder="e.g. Critical error spike" {...field} />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="trigger_type"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Trigger Type</FormLabel>
                      <Select onValueChange={field.onChange} value={field.value}>
                        <FormControl>
                          <SelectTrigger>
                            <SelectValue placeholder="Select trigger" />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          {TRIGGER_TYPES.map((t) => (
                            <SelectItem key={t.value} value={t.value}>
                              {t.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <FormDescription>
                        {TRIGGER_TYPES.find((t) => t.value === field.value)?.description}
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                {needsConfig(watchedTriggerType) && (
                  <div className="space-y-4 rounded-md border p-4">
                    <p className="text-sm font-medium">Trigger Configuration</p>
                    {watchedTriggerType === 'frequency' && (
                      <>
                        <FormField
                          control={form.control}
                          name="trigger_config.count"
                          render={({ field }) => (
                            <FormItem>
                              <FormLabel>Event Count</FormLabel>
                              <FormControl>
                                <Input
                                  type="number"
                                  placeholder="100"
                                  value={field.value ?? ''}
                                  onChange={(e) =>
                                    field.onChange(e.target.value ? Number(e.target.value) : undefined)
                                  }
                                />
                              </FormControl>
                              <FormDescription>
                                Number of events to trigger the alert
                              </FormDescription>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                        <FormField
                          control={form.control}
                          name="trigger_config.window_minutes"
                          render={({ field }) => (
                            <FormItem>
                              <FormLabel>Time Window (minutes)</FormLabel>
                              <FormControl>
                                <Input
                                  type="number"
                                  placeholder="60"
                                  value={field.value ?? ''}
                                  onChange={(e) =>
                                    field.onChange(e.target.value ? Number(e.target.value) : undefined)
                                  }
                                />
                              </FormControl>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                      </>
                    )}
                    {watchedTriggerType === 'user_count' && (
                      <FormField
                        control={form.control}
                        name="trigger_config.threshold"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel>User Threshold</FormLabel>
                            <FormControl>
                              <Input
                                type="number"
                                placeholder="10"
                                value={field.value ?? ''}
                                onChange={(e) =>
                                  field.onChange(e.target.value ? Number(e.target.value) : undefined)
                                }
                              />
                            </FormControl>
                            <FormDescription>
                              Trigger when this many unique users are affected
                            </FormDescription>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                    )}
                  </div>
                )}

                <FormField
                  control={form.control}
                  name="notification_priority"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Priority</FormLabel>
                      <Select onValueChange={field.onChange} value={field.value}>
                        <FormControl>
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          {PRIORITIES.map((p) => (
                            <SelectItem key={p.value} value={p.value}>
                              {p.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="cooldown_minutes"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Cooldown (minutes)</FormLabel>
                      <FormControl>
                        <Input
                          type="number"
                          {...field}
                          onChange={(e) => field.onChange(Number(e.target.value))}
                        />
                      </FormControl>
                      <FormDescription>
                        Minimum time between notifications for the same rule
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="error_level_filter"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Error Level Filter</FormLabel>
                      <Select
                        onValueChange={(v) => field.onChange(v === 'all' ? null : v)}
                        value={field.value ?? 'all'}
                      >
                        <FormControl>
                          <SelectTrigger>
                            <SelectValue placeholder="All levels" />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          <SelectItem value="all">All Levels</SelectItem>
                          <SelectItem value="error">Error</SelectItem>
                          <SelectItem value="warning">Warning</SelectItem>
                          <SelectItem value="fatal">Fatal</SelectItem>
                        </SelectContent>
                      </Select>
                      <FormDescription>
                        Only trigger for errors at this level
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="enabled"
                  render={({ field }) => (
                    <FormItem className="flex items-center justify-between rounded-lg border p-3">
                      <div>
                        <FormLabel>Enabled</FormLabel>
                        <FormDescription>
                          Activate this rule immediately
                        </FormDescription>
                      </div>
                      <FormControl>
                        <Switch
                          checked={field.value}
                          onCheckedChange={field.onChange}
                        />
                      </FormControl>
                    </FormItem>
                  )}
                />
              </form>
            </Form>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={closeDialog}>
              Cancel
            </Button>
            <Button
              type="submit"
              form="alert-rule-form"
              disabled={isMutating}
            >
              {isMutating
                ? 'Saving...'
                : editingRule
                  ? 'Update Rule'
                  : 'Create Rule'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
