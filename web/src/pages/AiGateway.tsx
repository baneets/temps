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
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { Progress } from '@/components/ui/progress'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible'
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
  Bot,
  ChevronRight,
  ArrowLeft,
  Wrench,
  MessageSquare,
  AlertTriangle,
  Database,
  Globe,
  Cpu,
  Hash,
  Info,
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
import { useProjects } from '@/contexts/ProjectsContext'

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
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
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
      <div className="grid gap-3 sm:gap-4 grid-cols-2 md:grid-cols-5">
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
                    <div key={m.model} className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between text-sm">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="font-mono truncate">{m.model}</span>
                        <Badge variant="outline" className="text-[10px] shrink-0">
                          {providerName(m.provider)}
                        </Badge>
                      </div>
                      <div className="flex items-center gap-3 sm:gap-4 shrink-0 text-muted-foreground text-xs sm:text-sm">
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

// ============================================================================
// GenAI Agent Activity Types & Component
// ============================================================================

interface GenAiTraceSummary {
  trace_id: string
  root_span_name: string
  service_name: string
  gen_ai_system: string | null
  gen_ai_model: string | null
  gen_ai_operation: string | null
  start_time: string
  duration_ms: number
  span_count: number
  error_count: number
  total_input_tokens: number | null
  total_output_tokens: number | null
  total_cache_creation_input_tokens: number | null
  total_cache_read_input_tokens: number | null
}

interface GenAiSpanDetail {
  span_id: string
  parent_span_id: string | null
  name: string
  kind: string
  start_time: string
  duration_ms: number
  status_code: string

  // Core identification
  gen_ai_system: string | null
  gen_ai_operation: string | null

  // Model
  gen_ai_model: string | null
  gen_ai_response_model: string | null

  // Request parameters
  request_temperature: number | null
  request_max_tokens: number | null
  request_top_p: number | null
  request_top_k: number | null
  request_frequency_penalty: number | null
  request_presence_penalty: number | null
  request_stop_sequences: string[] | null
  request_seed: number | null
  request_choice_count: number | null

  // Response
  response_id: string | null
  response_finish_reasons: string[] | null
  output_type: string | null

  // Token usage
  input_tokens: number | null
  output_tokens: number | null
  cache_creation_input_tokens: number | null
  cache_read_input_tokens: number | null

  // Conversation / Error / Server
  conversation_id: string | null
  error_type: string | null
  server_address: string | null
  server_port: number | null

  // Agent
  agent_id: string | null
  agent_name: string | null
  agent_description: string | null
  agent_version: string | null

  // Tool
  tool_name: string | null
  tool_call_id: string | null
  tool_type: string | null
  tool_description: string | null

  // Embeddings
  embeddings_dimension_count: number | null
  request_encoding_formats: string[] | null

  // Retrieval
  data_source_id: string | null

  // Provider-specific
  openai_api_type: string | null
  openai_request_service_tier: string | null
  openai_response_service_tier: string | null
  openai_system_fingerprint: string | null
  aws_bedrock_guardrail_id: string | null
  aws_bedrock_knowledge_base_id: string | null
  azure_resource_provider_namespace: string | null

  // Opt-in content
  input_messages: string | null
  output_messages: string | null
  system_instructions: string | null
  tool_definitions: string | null
  tool_call_arguments: string | null
  tool_call_result: string | null
  retrieval_query_text: string | null
  retrieval_documents: string | null

  attributes: Record<string, string>
}

interface GenAiEvent {
  span_id: string
  trace_id: string
  event_name: string
  timestamp: string
  attributes: Record<string, string>
}

interface GenAiTraceSummariesResponse {
  data: GenAiTraceSummary[]
  total: number
}

interface GenAiTraceDetailResponse {
  trace_id: string
  spans: GenAiSpanDetail[]
  span_count: number
  events: GenAiEvent[]
  event_count: number
}

function buildOtelUrl(path: string, params: Record<string, string | number | undefined>) {
  const searchParams = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) searchParams.set(key, String(value))
  }
  const qs = searchParams.toString()
  return `/api/otel/${path}${qs ? `?${qs}` : ''}`
}

// ── Span tree helpers ───────────────────────────────────────────────

interface SpanTreeNode {
  span: GenAiSpanDetail
  children: SpanTreeNode[]
}

function buildSpanTree(spans: GenAiSpanDetail[]): SpanTreeNode[] {
  const byId = new Map<string, SpanTreeNode>()
  for (const span of spans) {
    byId.set(span.span_id, { span, children: [] })
  }
  const roots: SpanTreeNode[] = []
  for (const node of byId.values()) {
    const parentId = node.span.parent_span_id
    if (parentId && byId.has(parentId)) {
      byId.get(parentId)!.children.push(node)
    } else {
      roots.push(node)
    }
  }
  return roots
}

function spanIcon(span: GenAiSpanDetail) {
  if (span.gen_ai_operation === 'execute_tool' || span.tool_name) return Wrench
  if (span.agent_name || span.gen_ai_operation === 'invoke_agent' || span.gen_ai_operation === 'create_agent') return Bot
  if (span.gen_ai_operation === 'embeddings') return Database
  if (span.gen_ai_operation === 'retrieval') return Globe
  if (span.gen_ai_operation === 'chat' || span.gen_ai_operation === 'generate_content' || span.gen_ai_operation === 'text_completion') return MessageSquare
  if (span.gen_ai_system) return Cpu
  if (span.kind === 'CLIENT') return Globe
  if (span.kind === 'SERVER') return Activity
  return Hash
}

function spanLabel(span: GenAiSpanDetail): string {
  if (span.tool_name) return span.tool_name
  if (span.agent_name) return span.agent_name
  return span.name
}

function cacheHitPct(trace: GenAiTraceSummary): number | null {
  const input = trace.total_input_tokens
  const cacheRead = trace.total_cache_read_input_tokens
  if (!input || !cacheRead || input === 0) return null
  return Math.round((cacheRead / input) * 100)
}

// ── SpanTreeRow ─────────────────────────────────────────────────────

