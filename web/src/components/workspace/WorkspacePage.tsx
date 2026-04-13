import { useEffect, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  GitBranch,
  Loader2,
  PanelLeft,
  Plus,
  RotateCcw,
  Send,
  Settings,
  Sparkles,
  Trash2,
  MoreHorizontal,
  X,
} from 'lucide-react'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'

import {
  cancelRun,
  closeSession,
  deleteSession,
  reopenSession,
  getSession,
  listSessions,
  sendMessage,
  sessionStreamUrl,
  updateSession,
  type WorkspaceMessage,
  type WorkspaceSession,
} from './api'
import { SessionPreviewCard } from './SessionPreviewCard'
import { TerminalTabs, type TerminalTabsHandle } from './TerminalTabs'
import { SandboxStatsBadge } from './SandboxStatsBadge'
import { TerminalKeysMenu } from './TerminalKeysMenu'
import { usePageTitle } from '@/hooks/usePageTitle'

/** Display title for a session — falls back to "Session #{id}" when blank. */
function sessionDisplayTitle(s: { id: number; title: string | null }): string {
  return s.title?.trim() ? s.title : `Session #${s.id}`
}

interface Project {
  id: number
  name: string
  slug: string
  repo_owner?: string
  repo_name?: string
  main_branch?: string
  git_provider_connection_id?: number
}

interface WorkspacePageProps {
  project: Project
}

