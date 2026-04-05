import { ProjectResponse } from '@/api/client'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Box, Loader2, Pencil, Play, Plus, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { AgentSettingsDialog } from './AgentSettingsDialog'
import {
  listAgents,
  listAllRuns,
  triggerAgent,
  type Agent,
  type AgentRun,
} from './api'
import { AutopilotStatusBadge } from './AutopilotStatusBadge'

interface AutopilotPageProps {
  project: ProjectResponse
}

function formatTimeAgo(dateStr: string): string {
  const now = Date.now()
  const then = new Date(dateStr).getTime()
  const diffMs = now - then
  const diffSecs = Math.floor(diffMs / 1000)
  const diffMins = Math.floor(diffSecs / 60)
  const diffHours = Math.floor(diffMins / 60)
  const diffDays = Math.floor(diffHours / 24)

  if (diffSecs < 60) return `${diffSecs}s ago`
  if (diffMins < 60) return `${diffMins}m ago`
  if (diffHours < 24) return `${diffHours}h ago`
  return `${diffDays}d ago`
}

function formatCost(cents: number | null): string {
  if (cents == null) return '-'
  return `$${(cents / 100).toFixed(2)}`
}

/** Parse a cron expression and return a human-readable "next run" string */
function nextCronRun(cron: string | null | undefined): string | null {
  if (!cron) return null
  // Simple cron parsing for display — covers common cases
  const parts = cron.split(' ')
  if (parts.length !== 5) return cron
  const [min, hour, , , dow] = parts
  const days = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']
  const dayName = dow !== '*' ? days[parseInt(dow)] || dow : 'Daily'
  const time = `${hour.padStart(2, '0')}:${min.padStart(2, '0')} UTC`
  return `${dayName} at ${time}`
}

function AgentCard({
  agent,
  projectId,
  queryClient,
  onEdit,
  onNavigate,
}: {
  agent: Agent
  projectId: number
  queryClient: ReturnType<typeof useQueryClient>
  onEdit: (agent: Agent) => void
  onNavigate: (slug: string) => void
}) {
  const [showTriggerDialog, setShowTriggerDialog] = useState(false)
  const [userContext, setUserContext] = useState('')

  const trigger = useMutation({
    mutationFn: (context?: string) =>
      triggerAgent(projectId, agent.slug, context ? { user_context: context } : undefined),
    onSuccess: () => {
      toast.success(`${agent.name} triggered`)
      setShowTriggerDialog(false)
      setUserContext('')
      queryClient.invalidateQueries({ queryKey: ['agent-runs', projectId] })
    },
    onError: (error: Error) => {
      toast.error(error.message || 'Failed to trigger')
    },
  })

  const cronSchedule = agent.trigger_config?.schedule?.cron
  const nextRun = nextCronRun(cronSchedule)

  return (
    <>
    <Card className="bg-background">
      <CardContent className="p-4">
        <div className="flex items-center justify-between mb-2">
          <div className="flex items-center gap-2">
            <button
              className="font-medium hover:underline text-left"
              onClick={() => onNavigate(agent.slug)}
            >
              {agent.name}
            </button>
            {agent.source === 'yaml' && (
              <span className="text-xs bg-blue-500/10 text-blue-400 px-1.5 py-0.5 rounded">YAML</span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <span className={`text-xs px-2 py-0.5 rounded ${agent.enabled ? 'bg-green-500/10 text-green-400' : 'bg-muted text-muted-foreground'}`}>
              {agent.enabled ? 'Active' : 'Disabled'}
            </span>
            {agent.source !== 'yaml' && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => onEdit(agent)}
              >
                <Pencil className="h-3 w-3" />
              </Button>
            )}
            {agent.trigger_config?.manual && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setShowTriggerDialog(true)}
                disabled={trigger.isPending || !agent.enabled}
              >
                <Play className="h-3 w-3 mr-1" />
                Run
              </Button>
            )}
          </div>
        </div>
        {agent.description && (
          <p className="text-sm text-muted-foreground mb-2">{agent.description}</p>
        )}
        <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
          <span>{agent.ai_provider === 'claude_cli' ? 'Claude' : 'Codex'}</span>
          {agent.trigger_config?.error?.new_issue && <span className="bg-muted px-1.5 py-0.5 rounded">new errors</span>}
          {agent.trigger_config?.error?.regression && <span className="bg-muted px-1.5 py-0.5 rounded">regressions</span>}
          {nextRun && <span className="bg-muted px-1.5 py-0.5 rounded">{nextRun}</span>}
          {agent.trigger_config?.manual && <span className="bg-muted px-1.5 py-0.5 rounded">manual</span>}
          {agent.sandbox_enabled && <span className="bg-orange-500/10 text-orange-400 px-1.5 py-0.5 rounded">sandbox</span>}
        </div>
      </CardContent>
    </Card>

    {/* Trigger dialog with optional context */}
    <Dialog open={showTriggerDialog} onOpenChange={setShowTriggerDialog}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Run {agent.name}</DialogTitle>
          <DialogDescription>
            {agent.description || 'Trigger this agent manually.'}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-2">
          <label className="text-sm font-medium">
            Context <span className="text-muted-foreground font-normal">(optional)</span>
          </label>
          <Textarea
            placeholder="e.g. Research edge caching best practices for Next.js in 2026..."
            value={userContext}
            onChange={(e) => setUserContext(e.target.value)}
            rows={4}
          />
          <p className="text-xs text-muted-foreground">
            Provide additional instructions or a topic for the agent. This is appended to the agent's prompt.
          </p>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => setShowTriggerDialog(false)}>
            Cancel
          </Button>
          <Button
            onClick={() => trigger.mutate(userContext || undefined)}
            disabled={trigger.isPending}
          >
            {trigger.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin mr-2" />
            ) : (
              <Play className="h-4 w-4 mr-2" />
            )}
            Run Agent
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
    </>
  )
}

