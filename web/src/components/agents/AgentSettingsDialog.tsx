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
import { useState } from 'react'
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
  const [sandboxEnabled, setSandboxEnabled] = useState<boolean | null>(agent?.sandbox_enabled ?? null)

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
    setSandboxEnabled(agent?.sandbox_enabled ?? null)
    setTriggerNewIssue(agent?.trigger_config?.error?.new_issue ?? true)
    setTriggerRegression(agent?.trigger_config?.error?.regression ?? true)
    setTriggerManual(agent?.trigger_config?.manual ?? true)
  }

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
    const triggerConfig = {
      error: { new_issue: triggerNewIssue, regression: triggerRegression },
      manual: triggerManual,
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
        sandbox_enabled: sandboxEnabled ?? undefined,
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
        sandbox_enabled: sandboxEnabled ?? undefined,
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

        <div className="space-y-6 py-4">
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
                  <SelectItem value="claude_cli">Claude CLI</SelectItem>
                  <SelectItem value="codex_cli">Codex CLI</SelectItem>
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
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={isPending}>
            {isPending
              ? isEdit
                ? 'Saving...'
                : 'Creating...'
              : isEdit
                ? 'Save'
                : 'Create Agent'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
