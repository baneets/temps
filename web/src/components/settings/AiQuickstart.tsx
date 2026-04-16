import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import {
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Circle,
  Container,
  GitBranch,
  Key,
  Loader2,
  Play,
  Rocket,
  Sparkles,
  Terminal,
  XCircle,
} from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'

interface SandboxStatus {
  docker_available: boolean
  image_ready: boolean
  image_name: string
  error: string | null
}

interface SmokeTestResult {
  passed: boolean
  environment: string
  cli_installed: boolean
  cli_authenticated: boolean
  cli_version?: string
  auth_info?: string
  setup_hint?: string
  detail?: string
}

interface QuickstartStep {
  id: string
  title: string
  description: string
  status: 'pending' | 'complete' | 'error' | 'loading'
  icon: React.ReactNode
}

interface AiQuickstartProps {
  provider: string
  sandboxEnabled: boolean
}

interface CatalogEntry {
  id: string
  credential_saved: boolean
  current_auth_type: string | null
}

interface CatalogResponse {
  default_provider: string
  providers: CatalogEntry[]
}

// localStorage cache for the last successful smoke-test result. Keyed by
// provider so switching providers doesn't surface a stale green checkmark
// for a different CLI. The test endpoint itself is fast but runs a real
// container — we cache so a reload doesn't reset the "Ready" state for
// users who already verified setup.
const SMOKE_CACHE_PREFIX = 'temps.quickstart.smoke.'
const smokeCacheKey = (provider: string) => `${SMOKE_CACHE_PREFIX}${provider}`

function loadCachedSmoke(provider: string): SmokeTestResult | null {
  try {
    const raw = localStorage.getItem(smokeCacheKey(provider))
    if (!raw) return null
    const parsed = JSON.parse(raw) as SmokeTestResult
    // Defensive: the shape might evolve. Require `passed` to be a boolean
    // before trusting the cache, otherwise drop it so a stale entry from
    // an older build can't poison the UI.
    if (typeof parsed?.passed !== 'boolean') return null
    return parsed
  } catch {
    return null
  }
}

function saveCachedSmoke(provider: string, result: SmokeTestResult) {
  try {
    localStorage.setItem(smokeCacheKey(provider), JSON.stringify(result))
  } catch {
    // Quota/private-mode — losing the cache is acceptable.
  }
}

