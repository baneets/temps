import { ProjectResponse } from '@/api/client'
import {
  getErrorGroupOptions,
  listErrorEventsOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  AlertTriangle,
  ArrowLeft,
  ExternalLink,
  GitBranch,
  Loader2,
  Send,
  Sparkles,
} from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { toast } from 'sonner'
import {
  startAnalysis,
  getAutofixerRun,
  addContext,
  startFix,
  createPR,
  cancelRun,
  reAnalyze,
  getLatestRunForError,
  type AutofixerRunLog,
} from './api'

interface AutofixerPanelProps {
  project: ProjectResponse
}

const activePhases = new Set(['analyzing', 'fixing'])

function parseConversationEvents(logs: AutofixerRunLog[]) {
  const events: Array<{
    type: string
    content?: string
    tool?: string
    toolInput?: Record<string, unknown>
    toolResult?: string
    result?: string
  }> = []

  for (const log of logs) {
    // User messages appear as their own bubble
    if (log.level === 'user_message') {
      events.push({ type: 'user_message', content: log.message })
      continue
    }

    if (log.level !== 'ai_event') continue
    const trimmed = log.message.trim()
    if (!trimmed.startsWith('{')) continue
    try {
      const parsed = JSON.parse(trimmed)
      if (parsed.type === 'assistant' && parsed.message?.content) {
        for (const block of parsed.message.content) {
          if (block.type === 'text' && block.text) {
            events.push({ type: 'text', content: block.text })
          } else if (block.type === 'tool_use') {
            events.push({ type: 'tool_call', tool: block.name, toolInput: block.input })
          }
        }
      } else if (parsed.type === 'tool_result') {
        const content = Array.isArray(parsed.content)
          ? parsed.content.map((c: { text?: string }) => c.text || '').join('')
          : typeof parsed.content === 'string' ? parsed.content : ''
        events.push({ type: 'tool_result', toolResult: content })
      } else if (parsed.type === 'result') {
        events.push({ type: 'result', result: parsed.result })
      }
    } catch {
      // skip
    }
  }
  return events
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

export function AutofixerPanel({ project }: AutofixerPanelProps) {
  const { errorGroupId } = useParams<{ errorGroupId: string }>()
  const queryClient = useQueryClient()
  const [runId, setRunId] = useState<number | null>(null)
  const [contextInput, setContextInput] = useState('')
  const [streamLogs, setStreamLogs] = useState<AutofixerRunLog[]>([])
  const [isStreaming, setIsStreaming] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)

  const groupId = parseInt(errorGroupId || '0')

  // On mount, check if there's an existing run for this error group
  useEffect(() => {
    if (groupId > 0 && !runId) {
      getLatestRunForError(project.id, groupId).then((existingRun) => {
        if (existingRun && !['cancelled'].includes(existingRun.status)) {
          setRunId(existingRun.id)
        }
      })
    }
  }, [groupId, project.id]) // eslint-disable-line react-hooks/exhaustive-deps

  // Fetch error group details
  const { data: errorGroup } = useQuery({
    ...getErrorGroupOptions({
      path: { group_id: groupId, project_id: project.id },
    }),
    enabled: groupId > 0,
  })

  // Fetch latest error event for stack trace
  const { data: errorEvents } = useQuery({
    ...listErrorEventsOptions({
      path: { group_id: groupId, project_id: project.id },
      query: { page_size: 1, page: 1 },
    }),
    enabled: groupId > 0,
  })

  const latestEvent = (errorEvents as any)?.data?.[0] || (errorEvents as any)?.[0]
  // Stack trace can be in data.stack_trace (temps format) or data.sentry.exception.values[0].stacktrace.frames (Sentry format)
  const stackTrace = latestEvent?.data?.stack_trace
    || latestEvent?.data?.sentry?.exception?.values?.[0]?.stacktrace?.frames
  const requestInfo = latestEvent?.data?.request || latestEvent?.data?.sentry?.request
  const environmentInfo = latestEvent?.data?.sentry?.environment || latestEvent?.data?.environment?.environment

  // Start analysis mutation
  const analyzeMutation = useMutation({
    mutationFn: () => startAnalysis(project.id, groupId),
    onSuccess: (run) => {
      setRunId(run.id)
      toast.success('Analysis started')
    },
    onError: (e: Error) => toast.error(e.message),
  })

  // Fetch run data when runId is set
  const { data, isLoading } = useQuery({
    queryKey: ['autofixer-run', project.id, runId],
    queryFn: () => getAutofixerRun(project.id, runId!),
    enabled: !!runId,
    refetchInterval: 3000,
  })

  const run = data?.run
  const fetchedLogs = data?.logs || []

  // SSE streaming
  useEffect(() => {
    if (!runId || !run) return
    const phase = run.phase || ''
    if (!activePhases.has(phase) && !['pending', 'cloning'].includes(run.status)) return

    setIsStreaming(true)
    const es = new EventSource(`/api/projects/${project.id}/autofixer/runs/${runId}/stream`)

    es.onmessage = (event) => {
      try {
        const log = JSON.parse(event.data) as AutofixerRunLog
        setStreamLogs((prev) => {
          if (prev.some((l) => l.id === log.id)) return prev
          return [...prev, log]
        })
      } catch { /* skip */ }
    }

    es.addEventListener('status', () => {
      es.close()
      setIsStreaming(false)
      queryClient.invalidateQueries({ queryKey: ['autofixer-run', project.id, runId] })
    })

    es.onerror = () => {
      es.close()
      setIsStreaming(false)
    }

    return () => { es.close(); setIsStreaming(false) }
  }, [runId, run?.phase, run?.status, project.id, queryClient])

  // Auto-scroll
  useEffect(() => {
    if (isStreaming && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [streamLogs.length, isStreaming])

  // Merge logs — deduplicate optimistic user messages with real server logs
  const allLogs = (() => {
    const merged = [...fetchedLogs]
    for (const sl of streamLogs) {
      // Skip if already in fetched logs by ID
      if (sl.id > 0 && merged.some((l) => l.id === sl.id)) continue
      // For optimistic user messages (negative ID), skip if a real server log
      // with the same message already exists
      if (sl.id < 0 && sl.level === 'user_message') {
        const hasDuplicate = merged.some(
          (l) => l.level === 'user_message' && l.message === sl.message
        )
        if (hasDuplicate) continue
      }
      merged.push(sl)
    }
    // Sort: negative IDs (optimistic) go after positive IDs with the same timestamp
    return merged.sort((a, b) => {
      if (a.id > 0 && b.id > 0) return a.id - b.id
      if (a.id < 0 && b.id < 0) return a.id - b.id // both negative, keep insertion order
      // Place optimistic messages (negative) at the end
      if (a.id < 0) return 1
      if (b.id < 0) return -1
      return 0
    })
  })()

  const conversationEvents = parseConversationEvents(allLogs)

  // Fix mutation
  const fixMutation = useMutation({
    mutationFn: () => startFix(project.id, runId!),
    onSuccess: () => {
      toast.success('Fix generation started')
      setStreamLogs([])
      queryClient.invalidateQueries({ queryKey: ['autofixer-run', project.id, runId] })
    },
    onError: (e: Error) => toast.error(e.message),
  })

  // Create PR mutation
  const prMutation = useMutation({
    mutationFn: () => createPR(project.id, runId!),
    onSuccess: () => {
      toast.success('PR created!')
      queryClient.invalidateQueries({ queryKey: ['autofixer-run', project.id, runId] })
    },
    onError: (e: Error) => toast.error(e.message),
  })

  const [isSendingContext, setIsSendingContext] = useState(false)

  // Send context — show message immediately, save to backend, and auto-trigger re-analysis if in "analyzed" phase
  const sendContext = async () => {
    if (!contextInput.trim() || !runId || isSendingContext) return
    const message = contextInput.trim()
    setContextInput('')
    setIsSendingContext(true)

    // Optimistic: show user message in the stream immediately
    const optimisticLog: AutofixerRunLog = {
      id: -Date.now(), // negative temp id to avoid collision with real DB ids
      run_id: runId,
      level: 'user_message',
      message,
      metadata: null,
      created_at: new Date().toISOString(),
    }
    setStreamLogs((prev) => [...prev, optimisticLog])

    try {
      await addContext(project.id, runId, message)

      // If we're in the "analyzed" phase, automatically trigger re-analysis
      if (run?.phase === 'analyzed') {
        await reAnalyze(project.id, runId)
        queryClient.invalidateQueries({ queryKey: ['autofixer-run', project.id, runId] })
      }
    } catch {
      toast.error('Failed to send message')
    } finally {
      setIsSendingContext(false)
    }
  }

  // Error detail sidebar (shared between initial and active states)
  const eg = errorGroup as any
  const errorDetailPanel = eg ? (
    <div className="space-y-4">
      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center gap-2 mb-2">
            <Badge variant="destructive" className="text-xs">
              {eg.error_type || 'Error'}
            </Badge>
            <span className="text-xs text-muted-foreground">
              {eg.total_count || 0} occurrences
            </span>
          </div>
          <CardTitle className="text-base leading-snug">
            <Link
              to={`/projects/${project.slug}/errors/${errorGroupId}`}
              className="hover:underline inline-flex items-center gap-1.5 group"
            >
              {eg.title}
              <ExternalLink className="h-3.5 w-3.5 text-muted-foreground flex-shrink-0" />
            </Link>
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {eg.first_seen && (
            <div className="text-xs text-muted-foreground">
              First seen: {new Date(eg.first_seen).toLocaleString()}
            </div>
          )}
          {stackTrace && Array.isArray(stackTrace) && stackTrace.length > 0 && (
            <div>
              <p className="text-xs font-medium mb-1">Stack Trace</p>
              <pre className="text-xs font-mono bg-black/30 p-3 rounded-lg overflow-x-auto max-h-64 overflow-y-auto text-muted-foreground leading-relaxed">
{stackTrace.map((frame: any) => {
  const file = frame.filename || frame.abs_path || '?'
  const func = frame.function || '?'
  const line = frame.lineno || '?'
  const col = frame.colno ? `:${frame.colno}` : ''
  return `  at ${func} (${file}:${line}${col})\n`
}).join('')}
              </pre>
            </div>
          )}
          {/* Exception message */}
          {latestEvent?.exception_value && (
            <div>
              <p className="text-xs font-medium mb-1">Error Message</p>
              <p className="text-sm text-muted-foreground">{latestEvent.exception_value}</p>
            </div>
          )}
          {requestInfo && (
            <div>
              <p className="text-xs font-medium mb-1">Request</p>
              <div className="text-xs text-muted-foreground bg-black/30 p-2 rounded">
                <p className="truncate">{requestInfo.url}</p>
              </div>
            </div>
          )}
          {environmentInfo && (
            <div>
              <p className="text-xs font-medium mb-1">Environment</p>
              <p className="text-xs text-muted-foreground">{environmentInfo}</p>
            </div>
          )}
          {latestEvent?.data?.sentry?.release && (
            <div>
              <p className="text-xs font-medium mb-1">Release</p>
              <p className="text-xs text-muted-foreground font-mono truncate">{latestEvent.data.sentry.release}</p>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  ) : null

  // Initial state — no run yet
  if (!runId && !analyzeMutation.isPending) {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="icon" asChild>
            <Link to={`/projects/${project.slug}/errors/${errorGroupId}`}>
              <ArrowLeft className="h-4 w-4" />
            </Link>
          </Button>
          <h1 className="text-xl font-semibold">Fix with AI</h1>
        </div>

        <div className="flex flex-col lg:flex-row gap-4">
          {/* Left: Error details */}
          <div className="w-full lg:w-[400px] flex-shrink-0">
            {errorDetailPanel}
          </div>

          {/* Right: AI interaction */}
          <div className="flex-1">
            <Card>
              <CardContent className="p-6 space-y-4">
                <div>
                  <Sparkles className="h-6 w-6 text-muted-foreground mb-2" />
                  <p className="text-sm text-muted-foreground">
                    Claude will read your codebase, trace the stack trace, and identify the root cause.
                    You can review the analysis before generating a fix.
                  </p>
                </div>
                <textarea
                  placeholder="Add context (optional): e.g., 'This started after the auth migration'"
                  value={contextInput}
                  onChange={(e) => {
                    setContextInput(e.target.value)
                    e.target.style.height = 'auto'
                    e.target.style.height = e.target.scrollHeight + 'px'
                  }}
                  rows={3}
                  className="w-full rounded-md border border-[var(--border)] bg-transparent px-3 py-2 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-[var(--primary)] resize-none overflow-hidden"
                />
                <Button
                  className="w-full"
                  onClick={() => analyzeMutation.mutate()}
                  disabled={analyzeMutation.isPending}
                >
                  {analyzeMutation.isPending ? (
                    <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  ) : (
                    <Sparkles className="h-4 w-4 mr-2" />
                  )}
                  Start Analysis
                </Button>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    )
  }

  if (isLoading && !run) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (!run) return null

  // Determine current step for the timeline
  const phase = run.phase || run.status
  const steps = [
    { id: 'analyze', label: 'Analyze', done: ['analyzed', 'fixing', 'fix_ready', 'completed'].includes(phase), active: phase === 'analyzing' },
    { id: 'review', label: 'Review', done: ['fixing', 'fix_ready', 'completed'].includes(phase), active: phase === 'analyzed' },
    { id: 'fix', label: 'Generate Fix', done: ['fix_ready', 'completed'].includes(phase), active: phase === 'fixing' },
    { id: 'pr', label: 'Create PR', done: phase === 'completed', active: phase === 'fix_ready' },
  ]

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="icon" asChild>
            <Link to={`/projects/${project.slug}/errors/${errorGroupId}`}>
              <ArrowLeft className="h-4 w-4" />
            </Link>
          </Button>
          <h1 className="text-xl font-semibold">Fix with AI</h1>
          {isStreaming && (
            <span className="text-xs text-green-400 animate-pulse">LIVE</span>
          )}
        </div>
        <div className="flex gap-2">
          {!['completed', 'failed', 'cancelled', 'analyzed', 'fix_ready'].includes(run.status) && (
            <Button
              variant="outline"
              size="sm"
              onClick={async () => {
                await cancelRun(project.id, run.id)
                queryClient.invalidateQueries({ queryKey: ['autofixer-run', project.id, runId] })
              }}
            >
              Cancel
            </Button>
          )}
        </div>
      </div>

      {/* Workflow timeline */}
      <div className="flex items-center gap-1 px-2">
        {steps.map((step, i) => (
          <div key={step.id} className="flex items-center gap-1 flex-1">
            <div className="flex items-center gap-2 flex-1">
              <div className={`flex items-center justify-center w-6 h-6 rounded-full text-xs font-medium flex-shrink-0 ${
                step.done ? 'bg-green-500/20 text-green-400 border border-green-500/30' :
                step.active ? 'bg-blue-500/20 text-blue-400 border border-blue-500/30 animate-pulse' :
                'bg-muted/30 text-muted-foreground border border-[var(--border)]'
              }`}>
                {step.done ? '✓' : i + 1}
              </div>
              <span className={`text-xs whitespace-nowrap ${
                step.done ? 'text-green-400' :
                step.active ? 'text-blue-400 font-medium' :
                'text-muted-foreground'
              }`}>
                {step.label}
              </span>
            </div>
            {i < steps.length - 1 && (
              <div className={`h-px flex-1 min-w-4 ${step.done ? 'bg-green-500/30' : 'bg-[var(--border)]'}`} />
            )}
          </div>
        ))}
      </div>

      {/* Phase action bar */}
      {(phase === 'analyzed' || phase === 'fix_ready' || phase === 'completed' || run.status === 'failed' || run.status === 'cancelled') && (
        <Card className={
          phase === 'completed' ? 'border-green-500/20 bg-green-500/5' :
          (run.status === 'failed' || run.status === 'cancelled') ? 'border-red-500/20 bg-red-500/5' :
          'border-blue-500/20 bg-blue-500/5'
        }>
          <CardContent className="p-4 flex items-center justify-between gap-4">
            <div className="text-sm">
              {phase === 'analyzed' && 'Analysis complete. Send feedback below to refine, or generate a fix.'}
              {phase === 'fix_ready' && 'Fix is ready. Review the changes, then create a pull request.'}
              {phase === 'completed' && run.pr_url && (
                <span className="flex items-center gap-2">
                  PR created successfully —{' '}
                  <a
                    href={run.pr_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-green-400 hover:underline font-medium inline-flex items-center gap-1"
                  >
                    View PR #{run.pr_number} <ExternalLink className="h-3 w-3" />
                  </a>
                </span>
              )}
              {phase === 'completed' && !run.pr_url && (
                <span>
                  Completed
                  {run.completed_at && (
                    <span className="text-muted-foreground"> — {new Date(run.completed_at).toLocaleString()}</span>
                  )}
                </span>
              )}
              {(run.status === 'failed' || run.status === 'cancelled') && (
                <span className="text-red-400">
                  {run.status === 'failed' ? 'Run failed' : 'Run cancelled'}
                  {run.completed_at && (
                    <span className="text-red-400/70"> — {new Date(run.completed_at).toLocaleString()}</span>
                  )}
                </span>
              )}
            </div>
            <div className="flex gap-2 flex-shrink-0">
              {['completed', 'failed', 'cancelled'].includes(run.status) && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    setRunId(null)
                    setStreamLogs([])
                    setContextInput('')
                  }}
                >
                  Start Over
                </Button>
              )}
              {phase === 'analyzed' && (
                <Button onClick={() => fixMutation.mutate()} disabled={fixMutation.isPending} size="sm">
                  {fixMutation.isPending ? (
                    <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  ) : (
                    <Sparkles className="h-4 w-4 mr-2" />
                  )}
                  Generate Fix
                </Button>
              )}
              {phase === 'fix_ready' && (
                <Button onClick={() => prMutation.mutate()} disabled={prMutation.isPending} size="sm">
                  {prMutation.isPending ? (
                    <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  ) : (
                    <GitBranch className="h-4 w-4 mr-2" />
                  )}
                  Create PR
                </Button>
              )}
            </div>
          </CardContent>
        </Card>
      )}

      <div className="flex flex-col lg:flex-row gap-4">
        {/* Left: Error details (persistent) */}
        <div className="w-full lg:w-[400px] flex-shrink-0">
          {errorDetailPanel}
        </div>

        {/* Right: AI interaction */}
        <div className="flex-1 space-y-4">

      {/* Error */}
      {run.error_message && (
        <Alert variant="destructive">
          <AlertTriangle className="h-4 w-4" />
          <AlertTitle>Error</AlertTitle>
          <AlertDescription className="text-xs font-mono whitespace-pre-wrap">
            {run.error_message}
          </AlertDescription>
        </Alert>
      )}


      {/* Loading state — show while waiting for first events */}
      {conversationEvents.length === 0 && ['analyzing', 'fixing'].includes(run.phase || '') && (
        <Card>
          <CardContent className="p-8 flex flex-col items-center justify-center">
            <Loader2 className="h-8 w-8 animate-spin text-muted-foreground mb-3" />
            <p className="text-sm text-muted-foreground">
              {run.phase === 'analyzing' ? 'Analyzing the error...' : 'Generating fix...'}
            </p>
          </CardContent>
        </Card>
      )}

      {/* Live AI conversation */}
      {conversationEvents.length > 0 && (
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle className="text-sm">
                {run.phase === 'analyzing' ? 'Analysis' : run.phase === 'fixing' ? 'Generating Fix' : 'AI Conversation'}
              </CardTitle>
              <div className="flex gap-3 text-xs text-muted-foreground">
                {run.tokens_input > 0 && (
                  <span>{run.tokens_input.toLocaleString()} / {run.tokens_output.toLocaleString()} tokens</span>
                )}
              </div>
            </div>
          </CardHeader>
          <CardContent>
            <div ref={scrollRef} className="space-y-3 max-h-[500px] overflow-y-auto">
              {conversationEvents.map((event, i) => {
                if (event.type === 'user_message') {
                  return (
                    <div key={i} className="flex justify-end">
                      <div className="max-w-[80%] rounded-lg bg-primary/10 border border-primary/20 px-3 py-2">
                        <p className="text-xs font-medium text-primary/70 mb-1">You</p>
                        <p className="text-sm">{event.content}</p>
                      </div>
                    </div>
                  )
                }
                if (event.type === 'text') {
                  return (
                    <div key={i} className="text-sm leading-relaxed">
                      {renderMarkdown(event.content || '')}
                    </div>
                  )
                }
                if (event.type === 'tool_call') {
                  const input = event.toolInput || {}
                  const preview =
                    event.tool === 'Read' ? (input.file_path as string) || '' :
                    event.tool === 'Edit' ? (input.file_path as string) || '' :
                    event.tool === 'Bash' ? (input.command as string) || '' :
                    event.tool === 'Grep' ? (input.pattern as string) || '' :
                    JSON.stringify(input).slice(0, 120)
                  return (
                    <div key={i} className="flex items-start gap-2 rounded-md bg-blue-500/5 border border-blue-500/10 px-3 py-2 min-w-0 overflow-hidden">
                      <span className="text-xs font-mono font-medium text-blue-400 whitespace-nowrap flex-shrink-0">{event.tool}</span>
                      <span className="text-xs font-mono text-muted-foreground truncate min-w-0">{preview}</span>
                    </div>
                  )
                }
                if (event.type === 'tool_result') {
                  const text = event.toolResult || ''
                  if (text.length < 3) return null
                  return (
                    <pre key={i} className="text-xs font-mono bg-muted/50 p-2 rounded overflow-x-auto max-h-24 overflow-y-auto text-muted-foreground">
                      {text.length > 300 ? text.slice(0, 300) + '...' : text}
                    </pre>
                  )
                }
                if (event.type === 'result' && event.result) {
                  return (
                    <div key={i} className="rounded-md bg-green-500/5 border border-green-500/10 p-4">
                      <p className="text-xs font-medium text-green-400 mb-2">Summary</p>
                      <div className="text-sm leading-relaxed">
                        {renderMarkdown(event.result)}
                      </div>
                    </div>
                  )
                }
                return null
              })}
              {/* Active working indicator */}
              {isStreaming && (
                <div className="flex items-center gap-2 pt-2 border-t border-[var(--border)]">
                  <div className="flex gap-1">
                    <div className="w-1.5 h-1.5 rounded-full bg-blue-400 animate-bounce" style={{ animationDelay: '0ms' }} />
                    <div className="w-1.5 h-1.5 rounded-full bg-blue-400 animate-bounce" style={{ animationDelay: '150ms' }} />
                    <div className="w-1.5 h-1.5 rounded-full bg-blue-400 animate-bounce" style={{ animationDelay: '300ms' }} />
                  </div>
                  <span className="text-xs text-muted-foreground">
                    {run.phase === 'analyzing' ? 'Claude is reading your code...' :
                     run.phase === 'fixing' ? 'Claude is writing the fix...' :
                     'Working...'}
                  </span>
                </div>
              )}
            </div>
          </CardContent>
        </Card>
      )}

      {/* Chat input — during analysis: adds context; during review: sends feedback and re-analyzes */}
      {(run.phase === 'analyzing' || run.phase === 'analyzed') && (
        <div className="flex gap-2 items-end">
          <textarea
            placeholder={
              run.phase === 'analyzed'
                ? "Send feedback to re-analyze: e.g., 'Check the auth middleware' or 'This is not a test bug, fix it anyway'"
                : "Add context: e.g., 'This started after the auth migration'"
            }
            value={contextInput}
            onChange={(e) => {
              setContextInput(e.target.value)
              e.target.style.height = 'auto'
              e.target.style.height = e.target.scrollHeight + 'px'
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                sendContext()
              }
            }}
            rows={1}
            className="flex-1 rounded-md border border-[var(--border)] bg-transparent px-3 py-2 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-[var(--primary)] resize-none overflow-hidden"
            disabled={isSendingContext}
          />
          <Button
            variant="outline"
            size="icon"
            onClick={sendContext}
            disabled={isSendingContext || !contextInput.trim()}
          >
            {isSendingContext ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Send className="h-4 w-4" />
            )}
          </Button>
        </div>
      )}

        </div>{/* end right panel */}
      </div>{/* end two-column */}
    </div>
  )
}
