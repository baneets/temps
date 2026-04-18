import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'

import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import {
  CheckCircle2,
  Loader2,
  Play,
  Save,
  Sparkles,
  XCircle,
} from 'lucide-react'

// ── Types mirroring the Rust catalog DTO ─────────────────────────────────────
// The shape is dictated by the `/settings/ai-providers` endpoint defined in
// `temps-agents::handlers::ai_providers`. Keep these in sync if the DTO
// changes — but adding a new provider only requires a new catalog entry on
// the Rust side, never a TS edit.

type CredentialFormat = 'api_key' | 'oauth_token' | 'config_file'

interface AuthFlavorDto {
  id: string
  label: string
  description: string
  format: CredentialFormat
  env_var: string | null
}

interface ProviderCatalogDto {
  id: string
  name: string
  install_command: string
  auth_command: string
  auth_flavors: AuthFlavorDto[]
  credential_saved: boolean
  current_auth_type: string | null
  /// Models the Rust catalog ships for this provider. Convenience list — the
  /// PATCH endpoint accepts arbitrary model ids too, so users can pin a model
  /// that's newer than what we've catalogued.
  models: string[]
  /// Currently saved per-provider default model. Empty/null means "use
  /// whatever the CLI picks by default".
  default_model: string | null
}

interface ProviderCatalogResponse {
  default_provider: string
  providers: ProviderCatalogDto[]
}

async function fetchCatalog(): Promise<ProviderCatalogResponse> {
  const res = await fetch('/api/settings/ai-providers')
  if (!res.ok) throw new Error(`Failed to load AI provider catalog (${res.status})`)
  return res.json()
}

interface AiProvidersCardProps {
  /// Currently selected default provider id from the wider settings form.
  /// Changing the radio in this card propagates back to the parent so the
  /// quickstart panel and Save button stay in sync.
  defaultProvider: string
  onDefaultProviderChange: (providerId: string) => void
}

export function AiProvidersCard({
  defaultProvider,
  onDefaultProviderChange,
}: AiProvidersCardProps) {
  const { data, isLoading, isError } = useQuery({
    queryKey: ['ai-provider-catalog'],
    queryFn: fetchCatalog,
    staleTime: 60 * 1000,
  })

  if (isLoading) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Sparkles className="h-5 w-5" />
            AI Providers
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        </CardContent>
      </Card>
    )
  }

  if (isError || !data) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Sparkles className="h-5 w-5" />
            AI Providers
          </CardTitle>
        </CardHeader>
        <CardContent>
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load AI provider catalog. Refresh the page to retry.
            </AlertDescription>
          </Alert>
        </CardContent>
      </Card>
    )
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Sparkles className="h-5 w-5" />
          AI Providers
        </CardTitle>
        <CardDescription>
          Workspaces always use the active provider below. Each provider has
          its own credential — saving Codex's API key won't replace your Claude
          subscription token.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {data.providers.map((provider) => (
          <ProviderCard
            key={provider.id}
            provider={provider}
            isActive={defaultProvider === provider.id}
            onSetActive={() => onDefaultProviderChange(provider.id)}
          />
        ))}
      </CardContent>
    </Card>
  )
}

// ── Single provider card ─────────────────────────────────────────────────────

interface ProviderCardProps {
  provider: ProviderCatalogDto
  isActive: boolean
  onSetActive: () => void
}

