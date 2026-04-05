import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import {
  AlertTriangle,
  Bot,
  CheckCircle2,
  Cpu,
  Globe,
  Loader2,
  RefreshCw,
  Save,
  XCircle,
} from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useSettings, useUpdateSettings } from '@/hooks/useSettings'
import { usePageTitle } from '@/hooks/usePageTitle'

interface SandboxStatus {
  docker_available: boolean
  image_ready: boolean
  image_name: string
  error: string | null
}

const RUNTIME_PRESETS = [
  {
    value: 'node',
    label: 'Node.js',
    description: 'Node.js 20, npm, npx',
    stacks: 'Next.js, Vite, Express, any JS/TS',
  },
  {
    value: 'bun',
    label: 'Bun',
    description: 'Bun runtime',
    stacks: 'Bun-based projects',
  },
  {
    value: 'python',
    label: 'Python',
    description: 'Python 3.12, pip, uv',
    stacks: 'Django, FastAPI, Flask',
  },
  {
    value: 'rust',
    label: 'Rust',
    description: 'Rust stable, cargo',
    stacks: 'Rust projects',
  },
  {
    value: 'go',
    label: 'Go',
    description: 'Go 1.23',
    stacks: 'Go projects',
  },
  {
    value: 'full',
    label: 'Full',
    description: 'Node, Python, Go, uv',
    stacks: 'Multi-language projects',
  },
  {
    value: 'custom',
    label: 'Custom Image',
    description: 'Your own Docker image',
    stacks: 'Any stack you pre-build',
  },
]

const RESOURCE_PRESETS = [
  { label: 'Light', cpu: 1, memory: 1024 },
  { label: 'Standard', cpu: 2, memory: 2048 },
  { label: 'Heavy', cpu: 4, memory: 4096 },
  { label: 'Custom', cpu: 0, memory: 0 },
]

function getResourcePresetLabel(cpu: number, memory: number): string {
  const match = RESOURCE_PRESETS.find(
    (p) => p.cpu === cpu && p.memory === memory
  )
  return match ? match.label : 'Custom'
}