export function WorkspacePage({ project }: WorkspacePageProps) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()

  // Active session id is persisted in the URL (?session=N) so a hard refresh
  // (F5) lands on the same conversation. setActiveSessionId is a thin wrapper
  // around setSearchParams that keeps callers ergonomic.
  const [searchParams, setSearchParams] = useSearchParams()
  const sessionParam = searchParams.get('session')
  const activeSessionId = sessionParam ? Number(sessionParam) || null : null
  const setActiveSessionId = (id: number | null) => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev)
        if (id === null) next.delete('session')
        else next.set('session', String(id))
        return next
      },
      { replace: false },
    )
  }
  const [streamedMessages, setStreamedMessages] = useState<WorkspaceMessage[]>([])
  const [inputValue, setInputValue] = useState('')
  // Sidebar is hidden by default; users open it via the toggle in the chat
  // panel header. The mobile-style session Select in the chat header always
  // works regardless of sidebar state.
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [previewOpenMobile, setPreviewOpenMobile] = useState(false)
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false)
  const eventSourceRef = useRef<EventSource | null>(null)
  const messagesEndRef = useRef<HTMLDivElement | null>(null)
  const terminalRef = useRef<TerminalTabsHandle | null>(null)

  // Fetch sessions list
  const sessionsQuery = useQuery({
    queryKey: ['workspace', project.id, 'sessions'],
    queryFn: () => listSessions(project.id),
  })

  // Fetch active session detail
  const activeSessionQuery = useQuery({
    queryKey: ['workspace', project.id, 'session', activeSessionId],
    queryFn: () => getSession(project.id, activeSessionId!),
    enabled: activeSessionId !== null,
  })

  // Navigate to the dedicated new-session page. Skills + MCP lists can be
  // long, so a full page lets them grow vertically with filtering.
  const openNewSession = () => {
    navigate('new', { relative: 'path' })
  }

  // Close session mutation
  const closeActiveSession = useMutation({
    mutationFn: (sessionId: number) => closeSession(project.id, sessionId),
    onSuccess: () => {
      setActiveSessionId(null)
      setStreamedMessages([])
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'sessions'],
      })
      toast.success('Session closed')
    },
    onError: (error: Error) => {
      toast.error(`Failed to close session: ${error.message}`)
    },
  })

  // Delete session mutation (hard delete: cancels run, destroys sandbox,
  // cascades messages).
  const deleteSessionMutation = useMutation({
    mutationFn: (sessionId: number) => deleteSession(project.id, sessionId),
    onSuccess: (_, sessionId) => {
      if (activeSessionId === sessionId) {
        setActiveSessionId(null)
        setStreamedMessages([])
      }
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'sessions'],
      })
      toast.success('Session deleted')
    },
    onError: (error: Error) => {
      toast.error(`Failed to delete session: ${error.message}`)
    },
  })

  // Inline title editing for the active session.
  const [editingTitle, setEditingTitle] = useState(false)
  const [titleDraft, setTitleDraft] = useState('')

  // Terminal ↔ Chat view toggle. Terminal is the canonical path now: it
  // attaches directly to the AI CLI running inside the sandbox via tmux,
  // with zero abstraction. Chat is kept for legacy users and will be
  // removed once terminal usage is proven in production.
  const [viewMode, setViewMode] = useState<'terminal' | 'chat'>('terminal')

  // When flipping back to the terminal view, the wrapper goes from
  // `display: none` to `display: flex`. ResizeObserver doesn't fire for
  // that transition reliably, so xterm.js stays stuck at whatever cols/rows
  // it had when it was first hidden (often ~10 cols, which produces the
  // narrow-column wrap-mess seen in the screenshot). We dispatch a
  // `temps:terminal-show` event after the DOM update; SessionTerminal
  // listens for it and re-fits, re-sends the size to the PTY, and asks
  // tmux to repaint with Ctrl-L.
  useEffect(() => {
    if (viewMode !== 'terminal') return
    const id = window.setTimeout(() => {
      window.dispatchEvent(new Event('temps:terminal-show'))
    }, 0)
    return () => window.clearTimeout(id)
  }, [viewMode])
  const renameSessionMutation = useMutation({
    mutationFn: (title: string | null) =>
      updateSession(project.id, activeSessionId!, { title }),
    onSuccess: () => {
      setEditingTitle(false)
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'session', activeSessionId],
      })
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'sessions'],
      })
      toast.success('Session renamed')
    },
    onError: (e: Error) => toast.error(e.message),
  })

  // Drive the browser tab title from the active session.
  const activeSession = activeSessionQuery.data?.session
  usePageTitle(
    activeSession ? sessionDisplayTitle(activeSession) : 'Workspace',
  )

  // Reopen session mutation
  const reopenActiveSession = useMutation({
    mutationFn: (sessionId: number) => reopenSession(project.id, sessionId),
    onSuccess: (session) => {
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'sessions'],
      })
      // Also invalidate the active session *detail* query — otherwise the
      // WorkspacePage keeps rendering the closed-session placeholder because
      // `activeSessionQuery.data.session.status` stays stale.
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'session', session.id],
      })
      setActiveSessionId(session.id)
      toast.success('Session reopened')
    },
    onError: (error: Error) => {
      toast.error(`Failed to reopen session: ${error.message}`)
    },
  })

  // Send message mutation
  const sendMessageMutation = useMutation({
    mutationFn: (content: string) =>
      sendMessage(project.id, activeSessionId!, { content }),
    onSuccess: () => {
      setInputValue('')
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'session', activeSessionId],
      })
    },
    onError: (error: Error) => {
      toast.error(`Failed to send message: ${error.message}`)
    },
  })

  // Manual cancel/reset escape hatch. Writes a synthetic terminal turn so
  // the UI's "Thinking…" indicator clears immediately, regardless of whether
  // the underlying executor is actually wedged. The user is never trapped.
  const cancelMutation = useMutation({
    mutationFn: () => cancelRun(project.id, activeSessionId!),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ['workspace', project.id, 'session', activeSessionId],
      })
      toast.success('Run cancelled')
    },
    onError: (error: Error) => {
      toast.error(`Failed to cancel: ${error.message}`)
    },
  })

  // SSE stream subscription
  useEffect(() => {
    if (!activeSessionId) {
      if (eventSourceRef.current) {
        eventSourceRef.current.close()
        eventSourceRef.current = null
      }
      return
    }

    // Determine the last known message ID
    const existingMessages = activeSessionQuery.data?.messages ?? []
    const lastId =
      existingMessages.length > 0
        ? existingMessages[existingMessages.length - 1]!.id
        : 0

    const url = sessionStreamUrl(project.id, activeSessionId, lastId)
    const eventSource = new EventSource(url)
    eventSourceRef.current = eventSource

    eventSource.addEventListener('message', (event) => {
      try {
        const msg = JSON.parse(event.data) as WorkspaceMessage
        setStreamedMessages((prev) => {
          // Dedupe by id
          if (prev.some((m) => m.id === msg.id)) return prev
          return [...prev, msg]
        })
        // The session row's sandbox_container_id is written by the backend
        // when initialize_sandbox finishes. The chat stream is the only thing
        // running while that happens, so refetch the session whenever we see
        // a new message and the cached row still says "not started" — that's
        // when we'd otherwise be stuck showing stale UI.
        const cached = activeSessionQuery.data?.session
        if (cached && !cached.sandbox_container_id) {
          queryClient.invalidateQueries({
            queryKey: ['workspace', project.id, 'session', activeSessionId],
          })
        }
      } catch (err) {
        console.error('Failed to parse SSE message', err)
      }
    })

    eventSource.addEventListener('status', (event) => {
      const status = (event as MessageEvent).data
      if (status === 'closed') {
        eventSource.close()
      }
    })

    eventSource.onerror = () => {
      // EventSource auto-reconnects; log and continue
      console.warn('SSE stream error, will retry')
    }

    return () => {
      eventSource.close()
      if (eventSourceRef.current === eventSource) {
        eventSourceRef.current = null
      }
    }
  }, [activeSessionId, project.id, activeSessionQuery.data?.messages])

  // Auto-scroll to bottom on new messages
  const allMessages = mergeMessages(
    activeSessionQuery.data?.messages ?? [],
    streamedMessages,
  )

  // Determine whether Claude is currently thinking. We say "yes" when the
  // latest non-event message is a user message — i.e. the user has spoken
  // and the canonical assistant row hasn't landed yet. The label is derived
  // from the most recent ai_event so the user sees what tool is running.
  const { isThinking, thinkingLabel } = (() => {
    let lastUserId = -1
    let lastAssistantId = -1
    for (const m of allMessages) {
      if (m.role === 'user' && m.id > lastUserId) lastUserId = m.id
      // A `system` "Error: ..." breadcrumb counts as a terminal turn so the
      // spinner clears even if the assistant row never lands.
      if (
        (m.role === 'assistant' ||
          (m.role === 'system' && m.content.startsWith('Error:'))) &&
        m.id > lastAssistantId
      )
        lastAssistantId = m.id
    }
    const thinking = lastUserId > lastAssistantId && lastUserId !== -1
    if (!thinking) return { isThinking: false, thinkingLabel: '' }
    let label = 'Thinking…'
    for (let i = allMessages.length - 1; i >= 0; i--) {
      const m = allMessages[i]!
      if (m.id <= lastUserId) break
      if (m.role === 'ai_event') {
        const hint = extractThinkingHint(m.content)
        if (hint) {
          label = hint
          break
        }
      }
    }
    return { isThinking: true, thinkingLabel: label }
  })()

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [allMessages.length])

  // Sending while a turn is in flight is allowed — the backend queues the
  // message and merges it into the next turn. We only block on local
  // mutation pending (network in-flight) or when there's no active session.
  const handleSend = () => {
    const trimmed = inputValue.trim()
    if (!trimmed || !activeSessionId || sendMessageMutation.isPending) return
    sendMessageMutation.mutate(trimmed)
  }

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSend()
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col lg:flex-row gap-2 lg:gap-4 -m-2 sm:-m-4 p-2 sm:p-4">
      {/* Left: Sessions list. Hidden by default; toggled via the PanelLeft
          button in the chat header. */}
      <Card
        className={`${sidebarOpen ? 'flex' : 'hidden'} w-full lg:w-72 flex-col`}
      >
        <div className="p-4 border-b">
          <div className="flex items-center justify-between mb-2">
            <h3 className="font-semibold flex items-center gap-2">
              <Sparkles className="h-4 w-4" />
              Sessions
            </h3>
            <Button size="sm" onClick={openNewSession}>
              <Plus className="h-4 w-4" />
              New
            </Button>
          </div>
          <p className="text-xs text-muted-foreground">
            Chat with AI that has full platform access
          </p>
        </div>
        <ScrollArea className="flex-1">
          <div className="p-2 space-y-1">
            {sessionsQuery.isPending && (
              <div className="text-sm text-muted-foreground p-2">Loading…</div>
            )}
            {sessionsQuery.data?.sessions.map((session) => (
              <SessionListItem
                key={session.id}
                session={session}
                active={session.id === activeSessionId}
                onClick={() => {
                  setActiveSessionId(session.id)
                  setStreamedMessages([])
                }}
              />
            ))}
            {sessionsQuery.data?.sessions.length === 0 && (
              <div className="text-sm text-muted-foreground p-2">
                No sessions yet. Click "New" to start.
              </div>
            )}
          </div>
        </ScrollArea>
      </Card>

      {/* Right: Chat panel */}
      <Card className="flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
        {activeSessionId === null ? (
          <SessionPickerState
            sessions={sessionsQuery.data?.sessions ?? []}
            loading={sessionsQuery.isLoading}
            onSelect={(id) => {
              setActiveSessionId(id)
              setStreamedMessages([])
            }}
            onCreate={openNewSession}
            creating={false}
          />
        ) : (
          <>
            <div className="p-2 lg:p-4 border-b flex items-center justify-between gap-2">
              <div className="min-w-0 flex items-center gap-2">
                <Button
                  size="icon"
                  variant="ghost"
                  onClick={() => setSidebarOpen((v) => !v)}
                  title={sidebarOpen ? 'Hide sessions' : 'Show sessions'}
                >
                  <PanelLeft className="h-4 w-4" />
                </Button>
                <div className="min-w-0">
                <h3 className="font-semibold flex items-center gap-2 min-w-0">
                  {editingTitle ? (
                    <Input
                      autoFocus
                      value={titleDraft}
                      onChange={(e) => setTitleDraft(e.target.value)}
                      onBlur={() => {
                        const trimmed = titleDraft.trim()
                        const current = activeSession?.title ?? ''
                        if (trimmed === current) {
                          setEditingTitle(false)
                          return
                        }
                        renameSessionMutation.mutate(trimmed || null)
                      }}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          e.currentTarget.blur()
                        } else if (e.key === 'Escape') {
                          setEditingTitle(false)
                          setTitleDraft(activeSession?.title ?? '')
                        }
                      }}
                      placeholder={`Session #${activeSessionId}`}
                      maxLength={200}
                      className="h-7 text-sm font-semibold max-w-[280px]"
                    />
                  ) : (
                    <button
                      type="button"
                      onClick={() => {
                        setTitleDraft(activeSession?.title ?? '')
                        setEditingTitle(true)
                      }}
                      className="truncate text-left hover:underline decoration-dotted underline-offset-4"
                      title="Click to rename"
                    >
                      {activeSession
                        ? sessionDisplayTitle(activeSession)
                        : `Session #${activeSessionId}`}
                    </button>
                  )}
                  {activeSessionQuery.data?.session.branch_name && (
                    <span className="inline-flex items-center gap-1 text-xs font-mono text-muted-foreground bg-muted px-1.5 py-0.5 rounded shrink-0">
                      <GitBranch className="h-3 w-3" />
                      {activeSessionQuery.data.session.branch_name}
                      {activeSessionQuery.data.session.base_branch_name && (
                        <span className="text-[10px] opacity-70">
                          ← {activeSessionQuery.data.session.base_branch_name}
                        </span>
                      )}
                    </span>
                  )}
                  {activeSessionQuery.data?.session.skills?.map((slug) => (
                    <a
                      key={`skill-${slug}`}
                      href={`/settings/skills/${slug}`}
                      title={`Skill: ${slug}`}
                      className="inline-flex items-center gap-1 text-xs font-mono text-muted-foreground bg-muted px-1.5 py-0.5 rounded shrink-0 hover:bg-muted/80"
                    >
                      <Sparkles className="h-3 w-3" />
                      {slug}
                    </a>
                  ))}
                  {activeSessionQuery.data?.session.mcp_servers?.map((slug) => (
                    <a
                      key={`mcp-${slug}`}
                      href={`/settings/mcp-servers/${slug}`}
                      title={`MCP server: ${slug}`}
                      className="inline-flex items-center gap-1 text-xs font-mono text-muted-foreground bg-muted px-1.5 py-0.5 rounded shrink-0 hover:bg-muted/80"
                    >
                      {'{mcp}'} {slug}
                    </a>
                  ))}
                </h3>
                <p className="text-xs text-muted-foreground hidden lg:block">
                  {activeSessionQuery.data?.session.ai_provider ?? 'claude_cli'}
                  {activeSessionQuery.data?.session.tokens_input
                    ? ` · ${activeSessionQuery.data.session.tokens_input} in / ${activeSessionQuery.data.session.tokens_output} out tokens`
                    : ''}
                </p>
                </div>
              </div>
              <div className="flex items-center gap-1 shrink-0">
                <Button
                  size="icon"
                  variant="ghost"
                  onClick={openNewSession}
                  title="New session"
                >
                  <Plus className="h-4 w-4" />
                </Button>
                <Button
                  size="icon"
                  variant="ghost"
                  onClick={() => setPreviewOpenMobile((v) => !v)}
                  title="Preview & sandbox settings"
                  aria-pressed={previewOpenMobile}
                >
                  <Settings className="h-4 w-4" />
                </Button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button size="icon" variant="ghost" title="Session actions">
                      <MoreHorizontal className="h-4 w-4" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    {activeSessionQuery.data?.session.status === 'closed' ? (
                      <DropdownMenuItem
                        onSelect={() =>
                          reopenActiveSession.mutate(activeSessionId)
                        }
                        disabled={reopenActiveSession.isPending}
                      >
                        <RotateCcw className="h-4 w-4" />
                        Reopen
                      </DropdownMenuItem>
                    ) : (
                      <DropdownMenuItem
                        onSelect={() =>
                          closeActiveSession.mutate(activeSessionId)
                        }
                        disabled={closeActiveSession.isPending}
                      >
                        <X className="h-4 w-4" />
                        Close
                      </DropdownMenuItem>
                    )}
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      disabled={deleteSessionMutation.isPending}
                      className="text-destructive focus:text-destructive"
                      onSelect={(e) => {
                        e.preventDefault()
                        setDeleteDialogOpen(true)
                      }}
                    >
                      <Trash2 className="h-4 w-4" />
                      Delete
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
                <AlertDialog
                  open={deleteDialogOpen}
                  onOpenChange={setDeleteDialogOpen}
                >
                  <AlertDialogContent>
                    <AlertDialogHeader>
                      <AlertDialogTitle>
                        Delete session #{activeSessionId}?
                      </AlertDialogTitle>
                      <AlertDialogDescription>
                        This permanently deletes the session, all its
                        messages, and destroys its sandbox container. This
                        action cannot be undone.
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel>Cancel</AlertDialogCancel>
                      <AlertDialogAction
                        onClick={() =>
                          deleteSessionMutation.mutate(activeSessionId)
                        }
                        className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                      >
                        Delete
                      </AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>
              </div>
            </div>

            {activeSessionQuery.data?.session && previewOpenMobile && (
              <div className="px-2 pt-2 lg:px-4 lg:pt-3">
                <SessionPreviewCard
                  projectId={project.id}
                  session={activeSessionQuery.data.session}
                  defaultExpanded
                />
              </div>
            )}

            {/* When the session is closed, replace both the view toggle and
                the terminal/chat panes with a single centered Reopen action.
                No point showing tabs over a dead sandbox. */}
            {activeSessionQuery.data?.session.status === 'closed' ? (
              <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 p-6 text-center">
                <p className="text-sm text-muted-foreground max-w-sm">
                  This session is closed. Reopen it to attach the terminal and
                  resume your workspace.
                </p>
                <Button
                  size="lg"
                  onClick={() => reopenActiveSession.mutate(activeSessionId)}
                  disabled={reopenActiveSession.isPending}
                >
                  {reopenActiveSession.isPending ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <RotateCcw className="h-4 w-4" />
                  )}
                  Reopen session
                </Button>
              </div>
            ) : (
              <>
            {/* View toggle: Terminal (raw PTY) ↔ Chat (alpha). */}
            <div className="border-t border-b bg-muted/30 px-2 py-1 flex items-center gap-1">
              <button
                type="button"
                onClick={() => setViewMode('terminal')}
                className={`px-2.5 py-1 text-xs rounded font-medium transition-colors ${
                  viewMode === 'terminal'
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                Terminal
              </button>
              <button
                type="button"
                onClick={() => setViewMode('chat')}
                className={`px-2.5 py-1 text-xs rounded font-medium transition-colors ${
                  viewMode === 'chat'
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                Chat <span className="ml-1 rounded bg-amber-500/15 px-1 py-0.5 text-[9px] font-semibold uppercase tracking-wide text-amber-600 dark:text-amber-400">Alpha</span>
              </button>
            </div>

            {/* Both panes are always mounted — only visibility toggles. This
                keeps the xterm.js instance + websocket alive when the user
                flips to Chat and back, instead of tearing down and re-attaching
                (which loses scrollback, drops the PTY connection, and forces
                tmux to re-render the whole screen). */}
            <div
              className={`relative flex-1 min-w-0 min-h-0 w-full overflow-hidden ${
                viewMode === 'terminal' ? 'flex' : 'hidden'
              }`}
            >
              {activeSessionId != null &&
              activeSessionQuery.data?.session.sandbox_container_id ? (
                <>
                  <TerminalTabs
                    ref={terminalRef}
                    projectId={project.id}
                    sessionId={activeSessionId}
                    aiProvider={
                      activeSessionQuery.data?.session.ai_provider ?? 'claude_cli'
                    }
                  />
                  {/* Floating special-keys dropdown — anchored bottom-right
                      of the terminal pane so it stays out of the way of the
                      Claude prompt but is one tap away on mobile. */}
                  <div className="absolute bottom-2 right-2 z-50">
                    <TerminalKeysMenu terminalRef={terminalRef} />
                  </div>
                </>
              ) : (
                <div className="h-full w-full flex items-center justify-center text-sm text-muted-foreground p-4 text-center">
                  {activeSessionQuery.data?.session.status === 'closed'
                    ? 'Reopen this session to attach a terminal.'
                    : 'Sandbox is not running yet. Send a chat message or refresh the sandbox to provision one, then come back to the terminal.'}
                </div>
              )}
            </div>
            <div
              className={`flex-1 min-w-0 min-h-0 w-full overflow-y-auto overflow-x-hidden p-2 lg:p-4 ${
                viewMode === 'chat' ? 'block' : 'hidden'
              }`}
            >
              <div className="space-y-3 max-w-3xl mx-auto min-w-0">
                {allMessages.map((msg) => (
                  <MessageItem key={msg.id} message={msg} />
                ))}
                {(isThinking || sendMessageMutation.isPending) && (
                  <div className="flex justify-start w-full min-w-0">
                    <div className="max-w-[85%] min-w-0 rounded-lg p-3 bg-muted text-sm text-muted-foreground flex items-center gap-2 break-words [overflow-wrap:anywhere]">
                      <Loader2 className="h-4 w-4 animate-spin shrink-0" />
                      <span className="flex-1">
                        {thinkingLabel || 'Thinking…'}
                      </span>
                    </div>
                  </div>
                )}
                <div ref={messagesEndRef} />
              </div>
            </div>
              </>
            )}

            {/* Session status bar — branch, sandbox state at a glance,
                pinned right above the composer so it's always visible. */}
            {activeSessionQuery.data?.session && (
              <div className="border-t px-3 py-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground bg-muted/30">
                <span
                  className={
                    activeSessionQuery.data.session.status === 'active'
                      ? 'inline-flex items-center gap-1 text-emerald-600 dark:text-emerald-400'
                      : activeSessionQuery.data.session.status === 'closed'
                        ? 'inline-flex items-center gap-1 text-muted-foreground'
                        : 'inline-flex items-center gap-1 text-amber-600 dark:text-amber-400'
                  }
                >
                  <span className="h-1.5 w-1.5 rounded-full bg-current" />
                  {activeSessionQuery.data.session.status}
                </span>
                {activeSessionQuery.data.session.branch_name && (
                  <span className="inline-flex items-center gap-1 font-mono">
                    <GitBranch className="h-3 w-3" />
                    {activeSessionQuery.data.session.branch_name}
                    {activeSessionQuery.data.session.base_branch_name && (
                      <span className="text-muted-foreground/70">
                        ← {activeSessionQuery.data.session.base_branch_name}
                      </span>
                    )}
                  </span>
                )}
                <span className="font-mono">
                  {activeSessionQuery.data.session.sandbox_container_id
                    ? `sandbox ${activeSessionQuery.data.session.sandbox_container_id.slice(0, 12)}`
                    : 'sandbox not started'}
                </span>
                <SandboxStatsBadge
                  projectId={project.id}
                  sessionId={activeSessionQuery.data.session.id}
                  enabled={
                    !!activeSessionQuery.data.session.sandbox_container_id &&
                    activeSessionQuery.data.session.status === 'active'
                  }
                />
                {activeSessionQuery.data.session.idle_timeout_minutes != null && (
                  <span>
                    idle {activeSessionQuery.data.session.idle_timeout_minutes}m
                  </span>
                )}
              </div>
            )}

            {viewMode === 'chat' && (
            <div className="p-2 lg:p-4 border-t">
              <div className="flex gap-2 max-w-3xl mx-auto">
                <Textarea
                  value={inputValue}
                  onChange={(e) => setInputValue(e.target.value)}
                  onKeyDown={handleKeyDown}
                  placeholder={
                    isThinking
                      ? 'Queue another message — it runs after the current turn.'
                      : 'Ask about errors, analytics, deploys, or data.'
                  }
                  className="min-h-[44px] lg:min-h-[60px] resize-none"
                  disabled={sendMessageMutation.isPending}
                />
                <div className="flex flex-col gap-1">
                  <Button
                    onClick={handleSend}
                    disabled={!inputValue.trim() || sendMessageMutation.isPending}
                    title={isThinking ? 'Queue message for next turn' : 'Send message'}
                  >
                    <Send className="h-4 w-4" />
                  </Button>
                  {(isThinking || sendMessageMutation.isPending) && (
                    <Button
                      variant="destructive"
                      size="sm"
                      onClick={() => cancelMutation.mutate()}
                      disabled={cancelMutation.isPending}
                      title="Cancel run and drop queued messages"
                    >
                      {cancelMutation.isPending ? (
                        <Loader2 className="h-4 w-4 animate-spin" />
                      ) : (
                        <X className="h-4 w-4" />
                      )}
                    </Button>
                  )}
                </div>
              </div>
            </div>
            )}
          </>
        )}
      </Card>

    </div>
  )
}

