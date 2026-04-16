import { useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ArrowLeft,
  Bot,
  Check,
  Cpu,
  GitBranch,
  Loader2,
  Plus,
  Search,
  Sparkles,
} from 'lucide-react'
import { toast } from 'sonner'

import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

import { startSession } from './api'
import {
  listGlobalMcpDefinitions,
  listGlobalSkillDefinitions,
  listMcpDefinitions,
  listSkillDefinitions,
  type McpDefinition,
  type SkillDefinition,
} from '@/components/agents/api'

interface Project {
  id: number
  name: string
  slug: string
  repo_owner?: string
  repo_name?: string
  main_branch?: string
  git_provider_connection_id?: number
}

interface NewSessionPageProps {
  project: Project
}

interface Branch {
  name: string
}

// ── AI provider catalog (subset of /api/settings/ai-providers shape) ─────────
// We only consume the fields needed to render the picker. Full DTO lives in
// `AiProvidersCard.tsx` — keep these compatible.
interface ProviderCatalogEntry {
  id: string
  name: string
  credential_saved: boolean
  /// Models the Rust catalog ships for this provider (convenience list — the
  /// backend accepts arbitrary ids too, so the user can pin a model newer
  /// than what we ship).
  models: string[]
  /// Provider-level default model from platform settings (the value the
  /// user set in Settings → AI Workflows). Empty string or null means the
  /// CLI picks its own default.
  default_model: string | null
}
interface ProviderCatalogResponse {
  default_provider: string
  providers: ProviderCatalogEntry[]
}

function mergeProjectAndGlobal<T extends { slug: string }>(
  projectScoped: T[],
  global: T[],
): T[] {
  return [
    ...projectScoped,
    ...global.filter((g) => !projectScoped.some((p) => p.slug === g.slug)),
  ]
}

// Mirrors the Docker provider defaults at
// crates/temps-agents/src/sandbox/docker.rs (lines ~251-252). When the user
// leaves these unchanged we omit them from the request and the backend
// applies the same defaults — so the inputs are purely informational until
// the user types over them.
const DEFAULT_CPU_LIMIT = 4
const DEFAULT_MEMORY_LIMIT_MB = 8192

