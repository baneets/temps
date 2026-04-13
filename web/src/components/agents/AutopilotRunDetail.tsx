import { ProjectResponse } from '@/api/client'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import {
  AlertTriangle,
  ArrowLeft,
  Bot,
  Box,
  Clock,
  Cpu,
  ExternalLink,
  FileCode,
  GitBranch,
  Hash,
  Loader2,
  RefreshCw,
  SquareTerminal,
  Terminal,
  Webhook,
  Zap,
} from 'lucide-react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { getAgentRun, retryRun } from './api'
import type { AgentRunLog } from './api'
import { AutopilotStatusBadge } from './AutopilotStatusBadge'
import { startSession } from '@/components/workspace/api'
import { cn } from '@/lib/utils'

const proseClasses = 'prose prose-sm dark:prose-invert max-w-none prose-pre:bg-black/30 prose-pre:text-muted-foreground prose-pre:text-xs prose-pre:border-0 prose-code:before:content-none prose-code:after:content-none prose-p:my-1.5 prose-headings:my-2 prose-ul:my-1.5 prose-ul:list-disc prose-ul:pl-5 prose-ol:my-1.5 prose-ol:list-decimal prose-ol:pl-5 prose-li:my-0.5 prose-li:marker:text-foreground/60 prose-hr:my-3 prose-hr:border-border prose-table:text-xs prose-th:px-2 prose-th:py-1 prose-td:px-2 prose-td:py-1'

interface AutopilotRunDetailProps {
  project: ProjectResponse
}

const activeStatuses = new Set([
  'pending',
  'cloning',
  'analyzing',
  'fixing',
  'pushing',
  'creating_pr',
  'deploying',
])

function formatDuration(startedAt: string | null, completedAt: string | null): string {
  if (!startedAt) return '-'
  const start = new Date(startedAt).getTime()
  const end = completedAt ? new Date(completedAt).getTime() : Date.now()
  const diffSecs = Math.floor((end - start) / 1000)
  if (diffSecs < 60) return `${diffSecs}s`
  const mins = Math.floor(diffSecs / 60)
  const secs = diffSecs % 60
  return `${mins}m ${secs}s`
}

