import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Loader2, MessageSquare, Send } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'

interface ChatMessage {
  role: string
  content: string
}

interface DebugChatProps {
  projectId: number
  /** The interaction this chat is attached to, e.g. 'deployment' | 'alert'. */
  contextType: string
  contextId: string | number
  title?: string
  description?: string
  /** Auto-asked when a new chat is started, so it opens already working. */
  startPrompt?: string
  triggerLabel?: string
}

/**
 * Persistent, resumable AI debugging chat attached to any entity (ADR-023).
 * Render only when the project's `ai_debug_chat_enabled` toggle is on. The
 * streaming reply is consumed via a manual SSE fetch reader (the generated SDK
 * can't stream); find/history use the same cookie-authed `/api` surface.
 */
export function DebugChat({
  projectId,
  contextType,
  contextId,
  title = 'Debug with AI',
  description = 'Start an AI chat to investigate this issue.',
  startPrompt = 'Diagnose this and suggest concrete next steps.',
  triggerLabel = 'Debug with AI',
}: DebugChatProps) {
  const base = `/api/projects/${projectId}/ai/conversations`
  const ctxId = String(contextId)
  const [publicId, setPublicId] = useState<string | null>(null)
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [input, setInput] = useState('')
  const [streaming, setStreaming] = useState(false)
  const [starting, setStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  // Load an existing conversation for this context, if one exists.
  useEffect(() => {
    let ignore = false
    fetch(
      `${base}?context_type=${encodeURIComponent(contextType)}&context_id=${encodeURIComponent(ctxId)}`,
      { credentials: 'include' }
    )
      .then((r) => (r.ok ? r.json() : null))
      .then(async (conv) => {
        if (ignore || !conv) return
        setPublicId(conv.public_id)
        const detail = await fetch(`${base}/${conv.public_id}`, {
          credentials: 'include',
        })
          .then((r) => (r.ok ? r.json() : null))
          .catch(() => null)
        if (!ignore && detail) setMessages(detail.messages ?? [])
      })
      .catch(() => {})
    return () => {
      ignore = true
    }
  }, [base, contextType, ctxId])

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight })
  }, [messages])

  const send = useCallback(
    async (text: string, conversationId?: string) => {
      const id = conversationId ?? publicId
      const content = text.trim()
      if (!id || !content) return
      setInput('')
      setError(null)
      setStreaming(true)
      setMessages((m) => [
        ...m,
        { role: 'user', content },
        { role: 'assistant', content: '' },
      ])
      try {
        const res = await fetch(`${base}/${id}/messages`, {
          method: 'POST',
          credentials: 'include',
          headers: {
            'Content-Type': 'application/json',
            Accept: 'text/event-stream',
          },
          body: JSON.stringify({ content }),
        })
        if (!res.ok || !res.body) {
          const problem = await res.json().catch(() => ({}))
          setError(problem.detail || 'The AI request failed.')
          return
        }
        const reader = res.body.getReader()
        const decoder = new TextDecoder()
        let buffer = ''
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          buffer += decoder.decode(value, { stream: true })
          let boundary
          while ((boundary = buffer.indexOf('\n\n')) >= 0) {
            const rawEvent = buffer.slice(0, boundary)
            buffer = buffer.slice(boundary + 2)
            let isError = false
            const dataParts: string[] = []
            for (const line of rawEvent.split('\n')) {
              if (line.startsWith('event:')) {
                if (line.slice(6).trim() === 'error') isError = true
              } else if (line.startsWith('data:')) {
                dataParts.push(line.slice(5).replace(/^ /, ''))
              }
            }
            const chunk = dataParts.join('\n')
            if (isError) {
              if (chunk) setError(chunk)
              continue
            }
            if (chunk) {
              setMessages((m) => {
                const copy = [...m]
                const last = copy[copy.length - 1]
                copy[copy.length - 1] = {
                  role: 'assistant',
                  content: (last?.content ?? '') + chunk,
                }
                return copy
              })
            }
          }
        }
      } catch {
        setError('Connection error while talking to the AI.')
      } finally {
        setStreaming(false)
      }
    },
    [base, publicId]
  )

  const start = useCallback(async () => {
    setStarting(true)
    setError(null)
    try {
      const res = await fetch(base, {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ context_type: contextType, context_id: ctxId }),
      })
      if (!res.ok) {
        const problem = await res.json().catch(() => ({}))
        setError(
          problem.detail ||
            'Could not start the chat. Make sure an AI provider is configured.'
        )
        return
      }
      const conv = await res.json()
      setPublicId(conv.public_id)
      setMessages([])
      void send(startPrompt, conv.public_id)
    } catch {
      setError('Could not start the chat.')
    } finally {
      setStarting(false)
    }
  }, [base, contextType, ctxId, startPrompt, send])

  const visible = messages.filter((m) => m.role !== 'system')

  if (!publicId) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <MessageSquare className="h-5 w-5" />
            {title}
          </CardTitle>
          <CardDescription>{description}</CardDescription>
        </CardHeader>
        <CardContent>
          <Button onClick={start} disabled={starting}>
            {starting ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <MessageSquare className="h-4 w-4" />
            )}
            <span className="ml-2">{triggerLabel}</span>
          </Button>
          {error && <p className="mt-3 text-sm text-destructive">{error}</p>}
        </CardContent>
      </Card>
    )
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <MessageSquare className="h-5 w-5" />
          {title}
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div
          ref={scrollRef}
          className="max-h-[28rem] space-y-3 overflow-y-auto rounded-md border p-3"
        >
          {visible.length === 0 && (
            <p className="text-sm text-muted-foreground">Analyzing…</p>
          )}
          {visible.map((m, i) => (
            <div key={i} className={m.role === 'user' ? 'text-right' : ''}>
              <div
                className={`inline-block max-w-[85%] whitespace-pre-wrap rounded-lg px-3 py-2 text-left text-sm ${
                  m.role === 'user'
                    ? 'bg-primary text-primary-foreground'
                    : 'bg-muted'
                }`}
              >
                {m.content ||
                  (streaming && i === visible.length - 1 ? '…' : '')}
              </div>
            </div>
          ))}
        </div>
        {error && <p className="text-sm text-destructive">{error}</p>}
        <div className="flex items-end gap-2">
          <Textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="Ask a follow-up…"
            rows={2}
            disabled={streaming}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                void send(input)
              }
            }}
          />
          <Button
            onClick={() => void send(input)}
            disabled={streaming || !input.trim()}
            size="icon"
          >
            {streaming ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Send className="h-4 w-4" />
            )}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
