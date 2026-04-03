import { ProjectResponse } from '@/api/client'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ArrowLeft,
  Box,
  Cpu,
  Pencil,
  Play,
  Shield,
  Zap,
} from 'lucide-react'
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { toast } from 'sonner'
import { AgentSettingsDialog } from './AgentSettingsDialog'
import {
  getAgent,
  listRunsForAgent,
  triggerAgent,
  type Agent,
  type AgentRun,
} from './api'
import { AutopilotStatusBadge } from './AutopilotStatusBadge'

interface AgentDetailPageProps {
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

function nextCronRun(cron: string | null | undefined): string | null {
  if (!cron) return null
  const parts = cron.split(' ')
  if (parts.length !== 5) return cron
  const [min, hour, , , dow] = parts
  const days = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']
  const dayName = dow !== '*' ? days[parseInt(dow)] || dow : 'Daily'
  const time = `${hour.padStart(2, '0')}:${min.padStart(2, '0')} UTC`
  return `${dayName} at ${time}`
}

function PropertyRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1 sm:flex-row sm:items-start sm:gap-4">
      <dt className="text-sm text-muted-foreground sm:w-40 sm:flex-shrink-0">{label}</dt>
      <dd className="text-sm">{children}</dd>
    </div>
  )
}

function AgentProperties({ agent }: { agent: Agent }) {
  const cronSchedule = agent.trigger_config?.schedule?.cron
  const nextRun = nextCronRun(cronSchedule)

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
      {/* General */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">General</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <PropertyRow label="Slug">
            <code className="text-xs bg-muted px-1.5 py-0.5 rounded">{agent.slug}</code>
          </PropertyRow>
          <PropertyRow label="Source">
            {agent.source === 'yaml' ? (
              <span className="text-xs bg-blue-500/10 text-blue-400 px-1.5 py-0.5 rounded">YAML</span>
            ) : (
              <span className="text-xs bg-muted px-1.5 py-0.5 rounded">Dashboard</span>
            )}
          </PropertyRow>
          <PropertyRow label="Status">
            <span className={`text-xs px-2 py-0.5 rounded ${agent.enabled ? 'bg-green-500/10 text-green-400' : 'bg-muted text-muted-foreground'}`}>
              {agent.enabled ? 'Active' : 'Disabled'}
            </span>
          </PropertyRow>
          {agent.description && (
            <PropertyRow label="Description">{agent.description}</PropertyRow>
          )}
          <PropertyRow label="Deliverable">
            <code className="text-xs bg-muted px-1.5 py-0.5 rounded">{agent.deliverable}</code>
          </PropertyRow>
          <PropertyRow label="Created">
            {new Date(agent.created_at).toLocaleDateString(undefined, {
              year: 'numeric',
              month: 'short',
              day: 'numeric',
              hour: '2-digit',
              minute: '2-digit',
            })}
          </PropertyRow>
        </CardContent>
      </Card>

      {/* AI Provider */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm flex items-center gap-2">
            <Cpu className="h-4 w-4" />
            AI Provider
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <PropertyRow label="Provider">
            {agent.ai_provider === 'claude_cli' ? 'Claude CLI' : 'Codex CLI'}
          </PropertyRow>
          <PropertyRow label="API Key">
            {agent.api_key_set ? (
              <span className="text-xs bg-green-500/10 text-green-400 px-1.5 py-0.5 rounded">Set</span>
            ) : (
              <span className="text-xs text-muted-foreground">Using system default</span>
            )}
          </PropertyRow>
          {agent.prompt && (
            <PropertyRow label="Custom Prompt">
              <pre className="whitespace-pre-wrap text-xs bg-muted p-3 rounded-md max-h-48 overflow-y-auto w-full">
                {agent.prompt}
              </pre>
            </PropertyRow>
          )}
          {!agent.prompt && (
            <PropertyRow label="Prompt">
              <span className="text-muted-foreground text-xs">Default prompt for trigger type</span>
            </PropertyRow>
          )}
        </CardContent>
      </Card>

      {/* Triggers */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm flex items-center gap-2">
            <Zap className="h-4 w-4" />
            Triggers
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <PropertyRow label="New errors">
            {agent.trigger_config?.error?.new_issue ? 'Enabled' : 'Disabled'}
          </PropertyRow>
          <PropertyRow label="Regressions">
            {agent.trigger_config?.error?.regression ? 'Enabled' : 'Disabled'}
          </PropertyRow>
          <PropertyRow label="Manual">
            {agent.trigger_config?.manual ? 'Enabled' : 'Disabled'}
          </PropertyRow>
          {cronSchedule && (
            <PropertyRow label="Schedule">
              <div className="flex flex-col gap-1">
                <code className="text-xs bg-muted px-1.5 py-0.5 rounded">{cronSchedule}</code>
                {nextRun && <span className="text-xs text-muted-foreground">{nextRun}</span>}
              </div>
            </PropertyRow>
          )}
        </CardContent>
      </Card>

      {/* Limits */}
      <Card>
        <CardHeader>
          <CardTitle className="text-sm flex items-center gap-2">
            <Shield className="h-4 w-4" />
            Limits & Execution
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          <PropertyRow label="Max turns">
            {agent.max_turns}
          </PropertyRow>
          <PropertyRow label="Timeout">
            {agent.timeout_seconds}s
          </PropertyRow>
          <PropertyRow label="Daily budget">
            ${(agent.daily_budget_cents / 100).toFixed(2)}
          </PropertyRow>
          <PropertyRow label="Cooldown">
            {agent.cooldown_minutes}m
          </PropertyRow>
          <PropertyRow label="Branch prefix">
            <code className="text-xs bg-muted px-1.5 py-0.5 rounded">{agent.branch_prefix}</code>
          </PropertyRow>
          <PropertyRow label="Sandbox">
            {agent.sandbox_enabled ? (
              <span className="text-xs bg-orange-500/10 text-orange-400 px-1.5 py-0.5 rounded flex items-center gap-1 w-fit">
                <Box className="h-3 w-3" />
                Docker sandbox
              </span>
            ) : (
              <span className="text-muted-foreground">Disabled (runs on host)</span>
            )}
          </PropertyRow>
        </CardContent>
      </Card>
    </div>
  )
}

export function AgentDetailPage({ project }: AgentDetailPageProps) {
  const { agentSlug } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [dialogOpen, setDialogOpen] = useState(false)

  const {
    data: agent,
    isLoading: isLoadingAgent,
    error: agentError,
  } = useQuery({
    queryKey: ['agent', project.id, agentSlug],
    queryFn: () => getAgent(project.id, agentSlug!),
    enabled: !!agentSlug,
  })

  const {
    data: runsData,
    isLoading: isLoadingRuns,
    isError: isRunsError,
  } = useQuery({
    queryKey: ['agent-runs', project.id, agentSlug],
    queryFn: () => listRunsForAgent(project.id, agentSlug!),
    refetchInterval: 5000,
    enabled: !!agentSlug,
  })

  const trigger = useMutation({
    mutationFn: () => triggerAgent(project.id, agentSlug!),
    onSuccess: () => {
      toast.success('Agent triggered')
      queryClient.invalidateQueries({ queryKey: ['agent-runs', project.id, agentSlug] })
    },
    onError: (error: Error) => {
      toast.error(error.message || 'Failed to trigger')
    },
  })

  const runs = runsData?.items ?? []

  if (isLoadingAgent) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (agentError || !agent) {
    return (
      <div className="flex flex-col items-center justify-center py-20">
        <p className="text-muted-foreground text-sm">Agent not found</p>
        <Button variant="ghost" size="sm" className="mt-4" asChild>
          <Link to={`/projects/${project.slug}/agents`}>Back to agents</Link>
        </Button>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="icon" asChild>
            <Link to={`/projects/${project.slug}/agents`}>
              <ArrowLeft className="h-4 w-4" />
            </Link>
          </Button>
          <h1 className="text-xl font-semibold">{agent.name}</h1>
          <span className={`text-xs px-2 py-0.5 rounded ${agent.enabled ? 'bg-green-500/10 text-green-400' : 'bg-muted text-muted-foreground'}`}>
            {agent.enabled ? 'Active' : 'Disabled'}
          </span>
          {agent.sandbox_enabled && (
            <span className="text-xs bg-orange-500/10 text-orange-400 px-1.5 py-0.5 rounded flex items-center gap-1">
              <Box className="h-3 w-3" />
              Sandbox
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {agent.source !== 'yaml' && (
            <Button variant="outline" size="sm" onClick={() => setDialogOpen(true)}>
              <Pencil className="h-3 w-3 mr-1" />
              Edit
            </Button>
          )}
          {agent.trigger_config?.manual && (
            <Button
              size="sm"
              onClick={() => trigger.mutate()}
              disabled={trigger.isPending || !agent.enabled}
            >
              <Play className="h-3 w-3 mr-1" />
              Run Now
            </Button>
          )}
        </div>
      </div>

      <AgentSettingsDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        projectId={project.id}
        agent={agent}
      />

      {/* Properties */}
      <AgentProperties agent={agent} />

      {/* Runs */}
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">Runs</h2>
      </div>

      {isLoadingRuns && !isRunsError ? (
        <Card>
          <CardContent className="p-6 space-y-3">
            {Array.from({ length: 3 }).map((_, i) => (
              <Skeleton key={i} className="h-10 w-full" />
            ))}
          </CardContent>
        </Card>
      ) : runs.length === 0 ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-12">
            <p className="text-muted-foreground text-sm">
              No runs yet. Trigger the agent to start a run.
            </p>
          </CardContent>
        </Card>
      ) : (
        <Card>
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Status</TableHead>
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
                    onClick={() => navigate(`/projects/${project.slug}/agents/${run.id}`)}
                  >
                    <TableCell>
                      <AutopilotStatusBadge status={run.status} />
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