export function AiQuickstart({
  provider,
  sandboxEnabled,
}: AiQuickstartProps) {
  const [collapsed, setCollapsed] = useState(false)
  const [sandboxStatus, setSandboxStatus] = useState<SandboxStatus | null>(null)
  // Seed from localStorage so a reload after a successful test keeps the
  // step green without re-running the test. `useState(fn)` runs the init
  // exactly once; provider-change rehydration is handled by the effect below.
  const [smokeResult, setSmokeResult] = useState<SmokeTestResult | null>(() =>
    loadCachedSmoke(provider),
  )
  const [testing, setTesting] = useState(false)
  const [statusChecked, setStatusChecked] = useState(false)
  const [catalog, setCatalog] = useState<CatalogResponse | null>(null)

  // Re-hydrate when the user switches providers — each provider has its
  // own cache entry, and a stale Claude result must not linger when the
  // card re-renders under Codex.
  useEffect(() => {
    setSmokeResult(loadCachedSmoke(provider))
  }, [provider])

  // The credentials step needs to know whether *this* provider has a saved
  // credential — pulled from the same catalog endpoint the AiProvidersCard
  // uses so the two stay in sync.
  const activeCatalogEntry = catalog?.providers.find((p) => p.id === provider)
  const tokenSaved = !!activeCatalogEntry?.credential_saved
  const authType = activeCatalogEntry?.current_auth_type ?? 'api_key'

  const providerName =
    provider === 'claude_cli'
      ? 'Claude Code'
      : provider === 'opencode'
        ? 'OpenCode'
        : 'Codex'

  const fetchSandboxStatus = useCallback(async () => {
    try {
      const response = await fetch('/api/settings/sandbox-status')
      if (response.ok) {
        setSandboxStatus(await response.json())
      }
    } catch {
      // Endpoint may not exist
    }
    setStatusChecked(true)
  }, [])

  useEffect(() => {
    fetchSandboxStatus()
    fetch('/api/settings/ai-providers')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (data) setCatalog(data)
      })
      .catch(() => {})
  }, [fetchSandboxStatus])

  const handleTest = async () => {
    setTesting(true)
    setSmokeResult(null)
    try {
      const response = await fetch('/api/projects/0/agents/smoke-test', {
        method: 'POST',
      })
      if (response.ok) {
        const result = (await response.json()) as SmokeTestResult
        setSmokeResult(result)
        // Cache every result — including failures — so the UI reflects
        // the last known state on reload instead of snapping back to
        // "pending" and asking the user to re-run the test.
        saveCachedSmoke(provider, result)
      }
    } catch {
      // Failed
    } finally {
      setTesting(false)
    }
  }

  // ── Steps aligned with Managed Agents pattern ──
  // Agent → Environment → Credentials → Test
  const steps: QuickstartStep[] = [
    {
      id: 'agent',
      title: 'Define your agent',
      icon: <Sparkles className="h-4 w-4" />,
      description: `Selected: ${providerName}. The agent defines the AI model, system prompt, and tools available during sessions.`,
      status: 'complete',
    },
    {
      id: 'environment',
      title: 'Configure environment',
      icon: <Container className="h-4 w-4" />,
      description: !statusChecked
        ? 'Checking container environment...'
        : !sandboxEnabled
          ? 'Sandbox disabled — sessions run directly on host. Enable sandbox below for isolated containers with pre-installed tools.'
          : !sandboxStatus?.docker_available
            ? 'Docker is not available. The environment needs Docker to create isolated containers for agent sessions.'
            : sandboxStatus?.image_ready
              ? `Environment ready (${sandboxStatus.image_name}). Agents and workspaces will run in isolated containers.`
              : 'Docker available. Container image will build automatically on first session.',
      status: !statusChecked
        ? 'loading'
        : !sandboxEnabled
          ? 'complete'
          : sandboxStatus?.docker_available
            ? 'complete'
            : 'error',
    },
    {
      id: 'credentials',
      title: 'Save credentials',
      icon: <Key className="h-4 w-4" />,
      description: tokenSaved
        ? `${authType === 'subscription' ? 'OAuth token' : 'API key'} encrypted and saved. Credentials are injected into the environment at session start.`
        : `Save your ${authType === 'subscription' ? 'OAuth token' : 'API key'} in the AI Provider section below. Credentials are encrypted at rest and injected into each session.`,
      status: tokenSaved ? 'complete' : 'pending',
    },
    {
      id: 'test',
      title: 'Test connection',
      icon: <Play className="h-4 w-4" />,
      description: testing
        ? `Creating a test session to verify ${providerName} is installed and authenticated...`
        : smokeResult
          ? smokeResult.passed
            ? `${providerName}${smokeResult.cli_version ? ` v${smokeResult.cli_version}` : ''} is ready${smokeResult.auth_info ? ` (${smokeResult.auth_info})` : ''}. Sessions will start successfully.`
            : smokeResult.setup_hint || 'Test session failed — check credentials and environment configuration.'
          : `Run a test session to verify ${providerName} is installed and authenticated in the environment.`,
      status: testing
        ? 'loading'
        : smokeResult
          ? smokeResult.passed
            ? 'complete'
            : 'error'
          : 'pending',
    },
  ]

  const allComplete = steps.every((s) => s.status === 'complete')
  const currentStepIndex = steps.findIndex((s) => s.status !== 'complete')

  if (allComplete && collapsed) {
    return null
  }

  return (
    <Card className="border-primary/30 bg-primary/[0.02]">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Rocket className="h-5 w-5 text-primary" />
            <CardTitle className="text-base">Quick Setup</CardTitle>
            {allComplete && (
              <span className="text-xs bg-green-500/10 text-green-600 dark:text-green-400 px-2 py-0.5 rounded-full">
                Ready
              </span>
            )}
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setCollapsed(!collapsed)}
            className="h-7 px-2"
          >
            {collapsed ? (
              <ChevronRight className="h-4 w-4" />
            ) : (
              <ChevronDown className="h-4 w-4" />
            )}
          </Button>
        </div>
        {!collapsed && (
          <CardDescription>
            Configure your agent, environment, and credentials to start AI-powered sessions.
          </CardDescription>
        )}
      </CardHeader>

      {!collapsed && (
        <CardContent className="space-y-3">
          {steps.map((step, i) => (
            <div
              key={step.id}
              className={`flex items-start gap-3 rounded-lg border p-3 transition-colors ${
                step.status === 'complete'
                  ? 'border-green-500/20 bg-green-500/5'
                  : step.status === 'error'
                    ? 'border-red-500/20 bg-red-500/5'
                    : i === currentStepIndex
                      ? 'border-primary/30 bg-primary/5'
                      : 'border-border/50 opacity-60'
              }`}
            >
              <div className="mt-0.5 shrink-0">
                {step.status === 'complete' ? (
                  <CheckCircle2 className="h-4 w-4 text-green-500" />
                ) : step.status === 'error' ? (
                  <XCircle className="h-4 w-4 text-red-500" />
                ) : step.status === 'loading' ? (
                  <Loader2 className="h-4 w-4 animate-spin text-primary" />
                ) : (
                  <Circle className="h-4 w-4 text-muted-foreground" />
                )}
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-muted-foreground">{step.icon}</span>
                  <p className="text-sm font-medium">{step.title}</p>
                </div>
                <p className="text-xs text-muted-foreground mt-0.5">
                  {step.description}
                </p>
              </div>

              {step.id === 'test' && step.status !== 'complete' && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleTest}
                  disabled={testing || !tokenSaved}
                  className="shrink-0 h-7 text-xs"
                >
                  {testing ? (
                    <Loader2 className="h-3 w-3 animate-spin mr-1" />
                  ) : (
                    <Play className="h-3 w-3 mr-1" />
                  )}
                  Test
                </Button>
              )}
            </div>
          ))}

          {/* Next steps — what you can do now */}
          {allComplete && (
            <div className="rounded-lg border border-green-500/20 bg-green-500/5 p-4 space-y-3">
              <p className="text-sm font-medium text-green-700 dark:text-green-400">
                Ready to go! Start a session:
              </p>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="flex items-start gap-2.5 text-sm">
                  <Terminal className="h-4 w-4 mt-0.5 text-muted-foreground shrink-0" />
                  <div>
                    <p className="font-medium">Start a Workspace</p>
                    <p className="text-xs text-muted-foreground">
                      Interactive session — open any project's Workspace tab and
                      send messages to {providerName} with full repo access.
                    </p>
                  </div>
                </div>
                <div className="flex items-start gap-2.5 text-sm">
                  <Bot className="h-4 w-4 mt-0.5 text-muted-foreground shrink-0" />
                  <div>
                    <p className="font-medium">Define an Agent</p>
                    <p className="text-xs text-muted-foreground">
                      Autonomous session — add a{' '}
                      <code className="bg-muted px-1 rounded text-xs">.temps/agents/*.yaml</code>{' '}
                      file with triggers (push, cron) and a system prompt.
                    </p>
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* Core concepts — collapsed by default */}
          {!allComplete && <CoreConcepts providerName={providerName} sandboxEnabled={sandboxEnabled} />}
        </CardContent>
      )}
    </Card>
  )
}

