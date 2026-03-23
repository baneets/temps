import {
  createStackRoute,
  deleteStack,
  deleteStackRoute,
  deployStack,
  getStack,
  getStackContainers,
  getStackLogs,
  getStackStats,
  listStackRoutes,
  restartStack,
  stopStack,
  pullStack,
  syncStack,
  toggleStackRoute,
  updatePortOverrides,
  type ComposeContainer,
  type ContainerStats,
  type Stack,
} from '@/api/stacks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import {
  Card,
  CardContent,
} from '@/components/ui/card'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible'
import { Progress } from '@/components/ui/progress'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  ArrowLeft,
  Box,
  ChevronRight,
  Circle,
  Cpu,
  Download,
  GitBranch,
  Globe,
  HardDrive,
  Loader2,
  Network,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Trash2,
  Upload,
} from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'
import { toast } from 'sonner'

function stateVariant(state: string) {
  switch (state) {
    case 'running':
      return 'default'
    case 'stopped':
      return 'secondary'
    case 'error':
      return 'destructive'
    default:
      return 'outline'
  }
}

function containerStateColor(state: string) {
  switch (state) {
    case 'running':
      return 'text-green-500'
    case 'exited':
      return 'text-muted-foreground'
    case 'restarting':
      return 'text-yellow-500'
    case 'dead':
      return 'text-red-500'
    default:
      return 'text-muted-foreground'
  }
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`
}

function parseContainers(raw: string): ComposeContainer[] {
  if (!raw || raw.trim() === '' || raw.trim() === '[]') return []
  try {
    const lines = raw.trim().split('\n').filter(Boolean)
    return lines.map((line) => JSON.parse(line))
  } catch {
    return []
  }
}

function StatsOverview({ stats }: { stats: ContainerStats[] }) {
  if (stats.length === 0) return null

  const totalCpu = stats.reduce((sum, s) => sum + s.cpu_percent, 0)
  const totalMemory = stats.reduce((sum, s) => sum + s.memory_bytes, 0)
  const totalMemoryLimit = stats.reduce((sum, s) => sum + s.memory_limit, 0)
  const totalRx = stats.reduce((sum, s) => sum + s.network_rx_bytes, 0)
  const totalTx = stats.reduce((sum, s) => sum + s.network_tx_bytes, 0)

  return (
    <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
      <Card>
        <CardContent className="pt-4 pb-3 px-4">
          <div className="flex items-center gap-2 text-muted-foreground mb-1">
            <Cpu className="h-3.5 w-3.5" />
            <span className="text-xs font-medium uppercase tracking-wide">CPU</span>
          </div>
          <p className="text-2xl font-semibold tabular-nums">{totalCpu.toFixed(1)}%</p>
          <p className="text-xs text-muted-foreground mt-0.5">
            across {stats.length} container{stats.length !== 1 ? 's' : ''}
          </p>
        </CardContent>
      </Card>
      <Card>
        <CardContent className="pt-4 pb-3 px-4">
          <div className="flex items-center gap-2 text-muted-foreground mb-1">
            <HardDrive className="h-3.5 w-3.5" />
            <span className="text-xs font-medium uppercase tracking-wide">Memory</span>
          </div>
          <p className="text-2xl font-semibold tabular-nums">{formatBytes(totalMemory)}</p>
          <p className="text-xs text-muted-foreground mt-0.5">
            {totalMemoryLimit > 0
              ? `${((totalMemory / totalMemoryLimit) * 100).toFixed(1)}% of ${formatBytes(totalMemoryLimit)}`
              : 'no limit'}
          </p>
        </CardContent>
      </Card>
      <Card>
        <CardContent className="pt-4 pb-3 px-4">
          <div className="flex items-center gap-2 text-muted-foreground mb-1">
            <Download className="h-3.5 w-3.5" />
            <span className="text-xs font-medium uppercase tracking-wide">Net In</span>
          </div>
          <p className="text-2xl font-semibold tabular-nums">{formatBytes(totalRx)}</p>
          <p className="text-xs text-muted-foreground mt-0.5">total received</p>
        </CardContent>
      </Card>
      <Card>
        <CardContent className="pt-4 pb-3 px-4">
          <div className="flex items-center gap-2 text-muted-foreground mb-1">
            <Upload className="h-3.5 w-3.5" />
            <span className="text-xs font-medium uppercase tracking-wide">Net Out</span>
          </div>
          <p className="text-2xl font-semibold tabular-nums">{formatBytes(totalTx)}</p>
          <p className="text-xs text-muted-foreground mt-0.5">total sent</p>
        </CardContent>
      </Card>
    </div>
  )
}

function ScrollToBottomLogs({
  logs,
  isLoading,
  maxHeight,
  minHeight,
  textSize = 'text-[11px]',
  padding = 'p-3',
}: {
  logs?: string
  isLoading: boolean
  maxHeight: string
  minHeight: string
  textSize?: string
  padding?: string
}) {
  const scrollRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [logs])

  return (
    <div
      ref={scrollRef}
      className={`bg-slate-950 text-slate-50 rounded-md border border-border font-mono ${textSize} leading-4 overflow-auto`}
      style={{ maxHeight, minHeight }}
    >
      {isLoading ? (
        <div className={`flex items-center justify-center text-muted-foreground`} style={{ height: minHeight }}>
          <Loader2 className="h-3 w-3 animate-spin mr-1.5" />
          Loading...
        </div>
      ) : logs ? (
        <pre className={`${padding} whitespace-pre-wrap break-all`}>{logs}</pre>
      ) : (
        <div className={`flex items-center justify-center text-muted-foreground text-xs`} style={{ height: minHeight }}>
          No logs available
        </div>
      )}
    </div>
  )
}

function ContainerLogs({ logs, isLoading }: { logs?: string; isLoading: boolean }) {
  return (
    <div>
      <p className="text-xs font-medium text-muted-foreground mb-1.5">Recent logs</p>
      <ScrollToBottomLogs
        logs={logs}
        isLoading={isLoading}
        maxHeight="200px"
        minHeight="80px"
      />
    </div>
  )
}

function ContainerCardExpanded({
  container,
  stats,
  stackId,
}: {
  container: ComposeContainer
  stats?: ContainerStats
  stackId: number
}) {
  const { data: logsData, isLoading: logsLoading } = useQuery({
    queryKey: ['stacks', stackId, 'logs', container.Service, 100],
    queryFn: async () => {
      const { data } = await getStackLogs(stackId, container.Service, 100)
      return data
    },
    enabled: container.State === 'running',
  })

  const ports = container.Publishers?.filter((p) => p.PublishedPort > 0) ?? []

  return (
    <div className="space-y-4 pt-1">
      <Separator />

      {/* Metrics row */}
      {stats && container.State === 'running' && (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-xs">
              <span className="text-muted-foreground flex items-center gap-1">
                <Cpu className="h-3 w-3" /> CPU
              </span>
              <span className="font-medium tabular-nums">{stats.cpu_percent.toFixed(1)}%</span>
            </div>
            <Progress value={Math.min(stats.cpu_percent, 100)} className="h-1.5" />
          </div>
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-xs">
              <span className="text-muted-foreground flex items-center gap-1">
                <HardDrive className="h-3 w-3" /> Memory
              </span>
              <span className="font-medium tabular-nums">{formatBytes(stats.memory_bytes)}</span>
            </div>
            <Progress value={Math.min(stats.memory_percent, 100)} className="h-1.5" />
            {stats.memory_limit > 0 && (
              <p className="text-[10px] text-muted-foreground">
                {stats.memory_percent.toFixed(1)}% of {formatBytes(stats.memory_limit)}
              </p>
            )}
          </div>
          <div className="space-y-0.5">
            <p className="text-xs text-muted-foreground flex items-center gap-1">
              <Download className="h-3 w-3" /> Net In
            </p>
            <p className="text-sm font-medium tabular-nums">{formatBytes(stats.network_rx_bytes)}</p>
          </div>
          <div className="space-y-0.5">
            <p className="text-xs text-muted-foreground flex items-center gap-1">
              <Upload className="h-3 w-3" /> Net Out
            </p>
            <p className="text-sm font-medium tabular-nums">{formatBytes(stats.network_tx_bytes)}</p>
          </div>
        </div>
      )}

      {/* Details grid */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-x-4 gap-y-2 text-xs">
        <div>
          <p className="text-muted-foreground">Container ID</p>
          <p className="font-mono">{container.ID?.slice(0, 12)}</p>
        </div>
        <div>
          <p className="text-muted-foreground">Image</p>
          <p className="font-mono truncate">{container.Image}</p>
        </div>
        <div>
          <p className="text-muted-foreground">Status</p>
          <p>{container.Status}</p>
        </div>
        <div>
          <p className="text-muted-foreground">Ports</p>
          <p className="font-mono">
            {ports.length > 0
              ? ports.map((p) => `${p.PublishedPort}:${p.TargetPort}/${p.Protocol}`).join(', ')
              : 'none'}
          </p>
        </div>
      </div>

      {/* Logs */}
      {container.State === 'running' && (
        <ContainerLogs logs={logsData?.logs} isLoading={logsLoading} />
      )}
    </div>
  )
}

function ContainerCard({
  container,
  stats,
  stackId,
}: {
  container: ComposeContainer
  stats?: ContainerStats
  stackId: number
}) {
  const [open, setOpen] = useState(false)
  const ports = container.Publishers?.filter((p) => p.PublishedPort > 0) ?? []

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <Card>
        <CardContent className="pt-0 pb-0 px-0">
          <CollapsibleTrigger asChild>
            <button className="w-full text-left px-4 py-3 flex items-center gap-3 hover:bg-muted/50 transition-colors rounded-t-lg">
              <ChevronRight
                className={`h-4 w-4 flex-shrink-0 text-muted-foreground transition-transform duration-200 ${
                  open ? 'rotate-90' : ''
                }`}
              />
              <Circle
                className={`h-2.5 w-2.5 flex-shrink-0 fill-current ${containerStateColor(container.State)}`}
              />
              <span className="font-medium text-sm truncate flex-1">{container.Service}</span>
              <span className="text-xs text-muted-foreground font-mono hidden sm:inline truncate max-w-[200px]">
                {container.Image}
              </span>
              {ports.length > 0 && (
                <span className="text-xs text-muted-foreground font-mono hidden md:inline-flex items-center gap-1">
                  <Network className="h-3 w-3" />
                  {ports.map((p) => `${p.PublishedPort}:${p.TargetPort}`).join(', ')}
                </span>
              )}
              {stats && container.State === 'running' && (
                <div className="flex gap-3 text-right flex-shrink-0">
                  <span className="text-xs tabular-nums">
                    <span className="text-muted-foreground">CPU </span>
                    <span className="font-medium">{stats.cpu_percent.toFixed(1)}%</span>
                  </span>
                  <span className="text-xs tabular-nums">
                    <span className="text-muted-foreground">MEM </span>
                    <span className="font-medium">{formatBytes(stats.memory_bytes)}</span>
                  </span>
                </div>
              )}
            </button>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div className="px-4 pb-4 pl-11">
              <ContainerCardExpanded
                container={container}
                stats={stats}
                stackId={stackId}
              />
            </div>
          </CollapsibleContent>
        </CardContent>
      </Card>
    </Collapsible>
  )
}

function ContainersTab({ stackId, stack }: { stackId: number; stack: Stack }) {
  const isRunning = stack.state === 'running'

  const { data: containersData, isLoading } = useQuery({
    queryKey: ['stacks', stackId, 'containers'],
    queryFn: async () => {
      const { data } = await getStackContainers(stackId)
      return data
    },
    refetchInterval: isRunning ? 5000 : false,
  })

  const { data: statsData } = useQuery({
    queryKey: ['stacks', stackId, 'stats'],
    queryFn: async () => {
      const { data } = await getStackStats(stackId)
      return data
    },
    enabled: isRunning,
    refetchInterval: isRunning ? 3000 : false,
  })

  const containers = parseContainers(containersData?.raw ?? '')
  const statsMap = new Map(
    (statsData?.containers ?? []).map((s) => [s.service, s])
  )

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-[200px]">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    )
  }

  if (containers.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-16 text-center">
        <Box className="h-10 w-10 text-muted-foreground mb-3" />
        <p className="text-sm font-medium">No containers</p>
        <p className="text-xs text-muted-foreground mt-1">
          {stack.state === 'stopped'
            ? 'Deploy the stack to start containers.'
            : 'No containers found.'}
        </p>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {isRunning && <StatsOverview stats={statsData?.containers ?? []} />}
      <div className="grid gap-2">
        {containers.map((c) => (
          <ContainerCard
            key={c.ID}
            container={c}
            stats={statsMap.get(c.Service)}
            stackId={stackId}
          />
        ))}
      </div>
    </div>
  )
}

function LogsTab({ stackId, stack }: { stackId: number; stack: Stack }) {
  const [selectedService, setSelectedService] = useState<string>('__all__')
  const [tail, setTail] = useState('200')

  const { data: containersData } = useQuery({
    queryKey: ['stacks', stackId, 'containers'],
    queryFn: async () => {
      const { data } = await getStackContainers(stackId)
      return data
    },
  })

  const containers = parseContainers(containersData?.raw ?? '')
  const services = [...new Set(containers.map((c) => c.Service))].sort()

  const {
    data: logsData,
    isLoading,
    refetch,
  } = useQuery({
    queryKey: ['stacks', stackId, 'logs', selectedService, tail],
    queryFn: async () => {
      const svc = selectedService === '__all__' ? undefined : selectedService
      const { data } = await getStackLogs(stackId, svc, parseInt(tail))
      return data
    },
    enabled: stack.state === 'running',
  })

  if (stack.state !== 'running') {
    return (
      <div className="flex flex-col items-center justify-center py-16 text-center">
        <Box className="h-10 w-10 text-muted-foreground mb-3" />
        <p className="text-sm font-medium">Stack is not running</p>
        <p className="text-xs text-muted-foreground mt-1">Deploy to see logs.</p>
      </div>
    )
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-2">
          <Select value={selectedService} onValueChange={setSelectedService}>
            <SelectTrigger className="w-[160px] h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all__">All services</SelectItem>
              {services.map((s) => (
                <SelectItem key={s} value={s}>{s}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={tail} onValueChange={setTail}>
            <SelectTrigger className="w-[120px] h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="50">50 lines</SelectItem>
              <SelectItem value="200">200 lines</SelectItem>
              <SelectItem value="500">500 lines</SelectItem>
              <SelectItem value="1000">1000 lines</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <Button variant="ghost" size="sm" onClick={() => refetch()} className="h-8">
          <RefreshCw className="h-3.5 w-3.5 mr-1.5" />
          Refresh
        </Button>
      </div>
      <ScrollToBottomLogs
        logs={logsData?.logs}
        isLoading={isLoading}
        maxHeight="500px"
        minHeight="300px"
        textSize="text-xs"
        padding="p-4"
      />
    </div>
  )
}

function PortOverridesCard({ stack }: { stack: Stack }) {
  const queryClient = useQueryClient()
  const servicePorts = parseServicePorts(stack.compose_content)
  const [overrides, setOverrides] = useState<Record<string, string>>({})
  const [initialized, setInitialized] = useState(false)

  // Seed form from stack's saved overrides (once)
  if (!initialized && servicePorts.length > 0) {
    const initial: Record<string, string> = {}
    for (const sp of servicePorts) {
      const key = sp.published.toString()
      const saved = stack.port_overrides?.[key]
      initial[key] = saved != null ? saved.toString() : ''
    }
    setOverrides(initial)
    setInitialized(true)
  }

  const saveMutation = useMutation({
    mutationFn: () => {
      const cleaned: Record<string, number> = {}
      for (const [origPort, newPort] of Object.entries(overrides)) {
        const parsed = parseInt(newPort)
        if (newPort.trim() !== '' && !isNaN(parsed) && parsed > 0 && parsed <= 65535) {
          if (parsed.toString() !== origPort) {
            cleaned[origPort] = parsed
          }
        }
      }
      return updatePortOverrides(
        stack.id,
        Object.keys(cleaned).length > 0 ? cleaned : null
      )
    },
    meta: { errorTitle: 'Failed to save port overrides' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks', stack.id] })
      toast.success('Port overrides saved')
    },
  })

  if (servicePorts.length === 0) return null

  const hasChanges = servicePorts.some((sp) => {
    const key = sp.published.toString()
    const saved = stack.port_overrides?.[key]
    const current = overrides[key]?.trim()
    if (!current && !saved) return false
    if (!current && saved) return true
    if (current && !saved) return current !== key
    return current !== saved?.toString()
  })

  return (
    <Card>
      <CardContent className="pt-4">
        <div className="flex items-center justify-between mb-3">
          <div>
            <p className="text-sm font-medium">Port Mapping</p>
            <p className="text-xs text-muted-foreground mt-0.5">
              Remap host ports to avoid conflicts with other stacks. Changes apply on next deploy.
            </p>
          </div>
          <Button
            size="sm"
            variant="outline"
            disabled={!hasChanges || saveMutation.isPending}
            onClick={() => saveMutation.mutate()}
            className="h-7 text-xs"
          >
            {saveMutation.isPending ? (
              <Loader2 className="h-3 w-3 animate-spin mr-1" />
            ) : null}
            Save
          </Button>
        </div>

        <div className="space-y-2">
          {servicePorts.map((sp) => {
            const key = sp.published.toString()
            const value = overrides[key] ?? ''
            const isRemapped = value.trim() !== '' && value.trim() !== key
            return (
              <div key={`${sp.service}-${sp.published}`} className="flex items-center gap-3">
                <div className="flex items-center gap-1.5 min-w-[140px]">
                  <Badge variant="outline" className="text-[10px] h-5 font-normal">
                    {sp.service}
                  </Badge>
                  <span className="text-xs font-mono text-muted-foreground">:{sp.published}</span>
                  <span className="text-xs text-muted-foreground">→ :{sp.target}</span>
                </div>
                <span className="text-xs text-muted-foreground">→</span>
                <Input
                  type="number"
                  min={1}
                  max={65535}
                  placeholder={key}
                  value={value}
                  onChange={(e) => setOverrides((prev) => ({ ...prev, [key]: e.target.value }))}
                  className={`h-7 w-[100px] text-xs font-mono ${isRemapped ? 'border-primary' : ''}`}
                />
                {isRemapped && (
                  <button
                    className="text-xs text-muted-foreground hover:text-foreground"
                    onClick={() => setOverrides((prev) => ({ ...prev, [key]: '' }))}
                  >
                    reset
                  </button>
                )}
              </div>
            )
          })}
        </div>

        {stack.port_overrides && Object.keys(stack.port_overrides).length > 0 && (
          <div className="mt-3 pt-3 border-t">
            <p className="text-[11px] text-muted-foreground">
              Active overrides:{' '}
              {Object.entries(stack.port_overrides).map(([from, to]) => (
                <span key={from} className="font-mono">
                  {from}→{to}{' '}
                </span>
              ))}
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function ConfigTab({ stack }: { stack: Stack }) {
  return (
    <div className="space-y-4">
      <PortOverridesCard stack={stack} />
      <Card>
        <CardContent className="pt-4">
          <p className="text-sm font-medium mb-2">docker-compose.yml</p>
          <pre className="bg-muted/50 rounded-md p-4 font-mono text-xs leading-5 overflow-auto max-h-[400px] whitespace-pre-wrap">
            {stack.compose_content}
          </pre>
        </CardContent>
      </Card>
      {stack.env_content && (
        <Card>
          <CardContent className="pt-4">
            <p className="text-sm font-medium mb-2">.env</p>
            <pre className="bg-muted/50 rounded-md p-4 font-mono text-xs leading-5 overflow-auto max-h-[200px] whitespace-pre-wrap">
              {stack.env_content}
            </pre>
          </CardContent>
        </Card>
      )}
      {stack.repo_url && (
        <Card>
          <CardContent className="pt-4">
            <p className="text-sm font-medium mb-2 flex items-center gap-2">
              <GitBranch className="h-4 w-4" />
              Repository Source
            </p>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-2 text-sm">
              <div>
                <p className="text-xs text-muted-foreground">URL</p>
                <p className="font-mono text-xs truncate">{stack.repo_url}</p>
              </div>
              <div>
                <p className="text-xs text-muted-foreground">Branch</p>
                <p className="font-mono text-xs">{stack.repo_branch ?? 'default'}</p>
              </div>
              <div>
                <p className="text-xs text-muted-foreground">Compose Path</p>
                <p className="font-mono text-xs">{stack.repo_compose_path ?? 'docker-compose.yml'}</p>
              </div>
              {stack.last_synced_at && (
                <div>
                  <p className="text-xs text-muted-foreground">Last Synced</p>
                  <p className="text-xs">{new Date(stack.last_synced_at).toLocaleString()}</p>
                </div>
              )}
            </div>
          </CardContent>
        </Card>
      )}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <Card>
          <CardContent className="pt-3 pb-3 px-4">
            <p className="text-xs text-muted-foreground">Stack ID</p>
            <p className="text-sm font-medium">{stack.id}</p>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="pt-3 pb-3 px-4">
            <p className="text-xs text-muted-foreground">Node</p>
            <p className="text-sm font-medium">{stack.node_id ?? 'Local'}</p>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="pt-3 pb-3 px-4">
            <p className="text-xs text-muted-foreground">Created</p>
            <p className="text-sm font-medium">{new Date(stack.created_at).toLocaleDateString()}</p>
          </CardContent>
        </Card>
        <Card>
          <CardContent className="pt-3 pb-3 px-4">
            <p className="text-xs text-muted-foreground">Updated</p>
            <p className="text-sm font-medium">{new Date(stack.updated_at).toLocaleDateString()}</p>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

interface ServicePort {
  service: string
  published: number
  target: number
}

function parseServicePorts(composeContent: string): ServicePort[] {
  const ports: ServicePort[] = []
  const lines = composeContent.split('\n')
  let currentService: string | null = null
  let inServices = false
  let inPorts = false
  let servicesIndent = -1
  let serviceIndent = -1
  let portsIndent = -1

  for (const line of lines) {
    const trimmed = line.trim()
    if (trimmed === '') continue

    const indent = line.search(/\S/)

    // Detect "services:" top-level key
    if (indent >= 0 && trimmed === 'services:' || trimmed.startsWith('services:')) {
      inServices = true
      servicesIndent = indent
      continue
    }

    if (!inServices) continue

    // A key deeper than services that ends with ":" and isn't a list item is a service name
    if (
      indent > servicesIndent &&
      (serviceIndent === -1 || indent <= serviceIndent) &&
      /^[\w][\w.-]*:$/.test(trimmed) &&
      !trimmed.startsWith('-') &&
      !trimmed.startsWith('#')
    ) {
      currentService = trimmed.replace(':', '').trim()
      serviceIndent = indent
      inPorts = false
      portsIndent = -1
      continue
    }

    // If we hit something at or before service level, reset
    if (indent <= servicesIndent && !trimmed.startsWith('#')) {
      inServices = false
      currentService = null
      inPorts = false
      continue
    }

    // Detect "ports:" under a service
    if (currentService && indent > serviceIndent && trimmed === 'ports:') {
      inPorts = true
      portsIndent = indent
      continue
    }

    // Exit ports block if we hit another key at or before ports level
    if (inPorts && indent <= portsIndent && !trimmed.startsWith('-') && !trimmed.startsWith('#')) {
      inPorts = false
      portsIndent = -1
    }

    // Parse port entries like "- 8080:80" or "- '3000:3000'" or "- \"8080:80\""
    if (inPorts && currentService && trimmed.startsWith('-')) {
      const portStr = trimmed
        .replace(/^-\s*/, '')
        .replace(/['"]/g, '')
        .trim()
      const match = portStr.match(/^(?:[\d.]+:)?(\d+):(\d+)/)
      if (match) {
        ports.push({
          service: currentService,
          published: parseInt(match[1]),
          target: parseInt(match[2]),
        })
      }
    }
  }

  return ports
}

function RoutesTab({ stackId, stack }: { stackId: number; stack: Stack }) {
  const queryClient = useQueryClient()
  const [mode, setMode] = useState<'auto' | 'manual'>('auto')
  const [domain, setDomain] = useState('')
  const [manualPort, setManualPort] = useState('')
  const [manualService, setManualService] = useState('')
  const [selectedServicePort, setSelectedServicePort] = useState('')

  const servicePorts = parseServicePorts(stack.compose_content)
  const hasServicePorts = servicePorts.length > 0

  // If no ports found in compose, default to manual
  const effectiveMode = hasServicePorts ? mode : 'manual'

  const { data: routes, isLoading } = useQuery({
    queryKey: ['stacks', stackId, 'routes'],
    queryFn: async () => {
      const { data } = await listStackRoutes(stackId)
      return data
    },
  })

  const createMutation = useMutation({
    mutationFn: () => {
      if (effectiveMode === 'auto') {
        const sp = servicePorts.find(
          (s) => `${s.service}:${s.published}` === selectedServicePort
        )
        if (!sp) throw new Error('Select a service and port')
        return createStackRoute(stackId, {
          domain,
          target_port: sp.published,
          service_name: sp.service,
        })
      }
      return createStackRoute(stackId, {
        domain,
        target_port: parseInt(manualPort),
        service_name: manualService || null,
      })
    },
    meta: { errorTitle: 'Failed to create route' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks', stackId, 'routes'] })
      setDomain('')
      setManualPort('')
      setManualService('')
      setSelectedServicePort('')
      toast.success('Route created')
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (routeId: number) => deleteStackRoute(stackId, routeId),
    meta: { errorTitle: 'Failed to delete route' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks', stackId, 'routes'] })
      toast.success('Route deleted')
    },
  })

  const toggleMutation = useMutation({
    mutationFn: ({ routeId, enabled }: { routeId: number; enabled: boolean }) =>
      toggleStackRoute(stackId, routeId, enabled),
    meta: { errorTitle: 'Failed to update route' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks', stackId, 'routes'] })
    },
  })

  const canSubmit =
    domain.trim() !== '' &&
    !createMutation.isPending &&
    (effectiveMode === 'auto'
      ? selectedServicePort !== ''
      : manualPort.trim() !== '')

  return (
    <div className="space-y-4">
      {/* Add route form */}
      <Card>
        <CardContent className="pt-4">
          <div className="flex items-center justify-between mb-3">
            <p className="text-sm font-medium">Add domain route</p>
            {hasServicePorts && (
              <div className="flex items-center gap-1 text-xs">
                <button
                  className={`px-2 py-1 rounded-md transition-colors ${
                    effectiveMode === 'auto'
                      ? 'bg-primary text-primary-foreground'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                  onClick={() => setMode('auto')}
                >
                  Auto
                </button>
                <button
                  className={`px-2 py-1 rounded-md transition-colors ${
                    effectiveMode === 'manual'
                      ? 'bg-primary text-primary-foreground'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                  onClick={() => setMode('manual')}
                >
                  Manual
                </button>
              </div>
            )}
          </div>

          <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
            <div className="flex-1 space-y-1">
              <label className="text-xs text-muted-foreground">Domain</label>
              <Input
                placeholder="app.example.com"
                value={domain}
                onChange={(e) => setDomain(e.target.value)}
                className="h-8 text-sm"
              />
            </div>

            {effectiveMode === 'auto' ? (
              <div className="w-full sm:w-[220px] space-y-1">
                <label className="text-xs text-muted-foreground">Service : Port</label>
                <Select value={selectedServicePort} onValueChange={setSelectedServicePort}>
                  <SelectTrigger className="h-8 text-sm">
                    <SelectValue placeholder="Select service" />
                  </SelectTrigger>
                  <SelectContent>
                    {servicePorts.map((sp) => (
                      <SelectItem
                        key={`${sp.service}:${sp.published}`}
                        value={`${sp.service}:${sp.published}`}
                      >
                        {sp.service} → :{sp.published}
                        {sp.published !== sp.target && (
                          <span className="text-muted-foreground"> (container :{sp.target})</span>
                        )}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            ) : (
              <>
                <div className="w-full sm:w-[100px] space-y-1">
                  <label className="text-xs text-muted-foreground">Port</label>
                  <Input
                    placeholder="8080"
                    type="number"
                    value={manualPort}
                    onChange={(e) => setManualPort(e.target.value)}
                    className="h-8 text-sm"
                  />
                </div>
                <div className="w-full sm:w-[140px] space-y-1">
                  <label className="text-xs text-muted-foreground">Service (optional)</label>
                  <Input
                    placeholder="web"
                    value={manualService}
                    onChange={(e) => setManualService(e.target.value)}
                    className="h-8 text-sm"
                  />
                </div>
              </>
            )}

            <Button
              size="sm"
              className="h-8"
              disabled={!canSubmit}
              onClick={() => createMutation.mutate()}
            >
              {createMutation.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
              ) : (
                <Plus className="h-3.5 w-3.5 mr-1.5" />
              )}
              Add
            </Button>
          </div>

          <p className="text-[11px] text-muted-foreground mt-2">
            {effectiveMode === 'auto'
              ? 'Select a service and its published port from your compose file.'
              : 'Enter the host port manually. The port must be published via Docker compose ports.'}
          </p>
        </CardContent>
      </Card>

      {/* Routes list */}
      {isLoading ? (
        <div className="flex items-center justify-center min-h-[120px]">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      ) : !routes || routes.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-12 text-center">
          <Globe className="h-10 w-10 text-muted-foreground mb-3" />
          <p className="text-sm font-medium">No domain routes</p>
          <p className="text-xs text-muted-foreground mt-1">
            Add a route above to expose a stack service on a custom domain.
          </p>
        </div>
      ) : (
        <div className="grid gap-2">
          {routes.map((route) => (
            <Card key={route.id}>
              <CardContent className="pt-0 pb-0 px-4 py-3">
                <div className="flex items-center gap-3">
                  <Switch
                    checked={route.enabled}
                    onCheckedChange={(enabled) =>
                      toggleMutation.mutate({ routeId: route.id, enabled })
                    }
                  />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <Globe className="h-3.5 w-3.5 text-muted-foreground flex-shrink-0" />
                      <span className="text-sm font-medium truncate">{route.domain}</span>
                      <span className="text-xs text-muted-foreground">→</span>
                      <span className="text-sm font-mono text-muted-foreground">
                        :{route.target_port}
                      </span>
                      {route.service_name && (
                        <Badge variant="outline" className="text-[10px] h-5">
                          {route.service_name}
                        </Badge>
                      )}
                    </div>
                  </div>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-destructive"
                    onClick={() => deleteMutation.mutate(route.id)}
                    disabled={deleteMutation.isPending}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
    </div>
  )
}

const VALID_TABS = ['containers', 'logs', 'routes', 'config'] as const

export function StackDetailView({ stackId }: { stackId: number }) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const tabParam = searchParams.get('tab')
  const activeTab = VALID_TABS.includes(tabParam as typeof VALID_TABS[number])
    ? tabParam!
    : 'containers'

  const { data: stack, isLoading, error } = useQuery({
    queryKey: ['stacks', stackId],
    queryFn: async () => {
      const { data } = await getStack(stackId)
      return data
    },
    refetchInterval: 5000,
  })

  const deployMutation = useMutation({
    mutationFn: () => deployStack(stackId),
    meta: { errorTitle: 'Failed to deploy stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack deployed')
    },
  })

  const stopMutation = useMutation({
    mutationFn: () => stopStack(stackId),
    meta: { errorTitle: 'Failed to stop stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack stopped')
    },
  })

  const restartMutation = useMutation({
    mutationFn: () => restartStack(stackId),
    meta: { errorTitle: 'Failed to restart stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack restarted')
    },
  })

  const pullMutation = useMutation({
    mutationFn: () => pullStack(stackId),
    meta: { errorTitle: 'Failed to pull images' },
    onSuccess: () => {
      toast.success('Images pulled')
    },
  })

  const syncMutation = useMutation({
    mutationFn: () => syncStack(stackId),
    meta: { errorTitle: 'Failed to sync from repository' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack synced from repository')
    },
  })

  const deleteMutation = useMutation({
    mutationFn: () => deleteStack(stackId),
    meta: { errorTitle: 'Failed to delete stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack deleted')
      navigate('/stacks')
    },
  })

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-[400px]">
        <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
      </div>
    )
  }

  if (error || !stack) {
    return (
      <div className="flex flex-col items-center justify-center min-h-[400px] gap-3">
        <p className="text-sm text-muted-foreground">
          {error ? 'Failed to load stack' : 'Stack not found'}
        </p>
        <Button asChild variant="outline" size="sm">
          <Link to="/stacks">Back to Stacks</Link>
        </Button>
      </div>
    )
  }

  const isActing =
    deployMutation.isPending ||
    stopMutation.isPending ||
    restartMutation.isPending ||
    pullMutation.isPending ||
    syncMutation.isPending ||
    deleteMutation.isPending

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="flex items-start gap-3">
          <Button asChild variant="ghost" size="icon" className="mt-0.5 h-8 w-8">
            <Link to="/stacks">
              <ArrowLeft className="h-4 w-4" />
            </Link>
          </Button>
          <div>
            <div className="flex items-center gap-2.5">
              <h1 className="text-xl font-semibold">{stack.name}</h1>
              <Badge variant={stateVariant(stack.state)} className="text-xs">
                {stack.state}
              </Badge>
              {stack.repo_url && (
                <Badge variant="outline" className="text-xs">
                  <GitBranch className="h-3 w-3 mr-1" />
                  {stack.repo_branch ?? 'default'}
                </Badge>
              )}
            </div>
            {stack.description && (
              <p className="text-sm text-muted-foreground mt-0.5">{stack.description}</p>
            )}
          </div>
        </div>
        <div className="flex items-center gap-2 ml-11 sm:ml-0">
          {stack.repo_url && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => syncMutation.mutate()}
              disabled={isActing}
            >
              {syncMutation.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
              ) : (
                <GitBranch className="h-3.5 w-3.5 mr-1.5" />
              )}
              <span className="hidden sm:inline">Sync</span>
            </Button>
          )}
          {stack.state !== 'running' ? (
            <Button size="sm" onClick={() => deployMutation.mutate()} disabled={isActing}>
              {deployMutation.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
              ) : (
                <Play className="h-3.5 w-3.5 mr-1.5" />
              )}
              Deploy
            </Button>
          ) : (
            <>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => pullMutation.mutate()}
                disabled={isActing}
              >
                {pullMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
                ) : (
                  <Download className="h-3.5 w-3.5 mr-1.5" />
                )}
                <span className="hidden sm:inline">Pull</span>
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => restartMutation.mutate()}
                disabled={isActing}
              >
                {restartMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
                ) : (
                  <RefreshCw className="h-3.5 w-3.5 mr-1.5" />
                )}
                <span className="hidden sm:inline">Restart</span>
              </Button>
              <Button
                variant="destructive"
                size="sm"
                onClick={() => stopMutation.mutate()}
                disabled={isActing}
              >
                {stopMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
                ) : (
                  <Pause className="h-3.5 w-3.5 mr-1.5" />
                )}
                <span className="hidden sm:inline">Stop</span>
              </Button>
            </>
          )}
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button variant="ghost" size="sm" disabled={isActing}>
                {deleteMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
                ) : (
                  <Trash2 className="h-3.5 w-3.5 mr-1.5" />
                )}
                <span className="hidden sm:inline">Delete</span>
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete stack "{stack.name}"?</AlertDialogTitle>
                <AlertDialogDescription>
                  This will stop all containers, remove volumes, and delete the
                  stack configuration. This action cannot be undone.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction
                  onClick={() => deleteMutation.mutate()}
                  className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                >
                  Delete
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>
      </div>

      {/* Tabs */}
      <Tabs
        value={activeTab}
        onValueChange={(v) => setSearchParams({ tab: v }, { replace: true })}
      >
        <TabsList>
          <TabsTrigger value="containers">Containers</TabsTrigger>
          <TabsTrigger value="logs">Logs</TabsTrigger>
          <TabsTrigger value="routes">Routes</TabsTrigger>
          <TabsTrigger value="config">Configuration</TabsTrigger>
        </TabsList>
        <TabsContent value="containers" className="mt-4">
          <ContainersTab stackId={stackId} stack={stack} />
        </TabsContent>
        <TabsContent value="logs" className="mt-4">
          <LogsTab stackId={stackId} stack={stack} />
        </TabsContent>
        <TabsContent value="routes" className="mt-4">
          <RoutesTab stackId={stackId} stack={stack} />
        </TabsContent>
        <TabsContent value="config" className="mt-4">
          <ConfigTab stack={stack} />
        </TabsContent>
      </Tabs>
    </div>
  )
}
