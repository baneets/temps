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
import {
  AlertTriangle,
  ArrowLeft,
  Box,
  Clock,
  ExternalLink,
  FileCode,
  GitBranch,
  Hash,
  Zap,
} from 'lucide-react'
import { Link, useParams } from 'react-router-dom'
import { getAgentRun } from './api'
import type { AgentRunLog } from './api'
import { AutopilotStatusBadge } from './AutopilotStatusBadge'
import { cn } from '@/lib/utils'

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

/** Render inline markdown: **bold**, `code`, *italic* */
function renderInline(text: string): React.ReactNode {
  const parts: React.ReactNode[] = []
  let remaining = text
  let key = 0

  while (remaining.length > 0) {
    // Bold
    const boldMatch = remaining.match(/\*\*(.+?)\*\*/)
    // Inline code
    const codeMatch = remaining.match(/`(.+?)`/)

    const boldIdx = boldMatch?.index ?? Infinity
    const codeIdx = codeMatch?.index ?? Infinity

    if (boldIdx === Infinity && codeIdx === Infinity) {
      parts.push(remaining)
      break
    }

    if (boldIdx <= codeIdx && boldMatch) {
      parts.push(remaining.slice(0, boldIdx))
      parts.push(<strong key={key++} className="font-semibold">{boldMatch[1]}</strong>)
      remaining = remaining.slice(boldIdx + boldMatch[0].length)
    } else if (codeMatch) {
      parts.push(remaining.slice(0, codeIdx))
      parts.push(<code key={key++} className="text-xs bg-muted px-1 py-0.5 rounded font-mono">{codeMatch[1]}</code>)
      remaining = remaining.slice(codeIdx + codeMatch[0].length)
    }
  }

  return parts.length === 1 ? parts[0] : <>{parts}</>
}

/** Simple markdown-like renderer for AI text output */
function renderMarkdown(text: string) {
  const lines = text.split('\n')
  const elements: React.ReactNode[] = []
  let inCodeBlock = false
  let codeLines: string[] = []

  lines.forEach((line, idx) => {
    if (line.startsWith('```')) {
      if (inCodeBlock) {
        elements.push(
          <pre key={`code-${idx}`} className="text-xs font-mono bg-black/30 p-3 rounded-lg overflow-x-auto my-2 text-muted-foreground">
            {codeLines.join('\n')}
          </pre>
        )
        codeLines = []
        inCodeBlock = false
      } else {
        inCodeBlock = true
      }
      return
    }
    if (inCodeBlock) {
      codeLines.push(line)
      return
    }

    // Headings
    if (line.startsWith('### ')) {
      elements.push(<h4 key={idx} className="text-sm font-semibold mt-3 mb-1">{line.slice(4)}</h4>)
    } else if (line.startsWith('## ')) {
      elements.push(<h3 key={idx} className="text-base font-semibold mt-4 mb-1">{line.slice(3)}</h3>)
    } else if (line.startsWith('# ')) {
      elements.push(<h2 key={idx} className="text-lg font-semibold mt-4 mb-2">{line.slice(2)}</h2>)
    } else if (line.startsWith('- ') || line.startsWith('* ')) {
      elements.push(
        <div key={idx} className="flex gap-2 ml-2">
          <span className="text-muted-foreground">•</span>
          <span>{renderInline(line.slice(2))}</span>
        </div>
      )
    } else if (/^\d+\.\s/.test(line)) {
      const match = line.match(/^(\d+)\.\s(.*)/)
      if (match) {
        elements.push(
          <div key={idx} className="flex gap-2 ml-2">
            <span className="text-muted-foreground">{match[1]}.</span>
            <span>{renderInline(match[2])}</span>
          </div>
        )
      }
    } else if (line.trim() === '') {
      elements.push(<div key={idx} className="h-2" />)
    } else {
      elements.push(<p key={idx}>{renderInline(line)}</p>)
    }
  })

  return elements
}

interface ConversationEvent {
  type: string
  role?: string
  content?: string
  tool?: string
  toolInput?: Record<string, unknown>
  toolResult?: string
  result?: string
  numTurns?: number
  costUsd?: number
  durationMs?: number
}

/** Parse stream-json output into conversation events */
function parseStreamOutput(output: string): ConversationEvent[] {
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
              toolInput: block.input,
            })
          }
        }
      } else if (parsed.type === 'tool_result') {
        const content = Array.isArray(parsed.content)
          ? parsed.content.map((c: { text?: string }) => c.text || '').join('')
          : typeof parsed.content === 'string'
            ? parsed.content
            : JSON.stringify(parsed.content)
        events.push({ type: 'tool_result', toolResult: content })
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
                  {renderMarkdown(event.content || '')}
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
              return (
                <div
                  key={i}
                  className="flex items-start gap-2 rounded-md bg-blue-500/5 border border-blue-500/10 px-3 py-2"
                >
                  <span className="text-xs font-mono font-medium text-blue-400 whitespace-nowrap">
                    {event.tool}
                  </span>
                  <span className="text-xs font-mono text-muted-foreground truncate">
                    {preview}
                  </span>
                </div>
              )
            }
            if (event.type === 'tool_result') {
              const text = event.toolResult || ''
              if (!text || text.length < 3) return null
              return (
                <pre
                  key={i}
                  className="text-xs font-mono bg-muted/50 p-2 rounded overflow-x-auto max-h-32 overflow-y-auto text-muted-foreground"
                >
                  {text.length > 500 ? text.slice(0, 500) + '...' : text}
                </pre>
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
                  <div className="text-sm">{renderMarkdown(event.result)}</div>
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
  const [streamLogs, setStreamLogs] = useState<AgentRunLog[]>([])
  const [isStreaming, setIsStreaming] = useState(false)

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
            : 'Failed to load agent run'}
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
      </div>

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
            <div className="text-sm prose prose-invert max-w-none">
              {renderMarkdown(run.analysis)}
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