// ── Helper components ──────────────────────────────────────────────────────

interface SessionListItemProps {
  session: WorkspaceSession
  active: boolean
  onClick: () => void
}

function SessionListItem({ session, active, onClick }: SessionListItemProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`w-full p-2 rounded-md text-sm text-left hover:bg-accent ${
        active ? 'bg-accent' : ''
      }`}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium truncate">{sessionDisplayTitle(session)}</span>
        <StatusBadge status={session.status} />
      </div>
      <div className="text-xs text-muted-foreground">
        {new Date(session.started_at).toLocaleString()}
      </div>
      {session.branch_name && (
        <div className="mt-1 inline-flex items-center gap-1 text-[11px] font-mono text-foreground/80 bg-muted px-1.5 py-0.5 rounded max-w-full">
          <GitBranch className="h-3 w-3 shrink-0" />
          <span className="truncate">{session.branch_name}</span>
          {session.base_branch_name && (
            <span className="text-[10px] opacity-60 shrink-0">
              ← {session.base_branch_name}
            </span>
          )}
        </div>
      )}
      <div className="text-[10px] text-muted-foreground font-mono mt-0.5 truncate">
        {session.sandbox_container_id
          ? `sandbox ${session.sandbox_container_id.slice(0, 12)}`
          : 'no sandbox'}
      </div>
    </button>
  )
}

