import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
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
import { Textarea } from '@/components/ui/textarea'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import {
  createAgent,
  updateAgent,
  type Agent,
  type CreateAgentRequest,
  type UpdateAgentRequest,
} from './api'

interface AgentSettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  projectId: number
  agent?: Agent | null
}

export function AgentSettingsDialog({
  open,
  onOpenChange,
  projectId,
  agent,
}: AgentSettingsDialogProps) {
  const isEdit = !!agent
  const queryClient = useQueryClient()

  const [slug, setSlug] = useState(agent?.slug ?? '')
  const [name, setName] = useState(agent?.name ?? '')
  const [description, setDescription] = useState(agent?.description ?? '')
  const [enabled, setEnabled] = useState(agent?.enabled ?? false)
  const [aiProvider, setAiProvider] = useState(agent?.ai_provider ?? 'claude_cli')
  const [prompt, setPrompt] = useState(agent?.prompt ?? '')
  const [maxTurns, setMaxTurns] = useState(agent?.max_turns ?? 25)
  const [timeoutSeconds, setTimeoutSeconds] = useState(agent?.timeout_seconds ?? 600)
  const [dailyBudgetCents, setDailyBudgetCents] = useState(agent?.daily_budget_cents ?? 500)
  const [cooldownMinutes, setCooldownMinutes] = useState(agent?.cooldown_minutes ?? 30)
  const [branchPrefix, setBranchPrefix] = useState(agent?.branch_prefix ?? 'agents/')
  const [deliverable, setDeliverable] = useState(agent?.deliverable ?? 'pull_request')
  const [sandboxEnabled, setSandboxEnabled] = useState<boolean | null>(agent?.sandbox_enabled ?? null)
  const [configRepoUrl, setConfigRepoUrl] = useState(agent?.config_repo_url ?? '')
  const [configRepoBranch, setConfigRepoBranch] = useState(agent?.config_repo_branch ?? '')

  // Triggers
  const [triggerNewIssue, setTriggerNewIssue] = useState(
    agent?.trigger_config?.error?.new_issue ?? true,
  )
  const [triggerRegression, setTriggerRegression] = useState(
    agent?.trigger_config?.error?.regression ?? true,
  )
  const [triggerManual, setTriggerManual] = useState(
    agent?.trigger_config?.manual ?? true,
  )
  const [triggerCron, setTriggerCron] = useState(
    agent?.trigger_config?.schedule?.cron ?? '',
  )

  // Reset form when dialog opens with different agent
  const resetForm = () => {
    setSlug(agent?.slug ?? '')
    setName(agent?.name ?? '')
    setDescription(agent?.description ?? '')
    setEnabled(agent?.enabled ?? false)
    setAiProvider(agent?.ai_provider ?? 'claude_cli')
    setPrompt(agent?.prompt ?? '')
    setMaxTurns(agent?.max_turns ?? 25)
    setTimeoutSeconds(agent?.timeout_seconds ?? 600)
    setDailyBudgetCents(agent?.daily_budget_cents ?? 500)
    setCooldownMinutes(agent?.cooldown_minutes ?? 30)
    setBranchPrefix(agent?.branch_prefix ?? 'agents/')
    setDeliverable(agent?.deliverable ?? 'pull_request')
    setSandboxEnabled(agent?.sandbox_enabled ?? null)
    setConfigRepoUrl(agent?.config_repo_url ?? '')
    setConfigRepoBranch(agent?.config_repo_branch ?? '')
    setTriggerNewIssue(agent?.trigger_config?.error?.new_issue ?? true)
    setTriggerRegression(agent?.trigger_config?.error?.regression ?? true)
    setTriggerManual(agent?.trigger_config?.manual ?? true)
    setTriggerCron(agent?.trigger_config?.schedule?.cron ?? '')
  }

  // Reset form when agent changes (opening dialog for a different agent)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { resetForm() }, [agent?.id, open])

  const createMutation = useMutation({
    mutationFn: (data: CreateAgentRequest) => createAgent(projectId, data),
    onSuccess: () => {
      toast.success('Agent created')
      queryClient.invalidateQueries({ queryKey: ['agents', projectId] })
      onOpenChange(false)
    },
    onError: (error: Error) => {
      toast.error(error.message || 'Failed to create agent')
    },
  })

  const updateMutation = useMutation({
    mutationFn: (data: UpdateAgentRequest) =>
      updateAgent(projectId, agent!.slug, data),
    onSuccess: () => {
      toast.success('Agent updated')
      queryClient.invalidateQueries({ queryKey: ['agents', projectId] })
      onOpenChange(false)
    },
    onError: (error: Error) => {
      toast.error(error.message || 'Failed to update agent')
    },
  })

  const isPending = createMutation.isPending || updateMutation.isPending

  const handleSubmit = () => {
    const triggerConfig: Record<string, unknown> = {
      error: { new_issue: triggerNewIssue, regression: triggerRegression },
      manual: triggerManual,
    }
    if (triggerCron.trim()) {
      triggerConfig.schedule = { cron: triggerCron.trim() }
    }

    if (isEdit) {
      updateMutation.mutate({
        name,
        description: description || undefined,
        enabled,
        ai_provider: aiProvider,
        trigger_config: triggerConfig,
        prompt: prompt || undefined,
        max_turns: maxTurns,
        timeout_seconds: timeoutSeconds,
        daily_budget_cents: dailyBudgetCents,
        cooldown_minutes: cooldownMinutes,
        branch_prefix: branchPrefix,
        deliverable,
        sandbox_enabled: sandboxEnabled ?? undefined,
        config_repo_url: configRepoUrl || null,
        config_repo_branch: configRepoBranch || null,
      })
    } else {
      if (!slug.trim()) {
        toast.error('Slug is required')
        return
      }
      if (!name.trim()) {
        toast.error('Name is required')
        return
      }
      createMutation.mutate({
        slug: slug.trim(),
        name: name.trim(),
        description: description || undefined,
        enabled,
        ai_provider: aiProvider,
        trigger_config: triggerConfig,
        prompt: prompt || undefined,
        max_turns: maxTurns,
        timeout_seconds: timeoutSeconds,
        daily_budget_cents: dailyBudgetCents,
        cooldown_minutes: cooldownMinutes,
        branch_prefix: branchPrefix,
        deliverable,
        sandbox_enabled: sandboxEnabled ?? undefined,
        config_repo_url: configRepoUrl || undefined,
        config_repo_branch: configRepoBranch || undefined,
      })
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (v) resetForm()
        onOpenChange(v)
      }}
    >
      <DialogContent className="max-w-lg max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{isEdit ? 'Edit Agent' : 'Create Agent'}</DialogTitle>
        </DialogHeader>

        <form onSubmit={(e) => { e.preventDefault(); handleSubmit() }} className="space-y-6 py-4">
          {/* Basic info */}
          <div className="space-y-3">
            {!isEdit && (
              <div className="space-y-1.5">
                <Label htmlFor="slug">Slug</Label>
                <Input
                  id="slug"
                  value={slug}
                  onChange={(e) => setSlug(e.target.value)}
                  placeholder="error-fixer"
                />
                <p className="text-xs text-muted-foreground">
                  Unique identifier. Used in URLs and YAML config.
                </p>
              </div>
            )}
            <div className="space-y-1.5">
              <Label htmlFor="name">Name</Label>
              <Input
                id="name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Error Fixer"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="description">Description</Label>
              <Input
                id="description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Automatically fixes production errors"
              />
            </div>
            <div className="flex items-center justify-between">
              <Label htmlFor="enabled">Enabled</Label>
              <Switch
                id="enabled"
                checked={enabled}
                onCheckedChange={setEnabled}
              />
            </div>
          </div>

          {/* Triggers */}
          <div className="space-y-3">
            <h3 className="text-sm font-medium">Triggers</h3>
            <div className="flex items-center justify-between">
              <Label htmlFor="trigger-new-issue" className="text-sm font-normal">
                New errors
              </Label>
              <Switch
                id="trigger-new-issue"
                checked={triggerNewIssue}
                onCheckedChange={setTriggerNewIssue}
              />
            </div>
            <div className="flex items-center justify-between">
              <Label htmlFor="trigger-regression" className="text-sm font-normal">
                Regressions
              </Label>
              <Switch
                id="trigger-regression"
                checked={triggerRegression}
                onCheckedChange={setTriggerRegression}
              />
            </div>
            <div className="flex items-center justify-between">
              <Label htmlFor="trigger-manual" className="text-sm font-normal">
                Manual trigger
              </Label>
              <Switch
                id="trigger-manual"
                checked={triggerManual}
                onCheckedChange={setTriggerManual}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="trigger-cron" className="text-sm font-normal">
                Schedule (cron)
              </Label>
              <Input
                id="trigger-cron"
                placeholder="e.g. 0 * * * * (every hour)"
                value={triggerCron}
                onChange={(e) => setTriggerCron(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                Leave empty for no schedule. Uses standard cron syntax.
              </p>
            </div>
          </div>

          {/* Deliverable */}
          <div className="space-y-3">
            <h3 className="text-sm font-medium">Deliverable</h3>
            <Select value={deliverable} onValueChange={setDeliverable}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="pull_request">Pull Request</SelectItem>
                <SelectItem value="report">Report</SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              {deliverable === 'report'
                ? 'Agent produces a report — no branch, PR, or deployment.'
                : 'Agent pushes a branch, creates a PR, and triggers a preview deployment.'}
            </p>
          </div>

          {/* AI Provider */}
          <div className="space-y-3">
            <h3 className="text-sm font-medium">AI Provider</h3>
            <div className="space-y-1.5">
              <Label htmlFor="ai-provider">Provider</Label>
              <Select value={aiProvider} onValueChange={setAiProvider}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="claude_cli">Claude Code</SelectItem>
                  <SelectItem value="opencode">OpenCode</SelectItem>
                  <SelectItem value="codex_cli">Codex</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="prompt">Custom prompt</Label>
              <Textarea
                id="prompt"
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                placeholder="Leave empty to use the default prompt for the trigger type"
                rows={3}
              />
            </div>
          </div>

          {/* Limits */}
          <div className="space-y-3">
            <h3 className="text-sm font-medium">Limits</h3>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label htmlFor="max-turns">Max turns</Label>
                <Input
                  id="max-turns"
                  type="number"
                  value={maxTurns}
                  onChange={(e) => setMaxTurns(parseInt(e.target.value) || 0)}
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="timeout">Timeout (sec)</Label>
                <Input
                  id="timeout"
                  type="number"
                  value={timeoutSeconds}
                  onChange={(e) =>
                    setTimeoutSeconds(parseInt(e.target.value) || 0)
                  }
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="budget">Daily budget (cents)</Label>
                <Input
                  id="budget"
                  type="number"
                  value={dailyBudgetCents}
                  onChange={(e) =>
                    setDailyBudgetCents(parseInt(e.target.value) || 0)
                  }
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="cooldown">Cooldown (min)</Label>
                <Input
                  id="cooldown"
                  type="number"
                  value={cooldownMinutes}
                  onChange={(e) =>
                    setCooldownMinutes(parseInt(e.target.value) || 0)
                  }
                />
              </div>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="branch-prefix">Branch prefix</Label>
              <Input
                id="branch-prefix"
                value={branchPrefix}
                onChange={(e) => setBranchPrefix(e.target.value)}
                placeholder="agents/"
              />
            </div>
          </div>

          {/* Sandbox */}
          <div className="space-y-3">
            <h3 className="text-sm font-medium">Sandbox</h3>
            <div className="space-y-1.5">
              <Label htmlFor="sandbox-mode" className="text-sm font-normal">
                Isolation mode
              </Label>
              <Select
                value={sandboxEnabled === null ? 'default' : sandboxEnabled ? 'on' : 'off'}
                onValueChange={(value) => {
                  setSandboxEnabled(
                    value === 'default' ? null : value === 'on'
                  )
                }}
              >
                <SelectTrigger id="sandbox-mode" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="default">Use global setting</SelectItem>
                  <SelectItem value="on">Always sandbox</SelectItem>
                  <SelectItem value="off">Always host</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                "Use global setting" follows the sandbox default from Settings.
              </p>
            </div>
          </div>

          {/* Config Repo */}
          <div className="space-y-3">
            <h3 className="text-sm font-medium">Config Repository</h3>
            <p className="text-xs text-muted-foreground">
              A private repository containing a <code className="bg-muted px-1 rounded">.claude/</code> directory
              with skills, MCP servers, and settings. Overlaid into the sandbox at runtime.
            </p>
            <div className="space-y-1.5">
              <Label htmlFor="config-repo-url">Repository</Label>
              <Input
                id="config-repo-url"
                value={configRepoUrl}
                onChange={(e) => setConfigRepoUrl(e.target.value)}
                placeholder="org/my-claude-config"
              />
              <p className="text-xs text-muted-foreground">
                GitHub repo path (e.g. <code className="bg-muted px-1 rounded">org/claude-skills</code>).
                Leave empty to use global config only.
              </p>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="config-repo-branch">Branch</Label>
              <Input
                id="config-repo-branch"
                value={configRepoBranch}
                onChange={(e) => setConfigRepoBranch(e.target.value)}
                placeholder="main"
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={isPending}>
              {isPending
                ? isEdit
                  ? 'Saving...'
                  : 'Creating...'
                : isEdit
                  ? 'Save'
                  : 'Create Agent'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
