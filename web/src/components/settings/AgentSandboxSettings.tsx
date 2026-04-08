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
  Play,
  RefreshCw,
  Save,
  Sparkles,
  XCircle,
} from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useSettings, useUpdateSettings } from '@/hooks/useSettings'
import { usePageTitle } from '@/hooks/usePageTitle'
import { PreviewGatewayCard } from './PreviewGatewayCard'
import { AgentSecrets } from '@/components/agents/ProjectSecrets'

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
  { label: 'Light', cpu: 2, memory: 4096 },
  { label: 'Standard', cpu: 4, memory: 8192 },
  { label: 'Heavy', cpu: 8, memory: 16384 },
  { label: 'Custom', cpu: 0, memory: 0 },
]

function getResourcePresetLabel(cpu: number, memory: number): string {
  const match = RESOURCE_PRESETS.find(
    (p) => p.cpu === cpu && p.memory === memory
  )
  return match ? match.label : 'Custom'
}

export function AgentSandboxSettings() {
  usePageTitle('AI Agents')

  const { data: settings, isLoading } = useSettings()
  const updateSettings = useUpdateSettings()

  const [defaultProvider, setDefaultProvider] = useState('claude_cli')
  const [defaultModel, setDefaultModel] = useState('')
  const [authType, setAuthType] = useState('subscription')
  const [tokenInput, setTokenInput] = useState('')
  const [tokenSaving, setTokenSaving] = useState(false)
  const [tokenSaved, setTokenSaved] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [runtime, setRuntime] = useState('node')
  const [customImage, setCustomImage] = useState('')
  const [cpuLimit, setCpuLimit] = useState(4)
  const [memoryLimitMb, setMemoryLimitMb] = useState(8192)
  const [networkMode, setNetworkMode] = useState('full')
  const [isDirty, setIsDirty] = useState(false)
  const [resourcePreset, setResourcePreset] = useState('Standard')
  const [globalConfigRepo, setGlobalConfigRepo] = useState('')
  const [globalConfigRepoBranch, setGlobalConfigRepoBranch] = useState('main')

  const [availableModels, setAvailableModels] = useState<string[]>([])
  const [sandboxStatus, setSandboxStatus] = useState<SandboxStatus | null>(null)
  const [statusLoading, setStatusLoading] = useState(false)
  const [rebuilding, setRebuilding] = useState(false)
  const [buildLog, setBuildLog] = useState<string[]>([])
  const [smokeTestLoading, setSmokeTestLoading] = useState(false)
  const [smokeTestResult, setSmokeTestResult] = useState<{
    passed: boolean
    environment: string
    cli_installed: boolean
    cli_authenticated: boolean
    cli_version?: string
    auth_info?: string
    setup_hint?: string
    detail?: string
  } | null>(null)

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
      setDefaultProvider(s.default_provider || 'claude_cli')
      setDefaultModel(s.default_model || '')
      setAuthType(s.auth_type || 'subscription')
      setTokenSaved(!!s.api_key_encrypted)
      setEnabled(s.enabled)
      setRuntime(s.runtime || 'node')
      setCustomImage(s.custom_image || '')
      setCpuLimit(s.cpu_limit)
      setMemoryLimitMb(s.memory_limit_mb)
      setNetworkMode(s.network_mode || 'full')
      setResourcePreset(getResourcePresetLabel(s.cpu_limit, s.memory_limit_mb))
    }
    if (settings?.ai_config) {
      setGlobalConfigRepo(settings.ai_config.config_repo || '')
      setGlobalConfigRepoBranch(settings.ai_config.config_repo_branch || 'main')
    }
  }, [settings])

  useEffect(() => {
    fetchSandboxStatus()
    // Fetch available models
    fetch('/api/settings/agent-models')
      .then((r) => r.ok ? r.json() : null)
      .then((data) => {
        if (data?.models) setAvailableModels(data.models)
      })
      .catch(() => {})
  }, [fetchSandboxStatus])

  const handleRebuildImage = async () => {
    setRebuilding(true)
    setBuildLog([])
    try {
      const response = await fetch('/api/settings/sandbox-rebuild', {
        method: 'POST',
      })
      if (!response.ok || !response.body) {
        toast.error('Failed to start image rebuild')
        setRebuilding(false)
        return
      }
      const reader = response.body.getReader()
      const decoder = new TextDecoder()
      let buffer = ''

      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        buffer += decoder.decode(value, { stream: true })

        // Parse SSE lines
        const lines = buffer.split('\n')
        buffer = lines.pop() || ''
        for (const line of lines) {
          if (line.startsWith('data:')) {
            const data = line.slice(5).trim()
            if (!data) continue
            // Check if this is the final "done" JSON message
            try {
              const parsed = JSON.parse(data)
              if (parsed.type === 'done') {
                if (parsed.success) {
                  toast.success(`Image rebuilt: ${parsed.image_name}`)
                } else {
                  toast.error(parsed.error || 'Build failed')
                }
                continue
              }
            } catch {
              // Not JSON — it's a plain build log line
            }
            setBuildLog((prev) => [...prev, data])
          }
        }
      }
      await fetchSandboxStatus()
    } catch {
      toast.error('Failed to rebuild image')
    } finally {
      setRebuilding(false)
    }
  }

  const handleSaveToken = async () => {
    if (!tokenInput.trim()) return
    setTokenSaving(true)
    try {
      const response = await fetch('/api/settings/agent-token', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: tokenInput.trim() }),
      })
      if (response.ok) {
        setTokenSaved(true)
        setTokenInput('')
        toast.success('Token saved and encrypted')
      } else {
        toast.error('Failed to save token')
      }
    } catch {
      toast.error('Failed to save token')
    } finally {
      setTokenSaving(false)
    }
  }

  const handleSmokeTest = async () => {
    setSmokeTestLoading(true)
    setSmokeTestResult(null)
    try {
      // Use project_id=0 as a global test — the endpoint doesn't actually use it
      const response = await fetch('/api/projects/0/agents/smoke-test', {
        method: 'POST',
      })
      if (response.ok) {
        const data = await response.json()
        setSmokeTestResult(data)
        if (data.passed) {
          toast.success('AI provider is configured correctly')
        }
      } else {
        toast.error('Smoke test failed')
      }
    } catch {
      toast.error('Failed to run smoke test')
    } finally {
      setSmokeTestLoading(false)
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
      // Preserve api_key_encrypted from current settings (saved via separate endpoint)
      const currentEncryptedKey = settings?.agent_sandbox?.api_key_encrypted
      await updateSettings.mutateAsync({
        agent_sandbox: {
          default_provider: defaultProvider,
          default_model: defaultModel,
          auth_type: authType,
          api_key_encrypted: currentEncryptedKey || undefined,
          enabled,
          runtime,
          custom_image: customImage,
          cpu_limit: cpuLimit,
          memory_limit_mb: memoryLimitMb,
          network_mode: networkMode,
        },
        ai_config: {
          config_repo: globalConfigRepo,
          config_repo_branch: globalConfigRepoBranch,
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
      {/* AI Provider */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Sparkles className="h-5 w-5" />
            AI Provider
          </CardTitle>
          <CardDescription>
            Agents need an AI coding assistant installed and authenticated on the
            server. Choose your provider below.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {/* Provider cards */}
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
            {[
              {
                id: 'claude_cli',
                name: 'Claude Code',
                install: 'npm install -g @anthropic-ai/claude-code',
                auth: 'claude setup-token',
              },
              {
                id: 'opencode',
                name: 'OpenCode',
                install: 'curl -fsSL https://opencode.ai/install | bash',
                auth: 'opencode auth add',
              },
              {
                id: 'codex_cli',
                name: 'Codex',
                install: 'npm install -g @openai/codex',
                auth: 'Set OPENAI_API_KEY',
              },
            ].map((provider) => (
              <button
                key={provider.id}
                onClick={() => {
                  setDefaultProvider(provider.id)
                  setIsDirty(true)
                }}
                className={`rounded-lg border p-4 space-y-2 text-left transition-colors ${
                  defaultProvider === provider.id
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-primary/50'
                }`}
              >
                <h4 className="text-sm font-medium">{provider.name}</h4>
                <div className="text-xs text-muted-foreground space-y-1">
                  <p>
                    Install:{' '}
                    <code className="bg-muted px-1 rounded">
                      {provider.install}
                    </code>
                  </p>
                  <p>
                    Auth:{' '}
                    <code className="bg-muted px-1 rounded">
                      {provider.auth}
                    </code>
                  </p>
                </div>
              </button>
            ))}
          </div>

          {/* Auth type */}
          <div className="rounded-lg border p-4 space-y-3">
            <h4 className="text-sm font-medium">Authentication</h4>
            <div className="grid grid-cols-2 gap-2">
              <button
                onClick={() => { setAuthType('subscription'); setIsDirty(true) }}
                className={`rounded-lg border p-3 text-left transition-colors ${
                  authType === 'subscription'
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-primary/50'
                }`}
              >
                <p className="text-sm font-medium">Subscription</p>
                <p className="text-xs text-muted-foreground">
                  Claude Max/Pro — uses OAuth token from <code className="bg-muted px-1 rounded">claude setup-token</code>
                </p>
              </button>
              <button
                onClick={() => { setAuthType('api_key'); setIsDirty(true) }}
                className={`rounded-lg border p-3 text-left transition-colors ${
                  authType === 'api_key'
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-primary/50'
                }`}
              >
                <p className="text-sm font-medium">API Key</p>
                <p className="text-xs text-muted-foreground">
                  Pay-per-use — uses ANTHROPIC_API_KEY or OPENAI_API_KEY
                </p>
              </button>
            </div>

            {/* Credential input */}
            <div className="space-y-2 pt-2">
              <Label>
                {authType === 'subscription'
                  ? 'OAuth Token'
                  : defaultProvider === 'codex_cli'
                    ? 'OpenAI API Key'
                    : 'Anthropic API Key'}
              </Label>
              <p className="text-xs text-muted-foreground">
                {authType === 'subscription'
                  ? 'Run `claude setup-token` in your terminal and paste the token here.'
                  : defaultProvider === 'codex_cli'
                    ? 'Your OpenAI API key (sk-...).'
                    : 'Your Anthropic API key (sk-ant-api03-...).'}
                {' '}Encrypted before storage.
              </p>
              <div className="flex gap-2">
                <Input
                  type="password"
                  placeholder={
                    tokenSaved
                      ? '••••••••••••• (saved)'
                      : authType === 'subscription'
                        ? 'Paste OAuth token from claude setup-token...'
                        : 'Paste API key...'
                  }
                  value={tokenInput}
                  onChange={(e) => {
                    setTokenInput(e.target.value)
                    setTokenSaved(false)
                  }}
                />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleSaveToken}
                  disabled={tokenSaving || !tokenInput.trim()}
                  className="shrink-0"
                >
                  {tokenSaving ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />
                  ) : (
                    <Save className="h-3.5 w-3.5 mr-1" />
                  )}
                  Save
                </Button>
              </div>
              {tokenSaved && !tokenInput && (
                <div className="flex items-center gap-1.5 text-xs text-green-500">
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  Credential encrypted and saved
                </div>
              )}
            </div>
          </div>

          {/* Model */}
          <div className="rounded-lg border p-4 space-y-3">
            <h4 className="text-sm font-medium">Model</h4>
            <Select
              value={
                defaultModel === ''
                  ? '_default'
                  : availableModels.includes(defaultModel)
                    ? defaultModel
                    : '_custom'
              }
              onValueChange={(v) => {
                if (v === '_default') {
                  setDefaultModel('')
                } else if (v === '_custom') {
                  setDefaultModel(defaultModel || '')
                } else {
                  setDefaultModel(v)
                }
                setIsDirty(true)
              }}
            >
              <SelectTrigger>
                <SelectValue placeholder="Use provider default" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="_default">Use provider default</SelectItem>
                {availableModels.map((model) => (
                  <SelectItem key={model} value={model}>
                    {model}
                  </SelectItem>
                ))}
                <SelectItem value="_custom">Custom model...</SelectItem>
              </SelectContent>
            </Select>
            {defaultModel !== '' && !availableModels.includes(defaultModel) && (
              <Input
                placeholder="e.g. anthropic/claude-sonnet-4-6"
                value={defaultModel}
                onChange={(e) => {
                  setDefaultModel(e.target.value)
                  setIsDirty(true)
                }}
              />
            )}
          </div>

          {/* Smoke test */}
          <div className="rounded-lg border p-4 space-y-3">
            <div className="flex items-center justify-between">
              <h4 className="text-sm font-medium">Connection Test</h4>
              <Button
                variant="outline"
                size="sm"
                onClick={handleSmokeTest}
                disabled={smokeTestLoading}
              >
                {smokeTestLoading ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-2" />
                ) : (
                  <Play className="h-3.5 w-3.5 mr-2" />
                )}
                {smokeTestLoading ? 'Testing...' : 'Test Connection'}
              </Button>
            </div>

            {smokeTestResult && (
              <div className="space-y-2">
                <div className="flex items-center gap-2 text-sm">
                  {smokeTestResult.cli_installed ? (
                    <CheckCircle2 className="h-4 w-4 text-green-500" />
                  ) : (
                    <XCircle className="h-4 w-4 text-red-500" />
                  )}
                  <span>
                    CLI:{' '}
                    {smokeTestResult.cli_installed
                      ? `installed${smokeTestResult.cli_version ? ` (${smokeTestResult.cli_version})` : ''}`
                      : 'not found'}
                  </span>
                </div>
                <div className="flex items-center gap-2 text-sm">
                  {smokeTestResult.cli_authenticated ? (
                    <CheckCircle2 className="h-4 w-4 text-green-500" />
                  ) : (
                    <XCircle className="h-4 w-4 text-red-500" />
                  )}
                  <span>
                    Auth:{' '}
                    {smokeTestResult.cli_authenticated
                      ? smokeTestResult.auth_info || 'authenticated'
                      : 'not authenticated'}
                  </span>
                </div>
                <div className="text-xs text-muted-foreground">
                  Environment: {smokeTestResult.environment === 'sandbox' ? 'sandbox container' : 'host'}
                </div>
                {smokeTestResult.setup_hint && (
                  <Alert variant="destructive" className="mt-2">
                    <AlertTriangle className="h-4 w-4" />
                    <AlertDescription className="text-sm">
                      {smokeTestResult.setup_hint}
                    </AlertDescription>
                  </Alert>
                )}
              </div>
            )}

            {!smokeTestResult && !smokeTestLoading && (
              <p className="text-sm text-muted-foreground">
                Tests the default AI provider (Claude CLI) in the environment
                where agents will run
                {enabled ? ' (sandbox container)' : ' (host)'}.
              </p>
            )}
          </div>
        </CardContent>
      </Card>

      {/* Sandbox */}
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
                  <div className="space-y-2 mt-2">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleRebuildImage}
                      disabled={rebuilding}
                    >
                      {rebuilding ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin mr-2" />
                      ) : (
                        <RefreshCw className="h-3.5 w-3.5 mr-2" />
                      )}
                      {rebuilding ? 'Rebuilding...' : 'Rebuild Image'}
                    </Button>
                    {buildLog.length > 0 && (
                      <div className="max-h-60 overflow-y-auto rounded border bg-black/80 p-3 font-mono text-xs text-green-400 space-y-0.5">
                        {buildLog.map((line, i) => (
                          <div key={i}>{line}</div>
                        ))}
                        {rebuilding && (
                          <div className="animate-pulse text-green-300">...</div>
                        )}
                      </div>
                    )}
                  </div>
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

      {/* Workspace Preview Gateway */}
      <PreviewGatewayCard />

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
                  max={32768}
                  step={256}
                  value={memoryLimitMb}
                  onChange={(e) => {
                    setMemoryLimitMb(parseInt(e.target.value) || 8192)
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

      {/* Global Config Repository */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            Global Config Repository
          </CardTitle>
          <CardDescription>
            A shared config repository applied to all agent runs. Contains a{' '}
            <code className="text-xs bg-muted px-1 rounded">.claude/</code>{' '}
            directory with skills, MCP servers, and settings. Per-agent config
            repos override conflicting files.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="global-config-repo">Repository</Label>
            <Input
              id="global-config-repo"
              placeholder="org/claude-config"
              value={globalConfigRepo}
              onChange={(e) => {
                setGlobalConfigRepo(e.target.value)
                setIsDirty(true)
              }}
            />
            <p className="text-xs text-muted-foreground">
              GitHub repo path. The repo must be accessible via the project's git
              provider connection. Leave empty to disable.
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor="global-config-branch">Branch</Label>
            <Input
              id="global-config-branch"
              placeholder="main"
              value={globalConfigRepoBranch}
              onChange={(e) => {
                setGlobalConfigRepoBranch(e.target.value)
                setIsDirty(true)
              }}
            />
          </div>
        </CardContent>
      </Card>

      {/* Global Secrets */}
      <AgentSecrets />

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
