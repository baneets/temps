import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import {
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Circle,
  ExternalLink,
  Loader2,
  Play,
  Rocket,
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
  detail?: string
}

interface AiQuickstartProps {
  provider: string
  authType: string
  tokenSaved: boolean
  sandboxEnabled: boolean
}

export function AiQuickstart({
  provider,
  authType,
  tokenSaved,
  sandboxEnabled,
}: AiQuickstartProps) {
  const [collapsed, setCollapsed] = useState(false)
  const [sandboxStatus, setSandboxStatus] = useState<SandboxStatus | null>(null)
  const [smokeResult, setSmokeResult] = useState<SmokeTestResult | null>(null)
  const [testing, setTesting] = useState(false)
  const [statusChecked, setStatusChecked] = useState(false)

  const providerName =
    provider === 'claude_cli'
      ? 'Claude Code'
      : provider === 'opencode'
        ? 'OpenCode'
        : 'Codex'

  const providerInstall =
    provider === 'claude_cli'
      ? 'npm install -g @anthropic-ai/claude-code'
      : provider === 'opencode'
        ? 'curl -fsSL https://opencode.ai/install | bash'
        : 'npm install -g @openai/codex'

  const providerAuth =
    provider === 'claude_cli'
      ? authType === 'subscription'
        ? 'claude setup-token'
        : 'Set ANTHROPIC_API_KEY'
      : provider === 'opencode'
        ? 'opencode auth add'
        : 'Set OPENAI_API_KEY'

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
  }, [fetchSandboxStatus])

  const handleTest = async () => {
    setTesting(true)
    setSmokeResult(null)
    try {
      const response = await fetch('/api/projects/0/agents/smoke-test', {
        method: 'POST',
      })
      if (response.ok) {
        setSmokeResult(await response.json())
      }
    } catch {
      // Failed
    } finally {
      setTesting(false)
    }
  }

  // Derive step statuses
  const steps: QuickstartStep[] = [
    {
      id: 'provider',
      title: `Choose AI provider`,
      description: `Selected: ${providerName}`,
      status: 'complete',
    },
    {
      id: 'credentials',
      title: 'Save credentials',
      description: tokenSaved
        ? `${authType === 'subscription' ? 'OAuth token' : 'API key'} encrypted and saved`
        : `Save your ${authType === 'subscription' ? 'OAuth token' : 'API key'} in the AI Provider section below`,
      status: tokenSaved ? 'complete' : 'pending',
    },
    {
      id: 'docker',
      title: 'Docker & sandbox image',
      description: !statusChecked
        ? 'Checking...'
        : !sandboxEnabled
          ? 'Sandbox disabled — agents will run on host (less secure)'
          : !sandboxStatus?.docker_available
            ? 'Docker is not available. Install Docker and restart.'
            : sandboxStatus?.image_ready
              ? `Image ready: ${sandboxStatus.image_name}`
              : 'Image not built yet — it will build automatically on first run',
      status: !statusChecked
        ? 'loading'
        : !sandboxEnabled
          ? 'complete' // Not required when sandbox is off
          : sandboxStatus?.docker_available && sandboxStatus?.image_ready
            ? 'complete'
            : sandboxStatus?.docker_available
              ? 'complete' // Image auto-builds
              : 'error',
    },
    {
      id: 'test',
      title: 'Test connection',
      description: testing
        ? 'Running smoke test...'
        : smokeResult
          ? smokeResult.passed
            ? `${providerName} ${smokeResult.cli_version ? `v${smokeResult.cli_version} ` : ''}is installed and authenticated${smokeResult.auth_info ? ` (${smokeResult.auth_info})` : ''}`
            : smokeResult.setup_hint || 'Connection test failed'
          : 'Run a smoke test to verify everything works',
      status: testing
        ? 'loading'
        : smokeResult
          ? smokeResult.passed
            ? 'complete'
            : 'error'
          : 'pending',
    },
  ]

  const allComplete = steps.every(
    (s) => s.status === 'complete'
  )
  const currentStepIndex = steps.findIndex(
    (s) => s.status !== 'complete'
  )

  // Auto-collapse when all done and user has dismissed before
  if (allComplete && collapsed) {
    return null // Fully hidden when done and dismissed
  }

  return (
    <Card className="border-primary/30 bg-primary/[0.02]">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Rocket className="h-5 w-5 text-primary" />
            <CardTitle className="text-base">Quick Setup</CardTitle>
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
            Get your first AI agent or workspace running in 4 steps.
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
                <p className="text-sm font-medium">{step.title}</p>
                <p className="text-xs text-muted-foreground mt-0.5">
                  {step.description}
                </p>
              </div>

              {/* Action buttons for specific steps */}
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

          {/* Next steps after completion */}
          {allComplete && (
            <div className="rounded-lg border border-green-500/20 bg-green-500/5 p-4 space-y-3">
              <p className="text-sm font-medium text-green-700 dark:text-green-400">
                Setup complete! Here's what you can do:
              </p>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                <div className="flex items-start gap-2 text-sm">
                  <Terminal className="h-4 w-4 mt-0.5 text-muted-foreground shrink-0" />
                  <div>
                    <p className="font-medium">Open a Workspace</p>
                    <p className="text-xs text-muted-foreground">
                      Go to any project → Workspace tab to start an interactive
                      AI coding session with {providerName}.
                    </p>
                  </div>
                </div>
                <div className="flex items-start gap-2 text-sm">
                  <ExternalLink className="h-4 w-4 mt-0.5 text-muted-foreground shrink-0" />
                  <div>
                    <p className="font-medium">Configure an Agent</p>
                    <p className="text-xs text-muted-foreground">
                      Add a <code className="bg-muted px-1 rounded text-xs">.temps/agents/*.yaml</code> file
                      to your repo for autonomous AI tasks on push or cron.
                    </p>
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* How it works — collapsed by default */}
          {!allComplete && (
            <HowItWorks
              providerName={providerName}
              providerInstall={providerInstall}
              providerAuth={providerAuth}
              sandboxEnabled={sandboxEnabled}
            />
          )}
        </CardContent>
      )}
    </Card>
  )
}