function ProviderCard({ provider, isActive, onSetActive }: ProviderCardProps) {
  const queryClient = useQueryClient()
  const defaultFlavor =
    provider.auth_flavors.find((f) => f.id === provider.current_auth_type) ??
    provider.auth_flavors[0]

  const [selectedFlavorId, setSelectedFlavorId] = useState(defaultFlavor.id)
  const [credential, setCredential] = useState('')
  const [saving, setSaving] = useState(false)
  const [activating, setActivating] = useState(false)

  // Smoke test state. The backend now accepts an explicit provider_id query
  // param so we can verify *any* saved credential, not just whichever one
  // happens to be active. The Test button is still hidden when no credential
  // has been saved yet — there's nothing meaningful to check in that case.
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<{
    passed: boolean
    environment: string
    cli_version: string | null
    auth_info: string | null
    setup_hint: string | null
  } | null>(null)

  // Per-provider model state. Server is the source of truth — `serverModel`
  // is what we last successfully PATCHead (or what the catalog reported on
  // load). `modelDraft` is the local input being edited. We auto-save on
  // dropdown change immediately; the custom-text input debounces to avoid
  // a PATCH per keystroke. Custom-mode is sticky so typing into the input
  // doesn't snap back to the dropdown when the value happens to match.
  const initialModel = provider.default_model ?? ''
  const [serverModel, setServerModel] = useState(initialModel)
  const [modelDraft, setModelDraft] = useState(initialModel)
  const [customMode, setCustomMode] = useState(
    provider.models.length === 0 ||
    (initialModel !== '' && !provider.models.includes(initialModel))
  )
  const [savingModel, setSavingModel] = useState(false)

  // If the catalog refetches and reports a different default_model (e.g. a
  // sibling tab saved one), reset our local draft to match — but only when
  // the user isn't mid-edit.
  useEffect(() => {
    const fresh = provider.default_model ?? ''
    if (fresh !== serverModel) {
      setServerModel(fresh)
      setModelDraft(fresh)
      setCustomMode(
        provider.models.length === 0 ||
        (fresh !== '' && !provider.models.includes(fresh))
      )
    }
    // We deliberately key off the canonical fresh value, not the whole
    // provider object, so spurious referential changes don't clobber edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider.default_model])

  const selectedFlavor =
    provider.auth_flavors.find((f) => f.id === selectedFlavorId) ?? defaultFlavor

  const persistModel = async (next: string) => {
    if (next === serverModel) return
    setSavingModel(true)
    try {
      const res = await fetch(`/api/settings/ai-providers/${provider.id}`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        // Empty string clears the field server-side ("use provider default").
        body: JSON.stringify({ default_model: next }),
      })
      if (!res.ok) {
        const detail = await res.text()
        toast.error(`Failed to save ${provider.name} model`, {
          description: detail.slice(0, 200),
        })
        // Roll the draft back so the UI matches the server.
        setModelDraft(serverModel)
        setCustomMode(
          provider.models.length === 0 ||
          (serverModel !== '' && !provider.models.includes(serverModel))
        )
        return
      }
      setServerModel(next)
      toast.success(
        next === ''
          ? `${provider.name} will use its default model`
          : `${provider.name} model set to ${next}`
      )
      await queryClient.invalidateQueries({ queryKey: ['ai-provider-catalog'] })
    } catch (e) {
      toast.error(`Failed to save ${provider.name} model`, {
        description: e instanceof Error ? e.message : 'Network error',
      })
      setModelDraft(serverModel)
    } finally {
      setSavingModel(false)
    }
  }

  // Debounce custom-text saves so we don't fire a PATCH on every keystroke.
  // 600ms is long enough for a steady typist to finish a model id, short
  // enough that the user notices the save without a manual confirm.
  const debounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    if (!customMode) return
    if (modelDraft === serverModel) return
    if (debounceTimerRef.current) clearTimeout(debounceTimerRef.current)
    debounceTimerRef.current = setTimeout(() => {
      void persistModel(modelDraft.trim())
    }, 600)
    return () => {
      if (debounceTimerRef.current) clearTimeout(debounceTimerRef.current)
    }
    // persistModel reads serverModel via closure; it's fine to omit since
    // we re-create the timer on every modelDraft/serverModel change anyway.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modelDraft, customMode, serverModel])

  const handleActivate = async () => {
    setActivating(true)
    try {
      const res = await fetch(
        `/api/settings/ai-providers/${provider.id}/activate`,
        { method: 'POST' }
      )
      if (!res.ok) {
        const detail = await res.text()
        toast.error(`Failed to activate ${provider.name}`, {
          description: detail.slice(0, 200),
        })
        return
      }
      onSetActive()
      toast.success(`${provider.name} is now the active provider`)
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['ai-provider-catalog'] }),
        queryClient.invalidateQueries({ queryKey: ['platform-settings'] }),
      ])
    } catch (e) {
      toast.error(`Failed to activate ${provider.name}`, {
        description: e instanceof Error ? e.message : 'Network error',
      })
    } finally {
      setActivating(false)
    }
  }

  const handleTest = async () => {
    setTesting(true)
    setTestResult(null)
    try {
      // project_id=0 is a sentinel — the endpoint doesn't use it. The
      // provider_id query param selects which CLI to probe, so Test buttons
      // on inactive cards still verify the right credential instead of
      // silently testing the active one.
      const res = await fetch(
        `/api/projects/0/agents/smoke-test?provider_id=${encodeURIComponent(provider.id)}`,
        { method: 'POST' }
      )
      if (!res.ok) {
        const detail = await res.text()
        toast.error(`Smoke test failed`, { description: detail.slice(0, 200) })
        return
      }
      const data = await res.json()
      setTestResult({
        passed: data.passed,
        environment: data.environment,
        cli_version: data.cli_version,
        auth_info: data.auth_info,
        setup_hint: data.setup_hint,
      })
      if (data.passed) {
        toast.success(`${provider.name} is ready`)
      } else {
        toast.error(`${provider.name} test failed`, {
          description: data.setup_hint ?? 'See card for details',
        })
      }
    } catch (e) {
      toast.error('Smoke test failed', {
        description: e instanceof Error ? e.message : 'Network error',
      })
    } finally {
      setTesting(false)
    }
  }

  const handleSave = async () => {
    if (!credential.trim()) return
    setSaving(true)
    try {
      const res = await fetch(
        `/api/settings/ai-providers/${provider.id}/credential`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            auth_type: selectedFlavor.id,
            credential: credential.trim(),
          }),
        }
      )
      if (!res.ok) {
        const detail = await res.text()
        toast.error(`Failed to save ${provider.name} credential`, {
          description: detail.slice(0, 200),
        })
        return
      }
      toast.success(`${provider.name} credential encrypted and saved`)
      setCredential('')
      // Refresh the catalog so credential_saved flips to true.
      await queryClient.invalidateQueries({ queryKey: ['ai-provider-catalog'] })
    } catch (e) {
      toast.error(`Failed to save ${provider.name} credential`, {
        description: e instanceof Error ? e.message : 'Network error',
      })
    } finally {
      setSaving(false)
    }
  }

  return (
    <div
      className={`rounded-lg border p-4 space-y-3 transition-colors ${
        isActive ? 'border-primary bg-primary/5' : 'border-border'
      }`}
    >
      {/* Header row: name + active toggle + saved badge */}
      <div className="flex items-start justify-between gap-3">
        <div>
          <h4 className="text-sm font-medium">{provider.name}</h4>
          <div className="text-xs text-muted-foreground space-y-0.5 mt-1">
            <p>
              Install:{' '}
              <code className="bg-muted px-1 rounded">
                {provider.install_command}
              </code>
            </p>
            <p>
              Auth:{' '}
              <code className="bg-muted px-1 rounded">
                {provider.auth_command}
              </code>
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          {provider.credential_saved && (
            <span className="inline-flex items-center gap-1 text-xs text-green-500">
              <CheckCircle2 className="h-3.5 w-3.5" />
              Configured
            </span>
          )}
          {provider.credential_saved ? (
            <>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={handleTest}
                disabled={testing}
              >
                {testing ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
                ) : (
                  <Play className="h-3.5 w-3.5 mr-1.5" />
                )}
                Test
              </Button>
              <Button
                type="button"
                variant={isActive ? 'default' : 'outline'}
                size="sm"
                onClick={handleActivate}
                disabled={isActive || activating}
              >
                {activating ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
                ) : null}
                {isActive ? 'Active' : 'Use this'}
              </Button>
            </>
          ) : (
            <span className="text-xs text-muted-foreground">
              Save a credential to enable
            </span>
          )}
        </div>
      </div>

      {/* Auth flavor selector — only shown when there's more than one option */}
      {provider.auth_flavors.length > 1 && (
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
          {provider.auth_flavors.map((flavor) => (
            <button
              key={flavor.id}
              type="button"
              onClick={() => setSelectedFlavorId(flavor.id)}
              className={`rounded-md border p-2.5 text-left transition-colors ${
                selectedFlavorId === flavor.id
                  ? 'border-primary bg-primary/10'
                  : 'border-border hover:border-primary/50'
              }`}
            >
              <p className="text-xs font-medium">{flavor.label}</p>
              <p className="text-[11px] text-muted-foreground mt-0.5">
                {flavor.description}
              </p>
            </button>
          ))}
        </div>
      )}

      {/* Credential input */}
      <div className="space-y-2">
        <Label htmlFor={`cred-${provider.id}`}>
          {selectedFlavor.label} credential
          {selectedFlavor.env_var && (
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              → injected as {selectedFlavor.env_var}
            </span>
          )}
        </Label>
        <p className="text-xs text-muted-foreground">{selectedFlavor.description}</p>
        {selectedFlavor.format === 'config_file' ? (
          <Textarea
            id={`cred-${provider.id}`}
            placeholder={
              provider.credential_saved
                ? '••••••••••••• (saved — paste a new file body to replace)'
                : 'Paste the full file contents here...'
            }
            value={credential}
            onChange={(e) => setCredential(e.target.value)}
            className="min-h-[120px] font-mono text-xs"
          />
        ) : (
          <Input
            id={`cred-${provider.id}`}
            type="password"
            placeholder={
              provider.credential_saved
                ? '••••••••••••• (saved — paste a new value to replace)'
                : selectedFlavor.format === 'oauth_token'
                  ? 'Paste OAuth token...'
                  : 'Paste API key...'
            }
            value={credential}
            onChange={(e) => setCredential(e.target.value)}
          />
        )}
        <div className="flex justify-end">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={handleSave}
            disabled={saving || !credential.trim()}
          >
            {saving ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
            ) : (
              <Save className="h-3.5 w-3.5 mr-1.5" />
            )}
            Save {provider.name} credential
          </Button>
        </div>
      </div>

      {/* Test result — shown after clicking Test. The smoke-test endpoint runs
          a provider-scoped auth check (claude auth status / codex --version /
          opencode --version) on the host or inside a sandbox depending on
          whether sandbox mode is enabled globally. */}
      {testResult && (
        <div
          className={`rounded-md border p-3 text-xs space-y-1 ${
            testResult.passed
              ? 'border-green-500/30 bg-green-500/5'
              : 'border-red-500/30 bg-red-500/5'
          }`}
        >
          <div className="flex items-center gap-1.5 font-medium">
            {testResult.passed ? (
              <CheckCircle2 className="h-3.5 w-3.5 text-green-500" />
            ) : (
              <XCircle className="h-3.5 w-3.5 text-red-500" />
            )}
            {testResult.passed ? 'Connected' : 'Test failed'}
            <span className="text-muted-foreground font-normal">
              ({testResult.environment})
            </span>
          </div>
          {testResult.cli_version && (
            <p className="text-muted-foreground">
              Version: <code className="bg-muted px-1 rounded">{testResult.cli_version}</code>
            </p>
          )}
          {testResult.auth_info && (
            <p className="text-muted-foreground">Auth: {testResult.auth_info}</p>
          )}
          {testResult.setup_hint && (
            <p className="text-muted-foreground">{testResult.setup_hint}</p>
          )}
        </div>
      )}

      {/* Model — auto-saves to /settings/ai-providers/{id} so changing the
          model never requires re-pasting the credential. OpenCode catalogues
          no models (it picks via its own config), so we hide the dropdown
          and only show the custom-input fallback for it. */}
      <div className="space-y-2 border-t border-border/60 pt-3">
        <div className="flex items-center justify-between">
          <Label htmlFor={`model-${provider.id}`}>Default model</Label>
          {savingModel && (
            <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
              <Loader2 className="h-3 w-3 animate-spin" />
              Saving…
            </span>
          )}
        </div>
        {provider.models.length > 0 && !customMode ? (
          <Select
            value={modelDraft === '' ? '_default' : modelDraft}
            onValueChange={(v) => {
              if (v === '_custom') {
                setCustomMode(true)
                // Don't persist yet — wait for the user to type something.
                return
              }
              const next = v === '_default' ? '' : v
              setModelDraft(next)
              void persistModel(next)
            }}
          >
            <SelectTrigger id={`model-${provider.id}`}>
              <SelectValue placeholder="Use provider default" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="_default">Use provider default</SelectItem>
              {provider.models.map((model) => (
                <SelectItem key={model} value={model}>
                  {model}
                </SelectItem>
              ))}
              <SelectItem value="_custom">Custom model…</SelectItem>
            </SelectContent>
          </Select>
        ) : (
          <div className="flex gap-2">
            <Input
              id={`model-${provider.id}`}
              placeholder={
                provider.id === 'claude_cli'
                  ? 'e.g. claude-sonnet-4-6'
                  : provider.id === 'codex_cli'
                    ? 'e.g. gpt-5-codex'
                    : provider.id === 'opencode'
                      ? 'e.g. anthropic/claude-sonnet-4-6'
                      : 'Model id'
              }
              value={modelDraft}
              onChange={(e) => setModelDraft(e.target.value)}
            />
            {provider.models.length > 0 && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  setCustomMode(false)
                  setModelDraft(serverModel)
                }}
              >
                Cancel
              </Button>
            )}
          </div>
        )}
        <p className="text-[11px] text-muted-foreground">
          {provider.id === 'opencode'
            ? 'OpenCode resolves models via ~/.config/opencode/config.json or per-session flags. Setting a value here exports it to OPENCODE_MODEL.'
            : 'Leave blank to let the CLI pick. Custom values are accepted — the catalog is just a convenience list.'}
        </p>
      </div>
    </div>
  )
}