export function AutopilotPage({ project }: AutopilotPageProps) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editingAgent, setEditingAgent] = useState<Agent | null>(null)

  const {
    data: agents,
    isLoading: isLoadingAgents,
  } = useQuery({
    queryKey: ['agents', project.id],
    queryFn: () => listAgents(project.id),
  })

  const {
    data: runsData,
    isLoading: isLoadingRuns,
  } = useQuery({
    queryKey: ['agent-runs', project.id],
    queryFn: () => listAllRuns(project.id),
    refetchInterval: 5000,
    enabled: !!agents && agents.length > 0,
  })

  const runs = runsData?.items ?? []

  if (isLoadingAgents) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (!agents || agents.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-20">
        <Sparkles className="h-12 w-12 text-muted-foreground mb-4" />
        <h2 className="text-lg font-semibold mb-2">No agents configured</h2>
        <p className="text-muted-foreground text-sm mb-4 text-center max-w-md">
          Create agents via the dashboard or add <code className="text-xs bg-muted px-1 py-0.5 rounded">.temps/agents/*.yaml</code> files to your repository.
        </p>
        <Button
          onClick={() => {
            setEditingAgent(null)
            setDialogOpen(true)
          }}
        >
          <Plus className="h-4 w-4 mr-2" />
          Create Agent
        </Button>
        <AgentSettingsDialog
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          projectId={project.id}
          agent={editingAgent}
        />
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <h1 className="text-xl font-semibold">Agents</h1>
        <Button
          size="sm"
          onClick={() => {
            setEditingAgent(null)
            setDialogOpen(true)
          }}
        >
          <Plus className="h-3 w-3 mr-1" />
          New Agent
        </Button>
      </div>

      <AgentSettingsDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        projectId={project.id}
        agent={editingAgent}
      />

      {/* Agent cards */}
      {agents && agents.length > 0 && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {agents.map((agent) => (
            <AgentCard
              key={agent.id}
              agent={agent}
              projectId={project.id}
              queryClient={queryClient}
              onEdit={(a) => {
                setEditingAgent(a)
                setDialogOpen(true)
              }}
              onNavigate={(slug) => navigate(`detail/${slug}`)}
            />
          ))}
        </div>
      )}

      {/* Runs */}
      <h2 className="text-lg font-semibold mt-4">Recent Runs</h2>

      {isLoadingRuns ? (
        <Card>
          <CardContent className="p-6 space-y-3">
            {Array.from({ length: 3 }).map((_, i) => (
              <Skeleton key={i} className="h-10 w-full" />
            ))}
          </CardContent>
        </Card>
      ) : runs.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-12">
          <p className="text-muted-foreground text-sm">
            No runs yet. Agents will run when triggers fire.
          </p>
        </div>
      ) : (
        <Card>
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Status</TableHead>
                  <TableHead>Agent</TableHead>
                  <TableHead>Trigger</TableHead>
                  <TableHead className="hidden md:table-cell">Sandbox</TableHead>
                  <TableHead className="hidden md:table-cell">PR</TableHead>
                  <TableHead className="hidden md:table-cell">Files</TableHead>
                  <TableHead className="hidden md:table-cell">Cost</TableHead>
                  <TableHead>Time</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {runs.map((run: AgentRun) => (
                  <TableRow
                    key={run.id}
                    className="cursor-pointer hover:bg-muted/50"
                    onClick={() => {
                      if (run.trigger_type === 'autofixer' && run.trigger_source_id) {
                        navigate(`/projects/${project.slug}/errors/${run.trigger_source_id}/autofix`)
                      } else {
                        navigate(`${run.id}`)
                      }
                    }}
                  >
                    <TableCell>
                      <AutopilotStatusBadge status={run.status} />
                    </TableCell>
                    <TableCell className="text-sm">
                      {run.agent_name || (run.trigger_type === 'autofixer' ? 'Autofix' : '-')}
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground">
                      {run.trigger_type === 'autofixer' ? 'Autofix' : run.trigger_type}
                    </TableCell>
                    <TableCell className="hidden md:table-cell">
                      {run.sandbox_enabled ? (
                        <span className="text-xs bg-orange-500/10 text-orange-400 px-1.5 py-0.5 rounded inline-flex items-center gap-1">
                          <Box className="h-3 w-3" />
                          Yes
                        </span>
                      ) : (
                        <span className="text-xs text-muted-foreground">No</span>
                      )}
                    </TableCell>
                    <TableCell className="hidden md:table-cell text-sm">
                      {run.pr_number ? `#${run.pr_number}` : '-'}
                    </TableCell>
                    <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                      {run.files_changed ?? '-'}
                    </TableCell>
                    <TableCell className="hidden md:table-cell text-sm text-muted-foreground">
                      {formatCost(run.estimated_cost_cents)}
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground whitespace-nowrap">
                      {formatTimeAgo(run.created_at)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        </Card>
      )}
    </div>
  )
}