function formatTimestamp(dateStr: string): string {
  return new Date(dateStr).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

function logLevelColor(level: string): string {
  switch (level) {
    case 'error':
      return 'bg-red-500'
    case 'warn':
    case 'warning':
      return 'bg-yellow-500'
    case 'info':
      return 'bg-blue-500'
    case 'debug':
      return 'bg-gray-500'
    default:
      return 'bg-gray-400'
  }
}

/** Render markdown content using prose styles */
function Markdown({ children }: { children: string }) {
  return (
    <div className={proseClasses}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  )
}

interface ConversationEvent {
  type: string
  role?: string
  content?: string
  tool?: string
  toolUseId?: string
  toolInput?: Record<string, unknown>
  toolResult?: string
  toolStatus?: 'in_progress' | 'completed' | 'failed'
  result?: string
  numTurns?: number
  costUsd?: number
  durationMs?: number
  tokensInput?: number
  tokensOutput?: number
}

/** Detect whether the output contains Codex-format JSON events.
 *  Codex emits `type: "item.completed"`, `type: "turn.completed"`, etc. */
function isCodexFormat(output: string): boolean {
  // Check first few JSON lines for Codex-specific event types
  for (const line of output.split('\n').slice(0, 10)) {
    const trimmed = line.trim()
    if (!trimmed.startsWith('{')) continue
    if (
      trimmed.includes('"thread.started"') ||
      trimmed.includes('"item.completed"') ||
      trimmed.includes('"turn.completed"') ||
      trimmed.includes('"item.started"')
    ) {
      return true
    }
  }
  return false
}

/** Parse Codex CLI --json output into conversation events.
 *
 *  Codex format:
 *  - {"type":"item.completed","item":{"type":"agent_message","text":"..."}}
 *  - {"type":"item.started","item":{"type":"command_execution","command":"...","status":"in_progress"}}
 *  - {"type":"item.completed","item":{"type":"command_execution","command":"...","aggregated_output":"...","exit_code":0,"status":"completed"}}
 *  - {"type":"turn.completed","usage":{"input_tokens":N,"output_tokens":N}}
 */
function parseCodexOutput(output: string): ConversationEvent[] {
  const events: ConversationEvent[] = []
  let totalInputTokens = 0
  let totalOutputTokens = 0

  for (const line of output.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed || !trimmed.startsWith('{')) continue
    try {
      const parsed = JSON.parse(trimmed)
      const item = parsed.item

      if (parsed.type === 'item.completed' && item?.type === 'agent_message' && item.text) {
        events.push({ type: 'assistant_text', content: item.text })
      } else if (parsed.type === 'item.started' && item?.type === 'command_execution') {
        // Command started — create a tool_call event that will be merged
        // with the completed event when it arrives
        events.push({
          type: 'tool_call',
          tool: 'Bash',
          toolUseId: item.id,
          toolInput: { command: item.command },
          toolStatus: 'in_progress',
        })
      } else if (parsed.type === 'item.completed' && item?.type === 'command_execution') {
        // Command completed — merge into existing tool_call or create new one
        const existing = item.id
          ? events.find((e) => e.type === 'tool_call' && e.toolUseId === item.id)
          : [...events].reverse().find(
              (e) => e.type === 'tool_call' && !e.toolResult && e.toolInput?.command === item.command,
            )

        const resultText = item.aggregated_output || ''
        const exitCode = item.exit_code ?? null
        const statusLabel =
          item.status === 'failed' || (exitCode != null && exitCode !== 0)
            ? `[exit ${exitCode}] `
            : ''
        const fullResult = `${statusLabel}${resultText}`

        if (existing) {
          existing.toolResult = fullResult
          existing.toolStatus = item.status === 'failed' ? 'failed' : 'completed'
        } else {
          events.push({
            type: 'tool_call',
            tool: 'Bash',
            toolUseId: item.id,
            toolInput: { command: item.command },
            toolResult: fullResult,
            toolStatus: item.status === 'failed' ? 'failed' : 'completed',
          })
        }
      } else if (parsed.type === 'item.completed' && item?.type === 'function_call') {
        // Codex function calls (MCP tools, etc.)
        events.push({
          type: 'tool_call',
          tool: item.name || 'function',
          toolUseId: item.id,
          toolInput: item.arguments ? JSON.parse(item.arguments) : {},
          toolResult: item.output || '',
          toolStatus: 'completed',
        })
      } else if (parsed.type === 'turn.completed' && parsed.usage) {
        totalInputTokens += parsed.usage.input_tokens || 0
        totalOutputTokens += parsed.usage.output_tokens || 0
      }
    } catch {
      // Not valid JSON
    }
  }

  // Add a synthetic result event with accumulated token usage
  if (events.length > 0 && (totalInputTokens > 0 || totalOutputTokens > 0)) {
    events.push({
      type: 'result',
      tokensInput: totalInputTokens,
      tokensOutput: totalOutputTokens,
    })
  }

  return events
}

/** Parse Claude stream-json output into conversation events.
 *  Merges consecutive tool_call → tool_result pairs so the result
 *  is available on the tool_call event itself (rendered collapsed). */