function SpanTreeRow({
  node,
  depth,
  maxDuration,
  onSelect,
}: {
  node: SpanTreeNode
  depth: number
  maxDuration: number
  onSelect: (span: GenAiSpanDetail) => void
}) {
  const [open, setOpen] = useState(depth < 3)
  const span = node.span
  const hasChildren = node.children.length > 0
  const Icon = spanIcon(span)
  const durationPct = maxDuration > 0 ? (span.duration_ms / maxDuration) * 100 : 0
  const isGenAi = !!span.gen_ai_system || !!span.gen_ai_operation
  const isError = span.status_code === 'ERROR'

  return (
    <>
      <div
        className={`flex items-center gap-1.5 py-1.5 px-2 hover:bg-muted/50 cursor-pointer rounded-sm group ${
          isError ? 'bg-destructive/5' : ''
        }`}
        style={{ paddingLeft: `${depth * 20 + 8}px` }}
        onClick={() => onSelect(span)}
      >
        {/* Expand/collapse toggle */}
        {hasChildren ? (
          <button
            className="h-4 w-4 shrink-0 text-muted-foreground hover:text-foreground"
            onClick={(e) => { e.stopPropagation(); setOpen(!open) }}
          >
            <ChevronRight className={`h-3.5 w-3.5 transition-transform ${open ? 'rotate-90' : ''}`} />
          </button>
        ) : (
          <span className="w-4 shrink-0" />
        )}

        {/* Icon */}
        <Icon className={`h-3.5 w-3.5 shrink-0 ${isGenAi ? 'text-primary' : 'text-muted-foreground'}`} />

        {/* Name */}
        <span className={`font-mono text-xs truncate max-w-[120px] sm:max-w-[260px] ${isGenAi ? 'text-foreground' : 'text-muted-foreground'}`}>
          {spanLabel(span)}
        </span>

        {/* Operation badge */}
        {span.gen_ai_operation && (
          <Badge variant="outline" className="text-[9px] px-1 py-0 h-4 shrink-0 hidden sm:inline-flex">
            {span.gen_ai_operation}
          </Badge>
        )}

        {/* Model badge */}
        {span.gen_ai_model && (
          <Badge variant="secondary" className="text-[9px] px-1 py-0 h-4 shrink-0 hidden lg:inline-flex">
            {span.gen_ai_model}
          </Badge>
        )}

        {/* Error indicator */}
        {isError && (
          <AlertTriangle className="h-3 w-3 text-destructive shrink-0" />
        )}

        {/* Spacer */}
        <div className="flex-1" />

        {/* Tokens */}
        {(span.input_tokens != null || span.output_tokens != null) && (
          <span className="text-[10px] text-muted-foreground tabular-nums hidden sm:inline">
            {span.input_tokens != null ? formatTokenCount(span.input_tokens) : '—'}
            {' / '}
            {span.output_tokens != null ? formatTokenCount(span.output_tokens) : '—'}
          </span>
        )}

        {/* Cache indicator */}
        {span.cache_read_input_tokens != null && span.cache_read_input_tokens > 0 && (
          <TooltipProvider>
            <Tooltip>
              <TooltipTrigger asChild>
                <Badge variant="outline" className="text-[9px] px-1 py-0 h-4 text-blue-500 border-blue-500/30 shrink-0">
                  cached
                </Badge>
              </TooltipTrigger>
              <TooltipContent>
                <p>{formatTokenCount(span.cache_read_input_tokens)} tokens from cache</p>
              </TooltipContent>
            </Tooltip>
          </TooltipProvider>
        )}

        {/* Duration bar */}
        <div className="w-16 shrink-0 hidden md:flex items-center gap-1">
          <div className="flex-1 h-1.5 bg-muted rounded-full overflow-hidden">
            <div
              className={`h-full rounded-full ${isError ? 'bg-destructive' : isGenAi ? 'bg-primary' : 'bg-muted-foreground/40'}`}
              style={{ width: `${Math.max(durationPct, 2)}%` }}
            />
          </div>
          <span className="text-[10px] text-muted-foreground tabular-nums w-10 text-right">
            {span.duration_ms >= 1000 ? `${(span.duration_ms / 1000).toFixed(1)}s` : `${span.duration_ms.toFixed(0)}ms`}
          </span>
        </div>
      </div>

      {/* Children */}
      {open && hasChildren && node.children.map((child) => (
        <SpanTreeRow
          key={child.span.span_id}
          node={child}
          depth={depth + 1}
          maxDuration={maxDuration}
          onSelect={onSelect}
        />
      ))}
    </>
  )
}

// ── SpanDetailSheet ─────────────────────────────────────────────────

function DetailRow({ label, value, mono }: { label: string; value: React.ReactNode; mono?: boolean }) {
  if (value == null || value === '' || value === '—') return null
  return (
    <div className="flex justify-between items-start gap-4 py-1.5">
      <span className="text-xs text-muted-foreground shrink-0">{label}</span>
      <span className={`text-xs text-right break-all ${mono ? 'font-mono' : ''}`}>{value}</span>
    </div>
  )
}

function DetailSection({ title, children }: { title: string; children: React.ReactNode }) {
  const hasContent = (() => {
    const arr = Array.isArray(children) ? children : [children]
    return arr.some((c) => c != null && c !== false)
  })()
  if (!hasContent) return null
  return (
    <div>
      <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wider mb-1">{title}</h4>
      <div className="divide-y divide-border">{children}</div>
    </div>
  )
}

interface ContentBlock {
  type: string
  text?: string
  thinking?: string
  id?: string
  name?: string
  input?: Record<string, unknown> | string
}

interface ChatMessage {
  role: string
  content?: string | ContentBlock[] | null
  tool_calls?: Array<{ id?: string; function?: { name?: string; arguments?: string } }>
  tool_call_id?: string
}