function HowItWorks({
  providerName,
  providerInstall,
  providerAuth,
  sandboxEnabled,
}: {
  providerName: string
  providerInstall: string
  providerAuth: string
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
        How does it work?
      </button>

      {expanded && (
        <div className="mt-2 rounded-lg border p-3 text-xs text-muted-foreground space-y-2">
          <p>
            <strong>Workspaces</strong> give you an interactive terminal where{' '}
            {providerName} has full access to your project's repository.{' '}
            {sandboxEnabled
              ? 'Each session runs in an isolated Docker container — code changes are safe and contained.'
              : 'Sessions run directly on the host. Enable sandbox mode for better isolation.'}
          </p>
          <p>
            <strong>Agents</strong> run autonomously on triggers (git push, cron,
            or manual). They analyze code, fix bugs, and create pull requests
            without human interaction.
          </p>
          <p>
            <strong>Setup:</strong> {providerName} needs to be installed (
            <code className="bg-muted px-1 rounded">{providerInstall}</code>) and
            authenticated (
            <code className="bg-muted px-1 rounded">{providerAuth}</code>).
            Credentials are encrypted and injected into the sandbox at runtime.
          </p>
          <p>
            <strong>Config repos</strong> let you share <code className="bg-muted px-1 rounded">.claude/</code>{' '}
            directories (skills, MCP servers, settings) across all agent runs.{' '}
            <strong>Secrets</strong> are encrypted values referenced as{' '}
            <code className="bg-muted px-1 rounded">{'${TEMPS_SECRET:name}'}</code>{' '}
            in config files and injected at runtime.
          </p>
        </div>
      )}
    </div>
  )
}
