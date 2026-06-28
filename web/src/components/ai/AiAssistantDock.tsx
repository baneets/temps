import {
  type GlobalConversationResponse,
  archiveConversation,
  listAllConversations,
} from '@/api/client'
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
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Skeleton } from '@/components/ui/skeleton'
import { TimeAgo } from '@/components/utils/TimeAgo'
import { cn } from '@/lib/utils'
import {
  Bell,
  ChevronLeft,
  ExternalLink,
  GitBranch,
  MessageSquare,
  RotateCcw,
  Sparkles,
  Trash2,
  X,
} from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { AiChatContext, useAiAssistant } from './AiAssistantContext'
import { DebugChatPanel } from './DebugChatPanel'

interface ActiveChat {
  projectId: number
  projectSlug?: string
  projectName?: string
  contextType: string
  contextId: string | number
  title?: string
  description?: string
  startPrompt?: string
  autoStart: boolean
}

const CONTEXT_META: Record<string, { label: string; Icon: typeof GitBranch }> =
  {
    deployment: { label: 'Deployment', Icon: GitBranch },
    alert: { label: 'Alert', Icon: Bell },
  }

function metaFor(contextType: string) {
  return (
    CONTEXT_META[contextType] ?? { label: contextType, Icon: MessageSquare }
  )
}

/** Route to the entity a chat was started from, when we know its project. */
function sourceHref(a: {
  contextType: string
  contextId: string | number
  projectSlug?: string
}): string | null {
  if (!a.projectSlug) return null
  if (a.contextType === 'deployment') {
    return `/projects/${a.projectSlug}/deployments/${a.contextId}`
  }
  if (a.contextType === 'alert') {
    return `/projects/${a.projectSlug}/metrics/alerts/${a.contextId}/edit`
  }
  return null
}

/**
 * The persistent AI assistant dock (ADR-023). Rendered once in the app shell as
 * a flex sibling of the page content, so opening it *pushes* the layout rather
 * than covering it with a modal backdrop — the rest of the console stays fully
 * interactive and the chat keeps streaming while the user navigates. Width
 * animates to zero when closed.
 */
export function AiAssistantDock() {
  const { isOpen, initialContext, openSeq, close } = useAiAssistant()

  return (
    <aside
      aria-hidden={!isOpen}
      className={cn(
        'shrink-0 overflow-hidden bg-background transition-[width] duration-300 ease-in-out',
        isOpen
          ? 'w-full border-l sm:w-[24rem] lg:w-[28rem] xl:w-[32rem]'
          : 'w-0 border-l-0'
      )}
    >
      {/* Pinned full-height column; page content scrolls independently to the
          left, so the dock stays put ("sticky") while navigating. */}
      <div className="sticky top-0 h-svh w-full">
        {isOpen && (
          <DockBody
            key={openSeq}
            initialContext={initialContext}
            onClose={close}
          />
        )}
      </div>
    </aside>
  )
}