function parseClaudeOutput(output: string): ConversationEvent[] {
  const events: ConversationEvent[] = []

  for (const line of output.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed || !trimmed.startsWith('{')) continue
    try {
      const parsed = JSON.parse(trimmed)

      if (parsed.type === 'assistant' && parsed.message?.content) {
        for (const block of parsed.message.content) {
          if (block.type === 'text' && block.text) {
            events.push({ type: 'assistant_text', content: block.text })
          } else if (block.type === 'tool_use') {
            events.push({
              type: 'tool_call',
              tool: block.name,
              toolUseId: block.id,
              toolInput: block.input,
            })
          }
        }
      } else if (parsed.type === 'user' && parsed.message?.content) {
        for (const block of parsed.message.content) {
          if (block.type === 'tool_result') {
            const content = Array.isArray(block.content)
              ? block.content.map((c: { text?: string }) => c.text || '').join('')
              : typeof block.content === 'string'
                ? block.content
                : JSON.stringify(block.content)
            const matchById = block.tool_use_id
              ? events.find(
                  (e) => e.type === 'tool_call' && e.toolUseId === block.tool_use_id && !e.toolResult,
                )
              : null
            const target = matchById || [...events].reverse().find((e) => e.type === 'tool_call' && !e.toolResult)
            if (target) {
              target.toolResult = content
            } else {
              events.push({ type: 'tool_result', toolResult: content })
            }
          }
        }
      } else if (parsed.type === 'tool_result') {
        const content = Array.isArray(parsed.content)
          ? parsed.content.map((c: { text?: string }) => c.text || '').join('')
          : typeof parsed.content === 'string'
            ? parsed.content
            : JSON.stringify(parsed.content)
        const prev = events.length > 0 ? events[events.length - 1] : null
        if (prev && prev.type === 'tool_call' && !prev.toolResult) {
          prev.toolResult = content
        } else {
          events.push({ type: 'tool_result', toolResult: content })
        }
      } else if (parsed.type === 'result') {
        events.push({
          type: 'result',
          result: parsed.result,
          numTurns: parsed.num_turns,
          costUsd: parsed.total_cost_usd,
          durationMs: parsed.duration_ms,
        })
      }
    } catch {
      // Not JSON
    }
  }

  return events
}

/** Parse stream output from any AI provider into conversation events.
 *  Auto-detects Codex vs Claude format. */
function parseStreamOutput(output: string): ConversationEvent[] {
  return isCodexFormat(output) ? parseCodexOutput(output) : parseClaudeOutput(output)
}