export function NewSessionPage({ project }: NewSessionPageProps) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const [selectedBranch, setSelectedBranch] = useState<string>(
    project.main_branch ?? '',
  )
  const [createNewBranch, setCreateNewBranch] = useState(false)
  const [newBranchName, setNewBranchName] = useState('')
  const [selectedProvider, setSelectedProvider] = useState<string | null>(null)
  // `null` = "inherit the provider's configured default_model". A non-null
  // value (even "") is treated as an explicit session-level override — but
  // we never send "" to the backend, we just omit ai_model when null/blank.
  const [selectedModel, setSelectedModel] = useState<string | null>(null)
  const [modelInCustomMode, setModelInCustomMode] = useState(false)
  const [selectedSkills, setSelectedSkills] = useState<string[]>([])
  const [selectedMcpServers, setSelectedMcpServers] = useState<string[]>([])
  const [skillFilter, setSkillFilter] = useState('')
  const [mcpFilter, setMcpFilter] = useState('')
  // Pre-fill with the platform defaults so the inputs are never empty —
  // users can override or leave as-is. Stored as strings to allow partial
  // edits ("8.") without immediately failing parse; we re-parse on submit.
  const [cpuLimit, setCpuLimit] = useState<string>(String(DEFAULT_CPU_LIMIT))
  const [memoryLimitMb, setMemoryLimitMb] = useState<string>(
    String(DEFAULT_MEMORY_LIMIT_MB),
  )
  // Whether the initial "select-all" default has been applied. Users can
  // still deselect everything after that — we only touch this once, when
  // the lists first land.
  const [skillsPrimed, setSkillsPrimed] = useState(false)
  const [mcpsPrimed, setMcpsPrimed] = useState(false)

  const repoConnected =
    !!project.repo_owner &&
    !!project.repo_name &&
    !!project.git_provider_connection_id

  // Fetch the AI provider catalog so we can offer the user the providers
  // they've actually configured. Falls back to the platform default when only
  // one is configured (no picker shown in that case).
  //
  // `staleTime: 0` is deliberate here — the model list inside the catalog
  // can change when we ship a backend update (e.g. dropping deprecated
  // model ids). New-Session is a low-traffic page, so we can afford a
  // fresh fetch each mount instead of showing a stale dropdown from an
  // hour ago. The shared settings page uses a longer staleTime; they
  // share the same query key, so if settings has a fresh copy we'll
  // still reuse it without a second round-trip.
  const providerCatalogQuery = useQuery({
    queryKey: ['ai-provider-catalog'],
    queryFn: async () => {
      const res = await fetch('/api/settings/ai-providers')
      if (!res.ok)
        throw new Error(`Failed to load provider catalog: ${res.status}`)
      return res.json() as Promise<ProviderCatalogResponse>
    },
    retry: false,
    staleTime: 0,
  })
  const configuredProviders: ProviderCatalogEntry[] = useMemo(
    () =>
      (providerCatalogQuery.data?.providers ?? []).filter(
        (p) => p.credential_saved,
      ),
    [providerCatalogQuery.data],
  )

  // Default the picker to the platform-active provider once the catalog
  // lands. If the active provider isn't configured (shouldn't happen, since
  // activation requires a credential — but a stale tab could see this),
  // fall back to the first configured one.
  useEffect(() => {
    if (selectedProvider !== null) return
    if (!providerCatalogQuery.data) return
    const active = providerCatalogQuery.data.default_provider
    const activeOk = configuredProviders.some((p) => p.id === active)
    if (activeOk) {
      setSelectedProvider(active)
    } else if (configuredProviders.length > 0) {
      setSelectedProvider(configuredProviders[0].id)
    }
  }, [providerCatalogQuery.data, configuredProviders, selectedProvider])

  // Currently-selected provider's catalog entry. Used to scope the model
  // dropdown so we never offer Claude models when Codex is the active
  // provider (their model id namespaces are disjoint).
  const selectedProviderEntry = useMemo(
    () => configuredProviders.find((p) => p.id === selectedProvider) ?? null,
    [configuredProviders, selectedProvider],
  )

  // Reset model override when the user picks a different provider — a
  // Claude model id has no meaning under Codex.
  useEffect(() => {
    setSelectedModel(null)
    setModelInCustomMode(false)
  }, [selectedProvider])

  // Fetch branches (only when repo is connected)
  const branchesQuery = useQuery({
    queryKey: [
      'workspace-branches',
      project.repo_owner,
      project.repo_name,
      project.git_provider_connection_id,
    ],
    queryFn: async () => {
      const res = await fetch(
        `/api/repositories/${encodeURIComponent(project.repo_owner!)}/${encodeURIComponent(project.repo_name!)}/branches?connection_id=${project.git_provider_connection_id}`,
      )
      if (!res.ok) throw new Error(`Failed to load branches: ${res.status}`)
      return res.json() as Promise<{ branches: Branch[] }>
    },
    enabled: repoConnected,
  })

  // Skip TanStack's default 3-retry/exponential-backoff: an unauth'd or
  // missing-endpoint response won't succeed on retry, and we'd rather show
  // an empty state than spin the "Loading…" spinner for ~7s.
  const projectSkillsQuery = useQuery({
    queryKey: ['skill-definitions', project.id],
    queryFn: () => listSkillDefinitions(project.id),
    retry: false,
  })
  const globalSkillsQuery = useQuery({
    queryKey: ['global-skills'],
    queryFn: () => listGlobalSkillDefinitions(),
    retry: false,
  })
  const projectMcpsQuery = useQuery({
    queryKey: ['mcp-definitions', project.id],
    queryFn: () => listMcpDefinitions(project.id),
    retry: false,
  })
  const globalMcpsQuery = useQuery({
    queryKey: ['global-mcp-servers'],
    queryFn: () => listGlobalMcpDefinitions(),
    retry: false,
  })

  const availableSkills: SkillDefinition[] = useMemo(
    () =>
      mergeProjectAndGlobal(
        projectSkillsQuery.data ?? [],
        globalSkillsQuery.data ?? [],
      ),
    [projectSkillsQuery.data, globalSkillsQuery.data],
  )
  const availableMcps: McpDefinition[] = useMemo(
    () =>
      mergeProjectAndGlobal(
        projectMcpsQuery.data ?? [],
        globalMcpsQuery.data ?? [],
      ),
    [projectMcpsQuery.data, globalMcpsQuery.data],
  )

  // Default to all skills / MCP servers selected once their lists land.
  // Fires exactly once per list; users can uncheck from there.
  useEffect(() => {
    if (skillsPrimed) return
    if (projectSkillsQuery.isPending || globalSkillsQuery.isPending) return
    setSelectedSkills(availableSkills.map((s) => s.slug))
    setSkillsPrimed(true)
  }, [
    skillsPrimed,
    projectSkillsQuery.isPending,
    globalSkillsQuery.isPending,
    availableSkills,
  ])
  useEffect(() => {
    if (mcpsPrimed) return
    if (projectMcpsQuery.isPending || globalMcpsQuery.isPending) return
    setSelectedMcpServers(availableMcps.map((m) => m.slug))
    setMcpsPrimed(true)
  }, [
    mcpsPrimed,
    projectMcpsQuery.isPending,
    globalMcpsQuery.isPending,
    availableMcps,
  ])

  const allSkillsSelected =
    availableSkills.length > 0 &&
    selectedSkills.length === availableSkills.length
  const allMcpsSelected =
    availableMcps.length > 0 &&
    selectedMcpServers.length === availableMcps.length
  const toggleAllSkills = () =>
    setSelectedSkills(allSkillsSelected ? [] : availableSkills.map((s) => s.slug))
  const toggleAllMcps = () =>
    setSelectedMcpServers(
      allMcpsSelected ? [] : availableMcps.map((m) => m.slug),
    )

  const filteredSkills = useMemo(() => {
    const q = skillFilter.trim().toLowerCase()
    if (!q) return availableSkills
    return availableSkills.filter(
      (s) =>
        s.slug.toLowerCase().includes(q) ||
        (s.name ?? '').toLowerCase().includes(q) ||
        (s.description ?? '').toLowerCase().includes(q),
    )
  }, [availableSkills, skillFilter])
  const filteredMcps = useMemo(() => {
    const q = mcpFilter.trim().toLowerCase()
    if (!q) return availableMcps
    return availableMcps.filter(
      (m) =>
        m.slug.toLowerCase().includes(q) ||
        (m.name ?? '').toLowerCase().includes(q) ||
        (m.description ?? '').toLowerCase().includes(q),
    )
  }, [availableMcps, mcpFilter])

  const toggleSkill = (slug: string) =>
    setSelectedSkills((prev) =>
      prev.includes(slug) ? prev.filter((s) => s !== slug) : [...prev, slug],
    )
  const toggleMcp = (slug: string) =>
    setSelectedMcpServers((prev) =>
      prev.includes(slug) ? prev.filter((s) => s !== slug) : [...prev, slug],
    )

  const createSession = useMutation({
    mutationFn: () => {
      const base = selectedBranch.trim()
      const body: Parameters<typeof startSession>[1] = {}
      if (createNewBranch) {
        const newBranch = newBranchName.trim()
        if (!newBranch) throw new Error('Enter a name for the new branch')
        if (!base) throw new Error('Pick a base branch to fork from')
        body.branch_name = newBranch
        body.base_branch_name = base
      } else if (base) {
        body.branch_name = base
      }
      if (selectedSkills.length > 0) body.skills = selectedSkills
      if (selectedMcpServers.length > 0) body.mcp_servers = selectedMcpServers
      if (selectedProvider) body.ai_provider = selectedProvider
      // Only send ai_model when the user explicitly chose one. An empty
      // string would mean "clear it" on the backend — we want "omit" to
      // fall through to the provider's configured default_model.
      if (selectedModel && selectedModel.trim() !== '') {
        body.ai_model = selectedModel.trim()
      }

      // Sandbox resources: send whatever the user sees in the input — even
      // when it matches the UI's own DEFAULT constants. Previously we
      // omitted the field in that case and the backend fell through to its
      // *own* configured default (AgentSandboxSettings.memory_limit_mb),
      // which on some installs is 2048 MB. The form then displayed "8192"
      // but the container actually got 2 GB — a silent mismatch between
      // what the user saw and what the sandbox enforced. An empty field
      // still means "use the platform default" (deliberate escape hatch).
      const cpuStr = cpuLimit.trim()
      if (cpuStr !== '') {
        const parsed = Number(cpuStr)
        if (!Number.isFinite(parsed) || parsed <= 0) {
          throw new Error('CPU limit must be a positive number')
        }
        body.cpu_limit = parsed
      }
      const memStr = memoryLimitMb.trim()
      if (memStr !== '') {
        const parsed = Number(memStr)
        if (!Number.isInteger(parsed) || parsed <= 0) {
          throw new Error('Memory limit must be a positive integer (MB)')
        }
        body.memory_limit_mb = parsed
      }
      return startSession(project.id, body)
    },
    onSuccess: (session) => {
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'sessions'],
      })
      toast.success(
        session.branch_name
          ? `Workspace session started on ${session.branch_name}`
          : 'Workspace session started',
      )
      navigate(
        { pathname: '../', search: `?session=${session.id}` },
        { relative: 'path' },
      )
    },
    onError: (e: Error) => toast.error(e.message),
  })

  return (
    <div className="mx-auto max-w-4xl w-full p-4 md:p-6 space-y-6">
      <div className="flex items-center gap-3">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => navigate('../', { relative: 'path' })}
        >
          <ArrowLeft className="h-4 w-4 mr-1" />
          Back
        </Button>
        <div>
          <h1 className="text-xl font-semibold">Start a new workspace session</h1>
          <p className="text-sm text-muted-foreground">
            Pick the branch, skills, and MCP servers the sandbox should ship with.
          </p>
        </div>
      </div>

      {/* AI provider + model picker.
          - 0 providers: tell the user to configure one (same state as before).
          - 1 provider: read-only summary showing exactly what will run. No
            picker — there's nothing to pick from. Still shows the model for
            transparency so the user knows before hitting Start.
          - >1 providers: full picker + per-session model override that
            defaults to the provider's configured default_model. */}
      {providerCatalogQuery.isPending ? null : configuredProviders.length === 0 ? (
        <Card className="p-4 space-y-2">
          <h2 className="text-sm font-semibold flex items-center gap-2">
            <Bot className="h-4 w-4" />
            AI provider
          </h2>
          <p className="text-sm text-muted-foreground">
            No AI provider is configured.{' '}
            <a href="/settings/ai-workflows" className="underline">
              Configure one
            </a>{' '}
            before starting a session.
          </p>
        </Card>
      ) : configuredProviders.length === 1 ? (
        (() => {
          // Single configured provider — read-only. The model displayed is
          // the provider's configured default (what the session will actually
          // use), or a hint when none is set.
          const only = configuredProviders[0]
          const effectiveModel =
            only.default_model && only.default_model.trim() !== ''
              ? only.default_model
              : null
          return (
            <Card className="p-4 space-y-2">
              <h2 className="text-sm font-semibold flex items-center gap-2">
                <Bot className="h-4 w-4" />
                AI provider
              </h2>
              <div className="text-sm">
                <p>
                  <span className="font-medium">{only.name}</span>
                  <span className="text-muted-foreground"> ({only.id})</span>
                </p>
                <p className="text-xs text-muted-foreground mt-1">
                  Model:{' '}
                  {effectiveModel ? (
                    <code className="text-[11px] bg-muted px-1 rounded">
                      {effectiveModel}
                    </code>
                  ) : (
                    <span className="italic">
                      provider default (unset — the CLI picks)
                    </span>
                  )}
                </p>
              </div>
              <p className="text-[11px] text-muted-foreground">
                Only one provider is configured.{' '}
                <a
                  href="/settings/ai-workflows"
                  className="underline"
                >
                  Configure another
                </a>{' '}
                to choose at session-start time.
              </p>
            </Card>
          )
        })()
      ) : (
        <Card className="p-4 space-y-4">
          <div>
            <h2 className="text-sm font-semibold flex items-center gap-2">
              <Bot className="h-4 w-4" />
              AI provider
            </h2>
            <p className="text-xs text-muted-foreground">
              Which CLI runs inside the sandbox, and which model to use.
              Only providers with a saved credential are shown.
            </p>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
            {configuredProviders.map((p) => {
              const active = selectedProvider === p.id
              return (
                <button
                  type="button"
                  key={p.id}
                  onClick={() => setSelectedProvider(p.id)}
                  className={`rounded-md border p-3 text-left transition-colors ${
                    active
                      ? 'border-primary bg-primary/10'
                      : 'border-border hover:border-primary/50'
                  }`}
                >
                  <p className="text-sm font-medium">{p.name}</p>
                  <p className="text-[11px] text-muted-foreground mt-0.5">
                    {p.id}
                    {p.id === providerCatalogQuery.data?.default_provider
                      ? ' · default'
                      : ''}
                    {p.default_model ? ` · ${p.default_model}` : ''}
                  </p>
                </button>
              )
            })}
          </div>

          {/* Model dropdown for the selected provider. Scoped to that
              provider's catalog so we never offer Claude models when
              Codex is active. `selectedModel === null` means "inherit the
              provider's configured default_model" (rendered as "use default"
              in the dropdown). */}
          {selectedProviderEntry && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium">Model</label>
              {selectedProviderEntry.models.length > 0 && !modelInCustomMode ? (
                <Select
                  value={selectedModel === null ? '_default' : selectedModel}
                  onValueChange={(v) => {
                    if (v === '_custom') {
                      setModelInCustomMode(true)
                      setSelectedModel('')
                      return
                    }
                    setSelectedModel(v === '_default' ? null : v)
                  }}
                >
                  {/* Width must fit the longest label — "Use provider
                      default (gpt-5-codex)" is ~36ch. 420px covers that
                      plus the chevron without wrapping or truncating. */}
                  <SelectTrigger className="w-full sm:w-[420px]">
                    <SelectValue placeholder="Use provider default" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="_default">
                      Use provider default
                      {selectedProviderEntry.default_model
                        ? ` (${selectedProviderEntry.default_model})`
                        : ''}
                    </SelectItem>
                    {selectedProviderEntry.models.map((m) => (
                      <SelectItem key={m} value={m}>
                        {m}
                      </SelectItem>
                    ))}
                    <SelectItem value="_custom">Custom model…</SelectItem>
                  </SelectContent>
                </Select>
              ) : (
                <div className="flex gap-2">
                  <Input
                    value={selectedModel ?? ''}
                    onChange={(e) => setSelectedModel(e.target.value)}
                    placeholder={
                      selectedProviderEntry.default_model
                        ? `${selectedProviderEntry.default_model} (default)`
                        : selectedProviderEntry.id === 'claude_cli'
                          ? 'e.g. claude-sonnet-4-6'
                          : selectedProviderEntry.id === 'codex_cli'
                            ? 'e.g. gpt-5-codex'
                            : selectedProviderEntry.id === 'opencode'
                              ? 'e.g. anthropic/claude-sonnet-4-6'
                              : 'Model id'
                    }
                    className="w-full sm:w-[420px]"
                  />
                  {selectedProviderEntry.models.length > 0 && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        setModelInCustomMode(false)
                        setSelectedModel(null)
                      }}
                    >
                      Cancel
                    </Button>
                  )}
                </div>
              )}
              <p className="text-[11px] text-muted-foreground">
                Leave on default to inherit{' '}
                <code className="text-[11px]">
                  {selectedProviderEntry.default_model || 'CLI default'}
                </code>
                . Session-level override only applies to this session.
              </p>
            </div>
          )}
        </Card>
      )}

      {/* Sandbox resources */}
      <Card className="p-4 space-y-4">
        <div>
          <h2 className="text-sm font-semibold flex items-center gap-2">
            <Cpu className="h-4 w-4" />
            Sandbox resources
          </h2>
          <p className="text-xs text-muted-foreground">
            CPU and memory granted to the sandbox container. Defaults:{' '}
            <code className="text-[11px]">{DEFAULT_CPU_LIMIT} cores</code>,{' '}
            <code className="text-[11px]">{DEFAULT_MEMORY_LIMIT_MB} MB</code>.
          </p>
        </div>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
          <div className="space-y-1.5">
            <label htmlFor="cpu-limit" className="text-xs font-medium">
              CPU (vCPU cores)
            </label>
            <Input
              id="cpu-limit"
              type="number"
              inputMode="decimal"
              min="0.5"
              step="0.5"
              value={cpuLimit}
              onChange={(e) => setCpuLimit(e.target.value)}
              placeholder={String(DEFAULT_CPU_LIMIT)}
              className="w-full sm:w-[200px]"
            />
            <p className="text-[11px] text-muted-foreground">
              e.g. <code className="text-[11px]">2</code> for 2 vCPUs.
            </p>
          </div>
          <div className="space-y-1.5">
            <label htmlFor="memory-limit" className="text-xs font-medium">
              Memory (MB)
            </label>
            <Input
              id="memory-limit"
              type="number"
              inputMode="numeric"
              min="256"
              step="256"
              value={memoryLimitMb}
              onChange={(e) => setMemoryLimitMb(e.target.value)}
              placeholder={String(DEFAULT_MEMORY_LIMIT_MB)}
              className="w-full sm:w-[200px]"
            />
            <p className="text-[11px] text-muted-foreground">
              e.g. <code className="text-[11px]">4096</code> for 4 GB.
            </p>
          </div>
        </div>
      </Card>

      {/* Branch */}
      <Card className="p-4 space-y-4">
        <div>
          <h2 className="text-sm font-semibold">Branch</h2>
          <p className="text-xs text-muted-foreground">
            The repo is cloned into a fresh sandbox checked out at this branch.
          </p>
        </div>

        {!repoConnected ? (
          <p className="text-sm text-muted-foreground">
            No git repository connected to this project — the session will
            start with an empty workspace.
          </p>
        ) : branchesQuery.isPending ? (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            Loading branches…
          </div>
        ) : branchesQuery.isError ? (
          <p className="text-sm text-destructive">
            Failed to load branches. Falls back to project default.
          </p>
        ) : (
          <div className="space-y-3">
            <div className="space-y-1.5">
              <label className="text-xs font-medium">
                {createNewBranch ? 'Base branch (fork from)' : 'Branch'}
              </label>
              <Select value={selectedBranch} onValueChange={setSelectedBranch}>
                <SelectTrigger className="w-full sm:w-[320px]">
                  <SelectValue placeholder="Select a branch" />
                </SelectTrigger>
                <SelectContent>
                  {branchesQuery.data?.branches.map((b) => (
                    <SelectItem key={b.name} value={b.name}>
                      <span className="inline-flex items-center gap-2">
                        <GitBranch className="h-3 w-3" />
                        {b.name}
                        {b.name === project.main_branch ? ' (default)' : ''}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="flex items-center gap-2">
              <Checkbox
                id="create-new-branch"
                checked={createNewBranch}
                onCheckedChange={(v) => setCreateNewBranch(v === true)}
              />
              <label
                htmlFor="create-new-branch"
                className="text-sm cursor-pointer select-none"
              >
                Create a new branch off this one
              </label>
            </div>

            {createNewBranch && (
              <div className="space-y-1">
                <label className="text-xs font-medium">New branch name</label>
                <Input
                  value={newBranchName}
                  onChange={(e) => setNewBranchName(e.target.value)}
                  placeholder="feature/my-change"
                  className="w-full sm:w-[320px]"
                />
                <p className="text-xs text-muted-foreground">
                  Created locally inside the sandbox — not pushed until you
                  (or the AI) push it.
                </p>
              </div>
            )}
          </div>
        )}
      </Card>

      {/* Skills */}
      <Card className="p-4 space-y-3">
        <div className="flex items-center justify-between gap-2">
          <div>
            <h2 className="text-sm font-semibold">Skills</h2>
            <p className="text-xs text-muted-foreground">
              Written to <code className="text-[11px]">.claude/skills/</code>.
              {' '}Claude auto-discovers them on the next run.
            </p>
          </div>
          <div className="flex items-center gap-3 shrink-0">
            {availableSkills.length > 0 && (
              <button
                type="button"
                onClick={toggleAllSkills}
                className="text-xs text-muted-foreground hover:text-foreground underline underline-offset-2 decoration-dotted"
              >
                {allSkillsSelected ? 'Deselect all' : 'Select all'}
              </button>
            )}
            <span className="text-xs text-muted-foreground">
              {selectedSkills.length} selected
            </span>
          </div>
        </div>
        <div className="relative">
          <Search className="absolute left-2 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground" />
          <Input
            value={skillFilter}
            onChange={(e) => setSkillFilter(e.target.value)}
            placeholder="Filter skills…"
            className="pl-8 h-8 text-sm"
          />
        </div>
        {projectSkillsQuery.isPending || globalSkillsQuery.isPending ? (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            Loading…
          </div>
        ) : projectSkillsQuery.isError && globalSkillsQuery.isError ? (
          <p className="text-sm text-destructive">
            Failed to load skills. Check you're signed in and reload the page.
          </p>
        ) : availableSkills.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No skills defined yet. Add one in{' '}
            <a href="/settings/skills" className="underline">
              Settings → Skills
            </a>
            .
          </p>
        ) : filteredSkills.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No skills match "{skillFilter}".
          </p>
        ) : (
          <div className="rounded border divide-y max-h-[360px] overflow-y-auto">
            {filteredSkills.map((s) => {
              const selected = selectedSkills.includes(s.slug)
              return (
                <button
                  type="button"
                  key={s.slug}
                  onClick={() => toggleSkill(s.slug)}
                  className={`w-full text-left flex items-start gap-3 px-3 py-2 text-sm hover:bg-muted/40 ${selected ? 'bg-muted/60' : ''}`}
                >
                  <span
                    className={`mt-0.5 inline-flex items-center justify-center h-4 w-4 rounded border ${selected ? 'bg-primary border-primary text-primary-foreground' : 'border-muted-foreground/40'}`}
                  >
                    {selected && <Check className="h-3 w-3" />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <Sparkles className="h-3 w-3 text-muted-foreground shrink-0" />
                      <span className="font-mono text-xs truncate">{s.slug}</span>
                      {s.name && (
                        <span className="text-xs text-muted-foreground truncate">
                          — {s.name}
                        </span>
                      )}
                    </div>
                    {s.description && (
                      <p className="text-xs text-muted-foreground mt-0.5 line-clamp-2">
                        {s.description}
                      </p>
                    )}
                  </div>
                </button>
              )
            })}
          </div>
        )}
      </Card>

      {/* MCP servers */}
      <Card className="p-4 space-y-3">
        <div className="flex items-center justify-between gap-2">
          <div>
            <h2 className="text-sm font-semibold">MCP servers</h2>
            <p className="text-xs text-muted-foreground">
              Merged into <code className="text-[11px]">.claude/settings.json</code>{' '}
              and <code className="text-[11px]">~/.claude.json</code>.
            </p>
          </div>
          <div className="flex items-center gap-3 shrink-0">
            {availableMcps.length > 0 && (
              <button
                type="button"
                onClick={toggleAllMcps}
                className="text-xs text-muted-foreground hover:text-foreground underline underline-offset-2 decoration-dotted"
              >
                {allMcpsSelected ? 'Deselect all' : 'Select all'}
              </button>
            )}
            <span className="text-xs text-muted-foreground">
              {selectedMcpServers.length} selected
            </span>
          </div>
        </div>
        <div className="relative">
          <Search className="absolute left-2 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground" />
          <Input
            value={mcpFilter}
            onChange={(e) => setMcpFilter(e.target.value)}
            placeholder="Filter MCP servers…"
            className="pl-8 h-8 text-sm"
          />
        </div>
        {projectMcpsQuery.isPending || globalMcpsQuery.isPending ? (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            Loading…
          </div>
        ) : projectMcpsQuery.isError && globalMcpsQuery.isError ? (
          <p className="text-sm text-destructive">
            Failed to load MCP servers. Check you're signed in and reload the page.
          </p>
        ) : availableMcps.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No MCP servers defined yet. Add one in{' '}
            <a href="/settings/mcp-servers" className="underline">
              Settings → MCP servers
            </a>
            .
          </p>
        ) : filteredMcps.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No MCP servers match "{mcpFilter}".
          </p>
        ) : (
          <div className="rounded border divide-y max-h-[360px] overflow-y-auto">
            {filteredMcps.map((m) => {
              const selected = selectedMcpServers.includes(m.slug)
              return (
                <button
                  type="button"
                  key={m.slug}
                  onClick={() => toggleMcp(m.slug)}
                  className={`w-full text-left flex items-start gap-3 px-3 py-2 text-sm hover:bg-muted/40 ${selected ? 'bg-muted/60' : ''}`}
                >
                  <span
                    className={`mt-0.5 inline-flex items-center justify-center h-4 w-4 rounded border ${selected ? 'bg-primary border-primary text-primary-foreground' : 'border-muted-foreground/40'}`}
                  >
                    {selected && <Check className="h-3 w-3" />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-xs text-muted-foreground shrink-0">
                        {'{mcp}'}
                      </span>
                      <span className="font-mono text-xs truncate">{m.slug}</span>
                      {m.name && (
                        <span className="text-xs text-muted-foreground truncate">
                          — {m.name}
                        </span>
                      )}
                    </div>
                    {m.description && (
                      <p className="text-xs text-muted-foreground mt-0.5 line-clamp-2">
                        {m.description}
                      </p>
                    )}
                  </div>
                </button>
              )
            })}
          </div>
        )}
      </Card>

      {/* Sticky action bar */}
      <div className="sticky bottom-0 bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/80 border-t -mx-4 md:-mx-6 px-4 md:px-6 py-3 flex items-center justify-between gap-3">
        <div className="text-xs text-muted-foreground">
          {selectedSkills.length > 0 || selectedMcpServers.length > 0 ? (
            <span>
              Injecting {selectedSkills.length} skill(s),{' '}
              {selectedMcpServers.length} MCP server(s).
            </span>
          ) : (
            <span>No skills or MCP servers selected.</span>
          )}
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            onClick={() => navigate('../', { relative: 'path' })}
            disabled={createSession.isPending}
          >
            Cancel
          </Button>
          <Button
            onClick={() => createSession.mutate()}
            disabled={createSession.isPending}
          >
            {createSession.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin mr-1" />
            ) : (
              <Plus className="h-4 w-4 mr-1" />
            )}
            Start session
          </Button>
        </div>
      </div>
    </div>
  )
}
