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
  Cpu,
  Globe,
  ImageIcon,
  Loader2,
  Save,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useSettings, useUpdateSettings } from '@/hooks/useSettings'
import { usePageTitle } from '@/hooks/usePageTitle'

const RESOURCE_PRESETS = [
  { label: 'Light', cpu: 1, memory: 1024, description: 'Simple analysis, reports' },
  { label: 'Standard', cpu: 2, memory: 2048, description: 'Most agents' },
  { label: 'Heavy', cpu: 4, memory: 4096, description: 'Large repos, complex tasks' },
  { label: 'Custom', cpu: 0, memory: 0, description: 'Set your own limits' },
]

function getPresetLabel(cpu: number, memory: number): string {
  const match = RESOURCE_PRESETS.find((p) => p.cpu === cpu && p.memory === memory)
  return match ? match.label : 'Custom'
}

export function AgentSandboxSettings() {
  usePageTitle('Agent Sandbox')

  const { data: settings, isLoading } = useSettings()
  const updateSettings = useUpdateSettings()

  const [enabled, setEnabled] = useState(false)
  const [image, setImage] = useState('')
  const [cpuLimit, setCpuLimit] = useState(2)
  const [memoryLimitMb, setMemoryLimitMb] = useState(2048)
  const [networkMode, setNetworkMode] = useState('full')
  const [isDirty, setIsDirty] = useState(false)
  const [selectedPreset, setSelectedPreset] = useState('Standard')

  useEffect(() => {
    if (settings?.agent_sandbox) {
      setEnabled(settings.agent_sandbox.enabled)
      setImage(settings.agent_sandbox.image || '')
      setCpuLimit(settings.agent_sandbox.cpu_limit)
      setMemoryLimitMb(settings.agent_sandbox.memory_limit_mb)
      setNetworkMode(settings.agent_sandbox.network_mode || 'full')
      setSelectedPreset(
        getPresetLabel(settings.agent_sandbox.cpu_limit, settings.agent_sandbox.memory_limit_mb)
      )
    }
  }, [settings])

  const handlePresetChange = (preset: string) => {
    setSelectedPreset(preset)
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
          image,
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
      {/* What is sandbox */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Bot className="h-5 w-5" />
            Agent Sandbox
          </CardTitle>
          <CardDescription>
            When sandbox is enabled, agents run inside isolated Docker
            containers. Code changes are contained and can't affect your server.
            The container has access to the cloned repository and Claude CLI.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
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

      {/* Docker image */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <ImageIcon className="h-4 w-4" />
            Container Image
          </CardTitle>
          <CardDescription>
            The Docker image used to run agents. Leave empty to use the built-in
            image (Node.js 20 + git + Claude CLI). Use a custom image if your
            project needs Python, Go, or other runtimes.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <Input
            placeholder="e.g. node:20-slim, python:3.12-slim, or your-registry/custom-agent:latest"
            value={image}
            onChange={(e) => {
              setImage(e.target.value)
              setIsDirty(true)
            }}
          />
          <p className="text-xs text-muted-foreground">
            Custom images must have <code className="text-xs bg-muted px-1 rounded">git</code> and{' '}
            <code className="text-xs bg-muted px-1 rounded">claude</code> (Claude CLI) installed.
            The repository is mounted at <code className="text-xs bg-muted px-1 rounded">/workspace</code>.
          </p>
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
          {/* Presets */}
          <div className="space-y-2">
            <Label>Preset</Label>
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
              {RESOURCE_PRESETS.map((preset) => (
                <button
                  key={preset.label}
                  onClick={() => handlePresetChange(preset.label)}
                  className={`rounded-lg border p-3 text-left transition-colors ${
                    selectedPreset === preset.label
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

          {selectedPreset === 'Custom' && (
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
            Controls whether sandbox containers can access the internet.
            Agents that use web search or API calls need network access.
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
                <div>
                  <span className="font-medium">Full access</span>
                  <span className="text-muted-foreground ml-2 text-xs">
                    — unrestricted internet (recommended)
                  </span>
                </div>
              </SelectItem>
              <SelectItem value="restricted">
                <div>
                  <span className="font-medium">Restricted</span>
                  <span className="text-muted-foreground ml-2 text-xs">
                    — Temps network only
                  </span>
                </div>
              </SelectItem>
              <SelectItem value="none">
                <div>
                  <span className="font-medium">No network</span>
                  <span className="text-muted-foreground ml-2 text-xs">
                    — fully isolated
                  </span>
                </div>
              </SelectItem>
            </SelectContent>
          </Select>
          {networkMode === 'none' && (
            <p className="text-xs text-amber-500 mt-2">
              Agents won't be able to install packages, use web search, or call
              external APIs.
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