function DockBody({
  initialContext,
  onClose,
}: {
  initialContext?: AiChatContext
  onClose: () => void
}) {
  const navigate = useNavigate()
  const { projectId: openedProjectId } = useAiAssistant()

  // Navigate to a chat's source. On narrow screens the dock covers the whole
  // viewport, so close it after navigating — otherwise it looks like nothing
  // happened. On wide screens the dock stays open beside the page.
  const goToSource = (href: string) => {
    navigate(href)
    if (typeof window !== 'undefined' && window.innerWidth < 1024) onClose()
  }

  const [active, setActive] = useState<ActiveChat | null>(
    initialContext && openedProjectId != null
      ? {
          projectId: openedProjectId,
          projectSlug: initialContext.projectSlug,
          projectName: initialContext.projectName,
          contextType: initialContext.contextType,
          contextId: initialContext.contextId,
          title: initialContext.title,
          description: initialContext.description,
          startPrompt: initialContext.startPrompt,
          autoStart: true,
        }
      : null
  )
  const [conversations, setConversations] = useState<
    GlobalConversationResponse[]
  >([])
  const [loadingList, setLoadingList] = useState(false)
  const [activePublicId, setActivePublicId] = useState<string | null>(null)
  const [resetKey, setResetKey] = useState(0)
  const [pendingDelete, setPendingDelete] = useState<{
    projectId: number
    publicId: string
    title: string
  } | null>(null)

  // Unified list across every project.
  const loadList = useCallback(() => {
    setLoadingList(true)
    listAllConversations()
      .then(({ data }) => setConversations(data ?? []))
      .catch(() => setConversations([]))
      .finally(() => setLoadingList(false))
  }, [])

  useEffect(() => {
    if (initialContext) return
    // Defer to a microtask so the synchronous setLoadingList(true) inside
    // loadList() doesn't fire during the effect's render-commit phase (which
    // would trigger a cascading render warning).
    let cancelled = false
    queueMicrotask(() => {
      if (!cancelled) loadList()
    })
    return () => {
      cancelled = true
    }
  }, [initialContext, loadList])

  const openConversation = (c: GlobalConversationResponse) => {
    setActivePublicId(null)
    setActive({
      projectId: c.project_id,
      projectSlug: c.project_slug ?? undefined,
      projectName: c.project_name ?? undefined,
      contextType: c.context_type,
      contextId: c.context_id,
      title: c.title ?? undefined,
      autoStart: false,
    })
  }

  const backToList = () => {
    setActive(null)
    setActivePublicId(null)
    loadList()
  }

  // Reset = archive the current conversation and start a fresh one for the same
  // source (re-seeds + re-diagnoses).
  const resetConversation = async () => {
    if (active && activePublicId) {
      await archiveConversation({
        path: { project_id: active.projectId, public_id: activePublicId },
      }).catch(() => {})
    }
    setActivePublicId(null)
    setActive((a) => (a ? { ...a, autoStart: true } : a))
    setResetKey((k) => k + 1)
  }

  // Delete = archive (soft delete): the chat drops out of the list. Confirmed
  // via the dialog so it isn't triggered by accident.
  const confirmDelete = async () => {
    const d = pendingDelete
    if (!d) return
    setPendingDelete(null)
    await archiveConversation({
      path: { project_id: d.projectId, public_id: d.publicId },
    }).catch(() => {})
    toast.success('Chat deleted')
    if (active && activePublicId === d.publicId) {
      backToList()
    } else {
      setConversations((prev) => prev.filter((c) => c.public_id !== d.publicId))
    }
  }

  const inConversation = active !== null
  const href = active ? sourceHref(active) : null
  const sourceLabel = active
    ? `${metaFor(active.contextType).label}${active.projectName ? ` · ${active.projectName}` : ''}`
    : ''

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0 space-y-1">
          <div className="flex items-center gap-2">
            {inConversation ? (
              <button
                type="button"
                onClick={backToList}
                className="-ml-1 inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-sm font-normal text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                aria-label="Back to all conversations"
              >
                <ChevronLeft className="h-4 w-4" />
                <span className="text-xs">All chats</span>
              </button>
            ) : (
              <Sparkles className="h-5 w-5 text-primary" />
            )}
            <h2 className="min-w-0 truncate text-lg font-semibold">
              {inConversation ? (active?.title ?? 'AI chat') : 'AI assistant'}
            </h2>
          </div>
          {inConversation ? (
            <p className="flex flex-wrap items-center gap-1.5 text-sm text-muted-foreground">
              <span>{sourceLabel}</span>
              {href && (
                <button
                  type="button"
                  onClick={() => goToSource(href)}
                  className="inline-flex items-center gap-0.5 text-primary hover:underline"
                >
                  View source
                  <ExternalLink className="h-3 w-3" />
                </button>
              )}
            </p>
          ) : (
            <p className="text-sm text-muted-foreground">
              Resume any AI conversation across your projects, or start one from
              a failed deployment or alert.
            </p>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-0.5">
          {inConversation && (
            <button
              type="button"
              onClick={resetConversation}
              className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              title="Reset — archive this chat and start a new one"
              aria-label="Reset conversation"
            >
              <RotateCcw className="h-4 w-4" />
            </button>
          )}
          {inConversation && activePublicId && (
            <button
              type="button"
              onClick={() =>
                setPendingDelete({
                  projectId: active!.projectId,
                  publicId: activePublicId,
                  title: active!.title ?? 'this chat',
                })
              }
              className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-destructive"
              title="Delete this chat"
              aria-label="Delete conversation"
            >
              <Trash2 className="h-4 w-4" />
            </button>
          )}
          <button
            type="button"
            onClick={onClose}
            className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            aria-label="Close AI assistant"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>

      <div className="min-h-0 flex-1">
        {inConversation ? (
          <DebugChatPanel
            key={`${active!.contextType}:${active!.contextId}:${resetKey}`}
            projectId={active!.projectId}
            contextType={active!.contextType}
            contextId={active!.contextId}
            startPrompt={
              active!.startPrompt ??
              'Diagnose this and suggest concrete next steps.'
            }
            autoStart={active!.autoStart}
            onConversationChange={setActivePublicId}
          />
        ) : (
          <ConversationList
            loading={loadingList}
            conversations={conversations}
            onOpen={openConversation}
            onOpenSource={(c) => {
              const h = sourceHref({
                contextType: c.context_type,
                contextId: c.context_id,
                projectSlug: c.project_slug ?? undefined,
              })
              if (h) goToSource(h)
            }}
            onDelete={(c) =>
              setPendingDelete({
                projectId: c.project_id,
                publicId: c.public_id,
                title:
                  c.title ?? `${metaFor(c.context_type).label} ${c.context_id}`,
              })
            }
          />
        )}
      </div>

      <AlertDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => !o && setPendingDelete(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this chat?</AlertDialogTitle>
            <AlertDialogDescription>
              “{pendingDelete?.title}” will be removed from your list. This
              can&apos;t be undone from here.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={confirmDelete}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

function ConversationList({
  loading,
  conversations,
  onOpen,
  onOpenSource,
  onDelete,
}: {
  loading: boolean
  conversations: GlobalConversationResponse[]
  onOpen: (c: GlobalConversationResponse) => void
  onOpenSource: (c: GlobalConversationResponse) => void
  onDelete: (c: GlobalConversationResponse) => void
}) {
  if (loading) {
    return (
      <div className="space-y-2">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-14 w-full" />
        ))}
      </div>
    )
  }
  if (conversations.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
        <MessageSquare className="h-7 w-7 text-muted-foreground" />
        <p className="text-sm font-medium">No AI conversations yet</p>
        <p className="max-w-xs text-sm text-muted-foreground">
          Open a failed deployment and choose “Debug with AI”, or investigate a
          firing alert, to start one.
        </p>
      </div>
    )
  }
  return (
    <div className="h-full space-y-1 overflow-y-auto pr-1">
      {conversations.map((c) => {
        const { label, Icon } = metaFor(c.context_type)
        const hasSource = !!sourceHref({
          contextType: c.context_type,
          contextId: c.context_id,
          projectSlug: c.project_slug ?? undefined,
        })
        return (
          <div
            key={c.public_id}
            className="group flex items-center gap-2 rounded-md border border-transparent pr-1 transition-colors hover:border-border hover:bg-accent"
          >
            <button
              type="button"
              onClick={() => onOpen(c)}
              className="flex min-w-0 flex-1 items-center gap-3 px-2 py-2.5 text-left"
            >
              <div className="relative shrink-0">
                <Avatar className="size-8 rounded-md">
                  <AvatarImage
                    src={`/api/projects/${c.project_id}/favicon`}
                    alt={c.project_name ?? 'Project'}
                  />
                  <AvatarFallback className="rounded-md bg-primary/10 text-xs font-medium text-primary">
                    {(c.project_name ?? label).slice(0, 1).toUpperCase()}
                  </AvatarFallback>
                </Avatar>
                {/* Context-type badge so the chat's source (deployment/alert)
                    stays visible over the project favicon. */}
                <span className="absolute -bottom-1 -right-1 flex h-4 w-4 items-center justify-center rounded-full border border-background bg-muted text-muted-foreground">
                  <Icon className="h-2.5 w-2.5" />
                </span>
              </div>
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">
                  {c.title ?? `${label} ${c.context_id}`}
                </div>
                <div className="flex items-center gap-1.5 truncate text-xs text-muted-foreground">
                  <span>{label}</span>
                  {c.project_name && (
                    <>
                      <span>·</span>
                      <span className="truncate">{c.project_name}</span>
                    </>
                  )}
                  <span>·</span>
                  <TimeAgo date={c.last_activity_at} />
                </div>
              </div>
            </button>
            {hasSource && (
              <button
                type="button"
                onClick={() => onOpenSource(c)}
                className="shrink-0 rounded-md p-1.5 text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-foreground group-hover:opacity-100"
                title="Go to source"
                aria-label="Go to source"
              >
                <ExternalLink className="h-3.5 w-3.5" />
              </button>
            )}
            <button
              type="button"
              onClick={() => onDelete(c)}
              className="shrink-0 rounded-md p-1.5 text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-destructive group-hover:opacity-100"
              title="Delete chat"
              aria-label="Delete chat"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </button>
          </div>
        )
      })}
    </div>
  )
}
