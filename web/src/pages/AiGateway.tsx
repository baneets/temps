import { useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
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
import { CopyButton } from '@/components/ui/copy-button'
import { CodeBlock } from '@/components/ui/code-block'
import { EmptyState } from '@/components/ui/empty-state'
import {
  ChartConfig,
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from '@/components/ui/chart'
import { Bar, BarChart, Line, LineChart, XAxis, YAxis, CartesianGrid } from 'recharts'
import {
  Plus,
  Trash2,
  Loader2,
  Sparkles,
  Power,
  PowerOff,
  Play,
  CheckCircle2,
  XCircle,
  Activity,
  Zap,
  Clock,
  TrendingUp,
  DollarSign,
} from 'lucide-react'
import { toast } from 'sonner'
import {
  listProviderKeys,
  createProviderKey,
  deleteProviderKey,
  updateProviderKey,
  type ProviderKeyResponse,
} from '@/api/client'
import { useSettings } from '@/hooks/useSettings'

// ============================================================================
// Usage Analytics types & fetchers
// ============================================================================

interface UsageSummary {
  total_requests: number
  total_input_tokens: number
  total_output_tokens: number
  total_tokens: number
  avg_latency_ms: number
  total_cost_microcents: number
  error_count: number
  streaming_count: number
  byok_count: number
}

interface ProviderUsage {
  provider: string
  request_count: number
  input_tokens: number
  output_tokens: number
  avg_latency_ms: number
  error_count: number
}

interface TimeseriesBucket {
  bucket: string
  request_count: number
  input_tokens: number
  output_tokens: number
  avg_latency_ms: number
}

interface ModelUsage {
  model: string
  provider: string
  request_count: number
  input_tokens: number
  output_tokens: number
  total_tokens: number
  avg_latency_ms: number
}

interface UsageLogEntry {
  id: number
  timestamp: string
  provider: string
  model: string
  input_tokens: number
  output_tokens: number
  latency_ms: number
  estimated_cost_microcents: number
  status: number
  is_streaming: boolean
  is_byok: boolean
}

interface ModelPricing {
  model: string
  provider: string
  input_per_million: number
  output_per_million: number
}

interface PricingResponse {
  models: ModelPricing[]
}

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url)
  if (!res.ok) throw new Error(`Failed to fetch ${url}: ${res.status}`)
  return res.json()
}

function buildUsageUrl(path: string, params: Record<string, string | undefined>) {
  const searchParams = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) searchParams.set(key, value)
  }
  const qs = searchParams.toString()
  return `/api/ai/usage/${path}${qs ? `?${qs}` : ''}`
}

function formatTokenCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toString()
}

function formatCost(dollars: number): string {
  if (dollars >= 1) return `$${dollars.toFixed(2)}`
  if (dollars >= 0.01) return `$${dollars.toFixed(3)}`
  if (dollars >= 0.001) return `$${dollars.toFixed(4)}`
  if (dollars === 0) return '$0.00'
  return `$${dollars.toFixed(6)}`
}

function findPricing(model: string, pricingMap: Map<string, ModelPricing>): ModelPricing | undefined {
  // Exact match first
  const exact = pricingMap.get(model)
  if (exact) return exact

  // Strip common suffixes: -preview, -latest, date stamps like -20250101
  const stripped = model
    .replace(/-preview$/, '')
    .replace(/-latest$/, '')
    .replace(/-\d{6,8}$/, '')
  if (stripped !== model) {
    const match = pricingMap.get(stripped)
    if (match) return match
  }

  // Prefix match: find the longest pricing key that the model starts with
  let best: ModelPricing | undefined
  let bestLen = 0
  for (const [key, pricing] of pricingMap) {
    if (model.startsWith(key) && key.length > bestLen) {
      best = pricing
      bestLen = key.length
    }
  }
  return best
}

function computeCost(
  model: string,
  inputTokens: number,
  outputTokens: number,
  pricingMap: Map<string, ModelPricing>
): number {
  const pricing = findPricing(model, pricingMap)
  if (!pricing) return 0
  return (
    (inputTokens / 1_000_000) * pricing.input_per_million +
    (outputTokens / 1_000_000) * pricing.output_per_million
  )
}

const TIME_RANGES = [
  { label: '24h', hours: 24 },
  { label: '7d', hours: 168 },
  { label: '30d', hours: 720 },
] as const

const SUPPORTED_PROVIDERS = [
  { id: 'openai', name: 'OpenAI', models: 'GPT-5.4, GPT-5 Mini, GPT-5 Nano, GPT-4.1, o3, o4-mini', defaultModel: 'gpt-5.4' },
  { id: 'anthropic', name: 'Anthropic', models: 'Claude Opus 4.6, Claude Sonnet 4.6, Claude Haiku 4.5', defaultModel: 'claude-sonnet-4-6' },
  { id: 'xai', name: 'xAI', models: 'Grok 4-1 Fast, Grok Code Fast, Grok 4 Fast, Grok 3', defaultModel: 'grok-4-1-fast-reasoning' },
  { id: 'gemini', name: 'Google Gemini', models: 'Gemini 3.1 Pro, Gemini 3 Flash, Gemini 2.5 Pro, Gemini 2.5 Flash', defaultModel: 'gemini-2.5-flash' },
] as const

