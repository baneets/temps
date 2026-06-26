import { ProjectResponse } from '@/api/client'
import type { Comparator } from '@/api/client'
// REGEN: bun run openapi-ts — these builders/types come from the new
// /otel/alerts endpoints (operationIds get_alert / create_alert / update_alert).
// They are absent from the committed SDK; imported here so the form is
// regen-ready instead of hand-rolling fetch (project rule).
import {
  createAlertMutation,
  getAlertOptions,
  listMetricNamesOptions,
  updateAlertMutation,
} from '@/api/client/@tanstack/react-query.gen'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
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
import { SearchableSelect } from '@/components/ui/searchable-select'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { AGGREGATIONS } from '@/components/metrics/metric-format'
import {
  AlertStateBadge,
  COMPARATORS,
  SEVERITIES,
} from '@/components/metrics/alert-format'
import { zodResolver } from '@hookform/resolvers/zod'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft } from 'lucide-react'
import { useMemo } from 'react'
import { useForm } from 'react-hook-form'
import { useNavigate, useParams } from 'react-router-dom'
import { toast } from 'sonner'
import { z } from 'zod'

interface MetricAlertFormProps {
  project: ProjectResponse
}

const AGGREGATION_VALUES = AGGREGATIONS.map((a) => a.value) as [
  string,
  ...string[],
]
const COMPARATOR_VALUES = COMPARATORS.map((c) => c.value) as [
  string,
  ...string[],
]
const SEVERITY_VALUES = SEVERITIES.map((s) => s.value) as [string, ...string[]]

const alertSchema = z.object({
  name: z.string().min(1, 'Name is required').max(200, 'Name is too long'),
  metric_name: z.string().min(1, 'Pick a metric'),
  aggregation: z.enum(AGGREGATION_VALUES),
  comparator: z.enum(COMPARATOR_VALUES),
  threshold: z.number({ message: 'Threshold must be a number' }),
  window_secs: z
    .number({ message: 'Window must be a number' })
    .int()
    .positive('Window must be greater than 0'),
  for_duration_secs: z
    .number({ message: 'Duration must be a number' })
    .int()
    .positive('Duration must be greater than 0'),
  severity: z.enum(SEVERITY_VALUES),
  enabled: z.boolean(),
})

type AlertFormData = z.infer<typeof alertSchema>

/** Narrow a stored token into a form enum union, falling back to a default. */
function coerce<T extends readonly string[]>(
  values: T,
  value: string,
  fallback: T[number],
): T[number] {
  return (values as readonly string[]).includes(value)
    ? (value as T[number])
    : fallback
}

function emptyDefaults(): AlertFormData {
  return {
    name: '',
    metric_name: '',
    aggregation: 'avg',
    comparator: 'gt',
    threshold: 0,
    window_secs: 300,
    // Must be > 0 (backend + Zod reject 0); default to one eval interval.
    for_duration_secs: 60,
    severity: 'warning',
    enabled: true,
  }
}