export function AgentSandboxSettings() {
  usePageTitle('Agent Sandbox')

  const { data: settings, isLoading } = useSettings()
  const updateSettings = useUpdateSettings()

  const [enabled, setEnabled] = useState(false)
  const [runtime, setRuntime] = useState('node')
  const [customImage, setCustomImage] = useState('')
  const [cpuLimit, setCpuLimit] = useState(2)
  const [memoryLimitMb, setMemoryLimitMb] = useState(2048)
  const [networkMode, setNetworkMode] = useState('full')
  const [isDirty, setIsDirty] = useState(false)
  const [resourcePreset, setResourcePreset] = useState('Standard')

  const [sandboxStatus, setSandboxStatus] = useState<SandboxStatus | null>(null)
  const [statusLoading, setStatusLoading] = useState(false)
  const [rebuilding, setRebuilding] = useState(false)

  const fetchSandboxStatus = useCallback(async () => {
    setStatusLoading(true)
    try {
      const response = await fetch('/api/settings/sandbox-status')
      if (response.ok) {
        setSandboxStatus(await response.json())
      }
    } catch {
      // Endpoint may not exist on older versions
    } finally {
      setStatusLoading(false)
    }
  }, [])

  useEffect(() => {
    if (settings?.agent_sandbox) {
      const s = settings.agent_sandbox
      setEnabled(s.enabled)
      setRuntime(s.runtime || 'node')
      setCustomImage(s.custom_image || '')
      setCpuLimit(s.cpu_limit)
      setMemoryLimitMb(s.memory_limit_mb)
      setNetworkMode(s.network_mode || 'full')
      setResourcePreset(getResourcePresetLabel(s.cpu_limit, s.memory_limit_mb))
    }
  }, [settings])

  useEffect(() => {
    fetchSandboxStatus()
  }, [fetchSandboxStatus])

  const handleRebuildImage = async () => {
    setRebuilding(true)
    try {
      const response = await fetch('/api/settings/sandbox-rebuild', {
        method: 'POST',
      })
      if (response.ok) {
        const data = await response.json()
        if (data.success) {
          toast.success(`Sandbox image rebuilt: ${data.image_name}`)
        } else {
          toast.error(data.error || 'Failed to rebuild image')
        }
      } else {
        toast.error('Failed to rebuild image')
      }
      await fetchSandboxStatus()
    } catch {
      toast.error('Failed to rebuild image')
    } finally {
      setRebuilding(false)
    }
  }

  const handleResourcePresetChange = (preset: string) => {
    setResourcePreset(preset)
    const p = RESOURCE_PRESETS.find((r) => r.label === preset)
    if (p && p.cpu > 0) {
      setCpuLimit(p.cpu)
      setMemoryLimitMb(p.memory)
    }
    setIsDirty(true)
  }

  const handleSave = async () => {
    try {
      await updateSettings.mutateAsync({
        agent_sandbox: {
          enabled,
          runtime,
          custom_image: customImage,
          cpu_limit: cpuLimit,
          memory_limit_mb: memoryLimitMb,
          network_mode: networkMode,
        },
      })
      setIsDirty(false)
      toast.success('Agent sandbox settings saved')
    } catch {
      toast.error('Failed to save settings')
    }
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {/* Header + Docker status */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Bot className="h-5 w-5" />
            Agent Sandbox
          </CardTitle>
          <CardDescription>
            When sandbox is enabled, agents run inside isolated Docker
            containers. Code changes are contained and can't affect your server.
            The container has access to the cloned repository, Claude CLI, and
            any MCP servers configured in your project.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {/* Docker status */}
          <div className="rounded-lg border p-4 space-y-3">
            <div className="flex items-center justify-between">
              <h4 className="text-sm font-medium">Docker Status</h4>
              <Button
                variant="ghost"
                size="sm"
                onClick={fetchSandboxStatus}
                disabled={statusLoading}
              >
                {statusLoading ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <RefreshCw className="h-3.5 w-3.5" />
                )}
              </Button>
            </div>
            {sandboxStatus ? (
              <div className="space-y-2">
                <div className="flex items-center gap-2 text-sm">
                  {sandboxStatus.docker_available ? (
                    <CheckCircle2 className="h-4 w-4 text-green-500" />
                  ) : (
                    <XCircle className="h-4 w-4 text-red-500" />
                  )}
                  <span>
                    Docker:{' '}
                    {sandboxStatus.docker_available
                      ? 'connected'
                      : 'not available'}
                  </span>
                </div>
                {sandboxStatus.docker_available && (
                  <div className="flex items-center gap-2 text-sm">
                    {sandboxStatus.image_ready ? (
                      <CheckCircle2 className="h-4 w-4 text-green-500" />
                    ) : (
                      <XCircle className="h-4 w-4 text-amber-500" />
                    )}
                    <span>
                      Image:{' '}
                      {sandboxStatus.image_ready
                        ? sandboxStatus.image_name
                        : 'not built (will build automatically on first run)'}
                    </span>
                  </div>
                )}
                {sandboxStatus.error && (
                  <p className="text-xs text-red-400 mt-1">
                    {sandboxStatus.error}
                  </p>
                )}
                {sandboxStatus.docker_available && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={handleRebuildImage}
                    disabled={rebuilding}
                    className="mt-2"
                  >
                    {rebuilding ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin mr-2" />
                    ) : (
                      <RefreshCw className="h-3.5 w-3.5 mr-2" />
                    )}
                    {rebuilding ? 'Rebuilding...' : 'Rebuild Image'}
                  </Button>
                )}
              </div>
            ) : statusLoading ? (
              <p className="text-sm text-muted-foreground">Checking...</p>
            ) : null}
          </div>

          {/* Enable toggle */}
          <div className="flex items-center justify-between">
            <div className="space-y-0.5">
              <Label htmlFor="sandbox-enabled">Enable sandbox by default</Label>
              <p className="text-sm text-muted-foreground">
                Agents use sandbox unless individually overridden. Agents with
                sandbox set to "off" in their config will still run on the host.
              </p>
            </div>
            <Switch
              id="sandbox-enabled"
              checked={enabled}
              onCheckedChange={(checked) => {
                setEnabled(checked)
                setIsDirty(true)
              }}
            />
          </div>

          {!enabled && (
            <Alert>
              <AlertTriangle className="h-4 w-4" />
              <AlertDescription>
                Sandbox is disabled. Agents run directly on the host with full
                system access. Enable sandbox for better security isolation.
              </AlertDescription>
            </Alert>
          )}
        </CardContent>
      </Card>

      {/* Runtime */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            Runtime
          </CardTitle>
          <CardDescription>
            Choose the runtime environment for sandbox containers. Each preset
            includes the language toolchain, git, and Claude CLI pre-installed.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-2">
            {RUNTIME_PRESETS.map((preset) => (
              <button
                key={preset.value}
                onClick={() => {
                  setRuntime(preset.value)
                  setIsDirty(true)
                }}
                className={`rounded-lg border p-3 text-left transition-colors ${
                  runtime === preset.value
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-primary/50'
                }`}
              >
                <p className="text-sm font-medium">{preset.label}</p>
                <p className="text-xs text-muted-foreground">
                  {preset.description}
                </p>
              </button>
            ))}
          </div>

          {runtime === 'custom' && (
            <div className="space-y-2 pt-2">
              <Label htmlFor="custom-image">Docker Image</Label>
              <Input
                id="custom-image"
                placeholder="e.g. your-registry/custom-agent:latest"
                value={customImage}
                onChange={(e) => {
                  setCustomImage(e.target.value)
                  setIsDirty(true)
                }}
              />
              <p className="text-xs text-muted-foreground">
                Custom images must have{' '}
                <code className="text-xs bg-muted px-1 rounded">git</code> and{' '}
                <code className="text-xs bg-muted px-1 rounded">claude</code>{' '}
                (Claude CLI) installed. The repository is mounted at{' '}
                <code className="text-xs bg-muted px-1 rounded">
                  /workspace
                </code>
                .
              </p>
            </div>
          )}

          {runtime !== 'custom' && (
            <p className="text-xs text-muted-foreground">
              {RUNTIME_PRESETS.find((p) => p.value === runtime)?.stacks}
            </p>
          )}
        </CardContent>
      </Card>

      {/* Resources */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Cpu className="h-4 w-4" />
            Resources
          </CardTitle>
          <CardDescription>
            CPU and memory limits per sandbox container.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="space-y-2">
            <Label>Preset</Label>
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
              {RESOURCE_PRESETS.map((preset) => (
                <button
                  key={preset.label}
                  onClick={() => handleResourcePresetChange(preset.label)}
                  className={`rounded-lg border p-3 text-left transition-colors ${
                    resourcePreset === preset.label
                      ? 'border-primary bg-primary/5'
                      : 'border-border hover:border-primary/50'
                  }`}
                >
                  <p className="text-sm font-medium">{preset.label}</p>
                  <p className="text-xs text-muted-foreground">
                    {preset.label === 'Custom'
                      ? 'Set your own'
                      : `${preset.cpu} CPU, ${preset.memory >= 1024 ? `${preset.memory / 1024}GB` : `${preset.memory}MB`}`}
                  </p>
                </button>
              ))}
            </div>
          </div>

          {resourcePreset === 'Custom' && (
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-4 pt-2">
              <div className="space-y-2">
                <Label htmlFor="cpu-limit">CPU cores</Label>
                <Input
                  id="cpu-limit"
                  type="number"
                  min={0.5}
                  max={16}
                  step={0.5}
                  value={cpuLimit}
                  onChange={(e) => {
                    setCpuLimit(parseFloat(e.target.value) || 2)
                    setIsDirty(true)
                  }}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="memory-limit">Memory (MB)</Label>
                <Input
                  id="memory-limit"
                  type="number"
                  min={256}
                  max={16384}
                  step={256}
                  value={memoryLimitMb}
                  onChange={(e) => {
                    setMemoryLimitMb(parseInt(e.target.value) || 2048)
                    setIsDirty(true)
                  }}
                />
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      {/* Network */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Globe className="h-4 w-4" />
            Network Access
          </CardTitle>
          <CardDescription>
            Controls whether sandbox containers can access the internet. Agents
            that use web search, MCP servers, or API calls need network access.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Select
            value={networkMode}
            onValueChange={(value) => {
              setNetworkMode(value)
              setIsDirty(true)
            }}
          >
            <SelectTrigger className="w-full sm:w-[300px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="full">
                <span className="font-medium">Full access</span>
                <span className="text-muted-foreground ml-2 text-xs">
                  — unrestricted internet
                </span>
              </SelectItem>
              <SelectItem value="restricted">
                <span className="font-medium">Restricted</span>
                <span className="text-muted-foreground ml-2 text-xs">
                  — Temps network only
                </span>
              </SelectItem>
              <SelectItem value="none">
                <span className="font-medium">No network</span>
                <span className="text-muted-foreground ml-2 text-xs">
                  — fully isolated
                </span>
              </SelectItem>
            </SelectContent>
          </Select>
          {networkMode === 'none' && (
            <p className="text-xs text-amber-500 mt-2">
              Agents won't be able to install packages, use web search, run MCP
              servers, or call external APIs.
            </p>
          )}
        </CardContent>
      </Card>

      {/* Save */}
      <div className="flex justify-end">
        <Button
          onClick={handleSave}
          disabled={!isDirty || updateSettings.isPending}
        >
          {updateSettings.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin mr-2" />
          ) : (
            <Save className="h-4 w-4 mr-2" />
          )}
          Save Changes
        </Button>
      </div>
    </div>
  )
}