function ChatMessageBubble({ msg }: { msg: ChatMessage }) {
  const isUser = msg.role === 'user'
  const isTool = msg.role === 'tool'
  const isSystem = msg.role === 'system'

  // Check if content is an array of content blocks (Anthropic format)
  const contentBlocks = Array.isArray(msg.content) ? msg.content : null
  const textContent = typeof msg.content === 'string' ? msg.content : null

  // Extract thinking blocks and text blocks separately
  const thinkingBlocks = contentBlocks?.filter(b => b.type === 'thinking') ?? []
  const textBlocks = contentBlocks?.filter(b => b.type === 'text') ?? []
  const toolUseBlocks = contentBlocks?.filter(b => b.type === 'tool_use') ?? []
  const toolResultBlocks = contentBlocks?.filter(b => b.type === 'tool_result') ?? []

  return (
    <div className="space-y-1.5">
      {/* Thinking blocks - shown before the main message */}
      {thinkingBlocks.map((block, i) => (
        <div key={`thinking-${i}`} className="flex justify-start">
          <div className="max-w-[90%] rounded-lg px-3 py-2 text-xs bg-violet-500/10 border border-violet-500/20">
            <div className="flex items-center gap-1.5 mb-1">
              <span className="text-[10px] font-semibold uppercase text-violet-500">thinking</span>
            </div>
            <div className="whitespace-pre-wrap break-words text-muted-foreground italic">
              {block.thinking || block.text}
            </div>
          </div>
        </div>
      ))}

      {/* Main message bubble */}
      <div className={`flex ${isUser ? 'justify-end' : 'justify-start'}`}>
        <div
          className={`max-w-[90%] rounded-lg px-3 py-2 text-xs ${
            isUser
              ? 'bg-primary text-primary-foreground'
              : isTool
                ? 'bg-amber-500/10 border border-amber-500/20'
                : isSystem
                  ? 'bg-muted border border-border italic'
                  : 'bg-muted'
          }`}
        >
          <div className="flex items-center gap-1.5 mb-1">
            <span className={`text-[10px] font-semibold uppercase ${
              isUser ? 'text-primary-foreground/70' : 'text-muted-foreground'
            }`}>
              {msg.role}
              {isTool && msg.tool_call_id && <span className="font-mono font-normal ml-1">({msg.tool_call_id})</span>}
            </span>
          </div>
          {/* String content */}
          {textContent && (
            <div className="whitespace-pre-wrap break-words">{textContent}</div>
          )}
          {/* Content block text */}
          {textBlocks.map((block, i) => (
            <div key={`text-${i}`} className="whitespace-pre-wrap break-words">
              {block.text}
            </div>
          ))}
          {/* Tool result blocks (Anthropic format) */}
          {toolResultBlocks.map((block, i) => (
            <div key={`result-${i}`} className="whitespace-pre-wrap break-words">
              {typeof block.text === 'string' ? block.text : JSON.stringify(block.input ?? block.text, null, 2)}
            </div>
          ))}
          {/* Tool calls from OpenAI format */}
          {msg.tool_calls && msg.tool_calls.length > 0 && (
            <div className="space-y-1 mt-1">
              {msg.tool_calls.map((tc, j) => (
                <div key={j} className="bg-background/50 rounded p-1.5 font-mono text-[10px]">
                  <span className="text-amber-500">{tc.function?.name}</span>
                  {tc.function?.arguments && (
                    <pre className="text-muted-foreground mt-0.5 whitespace-pre-wrap">{tc.function.arguments}</pre>
                  )}
                </div>
              ))}
            </div>
          )}
          {/* Tool use blocks from Anthropic format */}
          {toolUseBlocks.map((block, j) => (
            <div key={`tool-use-${j}`} className="bg-background/50 rounded p-1.5 font-mono text-[10px] mt-1">
              <span className="text-amber-500">{block.name}</span>
              {block.id && <span className="text-muted-foreground ml-1">({block.id})</span>}
              {block.input && (
                <pre className="text-muted-foreground mt-0.5 whitespace-pre-wrap">
                  {typeof block.input === 'string' ? block.input : JSON.stringify(block.input, null, 2)}
                </pre>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

interface ToolSpanInfo {
  tool_name: string | null
  tool_call_id: string | null
  tool_call_arguments: string | null
  tool_call_result: string | null
  duration_ms: number
}

function FullConversationView({ systemInstructions, inputMessages, outputMessages, toolSpans }: {
  systemInstructions?: string | null
  inputMessages?: string | null
  outputMessages?: string | null
  toolSpans?: ToolSpanInfo[]
}) {
  const allMessages: ChatMessage[] = []

  // 1. System prompt first
  if (systemInstructions) {
    allMessages.push({ role: 'system', content: systemInstructions })
  }

  // 2. Input messages
  if (inputMessages) {
    try {
      const parsed = JSON.parse(inputMessages)
      if (Array.isArray(parsed)) allMessages.push(...parsed)
    } catch {
      allMessages.push({ role: 'user', content: inputMessages })
    }
  }

  // 3. Output messages
  if (outputMessages) {
    try {
      const parsed = JSON.parse(outputMessages)
      if (Array.isArray(parsed)) allMessages.push(...parsed)
    } catch {
      allMessages.push({ role: 'assistant', content: outputMessages })
    }
  }

  // 4. Tool execution spans (from sibling execute_tool spans)
  if (toolSpans && toolSpans.length > 0) {
    for (const ts of toolSpans) {
      // Show tool call as a tool-execution bubble
      allMessages.push({
        role: 'tool_execution',
        content: [
          { type: 'tool_exec', text: ts.tool_call_result || undefined, name: ts.tool_name || undefined },
        ] as ContentBlock[],
        tool_call_id: ts.tool_call_id || undefined,
      })
    }
  }

  if (allMessages.length === 0) return null

  return (
    <div className="space-y-2">
      {allMessages.map((msg, i) => {
        // Special rendering for tool execution spans
        if (msg.role === 'tool_execution') {
          const blocks = Array.isArray(msg.content) ? msg.content : []
          const block = blocks[0]
          return (
            <div key={i} className="flex justify-start">
              <div className="max-w-[90%] rounded-lg px-3 py-2 text-xs bg-emerald-500/10 border border-emerald-500/20">
                <div className="flex items-center gap-1.5 mb-1">
                  <span className="text-[10px] font-semibold uppercase text-emerald-600">
                    tool execution
                  </span>
                  {block?.name && (
                    <span className="font-mono text-[10px] text-emerald-500">{block.name}</span>
                  )}
                  {msg.tool_call_id && (
                    <span className="font-mono text-[10px] text-muted-foreground">({msg.tool_call_id})</span>
                  )}
                </div>
                {block?.text && (
                  <pre className="whitespace-pre-wrap break-words font-mono text-[10px] text-muted-foreground mt-1">
                    {(() => { try { return JSON.stringify(JSON.parse(block.text), null, 2) } catch { return block.text } })()}
                  </pre>
                )}
              </div>
            </div>
          )
        }
        return <ChatMessageBubble key={i} msg={msg} />
      })}
    </div>
  )
}

function JsonBlock({ label, value }: { label: string; value: string | null }) {
  const [open, setOpen] = useState(false)
  if (!value) return null
  let formatted: string
  try { formatted = JSON.stringify(JSON.parse(value), null, 2) } catch { formatted = value }
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="py-1.5">
      <CollapsibleTrigger className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ChevronRight className={`h-3 w-3 transition-transform ${open ? 'rotate-90' : ''}`} />
        {label}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <pre className="mt-1 p-2 bg-muted rounded text-[10px] font-mono overflow-x-auto max-h-[200px] overflow-y-auto">
          {formatted}
        </pre>
      </CollapsibleContent>
    </Collapsible>
  )
}

function SpanDetailSheet({
  span,
  open,
  onOpenChange,
  allSpans,
}: {
  span: GenAiSpanDetail | null
  open: boolean
  onOpenChange: (open: boolean) => void
  allSpans?: GenAiSpanDetail[]
}) {
  if (!span) return null
  const Icon = spanIcon(span)
  const isError = span.status_code === 'ERROR'

  const totalTokens = (span.input_tokens ?? 0) + (span.output_tokens ?? 0)
  const cacheTokens = (span.cache_read_input_tokens ?? 0) + (span.cache_creation_input_tokens ?? 0)

  // Find sibling tool execution spans (children of the same parent, or children of this span)
  const siblingToolSpans: ToolSpanInfo[] = (allSpans ?? [])
    .filter(s =>
      s.span_id !== span.span_id &&
      (s.gen_ai_operation === 'execute_tool' || s.tool_name) &&
      (s.parent_span_id === span.parent_span_id || s.parent_span_id === span.span_id)
    )
    .map(s => ({
      tool_name: s.tool_name,
      tool_call_id: s.tool_call_id,
      tool_call_arguments: s.tool_call_arguments,
      tool_call_result: s.tool_call_result,
      duration_ms: s.duration_ms,
    }))

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full sm:max-w-lg overflow-y-auto">
        <SheetHeader className="pb-4">
          <div className="flex items-center gap-2">
            <Icon className="h-4 w-4 text-primary" />
            <SheetTitle className="text-sm font-mono">{spanLabel(span)}</SheetTitle>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            {span.gen_ai_operation && (
              <Badge variant="outline" className="text-[10px]">{span.gen_ai_operation}</Badge>
            )}
            {span.gen_ai_system && (
              <Badge variant="secondary" className="text-[10px]">{span.gen_ai_system}</Badge>
            )}
            <Badge
              variant={isError ? 'destructive' : 'default'}
              className={!isError ? 'bg-green-500/15 text-green-500' : ''}
            >
              {span.status_code}
            </Badge>
            {isError && span.error_type && (
              <Badge variant="destructive" className="text-[10px]">{span.error_type}</Badge>
            )}
          </div>
        </SheetHeader>

        <div className="space-y-5">
          {/* Token summary cards */}
          {totalTokens > 0 && (
            <div className="grid grid-cols-2 gap-2">
              <Card className="p-3">
                <div className="text-[10px] text-muted-foreground uppercase">Input</div>
                <div className="text-lg font-bold tabular-nums">{formatTokenCount(span.input_tokens ?? 0)}</div>
                {span.cache_read_input_tokens != null && span.cache_read_input_tokens > 0 && (
                  <div className="text-[10px] text-blue-500">{formatTokenCount(span.cache_read_input_tokens)} cached</div>
                )}
                {span.cache_creation_input_tokens != null && span.cache_creation_input_tokens > 0 && (
                  <div className="text-[10px] text-amber-500">{formatTokenCount(span.cache_creation_input_tokens)} cache write</div>
                )}
              </Card>
              <Card className="p-3">
                <div className="text-[10px] text-muted-foreground uppercase">Output</div>
                <div className="text-lg font-bold tabular-nums">{formatTokenCount(span.output_tokens ?? 0)}</div>
                {span.output_type && (
                  <div className="text-[10px] text-muted-foreground">{span.output_type}</div>
                )}
              </Card>
            </div>
          )}

          {/* Cache ratio bar */}
          {cacheTokens > 0 && span.input_tokens != null && span.input_tokens > 0 && (
            <div>
              <div className="flex justify-between text-[10px] text-muted-foreground mb-1">
                <span>Cache hit ratio</span>
                <span>{Math.round(((span.cache_read_input_tokens ?? 0) / span.input_tokens) * 100)}%</span>
              </div>
              <Progress
                value={((span.cache_read_input_tokens ?? 0) / span.input_tokens) * 100}
                className="h-1.5"
              />
            </div>
          )}

          <Separator />

          {/* Identification */}
          <DetailSection title="Identification">
            <DetailRow label="Span ID" value={span.span_id} mono />
            <DetailRow label="Parent Span" value={span.parent_span_id} mono />
            <DetailRow label="Kind" value={span.kind} />
            <DetailRow label="Duration" value={`${span.duration_ms.toFixed(1)}ms`} />
            <DetailRow label="Start" value={new Date(span.start_time).toLocaleString()} />
          </DetailSection>

          {/* Model */}
          <DetailSection title="Model">
            <DetailRow label="Requested Model" value={span.gen_ai_model} mono />
            <DetailRow label="Response Model" value={span.gen_ai_response_model} mono />
            <DetailRow label="Response ID" value={span.response_id} mono />
            <DetailRow label="Finish Reasons" value={span.response_finish_reasons?.join(', ')} />
            <DetailRow label="Conversation ID" value={span.conversation_id} mono />
          </DetailSection>

          {/* Request Parameters */}
          <DetailSection title="Request Parameters">
            <DetailRow label="Temperature" value={span.request_temperature?.toString()} />
            <DetailRow label="Max Tokens" value={span.request_max_tokens?.toString()} />
            <DetailRow label="Top P" value={span.request_top_p?.toString()} />
            <DetailRow label="Top K" value={span.request_top_k?.toString()} />
            <DetailRow label="Frequency Penalty" value={span.request_frequency_penalty?.toString()} />
            <DetailRow label="Presence Penalty" value={span.request_presence_penalty?.toString()} />
            <DetailRow label="Stop Sequences" value={span.request_stop_sequences?.join(', ')} />
            <DetailRow label="Seed" value={span.request_seed?.toString()} />
            <DetailRow label="Choice Count" value={span.request_choice_count?.toString()} />
          </DetailSection>

          {/* Agent */}
          {(span.agent_name || span.agent_id) && (
            <DetailSection title="Agent">
              <DetailRow label="Name" value={span.agent_name} />
              <DetailRow label="ID" value={span.agent_id} mono />
              <DetailRow label="Version" value={span.agent_version} />
              <DetailRow label="Description" value={span.agent_description} />
            </DetailSection>
          )}

          {/* Tool */}
          {(span.tool_name || span.tool_call_id) && (
            <DetailSection title="Tool Execution">
              <DetailRow label="Tool Name" value={span.tool_name} />
              <DetailRow label="Call ID" value={span.tool_call_id} mono />
              <DetailRow label="Type" value={span.tool_type} />
              <DetailRow label="Description" value={span.tool_description} />
            </DetailSection>
          )}

          {/* Embeddings */}
          {span.embeddings_dimension_count != null && (
            <DetailSection title="Embeddings">
              <DetailRow label="Dimensions" value={span.embeddings_dimension_count.toString()} />
              <DetailRow label="Encoding Formats" value={span.request_encoding_formats?.join(', ')} />
            </DetailSection>
          )}

          {/* Retrieval */}
          {span.data_source_id && (
            <DetailSection title="Retrieval">
              <DetailRow label="Data Source" value={span.data_source_id} mono />
            </DetailSection>
          )}

          {/* Server */}
          {(span.server_address || span.server_port) && (
            <DetailSection title="Server">
              <DetailRow label="Address" value={span.server_address} mono />
              <DetailRow label="Port" value={span.server_port?.toString()} />
            </DetailSection>
          )}

          {/* Provider-Specific */}
          {(span.openai_api_type || span.openai_system_fingerprint || span.aws_bedrock_guardrail_id || span.azure_resource_provider_namespace) && (
            <DetailSection title="Provider Specific">
              <DetailRow label="OpenAI API Type" value={span.openai_api_type} />
              <DetailRow label="Request Service Tier" value={span.openai_request_service_tier} />
              <DetailRow label="Response Service Tier" value={span.openai_response_service_tier} />
              <DetailRow label="System Fingerprint" value={span.openai_system_fingerprint} mono />
              <DetailRow label="Bedrock Guardrail" value={span.aws_bedrock_guardrail_id} mono />
              <DetailRow label="Bedrock Knowledge Base" value={span.aws_bedrock_knowledge_base_id} mono />
              <DetailRow label="Azure Namespace" value={span.azure_resource_provider_namespace} />
            </DetailSection>
          )}

          {/* Conversation / Messages */}
          {(span.system_instructions || span.input_messages || span.output_messages) && (
            <>
              <Separator />
              <div>
                <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wider mb-2">Conversation</h4>
                <FullConversationView
                  systemInstructions={span.system_instructions}
                  inputMessages={span.input_messages}
                  outputMessages={span.output_messages}
                  toolSpans={siblingToolSpans}
                />
              </div>
            </>
          )}

          {/* Tool Content */}
          {(span.tool_call_arguments || span.tool_call_result || span.tool_definitions) && (
            <>
              <Separator />
              <div>
                <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wider mb-1">Tool Content</h4>
                <JsonBlock label="Arguments" value={span.tool_call_arguments} />
                <JsonBlock label="Result" value={span.tool_call_result} />
                <JsonBlock label="Definitions" value={span.tool_definitions} />
              </div>
            </>
          )}

          {/* Retrieval Content */}
          {(span.retrieval_query_text || span.retrieval_documents) && (
            <>
              <Separator />
              <div>
                <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wider mb-1">Retrieval</h4>
                {span.retrieval_query_text && (
                  <DetailRow label="Query" value={span.retrieval_query_text} />
                )}
                <JsonBlock label="Documents" value={span.retrieval_documents} />
              </div>
            </>
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}

// ── Trace Detail View ───────────────────────────────────────────────

function TraceDetailView({
  traceId,
  traceDetail,
  isLoading: detailLoading,
  events,
  onBack,
}: {
  traceId: string
  traceDetail: GenAiTraceDetailResponse | undefined
  isLoading: boolean
  events: GenAiEvent[]
  onBack: () => void
}) {
  const [selectedSpan, setSelectedSpan] = useState<GenAiSpanDetail | null>(null)
  const [sheetOpen, setSheetOpen] = useState(false)

  const tree = useMemo(
    () => (traceDetail ? buildSpanTree(traceDetail.spans) : []),
    [traceDetail]
  )

  const maxDuration = useMemo(
    () => (traceDetail ? Math.max(...traceDetail.spans.map((s) => s.duration_ms), 1) : 1),
    [traceDetail]
  )

  // Summary stats
  const stats = useMemo(() => {
    if (!traceDetail) return null
    const spans = traceDetail.spans
    const genAiSpans = spans.filter((s) => s.gen_ai_system || s.gen_ai_operation)
    const totalInput = spans.reduce((s, sp) => s + (sp.input_tokens ?? 0), 0)
    const totalOutput = spans.reduce((s, sp) => s + (sp.output_tokens ?? 0), 0)
    const totalCacheRead = spans.reduce((s, sp) => s + (sp.cache_read_input_tokens ?? 0), 0)
    const totalCacheCreate = spans.reduce((s, sp) => s + (sp.cache_creation_input_tokens ?? 0), 0)
    const errors = spans.filter((s) => s.status_code === 'ERROR')
    const tools = spans.filter((s) => s.gen_ai_operation === 'execute_tool' || s.tool_name)
    const agents = spans.filter((s) => s.agent_name || s.gen_ai_operation === 'invoke_agent')
    const rootDuration = tree[0]?.span.duration_ms ?? 0
    return { genAiSpans, totalInput, totalOutput, totalCacheRead, totalCacheCreate, errors, tools, agents, rootDuration }
  }, [traceDetail, tree])

  const handleSelect = (span: GenAiSpanDetail) => {
    setSelectedSpan(span)
    setSheetOpen(true)
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeft className="h-4 w-4 mr-1" />
          Back to traces
        </Button>
        <span className="text-sm text-muted-foreground font-mono truncate">{traceId}</span>
      </div>

      {detailLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-20 w-full" />
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-12 w-full" />
        </div>
      ) : !traceDetail || traceDetail.spans.length === 0 ? (
        <EmptyState
          icon={Bot}
          title="No spans found"
          description="This trace has no spans."
        />
      ) : (
        <>
          {/* Summary stat cards */}
          {stats && (
            <div className="grid gap-3 grid-cols-2 md:grid-cols-4 lg:grid-cols-6">
              <Card className="p-3">
                <div className="text-[10px] text-muted-foreground uppercase">Total Spans</div>
                <div className="text-xl font-bold">{traceDetail.span_count}</div>
                <div className="text-[10px] text-muted-foreground">{stats.genAiSpans.length} GenAI</div>
              </Card>
              <Card className="p-3">
                <div className="text-[10px] text-muted-foreground uppercase">Duration</div>
                <div className="text-xl font-bold">
                  {stats.rootDuration >= 1000 ? `${(stats.rootDuration / 1000).toFixed(1)}s` : `${stats.rootDuration.toFixed(0)}ms`}
                </div>
              </Card>
              <Card className="p-3">
                <div className="text-[10px] text-muted-foreground uppercase">Input Tokens</div>
                <div className="text-xl font-bold tabular-nums">{formatTokenCount(stats.totalInput)}</div>
                {stats.totalCacheRead > 0 && (
                  <div className="text-[10px] text-blue-500">{formatTokenCount(stats.totalCacheRead)} cached</div>
                )}
              </Card>
              <Card className="p-3">
                <div className="text-[10px] text-muted-foreground uppercase">Output Tokens</div>
                <div className="text-xl font-bold tabular-nums">{formatTokenCount(stats.totalOutput)}</div>
              </Card>
              {stats.tools.length > 0 && (
                <Card className="p-3">
                  <div className="text-[10px] text-muted-foreground uppercase">Tool Calls</div>
                  <div className="text-xl font-bold">{stats.tools.length}</div>
                </Card>
              )}
              {stats.errors.length > 0 && (
                <Card className="p-3 border-destructive/50">
                  <div className="text-[10px] text-destructive uppercase">Errors</div>
                  <div className="text-xl font-bold text-destructive">{stats.errors.length}</div>
                  <div className="text-[10px] text-destructive/70">{stats.errors[0].error_type ?? 'Unknown'}</div>
                </Card>
              )}
            </div>
          )}

          {/* Span tree */}
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-base flex items-center gap-2">
                Span Tree
                <span className="text-xs text-muted-foreground font-normal">Click any span for details</span>
              </CardTitle>
            </CardHeader>
            <CardContent className="p-0">
              <ScrollArea className="max-h-[600px]">
                <div className="py-1">
                  {tree.map((node) => (
                    <SpanTreeRow
                      key={node.span.span_id}
                      node={node}
                      depth={0}
                      maxDuration={maxDuration}
                      onSelect={handleSelect}
                    />
                  ))}
                </div>
              </ScrollArea>
            </CardContent>
          </Card>

          {/* Events */}
          {events.length > 0 && (
            <Card>
              <CardHeader className="pb-2">
                <CardTitle className="text-base">Events ({events.length})</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="space-y-2">
                  {events.map((event, i) => (
                    <div key={`${event.span_id}-${i}`} className="flex items-start gap-3 py-2 border-b border-border last:border-0">
                      <Info className="h-3.5 w-3.5 text-muted-foreground mt-0.5 shrink-0" />
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="font-mono text-xs">{event.event_name}</span>
                          <span className="text-[10px] text-muted-foreground">
                            {new Date(event.timestamp).toLocaleTimeString()}
                          </span>
                        </div>
                        {Object.keys(event.attributes).length > 0 && (
                          <div className="mt-1 flex flex-wrap gap-1">
                            {Object.entries(event.attributes).slice(0, 6).map(([k, v]) => (
                              <Badge key={k} variant="outline" className="text-[9px] px-1 py-0 h-4 font-mono">
                                {k.replace('gen_ai.', '')}: {v.length > 30 ? v.slice(0, 30) + '...' : v}
                              </Badge>
                            ))}
                          </div>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              </CardContent>
            </Card>
          )}
        </>
      )}

      <SpanDetailSheet span={selectedSpan} open={sheetOpen} onOpenChange={setSheetOpen} allSpans={traceDetail?.spans} />
    </div>
  )
}

// ── AgentActivity ───────────────────────────────────────────────────

function AgentActivity() {
  const { projects } = useProjects()
  const [selectedProjectId, setSelectedProjectId] = useState<number | undefined>(
    projects[0]?.id
  )
  const [timeRange, setTimeRange] = useState<(typeof TIME_RANGES)[number]>(TIME_RANGES[0])
  const [systemFilter, setSystemFilter] = useState<string>('')
  const [selectedTraceId, setSelectedTraceId] = useState<string | null>(null)

  const projectId = selectedProjectId ?? projects[0]?.id

  const timeParams = useMemo(() => {
    const to = new Date().toISOString()
    const from = new Date(Date.now() - timeRange.hours * 3600_000).toISOString()
    return { from, to }
  }, [timeRange])

  const { data: tracesResponse, isLoading: tracesLoading } = useQuery({
    queryKey: ['genaiTraces', projectId, timeParams.from, timeParams.to, systemFilter],
    queryFn: () =>
      fetchJson<GenAiTraceSummariesResponse>(
        buildOtelUrl('genai/traces', {
          project_id: projectId,
          start_time: timeParams.from,
          end_time: timeParams.to,
          gen_ai_system: systemFilter || undefined,
          limit: 50,
        })
      ),
    enabled: !!projectId,
  })

  const { data: traceDetail, isLoading: detailLoading } = useQuery({
    queryKey: ['genaiTraceDetail', projectId, selectedTraceId],
    queryFn: () =>
      fetchJson<GenAiTraceDetailResponse>(
        buildOtelUrl(`genai/traces/${projectId}/${selectedTraceId}`, {})
      ),
    enabled: !!projectId && !!selectedTraceId,
  })

  const traces = tracesResponse?.data ?? []
  const events = traceDetail?.events ?? []

  // Trace detail view
  if (selectedTraceId && projectId) {
    return (
      <TraceDetailView
        traceId={selectedTraceId}
        traceDetail={traceDetail}
        isLoading={detailLoading}
        events={events}
        onBack={() => setSelectedTraceId(null)}
      />
    )
  }

  return (
    <div className="space-y-4">
      {/* Filters */}
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <h3 className="text-lg font-semibold">Agent Activity</h3>
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
          {projects.length > 0 && (
            <Select
              value={projectId?.toString() ?? ''}
              onValueChange={(v) => setSelectedProjectId(Number(v))}
            >
              <SelectTrigger className="w-full sm:w-[180px]">
                <SelectValue placeholder="Select project" />
              </SelectTrigger>
              <SelectContent>
                {projects.map((p) => (
                  <SelectItem key={p.id} value={p.id.toString()}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <Select
            value={systemFilter || 'all'}
            onValueChange={(v) => setSystemFilter(v === 'all' ? '' : v)}
          >
            <SelectTrigger className="w-full sm:w-[150px]">
              <SelectValue placeholder="All providers" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All providers</SelectItem>
              <SelectItem value="openai">OpenAI</SelectItem>
              <SelectItem value="anthropic">Anthropic</SelectItem>
              <SelectItem value="xai">xAI</SelectItem>
              <SelectItem value="gemini">Google Gemini</SelectItem>
              <SelectItem value="mistral">Mistral</SelectItem>
              <SelectItem value="deepseek">DeepSeek</SelectItem>
            </SelectContent>
          </Select>
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
      </div>

      {/* Traces list */}
      {!projectId ? (
        <EmptyState
          icon={Bot}
          title="No project selected"
          description="Select a project to view AI agent activity traces."
        />
      ) : tracesLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
        </div>
      ) : traces.length === 0 ? (
        <EmptyState
          icon={Bot}
          title="No AI traces found"
          description="Client applications need to emit OTel spans with gen_ai.* semantic conventions. Point your OTEL_EXPORTER_OTLP_ENDPOINT to this instance."
        />
      ) : (
        <Card>
          <CardContent className="p-0">
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Trace</TableHead>
                    <TableHead>Service</TableHead>
                    <TableHead>Provider</TableHead>
                    <TableHead>Model</TableHead>
                    <TableHead className="hidden md:table-cell">Spans</TableHead>
                    <TableHead className="hidden md:table-cell">Duration</TableHead>
                    <TableHead className="hidden lg:table-cell">Input</TableHead>
                    <TableHead className="hidden lg:table-cell">Output</TableHead>
                    <TableHead className="hidden xl:table-cell">Cache</TableHead>
                    <TableHead>Status</TableHead>
                    <TableHead className="w-8"></TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {traces.map((trace) => {
                    const cachePct = cacheHitPct(trace)
                    return (
                      <TableRow
                        key={trace.trace_id}
                        className="cursor-pointer hover:bg-muted/50"
                        onClick={() => setSelectedTraceId(trace.trace_id)}
                      >
                        <TableCell>
                          <div>
                            <span className="font-mono text-xs block">{trace.root_span_name}</span>
                            <div className="flex items-center gap-1">
                              <span className="text-[10px] text-muted-foreground">
                                {new Date(trace.start_time).toLocaleString([], {
                                  month: 'short',
                                  day: 'numeric',
                                  hour: '2-digit',
                                  minute: '2-digit',
                                })}
                              </span>
                              {trace.gen_ai_operation && (
                                <Badge variant="outline" className="text-[9px] px-1 py-0 h-3.5">
                                  {trace.gen_ai_operation}
                                </Badge>
                              )}
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className="text-xs text-muted-foreground">
                          {trace.service_name}
                        </TableCell>
                        <TableCell>
                          {trace.gen_ai_system ? (
                            <Badge variant="outline" className="text-[10px]">
                              {trace.gen_ai_system}
                            </Badge>
                          ) : '—'}
                        </TableCell>
                        <TableCell className="font-mono text-xs">
                          {trace.gen_ai_model ?? '—'}
                        </TableCell>
                        <TableCell className="hidden md:table-cell text-muted-foreground">
                          {trace.span_count}
                        </TableCell>
                        <TableCell className="hidden md:table-cell text-muted-foreground">
                          {trace.duration_ms >= 1000
                            ? `${(trace.duration_ms / 1000).toFixed(1)}s`
                            : `${trace.duration_ms.toFixed(0)}ms`}
                        </TableCell>
                        <TableCell className="hidden lg:table-cell text-muted-foreground tabular-nums">
                          {trace.total_input_tokens != null ? formatTokenCount(trace.total_input_tokens) : '—'}
                        </TableCell>
                        <TableCell className="hidden lg:table-cell text-muted-foreground tabular-nums">
                          {trace.total_output_tokens != null ? formatTokenCount(trace.total_output_tokens) : '—'}
                        </TableCell>
                        <TableCell className="hidden xl:table-cell">
                          {cachePct != null ? (
                            <TooltipProvider>
                              <Tooltip>
                                <TooltipTrigger>
                                  <Badge variant="outline" className="text-[10px] text-blue-500 border-blue-500/30">
                                    {cachePct}% hit
                                  </Badge>
                                </TooltipTrigger>
                                <TooltipContent>
                                  <p>{formatTokenCount(trace.total_cache_read_input_tokens ?? 0)} tokens served from cache</p>
                                  {trace.total_cache_creation_input_tokens != null && trace.total_cache_creation_input_tokens > 0 && (
                                    <p>{formatTokenCount(trace.total_cache_creation_input_tokens)} tokens written to cache</p>
                                  )}
                                </TooltipContent>
                              </Tooltip>
                            </TooltipProvider>
                          ) : (
                            <span className="text-muted-foreground">—</span>
                          )}
                        </TableCell>
                        <TableCell>
                          {trace.error_count > 0 ? (
                            <Badge variant="destructive">{trace.error_count} err</Badge>
                          ) : (
                            <Badge
                              variant="default"
                              className="bg-green-500/15 text-green-500 hover:bg-green-500/25"
                            >
                              OK
                            </Badge>
                          )}
                        </TableCell>
                        <TableCell>
                          <ChevronRight className="h-4 w-4 text-muted-foreground" />
                        </TableCell>
                      </TableRow>
                    )
                  })}
                </TableBody>
              </Table>
            </div>
          </CardContent>
        </Card>
      )}

      {tracesResponse && tracesResponse.total > traces.length && (
        <p className="text-sm text-muted-foreground text-center">
          Showing {traces.length} of {tracesResponse.total} traces
        </p>
      )}
    </div>
  )
}

// ── ProjectAgentActivity (exported for project detail page) ─────────

export function ProjectAgentActivity({ projectId }: { projectId: number }) {
  const [timeRange, setTimeRange] = useState<(typeof TIME_RANGES)[number]>(TIME_RANGES[0])
  const [systemFilter, setSystemFilter] = useState<string>('')
  const [searchParams, setSearchParams] = useSearchParams()
  const selectedTraceId = searchParams.get('trace') || null

  const setSelectedTraceId = (traceId: string | null) => {
    if (traceId) {
      setSearchParams({ trace: traceId })
    } else {
      setSearchParams({})
    }
  }

  const timeParams = useMemo(() => {
    const to = new Date().toISOString()
    const from = new Date(Date.now() - timeRange.hours * 3600_000).toISOString()
    return { from, to }
  }, [timeRange])

  const { data: tracesResponse, isLoading: tracesLoading } = useQuery({
    queryKey: ['genaiTraces', projectId, timeParams.from, timeParams.to, systemFilter],
    queryFn: () =>
      fetchJson<GenAiTraceSummariesResponse>(
        buildOtelUrl('genai/traces', {
          project_id: projectId,
          start_time: timeParams.from,
          end_time: timeParams.to,
          gen_ai_system: systemFilter || undefined,
          limit: 50,
        })
      ),
    enabled: !!projectId,
  })

  const { data: traceDetail, isLoading: detailLoading } = useQuery({
    queryKey: ['genaiTraceDetail', projectId, selectedTraceId],
    queryFn: () =>
      fetchJson<GenAiTraceDetailResponse>(
        buildOtelUrl(`genai/traces/${projectId}/${selectedTraceId}`, {})
      ),
    enabled: !!projectId && !!selectedTraceId,
  })

  const traces = tracesResponse?.data ?? []
  const events = traceDetail?.events ?? []

  if (selectedTraceId) {
    return (
      <TraceDetailView
        traceId={selectedTraceId}
        traceDetail={traceDetail}
        isLoading={detailLoading}
        events={events}
        onBack={() => setSelectedTraceId(null)}
      />
    )
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <h3 className="text-lg font-semibold">AI Activity</h3>
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
          <Select
            value={systemFilter || 'all'}
            onValueChange={(v) => setSystemFilter(v === 'all' ? '' : v)}
          >
            <SelectTrigger className="w-full sm:w-[150px]">
              <SelectValue placeholder="All providers" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All providers</SelectItem>
              <SelectItem value="openai">OpenAI</SelectItem>
              <SelectItem value="anthropic">Anthropic</SelectItem>
              <SelectItem value="xai">xAI</SelectItem>
              <SelectItem value="gemini">Google Gemini</SelectItem>
              <SelectItem value="mistral">Mistral</SelectItem>
              <SelectItem value="deepseek">DeepSeek</SelectItem>
            </SelectContent>
          </Select>
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
      </div>

      {tracesLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
        </div>
      ) : traces.length === 0 ? (
        <EmptyState
          icon={Bot}
          title="No AI traces found"
          description="Applications need to emit OTel spans with gen_ai.* semantic conventions. Point your OTEL_EXPORTER_OTLP_ENDPOINT to this instance."
        />
      ) : (
        <Card>
          <CardContent className="p-0">
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Trace</TableHead>
                    <TableHead>Service</TableHead>
                    <TableHead>Provider</TableHead>
                    <TableHead>Model</TableHead>
                    <TableHead className="hidden md:table-cell">Spans</TableHead>
                    <TableHead className="hidden md:table-cell">Duration</TableHead>
                    <TableHead className="hidden lg:table-cell">Input</TableHead>
                    <TableHead className="hidden lg:table-cell">Output</TableHead>
                    <TableHead className="hidden xl:table-cell">Cache</TableHead>
                    <TableHead>Status</TableHead>
                    <TableHead className="w-8"></TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {traces.map((trace) => {
                    const cachePct = cacheHitPct(trace)
                    return (
                      <TableRow
                        key={trace.trace_id}
                        className="cursor-pointer hover:bg-muted/50"
                        onClick={() => setSelectedTraceId(trace.trace_id)}
                      >
                        <TableCell>
                          <div>
                            <span className="font-mono text-xs block">{trace.root_span_name}</span>
                            <div className="flex items-center gap-1">
                              <span className="text-[10px] text-muted-foreground">
                                {new Date(trace.start_time).toLocaleString([], {
                                  month: 'short',
                                  day: 'numeric',
                                  hour: '2-digit',
                                  minute: '2-digit',
                                })}
                              </span>
                              {trace.gen_ai_operation && (
                                <Badge variant="outline" className="text-[9px] px-1 py-0 h-3.5">
                                  {trace.gen_ai_operation}
                                </Badge>
                              )}
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className="text-xs text-muted-foreground">
                          {trace.service_name}
                        </TableCell>
                        <TableCell>
                          {trace.gen_ai_system ? (
                            <Badge variant="outline" className="text-[10px]">
                              {trace.gen_ai_system}
                            </Badge>
                          ) : '—'}
                        </TableCell>
                        <TableCell className="font-mono text-xs">
                          {trace.gen_ai_model ?? '—'}
                        </TableCell>
                        <TableCell className="hidden md:table-cell text-muted-foreground">
                          {trace.span_count}
                        </TableCell>
                        <TableCell className="hidden md:table-cell text-muted-foreground">
                          {trace.duration_ms >= 1000
                            ? `${(trace.duration_ms / 1000).toFixed(1)}s`
                            : `${trace.duration_ms.toFixed(0)}ms`}
                        </TableCell>
                        <TableCell className="hidden lg:table-cell text-muted-foreground tabular-nums">
                          {trace.total_input_tokens != null ? formatTokenCount(trace.total_input_tokens) : '—'}
                        </TableCell>
                        <TableCell className="hidden lg:table-cell text-muted-foreground tabular-nums">
                          {trace.total_output_tokens != null ? formatTokenCount(trace.total_output_tokens) : '—'}
                        </TableCell>
                        <TableCell className="hidden xl:table-cell">
                          {cachePct != null ? (
                            <TooltipProvider>
                              <Tooltip>
                                <TooltipTrigger>
                                  <Badge variant="outline" className="text-[10px] text-blue-500 border-blue-500/30">
                                    {cachePct}% hit
                                  </Badge>
                                </TooltipTrigger>
                                <TooltipContent>
                                  <p>{formatTokenCount(trace.total_cache_read_input_tokens ?? 0)} tokens served from cache</p>
                                  {trace.total_cache_creation_input_tokens != null && trace.total_cache_creation_input_tokens > 0 && (
                                    <p>{formatTokenCount(trace.total_cache_creation_input_tokens)} tokens written to cache</p>
                                  )}
                                </TooltipContent>
                              </Tooltip>
                            </TooltipProvider>
                          ) : (
                            <span className="text-muted-foreground">—</span>
                          )}
                        </TableCell>
                        <TableCell>
                          {trace.error_count > 0 ? (
                            <Badge variant="destructive">{trace.error_count} err</Badge>
                          ) : (
                            <Badge
                              variant="default"
                              className="bg-green-500/15 text-green-500 hover:bg-green-500/25"
                            >
                              OK
                            </Badge>
                          )}
                        </TableCell>
                        <TableCell>
                          <ChevronRight className="h-4 w-4 text-muted-foreground" />
                        </TableCell>
                      </TableRow>
                    )
                  })}
                </TableBody>
              </Table>
            </div>
          </CardContent>
        </Card>
      )}

      {tracesResponse && tracesResponse.total > traces.length && (
        <p className="text-sm text-muted-foreground text-center">
          Showing {traces.length} of {tracesResponse.total} traces
        </p>
      )}
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

const VALID_TABS = ['keys', 'usage', 'activity', 'settings'] as const
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
    <div className="container mx-auto px-4 sm:px-6 py-4 sm:py-6 space-y-4 sm:space-y-6">
      {/* Page Header */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="text-2xl sm:text-3xl font-bold">AI Gateway</h1>
          <p className="text-muted-foreground mt-1 sm:mt-2 text-sm">
            Unified API for multiple AI providers with a single endpoint
          </p>
        </div>
        <Button onClick={() => setDialogOpen(true)} className="w-full sm:w-auto">
          <Plus className="mr-2 h-4 w-4" />
          Add Provider Key
        </Button>
      </div>

      {/* Quick Stats */}
      <div className="grid gap-3 sm:gap-4 grid-cols-2 md:grid-cols-4">
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
        <div className="overflow-x-auto -mx-1 px-1">
          <TabsList className="w-full sm:w-auto">
            <TabsTrigger value="keys">Provider Keys</TabsTrigger>
            <TabsTrigger value="usage">Usage</TabsTrigger>
            <TabsTrigger value="activity">Activity</TabsTrigger>
            <TabsTrigger value="settings">Settings</TabsTrigger>
          </TabsList>
        </div>

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

        {/* Activity Tab */}
        <TabsContent value="activity" className="space-y-4">
          <AgentActivity />
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