function providerName(id: string): string {
  return SUPPORTED_PROVIDERS.find((p) => p.id === id)?.name ?? id
}

function providerModels(id: string): string {
  return SUPPORTED_PROVIDERS.find((p) => p.id === id)?.models ?? ''
}

// ============================================================================
// Usage Analytics Component
// ============================================================================

const requestsChartConfig = {
  request_count: { label: 'Requests', color: 'var(--chart-1)' },
} satisfies ChartConfig

const tokensChartConfig = {
  input_tokens: { label: 'Input', color: 'var(--chart-1)' },
  output_tokens: { label: 'Output', color: 'var(--chart-2)' },
} satisfies ChartConfig

const latencyChartConfig = {
  avg_latency_ms: { label: 'Avg Latency (ms)', color: 'var(--chart-3)' },
} satisfies ChartConfig

const providerChartConfig = {
  request_count: { label: 'Requests', color: 'var(--chart-1)' },
} satisfies ChartConfig

function LogCostCell({ log, pricingMap }: { log: UsageLogEntry; pricingMap: Map<string, ModelPricing> }) {
  const cost = computeCost(log.model, log.input_tokens, log.output_tokens, pricingMap)
  return <>{cost > 0 ? formatCost(cost) : '—'}</>
}

function UsageAnalytics() {
  const [timeRange, setTimeRange] = useState<(typeof TIME_RANGES)[number]>(TIME_RANGES[0])

  const timeParams = useMemo(() => {
    const to = new Date().toISOString()
    const from = new Date(Date.now() - timeRange.hours * 3600_000).toISOString()
    const bucket = timeRange.hours <= 24 ? 'hour' : 'day'
    return { from, to, bucket }
  }, [timeRange])

  const { data: summary, isLoading: summaryLoading } = useQuery({
    queryKey: ['aiUsage', 'summary', timeParams.from, timeParams.to],
    queryFn: () =>
      fetchJson<UsageSummary>(
        buildUsageUrl('summary', { from: timeParams.from, to: timeParams.to })
      ),
  })

  const { data: timeseries, isLoading: timeseriesLoading } = useQuery({
    queryKey: ['aiUsage', 'timeseries', timeParams.from, timeParams.to, timeParams.bucket],
    queryFn: () =>
      fetchJson<TimeseriesBucket[]>(
        buildUsageUrl('timeseries', {
          from: timeParams.from,
          to: timeParams.to,
          bucket: timeParams.bucket,
        })
      ),
  })

  const { data: byProvider } = useQuery({
    queryKey: ['aiUsage', 'by-provider', timeParams.from, timeParams.to],
    queryFn: () =>
      fetchJson<ProviderUsage[]>(
        buildUsageUrl('by-provider', { from: timeParams.from, to: timeParams.to })
      ),
  })

  const { data: topModels } = useQuery({
    queryKey: ['aiUsage', 'top-models', timeParams.from, timeParams.to],
    queryFn: () =>
      fetchJson<ModelUsage[]>(
        buildUsageUrl('top-models', { from: timeParams.from, to: timeParams.to, limit: '10' })
      ),
  })

  const { data: recentLogs } = useQuery({
    queryKey: ['aiUsage', 'recent'],
    queryFn: () =>
      fetchJson<UsageLogEntry[]>(buildUsageUrl('recent', { limit: '20' })),
  })

  const { data: pricingData } = useQuery({
    queryKey: ['aiPricing'],
    queryFn: () => fetchJson<PricingResponse>('/api/ai/pricing'),
    staleTime: 60 * 60 * 1000,
  })

  const pricingMap = useMemo(() => {
    const map = new Map<string, ModelPricing>()
    for (const m of pricingData?.models ?? []) {
      map.set(m.model, m)
    }
    return map
  }, [pricingData])

  const totalEstimatedCost = useMemo(() => {
    if (!topModels || pricingMap.size === 0) return 0
    return topModels.reduce(
      (sum, m) => sum + computeCost(m.model, m.input_tokens, m.output_tokens, pricingMap),
      0
    )
  }, [topModels, pricingMap])

  const chartTimeseries = useMemo(
    () =>
      (timeseries ?? []).map((b) => ({
        ...b,
        label: timeParams.bucket === 'hour'
          ? new Date(b.bucket).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
          : new Date(b.bucket).toLocaleDateString([], { month: 'short', day: 'numeric' }),
      })),
    [timeseries, timeParams.bucket]
  )

  return (
    <div className="space-y-4">
      {/* Time range selector */}
      <div className="flex items-center justify-between">
        <h3 className="text-lg font-semibold">Usage Analytics</h3>
        <div className="flex gap-1">
          {TIME_RANGES.map((range) => (
            <Button
              key={range.label}
              variant={timeRange.label === range.label ? 'default' : 'outline'}
              size="sm"
              onClick={() => setTimeRange(range)}
            >
              {range.label}
            </Button>
          ))}
        </div>
      </div>

      {/* Summary stat cards */}
      <div className="grid gap-4 grid-cols-2 md:grid-cols-5">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <Activity className="h-4 w-4 text-muted-foreground" />
              Requests
            </CardTitle>
          </CardHeader>
          <CardContent>
            {summaryLoading ? (
              <Skeleton className="h-7 w-16" />
            ) : (
              <div className="text-2xl font-bold">
                {formatTokenCount(summary?.total_requests ?? 0)}
              </div>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <Zap className="h-4 w-4 text-muted-foreground" />
              Total Tokens
            </CardTitle>
          </CardHeader>
          <CardContent>
            {summaryLoading ? (
              <Skeleton className="h-7 w-16" />
            ) : (
              <div className="text-2xl font-bold">
                {formatTokenCount(summary?.total_tokens ?? 0)}
              </div>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <Clock className="h-4 w-4 text-muted-foreground" />
              Avg Latency
            </CardTitle>
          </CardHeader>
          <CardContent>
            {summaryLoading ? (
              <Skeleton className="h-7 w-16" />
            ) : (
              <div className="text-2xl font-bold">
                {Math.round(summary?.avg_latency_ms ?? 0)}ms
              </div>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <TrendingUp className="h-4 w-4 text-muted-foreground" />
              Error Rate
            </CardTitle>
          </CardHeader>
          <CardContent>
            {summaryLoading ? (
              <Skeleton className="h-7 w-16" />
            ) : (
              <div className="text-2xl font-bold">
                {summary && summary.total_requests > 0
                  ? `${((summary.error_count / summary.total_requests) * 100).toFixed(1)}%`
                  : '0%'}
              </div>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <DollarSign className="h-4 w-4 text-muted-foreground" />
              Est. Cost
            </CardTitle>
          </CardHeader>
          <CardContent>
            {summaryLoading ? (
              <Skeleton className="h-7 w-16" />
            ) : (
              <div className="text-2xl font-bold">
                {formatCost(totalEstimatedCost)}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Requests over time chart */}
      <Card>
        <CardHeader>
          <CardTitle className="text-base">Requests Over Time</CardTitle>
        </CardHeader>
        <CardContent>
          {timeseriesLoading ? (
            <Skeleton className="h-[200px] w-full" />
          ) : chartTimeseries.length === 0 ? (
            <EmptyState
              icon={Activity}
              title="No usage data"
              description="Make some requests through the AI Gateway to see analytics here."
            />
          ) : (
            <ChartContainer config={requestsChartConfig} className="h-[200px] w-full">
              <BarChart data={chartTimeseries} accessibilityLayer>
                <CartesianGrid vertical={false} />
                <XAxis dataKey="label" tickLine={false} axisLine={false} fontSize={12} />
                <YAxis tickLine={false} axisLine={false} fontSize={12} />
                <ChartTooltip content={<ChartTooltipContent />} />
                <Bar dataKey="request_count" fill="var(--color-request_count)" radius={[4, 4, 0, 0]} />
              </BarChart>
            </ChartContainer>
          )}
        </CardContent>
      </Card>

      {/* Token usage + Latency charts side by side */}
      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Token Usage</CardTitle>
          </CardHeader>
          <CardContent>
            {timeseriesLoading ? (
              <Skeleton className="h-[200px] w-full" />
            ) : chartTimeseries.length === 0 ? (
              <div className="h-[200px] flex items-center justify-center text-sm text-muted-foreground">
                No data
              </div>
            ) : (
              <ChartContainer config={tokensChartConfig} className="h-[200px] w-full">
                <LineChart data={chartTimeseries} accessibilityLayer>
                  <CartesianGrid vertical={false} />
                  <XAxis dataKey="label" tickLine={false} axisLine={false} fontSize={12} />
                  <YAxis tickLine={false} axisLine={false} fontSize={12} tickFormatter={formatTokenCount} />
                  <ChartTooltip content={<ChartTooltipContent />} />
                  <Line type="monotone" dataKey="input_tokens" stroke="var(--color-input_tokens)" strokeWidth={2} dot={false} />
                  <Line type="monotone" dataKey="output_tokens" stroke="var(--color-output_tokens)" strokeWidth={2} dot={false} />
                </LineChart>
              </ChartContainer>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Avg Latency</CardTitle>
          </CardHeader>
          <CardContent>
            {timeseriesLoading ? (
              <Skeleton className="h-[200px] w-full" />
            ) : chartTimeseries.length === 0 ? (
              <div className="h-[200px] flex items-center justify-center text-sm text-muted-foreground">
                No data
              </div>
            ) : (
              <ChartContainer config={latencyChartConfig} className="h-[200px] w-full">
                <LineChart data={chartTimeseries} accessibilityLayer>
                  <CartesianGrid vertical={false} />
                  <XAxis dataKey="label" tickLine={false} axisLine={false} fontSize={12} />
                  <YAxis tickLine={false} axisLine={false} fontSize={12} unit="ms" />
                  <ChartTooltip content={<ChartTooltipContent />} />
                  <Line type="monotone" dataKey="avg_latency_ms" stroke="var(--color-avg_latency_ms)" strokeWidth={2} dot={false} />
                </LineChart>
              </ChartContainer>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Per-provider breakdown + Top models */}
      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">By Provider</CardTitle>
          </CardHeader>
          <CardContent>
            {!byProvider || byProvider.length === 0 ? (
              <div className="h-[200px] flex items-center justify-center text-sm text-muted-foreground">
                No data
              </div>
            ) : (
              <ChartContainer config={providerChartConfig} className="h-[200px] w-full">
                <BarChart
                  data={byProvider.map((p) => ({ ...p, name: providerName(p.provider) }))}
                  layout="vertical"
                  accessibilityLayer
                >
                  <CartesianGrid horizontal={false} />
                  <XAxis type="number" tickLine={false} axisLine={false} fontSize={12} />
                  <YAxis dataKey="name" type="category" tickLine={false} axisLine={false} fontSize={12} width={80} />
                  <ChartTooltip content={<ChartTooltipContent />} />
                  <Bar dataKey="request_count" fill="var(--color-request_count)" radius={[0, 4, 4, 0]} />
                </BarChart>
              </ChartContainer>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Top Models</CardTitle>
          </CardHeader>
          <CardContent>
            {!topModels || topModels.length === 0 ? (
              <div className="h-[200px] flex items-center justify-center text-sm text-muted-foreground">
                No data
              </div>
            ) : (
              <div className="space-y-3">
                {topModels.map((m) => {
                  const cost = computeCost(m.model, m.input_tokens, m.output_tokens, pricingMap)
                  return (
                    <div key={m.model} className="flex items-center justify-between text-sm">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="font-mono truncate">{m.model}</span>
                        <Badge variant="outline" className="text-[10px] shrink-0">
                          {providerName(m.provider)}
                        </Badge>
                      </div>
                      <div className="flex items-center gap-4 shrink-0 text-muted-foreground">
                        <span>{formatTokenCount(m.request_count)} req</span>
                        <span>{formatTokenCount(m.total_tokens)} tok</span>
                        {cost > 0 && (
                          <span className="text-orange-500 font-medium">{formatCost(cost)}</span>
                        )}
                      </div>
                    </div>
                  )
                })}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Recent requests */}
      <Card>
        <CardHeader>
          <CardTitle className="text-base">Recent Requests</CardTitle>
        </CardHeader>
        <CardContent>
          {!recentLogs || recentLogs.length === 0 ? (
            <div className="py-8 text-center text-sm text-muted-foreground">
              No recent requests
            </div>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Time</TableHead>
                    <TableHead>Provider</TableHead>
                    <TableHead>Model</TableHead>
                    <TableHead className="hidden md:table-cell">Tokens</TableHead>
                    <TableHead className="hidden md:table-cell">Latency</TableHead>
                    <TableHead className="hidden md:table-cell">Cost</TableHead>
                    <TableHead>Status</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {recentLogs.map((log) => (
                    <TableRow key={log.id}>
                      <TableCell className="text-muted-foreground text-xs whitespace-nowrap">
                        {new Date(log.timestamp).toLocaleString([], {
                          month: 'short',
                          day: 'numeric',
                          hour: '2-digit',
                          minute: '2-digit',
                        })}
                      </TableCell>
                      <TableCell className="font-medium">
                        {providerName(log.provider)}
                      </TableCell>
                      <TableCell className="font-mono text-xs">{log.model}</TableCell>
                      <TableCell className="hidden md:table-cell text-muted-foreground">
                        {formatTokenCount(log.input_tokens + log.output_tokens)}
                      </TableCell>
                      <TableCell className="hidden md:table-cell text-muted-foreground">
                        {log.latency_ms}ms
                      </TableCell>
                      <TableCell className="hidden md:table-cell text-muted-foreground">
                        <LogCostCell log={log} pricingMap={pricingMap} />
                      </TableCell>
                      <TableCell>
                        <div className="flex items-center gap-1.5">
                          <Badge
                            variant={log.status < 400 ? 'default' : 'destructive'}
                            className={
                              log.status < 400
                                ? 'bg-green-500/15 text-green-500 hover:bg-green-500/25'
                                : ''
                            }
                          >
                            {log.status}
                          </Badge>
                          {log.is_streaming && (
                            <Badge variant="outline" className="text-[10px]">
                              stream
                            </Badge>
                          )}
                          {log.is_byok && (
                            <Badge variant="outline" className="text-[10px]">
                              BYOK
                            </Badge>
                          )}
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

function DeleteConfirmDialog({
  open,
  onOpenChange,
  providerKey,
  onConfirm,
  isPending,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  providerKey: ProviderKeyResponse | null
  onConfirm: (id: number) => void
  isPending: boolean
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Delete Provider Key</DialogTitle>
          <DialogDescription>
            Are you sure you want to delete &quot;{providerKey?.display_name}&quot;?
            This will stop routing requests through this provider key.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            onClick={() => providerKey && onConfirm(providerKey.id)}
            disabled={isPending}
          >
            {isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

const VALID_TABS = ['keys', 'usage', 'settings'] as const
type TabValue = (typeof VALID_TABS)[number]

export function AiGatewayPage() {
  const queryClient = useQueryClient()
  const { data: settings } = useSettings()
  const [searchParams, setSearchParams] = useSearchParams()

  const tabParam = searchParams.get('tab') as TabValue | null
  const activeTab = tabParam && VALID_TABS.includes(tabParam) ? tabParam : 'keys'
  const setActiveTab = (tab: string) => {
    setSearchParams({ tab }, { replace: true })
  }

  const [dialogOpen, setDialogOpen] = useState(false)
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false)
  const [selectedKey, setSelectedKey] = useState<ProviderKeyResponse | null>(null)
  const [snippetLang, setSnippetLang] = useState<'bash' | 'python' | 'typescript'>('bash')
  const [snippetProvider, setSnippetProvider] = useState<string>('')

  // Form state
  const [newProvider, setNewProvider] = useState('')
  const [newDisplayName, setNewDisplayName] = useState('')
  const [newApiKey, setNewApiKey] = useState('')
  const [newBaseUrl, setNewBaseUrl] = useState('')

  // Derive the gateway endpoint from platform settings
  const externalUrl = settings?.external_url || window.location.origin
  const gatewayEndpoint = `${externalUrl}/api/ai/v1`

  // Fetch provider keys
  const { data: keysData, isLoading } = useQuery({
    queryKey: ['providerKeys'],
    queryFn: async () => {
      const response = await listProviderKeys()
      return response.data
    },
  })

  const keys = keysData ?? []

  // Create mutation
  const createMutation = useMutation({
    mutationFn: (data: { provider: string; display_name: string; api_key: string; base_url?: string }) =>
      createProviderKey({ body: data }),
    meta: { errorTitle: 'Failed to create provider key' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['providerKeys'] })
      setDialogOpen(false)
      resetForm()
      toast.success('Provider key added')
    },
  })

  // Delete mutation
  const deleteMutation = useMutation({
    mutationFn: (id: number) => deleteProviderKey({ path: { id } }),
    meta: { errorTitle: 'Failed to delete provider key' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['providerKeys'] })
      setDeleteDialogOpen(false)
      setSelectedKey(null)
      toast.success('Provider key deleted')
    },
  })

  // Toggle active mutation
  const toggleMutation = useMutation({
    mutationFn: ({ id, is_active }: { id: number; is_active: boolean }) =>
      updateProviderKey({ path: { id }, body: { is_active } }),
    meta: { errorTitle: 'Failed to update provider key' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['providerKeys'] })
    },
  })

  // Test existing key by ID
  const [testingKeyId, setTestingKeyId] = useState<number | null>(null)
  const testKeyMutation = useMutation({
    mutationFn: async (id: number) => {
      setTestingKeyId(id)
      const res = await fetch(`/api/ai/providers/${id}/test`, { method: 'POST' })
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      return res.json() as Promise<{ success: boolean; provider: string; error?: string; latency_ms: number }>
    },
    onSuccess: (data) => {
      if (data.success) {
        toast.success(`${providerName(data.provider)} key is valid (${data.latency_ms}ms)`)
      } else {
        toast.error(`${providerName(data.provider)} key test failed`, {
          description: data.error,
        })
      }
      setTestingKeyId(null)
    },
    onError: (err) => {
      toast.error('Failed to test provider key', { description: String(err) })
      setTestingKeyId(null)
    },
  })

  // Test inline key (for the add dialog)
  const [inlineTestResult, setInlineTestResult] = useState<{ success: boolean; error?: string } | null>(null)
  const testInlineMutation = useMutation({
    mutationFn: async (body: { provider: string; api_key: string; base_url?: string }) => {
      const res = await fetch('/api/ai/providers/test', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      return res.json() as Promise<{ success: boolean; provider: string; error?: string; latency_ms: number }>
    },
    onSuccess: (data) => {
      setInlineTestResult({ success: data.success, error: data.error })
      if (data.success) {
        toast.success(`Key verified (${data.latency_ms}ms)`)
      } else {
        toast.error('Key test failed', { description: data.error })
      }
    },
    onError: (err) => {
      setInlineTestResult({ success: false, error: String(err) })
      toast.error('Failed to test key', { description: String(err) })
    },
  })

  const resetForm = () => {
    setNewProvider('')
    setNewDisplayName('')
    setNewApiKey('')
    setNewBaseUrl('')
    setInlineTestResult(null)
  }

  const handleCreate = () => {
    if (!newProvider) {
      toast.error('Please select a provider')
      return
    }
    if (!newApiKey.trim()) {
      toast.error('Please enter an API key')
      return
    }
    if (!newDisplayName.trim()) {
      toast.error('Please enter a display name')
      return
    }

    createMutation.mutate({
      provider: newProvider,
      display_name: newDisplayName,
      api_key: newApiKey,
      base_url: newBaseUrl || undefined,
    })
  }

  const handleDelete = (key: ProviderKeyResponse) => {
    setSelectedKey(key)
    setDeleteDialogOpen(true)
  }

  const handleToggle = (key: ProviderKeyResponse) => {
    toggleMutation.mutate({ id: key.id, is_active: !key.is_active })
  }

  const activeCount = keys.filter((k) => k.is_active).length

  // Pick the first configured provider, or fall back to the first in the list
  const firstConfiguredProvider = SUPPORTED_PROVIDERS.find((p) =>
    keys.some((k) => k.provider === p.id && k.is_active)
  )
  const effectiveSnippetProvider =
    snippetProvider ||
    firstConfiguredProvider?.id ||
    SUPPORTED_PROVIDERS[0].id
  const snippetModel =
    SUPPORTED_PROVIDERS.find((p) => p.id === effectiveSnippetProvider)?.defaultModel ??
    SUPPORTED_PROVIDERS[0].defaultModel

  const codeSnippets = {
    bash: `curl ${gatewayEndpoint}/chat/completions \\
  -H "Authorization: Bearer YOUR_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "${snippetModel}",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'`,
    python: `from openai import OpenAI

client = OpenAI(
    base_url="${gatewayEndpoint}",
    api_key="YOUR_API_KEY",
)

response = client.chat.completions.create(
    model="${snippetModel}",
    messages=[{"role": "user", "content": "Hello!"}],
)
print(response.choices[0].message.content)`,
    typescript: `import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "${gatewayEndpoint}",
  apiKey: "YOUR_API_KEY",
});

const response = await client.chat.completions.create({
  model: "${snippetModel}",
  messages: [{ role: "user", content: "Hello!" }],
});
console.log(response.choices[0].message.content);`,
  }

  return (
    <div className="container mx-auto py-6 space-y-6">
      {/* Page Header */}
      <div className="flex justify-between items-center">
        <div>
          <h1 className="text-3xl font-bold">AI Gateway</h1>
          <p className="text-muted-foreground mt-2">
            Unified API for multiple AI providers with a single endpoint
          </p>
        </div>
        <Button onClick={() => setDialogOpen(true)}>
          <Plus className="mr-2 h-4 w-4" />
          Add Provider Key
        </Button>
      </div>

      {/* Quick Stats */}
      <div className="grid gap-4 grid-cols-2 md:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">Active Providers</CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-7 w-8" />
            ) : (
              <div className="text-2xl font-bold">{activeCount}</div>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">Total Keys</CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <Skeleton className="h-7 w-8" />
            ) : (
              <div className="text-2xl font-bold">{keys.length}</div>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">Gateway Endpoint</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-1">
              <code className="text-xs text-muted-foreground truncate">
                {gatewayEndpoint}
              </code>
              <CopyButton value={gatewayEndpoint} minimal className="h-6 w-6 shrink-0" />
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">Status</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              <div className="h-2 w-2 rounded-full bg-green-500" />
              <span className="text-sm font-medium">Operational</span>
            </div>
          </CardContent>
        </Card>
      </div>

      <Tabs value={activeTab} onValueChange={setActiveTab} className="space-y-4">
        <TabsList>
          <TabsTrigger value="keys">Provider Keys</TabsTrigger>
          <TabsTrigger value="usage">Usage</TabsTrigger>
          <TabsTrigger value="settings">Settings</TabsTrigger>
        </TabsList>

        {/* Provider Keys Tab */}
        <TabsContent value="keys" className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle>Provider Keys</CardTitle>
              <CardDescription>
                {isLoading ? (
                  <Skeleton className="h-4 w-28 inline-block" />
                ) : (
                  `${keys.length} key${keys.length !== 1 ? 's' : ''} configured`
                )}
              </CardDescription>
            </CardHeader>
            <CardContent>
              {isLoading ? (
                <div className="space-y-3">
                  {[1, 2, 3].map((i) => (
                    <Skeleton key={i} className="h-12 w-full" />
                  ))}
                </div>
              ) : keys.length === 0 ? (
                <EmptyState
                  icon={Sparkles}
                  title="No provider keys yet"
                  description="Add your first AI provider key to start routing requests through the gateway."
                  action={
                    <Button onClick={() => setDialogOpen(true)}>
                      <Plus className="mr-2 h-4 w-4" />
                      Add Provider Key
                    </Button>
                  }
                />
              ) : (
                <div className="overflow-x-auto">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Provider</TableHead>
                        <TableHead>Name</TableHead>
                        <TableHead className="hidden md:table-cell">Key</TableHead>
                        <TableHead>Status</TableHead>
                        <TableHead className="hidden md:table-cell">Added</TableHead>
                        <TableHead className="w-[100px]"></TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {keys.map((key) => (
                        <TableRow key={key.id}>
                          <TableCell className="font-medium">
                            {providerName(key.provider)}
                          </TableCell>
                          <TableCell>{key.display_name}</TableCell>
                          <TableCell className="hidden md:table-cell font-mono text-sm text-muted-foreground">
                            {key.api_key_masked}
                          </TableCell>
                          <TableCell>
                            <Badge
                              variant={key.is_active ? 'default' : 'secondary'}
                              className={
                                key.is_active
                                  ? 'bg-green-500/15 text-green-500 hover:bg-green-500/25'
                                  : ''
                              }
                            >
                              {key.is_active ? 'Active' : 'Inactive'}
                            </Badge>
                          </TableCell>
                          <TableCell className="hidden md:table-cell text-muted-foreground">
                            {new Date(key.created_at).toLocaleDateString()}
                          </TableCell>
                          <TableCell>
                            <div className="flex items-center gap-1">
                              <Button
                                variant="ghost"
                                size="icon"
                                className="h-8 w-8"
                                onClick={() => testKeyMutation.mutate(key.id)}
                                disabled={testingKeyId === key.id}
                                title="Test key"
                              >
                                {testingKeyId === key.id ? (
                                  <Loader2 className="h-4 w-4 animate-spin" />
                                ) : (
                                  <Play className="h-4 w-4" />
                                )}
                              </Button>
                              <Button
                                variant="ghost"
                                size="icon"
                                className="h-8 w-8"
                                onClick={() => handleToggle(key)}
                                disabled={toggleMutation.isPending}
                                title={key.is_active ? 'Disable key' : 'Enable key'}
                              >
                                {key.is_active ? (
                                  <Power className="h-4 w-4" />
                                ) : (
                                  <PowerOff className="h-4 w-4 text-muted-foreground" />
                                )}
                              </Button>
                              <Button
                                variant="ghost"
                                size="icon"
                                className="h-8 w-8"
                                onClick={() => handleDelete(key)}
                              >
                                <Trash2 className="h-4 w-4 text-destructive" />
                              </Button>
                            </div>
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>
              )}
            </CardContent>
          </Card>
        </TabsContent>

        {/* Usage Tab */}
        <TabsContent value="usage" className="space-y-4">
          <UsageAnalytics />
        </TabsContent>

        {/* Settings Tab */}
        <TabsContent value="settings" className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle>Gateway Endpoint</CardTitle>
              <CardDescription>
                Use this endpoint with any OpenAI-compatible SDK. Just swap the
                base URL and use your Temps API key.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex items-center gap-2">
                <code className="flex-1 rounded-md bg-muted px-3 py-2 text-sm font-mono">
                  {gatewayEndpoint}
                </code>
                <CopyButton value={gatewayEndpoint} className="shrink-0" />
              </div>
              <p className="text-xs text-muted-foreground">
                The gateway is OpenAI-compatible — use any model from any
                configured provider with the same endpoint.
              </p>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Quick Start</CardTitle>
              <CardDescription>
                Copy a code snippet to start making requests.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                <div className="flex gap-2">
                  {(['bash', 'python', 'typescript'] as const).map((lang) => (
                    <Button
                      key={lang}
                      variant={snippetLang === lang ? 'default' : 'outline'}
                      size="sm"
                      onClick={() => setSnippetLang(lang)}
                    >
                      {lang === 'bash'
                        ? 'cURL'
                        : lang === 'python'
                          ? 'Python'
                          : 'Node.js'}
                    </Button>
                  ))}
                </div>
                <Select value={effectiveSnippetProvider} onValueChange={setSnippetProvider}>
                  <SelectTrigger className="w-full sm:w-[180px]">
                    <SelectValue placeholder="Provider" />
                  </SelectTrigger>
                  <SelectContent>
                    {SUPPORTED_PROVIDERS.map((p) => (
                      <SelectItem key={p.id} value={p.id}>
                        {p.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <CodeBlock
                code={codeSnippets[snippetLang]}
                language={snippetLang === 'bash' ? 'bash' : snippetLang}
              />
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Supported Providers</CardTitle>
              <CardDescription>
                Models available through the AI Gateway.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <div className="grid gap-3 sm:grid-cols-2">
                {SUPPORTED_PROVIDERS.map((provider) => {
                  const configured = keys.some(
                    (k) => k.provider === provider.id && k.is_active
                  )
                  return (
                    <div
                      key={provider.id}
                      className="flex items-start justify-between rounded-lg border p-3"
                    >
                      <div>
                        <div className="flex items-center gap-2">
                          <p className="text-sm font-medium">{provider.name}</p>
                          {configured && (
                            <Badge
                              variant="default"
                              className="bg-green-500/15 text-green-500 text-[10px] px-1.5 py-0"
                            >
                              Active
                            </Badge>
                          )}
                        </div>
                        <p className="text-xs text-muted-foreground mt-0.5">
                          {provider.models}
                        </p>
                      </div>
                      {!configured && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-xs h-7"
                          onClick={() => {
                            setNewProvider(provider.id)
                            setDialogOpen(true)
                          }}
                        >
                          <Plus className="mr-1 h-3 w-3" />
                          Add
                        </Button>
                      )}
                    </div>
                  )
                })}
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">Bring Your Own Key (BYOK)</CardTitle>
            </CardHeader>
            <CardContent className="text-sm text-muted-foreground space-y-3">
              <p>
                You can also pass provider keys per-request using HTTP headers,
                bypassing the stored keys above. This is useful for testing or
                when you want to use a different key for specific requests.
              </p>
              <CodeBlock
                code={`X-Provider-Api-Key: sk-your-key-here\nX-Provider-Base-Url: https://custom-endpoint.example.com/v1`}
                language="text"
                title="BYOK Headers"
              />
              <p className="text-xs">
                When BYOK headers are present, stored keys are not used for that
                request. The response will include a{' '}
                <code className="bg-muted px-1 py-0.5 rounded text-foreground">
                  x-temps-credential-type: byok
                </code>{' '}
                header.
              </p>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>

      {/* Add Provider Key Dialog */}
      <Dialog open={dialogOpen} onOpenChange={(open) => { setDialogOpen(open); if (!open) resetForm() }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add Provider Key</DialogTitle>
            <DialogDescription>
              Add an API key for an AI provider. The key will be encrypted and
              used to route requests through the gateway.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-4 py-4">
            <div className="grid gap-2">
              <Label htmlFor="provider">Provider</Label>
              <Select value={newProvider} onValueChange={(value) => {
                setNewProvider(value)
                if (!newDisplayName.trim()) {
                  const provider = SUPPORTED_PROVIDERS.find((p) => p.id === value)
                  if (provider) setNewDisplayName(provider.name)
                }
              }}>
                <SelectTrigger id="provider">
                  <SelectValue placeholder="Select a provider" />
                </SelectTrigger>
                <SelectContent>
                  {SUPPORTED_PROVIDERS.map((p) => (
                    <SelectItem key={p.id} value={p.id}>
                      {p.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {newProvider && (
                <p className="text-xs text-muted-foreground">
                  Models: {providerModels(newProvider)}
                </p>
              )}
            </div>
            <div className="grid gap-2">
              <Label htmlFor="displayName">Display Name</Label>
              <Input
                id="displayName"
                placeholder="Production API Key"
                value={newDisplayName}
                onChange={(e) => setNewDisplayName(e.target.value)}
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="apiKey">API Key</Label>
              <Input
                id="apiKey"
                type="password"
                placeholder="sk-..."
                value={newApiKey}
                onChange={(e) => setNewApiKey(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                Your key is encrypted at rest and never exposed in the dashboard.
              </p>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="baseUrl">
                Custom Base URL{' '}
                <span className="text-muted-foreground font-normal">(optional)</span>
              </Label>
              <Input
                id="baseUrl"
                placeholder="https://api.openai.com/v1"
                value={newBaseUrl}
                onChange={(e) => setNewBaseUrl(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                Override the default API endpoint for this provider.
              </p>
            </div>
          </div>
          {inlineTestResult && (
            <div
              className={`flex items-center gap-2 rounded-md px-3 py-2 text-sm ${
                inlineTestResult.success
                  ? 'bg-green-500/10 text-green-500'
                  : 'bg-destructive/10 text-destructive'
              }`}
            >
              {inlineTestResult.success ? (
                <CheckCircle2 className="h-4 w-4 shrink-0" />
              ) : (
                <XCircle className="h-4 w-4 shrink-0" />
              )}
              <span>
                {inlineTestResult.success
                  ? 'Key is valid'
                  : inlineTestResult.error || 'Key test failed'}
              </span>
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>
              Cancel
            </Button>
            <Button
              variant="secondary"
              onClick={() => {
                if (!newProvider || !newApiKey.trim()) {
                  toast.error('Select a provider and enter an API key to test')
                  return
                }
                setInlineTestResult(null)
                testInlineMutation.mutate({
                  provider: newProvider,
                  api_key: newApiKey,
                  base_url: newBaseUrl || undefined,
                })
              }}
              disabled={testInlineMutation.isPending}
            >
              {testInlineMutation.isPending ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Play className="mr-2 h-4 w-4" />
              )}
              Test Key
            </Button>
            <Button onClick={handleCreate} disabled={createMutation.isPending}>
              {createMutation.isPending && (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              )}
              Add Key
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Delete Confirmation Dialog */}
      <DeleteConfirmDialog
        open={deleteDialogOpen}
        onOpenChange={(open) => {
          setDeleteDialogOpen(open)
          if (!open) setSelectedKey(null)
        }}
        providerKey={selectedKey}
        onConfirm={(id) => deleteMutation.mutate(id)}
        isPending={deleteMutation.isPending}
      />
    </div>
  )
}