/** Render the full AI conversation */
function AiOutputCard({ output, live }: { output: string; live?: boolean }) {
  const events = parseStreamOutput(output)
  const scrollRef = useRef<HTMLDivElement>(null)

  // Auto-scroll to bottom when live and new events arrive
  useEffect(() => {
    if (live && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [events.length, live])

  // If no events parsed, show raw output
  if (events.length === 0) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">AI Output</CardTitle>
        </CardHeader>
        <CardContent>
          <pre className="whitespace-pre-wrap text-xs font-mono bg-muted p-4 rounded-md overflow-x-auto max-h-96 overflow-y-auto">
            {output}
          </pre>
        </CardContent>
      </Card>
    )
  }

  const resultEvent = events.find((e) => e.type === 'result')

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between">
          <CardTitle className="text-sm">AI Conversation</CardTitle>
          {resultEvent && (
            <div className="flex gap-3 text-xs text-muted-foreground">
              {resultEvent.numTurns != null && (
                <span>{resultEvent.numTurns} turns</span>
              )}
              {resultEvent.durationMs != null && (
                <span>{Math.round(resultEvent.durationMs / 1000)}s</span>
              )}
              {resultEvent.costUsd != null && (
                <span>${resultEvent.costUsd.toFixed(2)}</span>
              )}
              {resultEvent.tokensInput != null && (
                <span>{resultEvent.tokensInput.toLocaleString()} in</span>
              )}
              {resultEvent.tokensOutput != null && (
                <span>{resultEvent.tokensOutput.toLocaleString()} out</span>
              )}
            </div>
          )}
        </div>
      </CardHeader>
      <CardContent>
        <div ref={scrollRef} className="space-y-3 max-h-[600px] overflow-y-auto">
          {events.map((event, i) => {
            if (event.type === 'assistant_text') {
              return (
                <div key={i} className="text-sm">
                  <Markdown>{event.content || ''}</Markdown>
                </div>
              )
            }
            if (event.type === 'tool_call') {
              const input = event.toolInput || {}
              const preview =
                event.tool === 'Read'
                  ? (input.file_path as string) || ''
                  : event.tool === 'Edit'
                    ? (input.file_path as string) || ''
                    : event.tool === 'Write'
                      ? (input.file_path as string) || ''
                      : event.tool === 'Bash'
                        ? (input.command as string) || ''
                        : event.tool === 'Grep'
                          ? (input.pattern as string) || ''
                          : JSON.stringify(input).slice(0, 120)
              const resultText = event.toolResult || ''
              const hasResult = resultText.length > 2
              const isFailed = event.toolStatus === 'failed'
              const borderColor = isFailed ? 'border-red-500/20' : 'border-blue-500/10'
              const bgColor = isFailed ? 'bg-red-500/5' : 'bg-blue-500/5'
              const labelColor = isFailed ? 'text-red-400' : 'text-blue-400'
              return (
                <div key={i} className={`rounded-md ${bgColor} border ${borderColor}`}>
                  <div className="flex items-start gap-2 px-3 py-2">
                    <span className={`text-xs font-mono font-medium ${labelColor} whitespace-nowrap`}>
                      {event.tool}
                    </span>
                    <span className="text-xs font-mono text-muted-foreground truncate flex-1">
                      {preview}
                    </span>
                    {isFailed && (
                      <span className="text-[10px] font-medium text-red-400 shrink-0">FAILED</span>
                    )}
                    {event.toolStatus === 'in_progress' && (
                      <Loader2 className="h-3 w-3 text-blue-400 animate-spin shrink-0" />
                    )}
                  </div>
                  {hasResult && (
                    <details className={`border-t ${borderColor}`}>
                      <summary className={`px-3 py-1.5 text-[11px] text-muted-foreground cursor-pointer hover:${bgColor} select-none`}>
                        Output ({resultText.length > 1000 ? `${Math.round(resultText.length / 1000)}k chars` : `${resultText.length} chars`})
                      </summary>
                      <pre className="text-xs font-mono bg-muted/30 px-3 py-2 overflow-x-auto max-h-48 overflow-y-auto text-muted-foreground whitespace-pre-wrap break-words">
                        {resultText.length > 2000 ? resultText.slice(0, 2000) + '\n...' : resultText}
                      </pre>
                    </details>
                  )}
                </div>
              )
            }
            if (event.type === 'tool_result') {
              // Only render standalone results that weren't merged into a tool_call
              const text = event.toolResult || ''
              if (!text || text.length < 3) return null
              return (
                <details
                  key={i}
                  className="rounded-md bg-muted/30 border border-border"
                >
                  <summary className="px-3 py-1.5 text-[11px] text-muted-foreground cursor-pointer hover:bg-muted/50 select-none">
                    Tool output ({text.length > 1000 ? `${Math.round(text.length / 1000)}k chars` : `${text.length} chars`})
                  </summary>
                  <pre className="text-xs font-mono px-3 py-2 overflow-x-auto max-h-48 overflow-y-auto text-muted-foreground whitespace-pre-wrap break-words">
                    {text.length > 2000 ? text.slice(0, 2000) + '\n...' : text}
                  </pre>
                </details>
              )
            }
            if (event.type === 'result' && event.result) {
              return (
                <div
                  key={i}
                  className="rounded-md bg-green-500/5 border border-green-500/10 p-3"
                >
                  <p className="text-xs font-medium text-green-400 mb-1">
                    Result
                  </p>
                  <div className="text-sm"><Markdown>{event.result}</Markdown></div>
                </div>
              )
            }
            return null
          })}
        </div>
      </CardContent>
    </Card>
  )
}