function StatusBadge({ status }: { status: string }) {
  const color =
    status === 'active'
      ? 'bg-green-500/20 text-green-700 dark:text-green-400'
      : status === 'closed'
        ? 'bg-muted text-muted-foreground'
        : 'bg-yellow-500/20 text-yellow-700 dark:text-yellow-400'
  return (
    <span className={`text-xs px-1.5 py-0.5 rounded ${color}`}>{status}</span>
  )
}

interface MessageItemProps {
  message: WorkspaceMessage
}

function MessageItem({ message }: MessageItemProps) {
  // Parse stream-json ai_event lines. Tool calls/results render as a
  // compact timeline entry; everything else is suppressed (the canonical
  // assistant row carries the final text).
  if (message.role === 'ai_event') {
    const event = extractToolEvent(message.content)
    if (!event) return null
    return <ToolEventItem event={event} />
  }

  const isUser = message.role === 'user'
  const isSystem = message.role === 'system'
  // Backend marks failed assistant turns with `error: true` so the user sees
  // *why* the run died instead of an indefinite spinner.
  const isError =
    (message.metadata as { error?: boolean } | null)?.error === true ||
    (isSystem && message.content.startsWith('Error:'))

  const bubbleClass = isUser
    ? 'bg-primary text-primary-foreground ml-auto'
    : isError
      ? 'bg-destructive/10 text-destructive border border-destructive/30'
      : isSystem
        ? 'bg-muted/60 text-muted-foreground'
        : 'bg-muted'

  return (
    <div className={`flex w-full min-w-0 ${isUser ? 'justify-end' : 'justify-start'}`}>
      <div
        className={`max-w-[85%] min-w-0 rounded-lg p-3 ${bubbleClass}`}
      >
        <div className="text-xs font-medium mb-1 opacity-70">
          {isUser ? 'You' : isError ? 'Error' : isSystem ? 'System' : 'Assistant'}
        </div>
        {!isUser && !isSystem && !isError ? (
          <div className="text-sm break-words [overflow-wrap:anywhere] prose prose-sm dark:prose-invert max-w-none prose-pre:bg-background/60 prose-pre:text-foreground prose-pre:border prose-pre:border-border prose-code:before:content-none prose-code:after:content-none prose-p:my-2 prose-headings:my-3 prose-ul:my-2 prose-ul:list-disc prose-ul:pl-5 prose-ol:my-2 prose-ol:list-decimal prose-ol:pl-5 prose-li:my-0.5 prose-li:marker:text-foreground/60">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {message.content}
            </ReactMarkdown>
          </div>
        ) : (
          <div className="text-sm whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
            {message.content}
          </div>
        )}
      </div>
    </div>
  )
}