export default function MetricAlertForm({ project }: MetricAlertFormProps) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { alertId } = useParams()
  const isEditing = !!alertId
  const id = Number(alertId)

  const existingQuery = useQuery({
    ...getAlertOptions({ path: { id }, query: { project_id: project.id } }),
    enabled: isEditing && Number.isFinite(id),
  })

  const namesQuery = useQuery({
    ...listMetricNamesOptions({ path: { project_id: project.id } }),
    enabled: !!project.id,
  })
  const metricOptions = useMemo(
    () =>
      (namesQuery.data?.names ?? []).map((n) => ({
        value: n,
        label: n,
      })),
    [namesQuery.data],
  )

  const defaultValues = useMemo<AlertFormData>(() => {
    const existing = existingQuery.data
    if (existing) {
      // The form edits a static threshold; pull comparator/threshold out of the
      // typed detector union (only `static` is editable here for now).
      const cfg = existing.detection_config
      const isStatic = cfg.kind === 'static'
      return {
        name: existing.name,
        metric_name: existing.metric_name,
        aggregation: coerce(AGGREGATION_VALUES, existing.aggregation, 'avg'),
        comparator: coerce(
          COMPARATOR_VALUES,
          isStatic ? cfg.comparator : 'gt',
          'gt',
        ),
        threshold: isStatic ? cfg.threshold : 0,
        window_secs: existing.window_secs,
        for_duration_secs: existing.for_duration_secs,
        severity: coerce(SEVERITY_VALUES, existing.severity, 'warning'),
        enabled: existing.enabled,
      }
    }
    return emptyDefaults()
  }, [existingQuery.data])

  const form = useForm<AlertFormData>({
    resolver: zodResolver(alertSchema),
    values: defaultValues,
  })

  const createMutation = useMutation({
    ...createAlertMutation(),
    meta: { errorTitle: 'Failed to create alert rule' },
    onSuccess: () => {
      toast.success('Alert rule created')
      queryClient.invalidateQueries({
        predicate: (query) =>
          (query.queryKey[0] as Record<string, unknown>)?._id === 'listAlerts',
      })
      navigate('..')
    },
  })

  const updateMutation = useMutation({
    ...updateAlertMutation(),
    meta: { errorTitle: 'Failed to update alert rule' },
    onSuccess: () => {
      toast.success('Alert rule updated')
      queryClient.invalidateQueries({
        predicate: (query) => {
          const key = (query.queryKey[0] as Record<string, unknown>)?._id
          return key === 'listAlerts' || key === 'getAlert'
        },
      })
      navigate(-1)
    },
  })

  const isMutating = createMutation.isPending || updateMutation.isPending

  const onSubmit = async (data: AlertFormData) => {
    // The form authors a static threshold detector; wrap the flat comparator +
    // threshold into the typed `detection_config` discriminated union. The Zod
    // enum guarantees `comparator` is one of the Comparator literals, so the
    // narrowing cast is sound.
    const detection_config = {
      kind: 'static' as const,
      comparator: data.comparator as Comparator,
      threshold: data.threshold,
    }
    if (isEditing) {
      await updateMutation.mutateAsync({
        path: { id },
        query: { project_id: project.id },
        body: {
          name: data.name,
          metric_name: data.metric_name,
          aggregation: data.aggregation,
          detection_config,
          window_secs: data.window_secs,
          for_duration_secs: data.for_duration_secs,
          severity: data.severity,
          enabled: data.enabled,
        },
      })
    } else {
      await createMutation.mutateAsync({
        body: {
          project_id: project.id,
          name: data.name,
          metric_name: data.metric_name,
          aggregation: data.aggregation,
          detection_config,
          window_secs: data.window_secs,
          for_duration_secs: data.for_duration_secs,
          severity: data.severity,
          enabled: data.enabled,
        },
      })
    }
  }

  if (isEditing && existingQuery.isPending) {
    return (
      <div className="w-full space-y-6">
        <div className="flex items-center gap-4">
          <Skeleton className="size-9" />
          <Skeleton className="h-8 w-48" />
        </div>
        <Skeleton className="h-[500px] w-full" />
      </div>
    )
  }

  return (
    <div className="w-full space-y-6">
      <div className="flex items-center gap-4">
        <Button variant="ghost" size="icon" onClick={() => navigate(-1)}>
          <ArrowLeft className="size-4" />
        </Button>
        <div className="flex flex-1 flex-col gap-1">
          <div className="flex items-center gap-2">
            <h1 className="text-lg font-semibold">
              {isEditing ? 'Edit alert' : 'New alert'}
            </h1>
            {isEditing && existingQuery.data && (
              <AlertStateBadge state={existingQuery.data.last_state} />
            )}
          </div>
          <p className="text-sm text-muted-foreground">
            {isEditing
              ? 'Update the signal, threshold, and notification settings.'
              : 'Fire a notification when a metric crosses a threshold.'}
          </p>
        </div>
      </div>

      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Rule</CardTitle>
            </CardHeader>
            <CardContent className="space-y-6">
              <FormField
                control={form.control}
                name="name"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Name</FormLabel>
                    <FormControl>
                      <Input
                        placeholder="e.g. API p95 latency too high"
                        {...field}
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />

              {/* Signal: metric + aggregation */}
              <div className="grid gap-4 sm:grid-cols-2">
                <FormField
                  control={form.control}
                  name="metric_name"
                  render={({ field }) => (
                    <FormItem className="min-w-0">
                      <FormLabel>Metric</FormLabel>
                      <FormControl>
                        <SearchableSelect
                          value={field.value}
                          onValueChange={field.onChange}
                          options={metricOptions}
                          placeholder={
                            namesQuery.isPending
                              ? 'Loading metrics…'
                              : 'Select a metric…'
                          }
                          searchPlaceholder="Filter metrics…"
                          emptyText="No metrics ingested yet."
                          disabled={namesQuery.isPending}
                          className="font-mono text-xs"
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name="aggregation"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Aggregation</FormLabel>
                      <Select onValueChange={field.onChange} value={field.value}>
                        <FormControl>
                          <SelectTrigger className="font-mono text-xs">
                            <SelectValue />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          {AGGREGATIONS.map((a) => (
                            <SelectItem key={a.value} value={a.value}>
                              {a.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              {/* Condition: comparator + threshold */}
              <div className="grid gap-4 sm:grid-cols-2">
                <FormField
                  control={form.control}
                  name="comparator"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Comparator</FormLabel>
                      <Select onValueChange={field.onChange} value={field.value}>
                        <FormControl>
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          {COMPARATORS.map((c) => (
                            <SelectItem key={c.value} value={c.value}>
                              {c.label}
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
                  name="threshold"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Threshold</FormLabel>
                      <FormControl>
                        <Input
                          type="number"
                          step="any"
                          {...field}
                          onChange={(e) =>
                            field.onChange(
                              e.target.value === ''
                                ? undefined
                                : e.target.valueAsNumber,
                            )
                          }
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              {/* Timing: window + for-duration */}
              <div className="grid gap-4 sm:grid-cols-2">
                <FormField
                  control={form.control}
                  name="window_secs"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Evaluation window (seconds)</FormLabel>
                      <FormControl>
                        <Input
                          type="number"
                          min={1}
                          {...field}
                          onChange={(e) =>
                            field.onChange(
                              e.target.value === ''
                                ? undefined
                                : e.target.valueAsNumber,
                            )
                          }
                        />
                      </FormControl>
                      <FormDescription>
                        The metric is aggregated over this trailing window each
                        evaluation (e.g. 300 = last 5 minutes).
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name="for_duration_secs"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>For duration (seconds)</FormLabel>
                      <FormControl>
                        <Input
                          type="number"
                          min={1}
                          {...field}
                          onChange={(e) =>
                            field.onChange(
                              e.target.value === ''
                                ? undefined
                                : e.target.valueAsNumber,
                            )
                          }
                        />
                      </FormControl>
                      <FormDescription>
                        The breach must persist this long before the alert fires,
                        to avoid flapping.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              {/* Severity */}
              <FormField
                control={form.control}
                name="severity"
                render={({ field }) => (
                  <FormItem className="sm:max-w-[240px]">
                    <FormLabel>Severity</FormLabel>
                    <Select onValueChange={field.onChange} value={field.value}>
                      <FormControl>
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        {SEVERITIES.map((s) => (
                          <SelectItem key={s.value} value={s.value}>
                            {s.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <FormDescription>
                      Maps to the notification severity. Alerts are delivered
                      through this project's configured notification channels.
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />

              {/* Enabled toggle */}
              <FormField
                control={form.control}
                name="enabled"
                render={({ field }) => (
                  <FormItem className="flex flex-row items-center justify-between rounded-md border border-border/60 p-3">
                    <div className="space-y-0.5">
                      <FormLabel>Enabled</FormLabel>
                      <FormDescription>
                        Only enabled rules are evaluated by the background
                        evaluator.
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
            </CardContent>
          </Card>

          <div className="flex items-center gap-3 border-t pt-4">
            <Button type="submit" disabled={isMutating}>
              {isMutating
                ? 'Saving…'
                : isEditing
                  ? 'Save changes'
                  : 'Create alert'}
            </Button>
            <Button type="button" variant="outline" onClick={() => navigate(-1)}>
              Cancel
            </Button>
          </div>
        </form>
      </Form>
    </div>
  )
}