export function AutopilotRunDetail({ project }: AutopilotRunDetailProps) {
  const { runId } = useParams()
  const navigate = useNavigate()
  const [streamLogs, setStreamLogs] = useState<AgentRunLog[]>([])
  const [isStreaming, setIsStreaming] = useState(false)
  const [isOpeningWorkspace, setIsOpeningWorkspace] = useState(false)
  const [isRetrying, setIsRetrying] = useState(false)

  const { data, isLoading, error } = useQuery({
    queryKey: ['agent-run', project.id, runId],
    queryFn: () => getAgentRun(project.id, runId!),
    enabled: !!runId,
    refetchInterval: (query) => {
      const run = query.state.data?.run
      if (run && activeStatuses.has(run.status)) {
        return 5000
      }
      return false
    },
  })

  // SSE real-time streaming
  useEffect(() => {
    if (!runId || !data?.run) return
    if (!activeStatuses.has(data.run.status)) return

    setIsStreaming(true)
    const eventSource = new EventSource(
      `/api/projects/${project.id}/agents/runs/${runId}/stream`
    )

    eventSource.onmessage = (event) => {
      try {
        const log = JSON.parse(event.data) as AgentRunLog
        setStreamLogs((prev) => {
          // Dedup by id
          if (prev.some((l) => l.id === log.id)) return prev
          return [...prev, log]
        })
      } catch {
        // Not JSON
      }
    }

    eventSource.addEventListener('status', () => {
      // Run completed — close stream
      eventSource.close()
      setIsStreaming(false)
    })

    eventSource.onerror = () => {
      eventSource.close()
      setIsStreaming(false)
    }

    return () => {
      eventSource.close()
      setIsStreaming(false)
    }
  }, [runId, data?.run?.status, project.id])

  if (isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-40 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (error || !data) {
    return (
      <Alert variant="destructive">
        <AlertTriangle className="h-4 w-4" />
        <AlertTitle>Error</AlertTitle>
        <AlertDescription>
          {error instanceof Error
            ? error.message
            : 'Failed to load run'}
        </AlertDescription>
      </Alert>
    )
  }

  const { run, logs: fetchedLogs } = data

  // Merge fetched logs with streamed logs, dedup by id
  const allLogs = (() => {
    const merged = [...fetchedLogs]
    for (const sl of streamLogs) {
      if (!merged.some((l) => l.id === sl.id)) {
        merged.push(sl)
      }
    }
    return merged.sort((a, b) => a.id - b.id)
  })()

  const logs = allLogs

  return (
    <div className="space-y-6">
      {/* Back link + header */}
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="icon" asChild>
            <Link to="../agents">
              <ArrowLeft className="h-4 w-4" />
            </Link>
          </Button>
          <h1 className="text-xl font-semibold">Run #{run.id}</h1>
          <AutopilotStatusBadge status={run.status} />
          {isStreaming && (
            <span className="text-xs text-green-400 animate-pulse">LIVE</span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {!activeStatuses.has(run.status) && (
            <>
              <Button
                variant="outline"
                size="sm"
                disabled={isRetrying}
                onClick={async () => {
                  setIsRetrying(true)
                  try {
                    const newRun = await retryRun(project.id, run.id)
                    navigate(`../agents/${newRun.id}`)
                  } catch (e) {
                    console.error('Failed to retry run:', e)
                  } finally {
                    setIsRetrying(false)
                  }
                }}
              >
                {isRetrying ? (
                  <Loader2 className="h-4 w-4 animate-spin mr-1" />
                ) : (
                  <RefreshCw className="h-4 w-4 mr-1" />
                )}
                Retry
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={isOpeningWorkspace}
                onClick={async () => {
                  setIsOpeningWorkspace(true)
                  try {
                    const session = await startSession(project.id, {
                      branch_name: run.branch_name || undefined,
                      agent_run_id: run.id,
                    })
                    navigate(`../workspace?session=${session.id}`)
                  } catch (e) {
                    console.error('Failed to open workspace:', e)
                  } finally {
                    setIsOpeningWorkspace(false)
                  }
                }}
              >
                {isOpeningWorkspace ? (
                  <Loader2 className="h-4 w-4 animate-spin mr-1" />
                ) : (
                  <SquareTerminal className="h-4 w-4 mr-1" />
                )}
                Open in Workspace
              </Button>
            </>
          )}
          {activeStatuses.has(run.status) && (
            <Button
              variant="destructive"
              size="sm"
              onClick={async () => {
                try {
                  await fetch(`/api/projects/${project.id}/agents/runs/${run.id}/cancel`, {
                    method: 'POST',
                  })
                  window.location.reload()
                } catch {
                  // ignore
                }
              }}
            >
              Cancel
            </Button>
          )}
        </div>
      </div>

      {/* Info cards grid */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        {run.pr_url && (
          <Card>
            <CardContent className="p-4 flex items-center gap-2">
              <Hash className="h-4 w-4 text-muted-foreground flex-shrink-0" />
              <div className="min-w-0">
                <p className="text-xs text-muted-foreground">Pull Request</p>
                <a
                  href={run.pr_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-sm font-medium text-primary hover:underline flex items-center gap-1"
                >
                  #{run.pr_number}
                  <ExternalLink className="h-3 w-3" />
                </a>
              </div>
            </CardContent>
          </Card>
        )}

        {run.preview_url && (
          <Card>
            <CardContent className="p-4 flex items-center gap-2">
              <ExternalLink className="h-4 w-4 text-muted-foreground flex-shrink-0" />
              <div className="min-w-0">
                <p className="text-xs text-muted-foreground">Preview</p>
                <a
                  href={run.preview_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-sm font-medium text-primary hover:underline truncate block"
                >
                  Open preview
                </a>
              </div>
            </CardContent>
          </Card>
        )}

        {run.branch_name && (
          <Card>
            <CardContent className="p-4 flex items-center gap-2">
              <GitBranch className="h-4 w-4 text-muted-foreground flex-shrink-0" />
              <div className="min-w-0">
                <p className="text-xs text-muted-foreground">Branch</p>
                <p className="text-sm font-medium truncate">
                  {run.branch_name}
                </p>
              </div>
            </CardContent>
          </Card>
        )}

        <Card>
          <CardContent className="p-4 flex items-center gap-2">
            <FileCode className="h-4 w-4 text-muted-foreground flex-shrink-0" />
            <div className="min-w-0">
              <p className="text-xs text-muted-foreground">Files Changed</p>
              <p className="text-sm font-medium">
                {run.files_changed ?? '-'}
              </p>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-2">
            <Zap className="h-4 w-4 text-muted-foreground flex-shrink-0" />
            <div className="min-w-0">
              <p className="text-xs text-muted-foreground">Tokens</p>
              <p className="text-sm font-medium">
                {run.tokens_input != null
                  ? `${run.tokens_input.toLocaleString()} / ${(run.tokens_output ?? 0).toLocaleString()}`
                  : '-'}
              </p>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-2">
            <Zap className="h-4 w-4 text-muted-foreground flex-shrink-0" />
            <div className="min-w-0">
              <p className="text-xs text-muted-foreground">Cost</p>
              <p className="text-sm font-medium">
                {run.estimated_cost_cents != null
                  ? `$${(run.estimated_cost_cents / 100).toFixed(2)}`
                  : '-'}
              </p>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-2">
            <Clock className="h-4 w-4 text-muted-foreground flex-shrink-0" />
            <div className="min-w-0">
              <p className="text-xs text-muted-foreground">Duration</p>
              <p className="text-sm font-medium">
                {formatDuration(run.started_at, run.completed_at)}
              </p>
            </div>
          </CardContent>
        </Card>

        {run.ai_provider && (
          <Card>
            <CardContent className="p-4 flex items-center gap-2">
              <Bot className="h-4 w-4 text-muted-foreground flex-shrink-0" />
              <div className="min-w-0">
                <p className="text-xs text-muted-foreground">AI Provider</p>
                <p className="text-sm font-medium">
                  {run.ai_provider === 'claude_cli'
                    ? 'Claude Code'
                    : run.ai_provider === 'codex_cli'
                      ? 'Codex'
                      : run.ai_provider === 'opencode'
                        ? 'OpenCode'
                        : run.ai_provider}
                </p>
              </div>
            </CardContent>
          </Card>
        )}

        {run.ai_model && (
          <Card>
            <CardContent className="p-4 flex items-center gap-2">
              <Cpu className="h-4 w-4 text-muted-foreground flex-shrink-0" />
              <div className="min-w-0 flex-1">
                <p className="text-xs text-muted-foreground">AI Model</p>
                <p className="text-sm font-medium truncate" title={run.ai_model}>
                  {run.ai_model}
                </p>
              </div>
            </CardContent>
          </Card>
        )}

        <Card>
          <CardContent className="p-4 flex items-center gap-2">
            <Box className="h-4 w-4 text-muted-foreground flex-shrink-0" />
            <div className="min-w-0">
              <p className="text-xs text-muted-foreground">Sandbox</p>
              <p className="text-sm font-medium">
                {run.sandbox_enabled ? (
                  <span className="text-orange-400">Docker</span>
                ) : (
                  'Host'
                )}
              </p>
            </div>
          </CardContent>
        </Card>
        {run.ai_session_id && (
          <Card>
            <CardContent className="p-4 flex items-center gap-2">
              <SquareTerminal className="h-4 w-4 text-muted-foreground flex-shrink-0" />
              <div className="min-w-0 flex-1">
                <p className="text-xs text-muted-foreground">Session ID</p>
                <p className="text-sm font-mono truncate" title={run.ai_session_id}>
                  {run.ai_session_id.slice(0, 8)}…
                </p>
              </div>
            </CardContent>
          </Card>
        )}
      </div>

      {/* Trigger & Arguments */}
      {(run.trigger_type || run.user_context) && (
        <Card>
          <CardHeader className="pb-3">
            <div className="flex items-center gap-2">
              {run.trigger_type === 'webhook' ? (
                <Webhook className="h-4 w-4 text-muted-foreground" />
              ) : (
                <Terminal className="h-4 w-4 text-muted-foreground" />
              )}
              <CardTitle className="text-sm">
                Trigger: <span className="capitalize">{run.trigger_type}</span>
                {run.agent_name && (
                  <span className="text-muted-foreground font-normal">
                    {' '}— {run.agent_name}
                  </span>
                )}
              </CardTitle>
            </div>
          </CardHeader>
          {run.user_context && (
            <CardContent className="pt-0">
              <pre className="whitespace-pre-wrap text-xs font-mono bg-muted p-3 rounded-md overflow-x-auto max-h-48 overflow-y-auto">
                {(() => {
                  try {
                    return JSON.stringify(JSON.parse(run.user_context), null, 2)
                  } catch {
                    return run.user_context
                  }
                })()}
              </pre>
            </CardContent>
          )}
        </Card>
      )}

      {/* Error message */}
      {run.error_message && (
        <Alert variant="destructive">
          <AlertTriangle className="h-4 w-4" />
          <AlertTitle>Error</AlertTitle>
          <AlertDescription className="whitespace-pre-wrap font-mono text-xs">
            {run.error_message}
          </AlertDescription>
        </Alert>
      )}

      {/* Report / Analysis */}
      {run.analysis && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Report</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-sm">
              <Markdown>{run.analysis}</Markdown>
            </div>
          </CardContent>
        </Card>
      )}

      {/* AI Result */}
      {run.ai_output && <AiOutputCard output={run.ai_output} />}

      {/* AI Conversation from streamed events */}
      {(() => {
        const aiEvents = logs.filter((l: AgentRunLog) => l.level === 'ai_event')
        if (aiEvents.length > 0 && !run.ai_output) {
          // Show real-time conversation from streamed logs (while running or if ai_output not saved)
          const combinedOutput = aiEvents.map((l: AgentRunLog) => l.message).join('\n')
          return <AiOutputCard output={combinedOutput} live={isStreaming} />
        }
        return null
      })()}

      {/* System logs (non-AI events) */}
      {(() => {
        const systemLogs = logs.filter((l: AgentRunLog) => l.level !== 'ai_event')
        if (systemLogs.length === 0) return null
        return (
          <Card>
            <CardHeader>
              <CardTitle className="text-sm">Logs</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="space-y-2">
                {systemLogs.map((log: AgentRunLog) => (
                  <div key={log.id} className="flex items-start gap-3">
                    <div className="flex flex-col items-center pt-1.5">
                      <div
                        className={cn(
                          'h-2 w-2 rounded-full flex-shrink-0',
                          logLevelColor(log.level)
                        )}
                      />
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="text-xs text-muted-foreground whitespace-nowrap">
                          {formatTimestamp(log.created_at)}
                        </span>
                        <span className="text-xs font-medium uppercase text-muted-foreground">
                          {log.level}
                        </span>
                      </div>
                      <p className="text-sm break-words">{log.message}</p>
                    </div>
                  </div>
                ))}
              </div>
            </CardContent>
          </Card>
        )
      })()}
    </div>
  )
}