interface SessionPickerStateProps {
  sessions: WorkspaceSession[]
  loading: boolean
  onSelect: (id: number) => void
  onCreate: () => void
  creating: boolean
}

function SessionPickerState({
  sessions,
  loading,
  onSelect,
  onCreate,
  creating,
}: SessionPickerStateProps) {
  return (
    <div className="flex-1 min-h-0 overflow-y-auto flex flex-col items-center p-6 lg:p-10">
      <div className="w-full max-w-xl flex flex-col items-center my-auto">
        <Sparkles className="h-10 w-10 mb-3 text-muted-foreground" />
        <h3 className="text-lg font-semibold mb-1 text-center">
          Pick a session or start a new one
        </h3>
        <p className="text-sm text-muted-foreground mb-6 text-center">
          Chat with an AI that has full platform access — code, errors,
          analytics, deploys, and databases.
        </p>
        <Button
          onClick={onCreate}
          disabled={creating}
          className="mb-6"
        >
          {creating ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Plus className="h-4 w-4" />
          )}
          New session
        </Button>
        {loading ? (
          <div className="text-sm text-muted-foreground">
            Loading sessions…
          </div>
        ) : sessions.length === 0 ? (
          <div className="text-sm text-muted-foreground">No sessions yet.</div>
        ) : (
          <div className="w-full">
            <div className="text-xs uppercase tracking-wide text-muted-foreground mb-2">
              Recent sessions
            </div>
            <div className="flex flex-col divide-y rounded-md border max-h-[400px] overflow-y-auto">
              {sessions.slice(0, 10).map((s) => (
                <button
                  key={s.id}
                  type="button"
                  onClick={() => onSelect(s.id)}
                  className="flex items-center justify-between gap-3 px-3 py-2.5 text-left hover:bg-accent transition-colors first:rounded-t-md last:rounded-b-md"
                >
                  <div className="min-w-0 flex items-center gap-2">
                    <span className="font-medium text-sm truncate">
                      {sessionDisplayTitle(s)}
                    </span>
                    {s.branch_name && (
                      <span className="inline-flex items-center gap-1 text-xs font-mono text-muted-foreground">
                        <GitBranch className="h-3 w-3" />
                        {s.branch_name}
                      </span>
                    )}
                  </div>
                  <span
                    className={`text-[11px] uppercase tracking-wide ${
                      s.status === 'closed'
                        ? 'text-muted-foreground'
                        : 'text-emerald-500'
                    }`}
                  >
                    {s.status}
                  </span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

// ── Utilities ──────────────────────────────────────────────────────────────

function mergeMessages(
  fromQuery: WorkspaceMessage[],
  fromStream: WorkspaceMessage[],
): WorkspaceMessage[] {
  const byId = new Map<number, WorkspaceMessage>()
  for (const msg of fromQuery) byId.set(msg.id, msg)
  for (const msg of fromStream) byId.set(msg.id, msg)
  return Array.from(byId.values()).sort((a, b) => a.id - b.id)
}

// ── Tool event timeline ────────────────────────────────────────────────────

interface ToolEvent {
  kind: 'call' | 'result'
  toolUseId: string
  name?: string
  summary: string
  detail?: string
  isError?: boolean
}

/**
 * Parse a Claude CLI stream-json line into a ToolEvent (tool_use or tool_result).
 * Returns null for events we don't render (assistant text deltas, system init,
 * thinking, rate limit, etc.).
 */
function extractToolEvent(line: string): ToolEvent | null {
  let obj: Record<string, unknown>
  try {
    obj = JSON.parse(line)
  } catch {
    return null
  }
  const type = obj.type as string | undefined
  if (type === 'assistant') {
    const message = obj.message as { content?: unknown[] } | undefined
    const blocks = message?.content
    if (!Array.isArray(blocks)) return null
    for (const block of blocks) {
      const b = block as Record<string, unknown>
      if (b.type === 'tool_use') {
        const name = (b.name as string) ?? 'tool'
        const input = b.input as Record<string, unknown> | undefined
        return {
          kind: 'call',
          toolUseId: (b.id as string) ?? '',
          name,
          summary: summarizeToolInput(name, input),
          detail: input ? JSON.stringify(input, null, 2) : undefined,
        }
      }
    }
    return null
  }
  if (type === 'user') {
    const message = obj.message as { content?: unknown[] } | undefined
    const blocks = message?.content
    if (!Array.isArray(blocks)) return null
    for (const block of blocks) {
      const b = block as Record<string, unknown>
      if (b.type === 'tool_result') {
        const content = b.content
        let text = ''
        if (typeof content === 'string') {
          text = content
        } else if (Array.isArray(content)) {
          text = content
            .map((c) => {
              const cc = c as Record<string, unknown>
              if (typeof cc.text === 'string') return cc.text
              if (typeof cc.content === 'string') return cc.content
              return ''
            })
            .join('\n')
        }
        const trimmed = text.trim()
        return {
          kind: 'result',
          toolUseId: (b.tool_use_id as string) ?? '',
          summary: trimmed.length > 120 ? trimmed.slice(0, 120) + '…' : trimmed,
          detail: text,
          isError: b.is_error === true,
        }
      }
    }
    return null
  }
  return null
}

function summarizeToolInput(
  name: string,
  input: Record<string, unknown> | undefined,
): string {
  if (!input) return name
  switch (name) {
    case 'Read':
      return `Read ${(input.file_path as string) ?? ''}`
    case 'Write':
      return `Write ${(input.file_path as string) ?? ''}`
    case 'Edit':
      return `Edit ${(input.file_path as string) ?? ''}`
    case 'Bash': {
      const cmd = (input.command as string) ?? ''
      return `$ ${cmd.length > 100 ? cmd.slice(0, 100) + '…' : cmd}`
    }
    case 'Glob':
      return `Glob ${(input.pattern as string) ?? ''}`
    case 'Grep':
      return `Grep ${(input.pattern as string) ?? ''}`
    case 'Skill':
      return `Skill: ${(input.skill as string) ?? ''}`
    case 'WebFetch':
      return `Fetch ${(input.url as string) ?? ''}`
    case 'WebSearch':
      return `Search ${(input.query as string) ?? ''}`
    default:
      return name
  }
}

function ToolEventItem({ event }: { event: ToolEvent }) {
  const [open, setOpen] = useState(false)
  const isCall = event.kind === 'call'
  const colorClass = event.isError
    ? 'border-destructive/40 text-destructive'
    : isCall
      ? 'border-blue-500/30 text-blue-700 dark:text-blue-400'
      : 'border-muted-foreground/20 text-muted-foreground'
  const icon = isCall ? '→' : event.isError ? '✕' : '←'
  return (
    <div className="flex w-full min-w-0 justify-start">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className={`max-w-[85%] min-w-0 text-left text-xs font-mono border-l-2 pl-2 py-1 hover:bg-accent/30 rounded-r overflow-hidden ${colorClass}`}
      >
        <div className="flex items-center gap-1 min-w-0">
          <span className="shrink-0">{icon}</span>
          <span className="truncate min-w-0">
            {event.summary || '(empty)'}
          </span>
        </div>
        {open && event.detail && (
          <pre className="mt-1 text-[10px] whitespace-pre-wrap break-words [overflow-wrap:anywhere] opacity-80 max-h-60 overflow-y-auto overflow-x-hidden">
            {event.detail}
          </pre>
        )}
      </button>
    </div>
  )
}

/**
 * Inspect a Claude CLI stream-json line and return a short human-readable
 * status (e.g. "Reading file…", "Running bash…") if the event represents
 * Claude actively doing work. Returns null for events we don't want to
 * surface as a thinking hint.
 */
function extractThinkingHint(line: string): string | null {
  const trimmed = line.trim()
  if (!trimmed) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== 'object') return null
  const obj = parsed as Record<string, unknown>

  // tool_use lives inside assistant.message.content[]
  if (obj.type === 'assistant' && obj.message && typeof obj.message === 'object') {
    const msg = obj.message as Record<string, unknown>
    const content = msg.content
    if (Array.isArray(content)) {
      for (const part of content) {
        if (part && typeof part === 'object' && (part as { type?: string }).type === 'tool_use') {
          const name = (part as { name?: string }).name ?? 'tool'
          return labelForTool(name)
        }
      }
      // No tool_use → it's a text delta, meaning Claude is actively replying.
      return 'Writing response…'
    }
  }

  if (obj.type === 'user' && obj.message && typeof obj.message === 'object') {
    // tool_result echoes — Claude is processing the result of a tool call.
    return 'Processing tool result…'
  }

  return null
}

function labelForTool(name: string): string {
  switch (name) {
    case 'Read':
      return 'Reading file…'
    case 'Write':
      return 'Writing file…'
    case 'Edit':
      return 'Editing file…'
    case 'Bash':
      return 'Running command…'
    case 'Glob':
      return 'Searching files…'
    case 'Grep':
      return 'Searching code…'
    case 'WebFetch':
    case 'WebSearch':
      return 'Searching the web…'
    default:
      return `Running ${name}…`
  }
}