function CoreConcepts({
  providerName,
  sandboxEnabled,
}: {
  providerName: string
  sandboxEnabled: boolean
}) {
  const [expanded, setExpanded] = useState(false)

  return (
    <div className="pt-1">
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
      >
        {expanded ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        Core concepts
      </button>

      {expanded && (
        <div className="mt-2 rounded-lg border p-3 text-xs text-muted-foreground space-y-3">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="flex items-start gap-2">
              <Sparkles className="h-3.5 w-3.5 mt-0.5 shrink-0" />
              <div>
                <p className="font-medium text-foreground">Agent</p>
                <p>
                  The AI model, system prompt, and tools that define how {providerName} behaves.
                  Configured globally here or per-project via YAML.
                </p>
              </div>
            </div>
            <div className="flex items-start gap-2">
              <Container className="h-3.5 w-3.5 mt-0.5 shrink-0" />
              <div>
                <p className="font-medium text-foreground">Environment</p>
                <p>
                  {sandboxEnabled
                    ? 'An isolated Docker container with pre-installed tools, network access, and mounted repository.'
                    : 'The host machine where agents run directly. Enable sandbox for isolated containers.'}
                </p>
              </div>
            </div>
            <div className="flex items-start gap-2">
              <Terminal className="h-3.5 w-3.5 mt-0.5 shrink-0" />
              <div>
                <p className="font-medium text-foreground">Session</p>
                <p>
                  A running agent instance — either interactive (workspace) or autonomous (agent run).
                  Each session gets its own environment and conversation history.
                </p>
              </div>
            </div>
            <div className="flex items-start gap-2">
              <GitBranch className="h-3.5 w-3.5 mt-0.5 shrink-0" />
              <div>
                <p className="font-medium text-foreground">Config & Secrets</p>
                <p>
                  Config repos overlay{' '}
                  <code className="bg-muted px-0.5 rounded">.claude/</code> directories (skills, MCP servers)
                  into sessions. Secrets are encrypted values injected at runtime via{' '}
                  <code className="bg-muted px-0.5 rounded">{'${TEMPS_SECRET:name}'}</code>.
                </p>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
