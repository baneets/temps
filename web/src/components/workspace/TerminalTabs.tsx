// Tab bar + multi-terminal manager for a workspace session.
//
// One workspace session = one sandbox container = N terminal tabs. Each tab
// is its own tmux session inside the container, so they're fully independent
// (one running claude, one running htop, one running a build watcher). The
// tabs themselves are per-browser local state, but the tmux sessions persist
// across reloads — we list them on mount via `listTerminalTabs` so closing
// and reopening the workspace surfaces previously-running tabs.

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from 'react'
import {
  Plus,
  X,
  Terminal as TerminalIcon,
  Sparkles,
  RotateCw,
} from 'lucide-react'

import {
  SessionTerminal,
  type SessionTerminalHandle,
  type TerminalStatus,
} from './SessionTerminal'
import {
  deleteTerminalTab,
  listTerminalTabs,
  type TerminalTab as TerminalTabRecord,
} from './api'

interface TerminalTabsProps {
  projectId: number
  sessionId: number
  /// Provider id from the session (`claude_cli`, `codex_cli`, `opencode`).
  /// Drives the primary tab's label + icon so it matches which CLI actually
  /// runs inside the sandbox. `claude_cli` is the fallback for older
  /// sessions created before this field existed.
  aiProvider: string
}

/// Display label for the primary AI tab — derived from the session's
/// provider so we never show "claude" when the container is actually
/// running codex. Keep short so it fits the tab strip on small screens.
function providerTabLabel(providerId: string): string {
  switch (providerId) {
    case 'codex_cli':
      return 'codex'
    case 'opencode':
      return 'opencode'
    case 'claude_cli':
    default:
      return 'claude'
  }
}

export type TerminalTabsHandle = SessionTerminalHandle

interface LocalTab {
  kind: 'claude' | 'shell'
  /** Stable id; combined with kind = tmux session name. */
  id: string
  /** Display label in the tab bar. */
  label: string
}

function tabKey(t: { kind: string; id: string }): string {
  return `${t.kind}-${t.id}`
}

function makeShellId(): string {
  // Short, URL-safe id. Doesn't need to be cryptographic — just unique per
  // workspace session for the lifetime of the user's tabs.
  return Math.random().toString(36).slice(2, 8)
}

export const TerminalTabs = forwardRef<TerminalTabsHandle, TerminalTabsProps>(
  function TerminalTabs({ projectId, sessionId, aiProvider }, ref) {
  const handlesRef = useRef<Map<string, SessionTerminalHandle | null>>(new Map())
  const activeKeyRef = useRef<string>('claude-main')
  const primaryLabel = providerTabLabel(aiProvider)

  useImperativeHandle(ref, () => ({
    sendKeys: (data: string) => {
      const h = handlesRef.current.get(activeKeyRef.current)
      h?.sendKeys(data)
    },
    focus: () => {
      const h = handlesRef.current.get(activeKeyRef.current)
      h?.focus()
    },
  }))
  // The default primary tab always exists. Its label tracks the session's
  // provider (claude/codex/opencode) so the UI matches what's actually
  // running inside the sandbox. Anything else is restored from the server
  // on mount or added by the user clicking +.
  const [tabs, setTabs] = useState<LocalTab[]>([
    { kind: 'claude', id: 'main', label: primaryLabel },
  ])
  const [activeKey, setActiveKey] = useState<string>('claude-main')
  activeKeyRef.current = activeKey

  // Per-tab websocket status + reconnect counter. Bumping the reconnect
  // counter for a tab key forces the underlying SessionTerminal to drop
  // its websocket and reopen a fresh one (same tmux session on the
  // server, so no state is lost — tmux just re-attaches a new client).
  const [statuses, setStatuses] = useState<Record<string, TerminalStatus>>({})
  const [reconnectKeys, setReconnectKeys] = useState<Record<string, number>>({})
  const setStatusFor = (key: string, s: TerminalStatus) =>
    setStatuses((prev) => (prev[key] === s ? prev : { ...prev, [key]: s }))
  const reconnect = (key: string) =>
    setReconnectKeys((prev) => ({ ...prev, [key]: (prev[key] ?? 0) + 1 }))

  // Keep the primary AI tab's label in sync with the active session's
  // provider. Without this, switching sessions (or the user activating a
  // new provider in settings) would leave the tab stuck on the label the
  // component was first mounted with.
  useEffect(() => {
    setTabs((current) =>
      current.map((t) =>
        t.kind === 'claude' && t.id === 'main'
          ? { ...t, label: primaryLabel }
          : t
      )
    )
  }, [primaryLabel])

  // Rehydrate tabs from the container on mount: any tmux session named
  // `temps-{kind}-{id}` that we don't already know about gets added.
  useEffect(() => {
    let cancelled = false
    listTerminalTabs(projectId, sessionId)
      .then((existing) => {
        if (cancelled) return
        setTabs((current) => {
          const seen = new Set(current.map(tabKey))
          const additions: LocalTab[] = []
          let shellCount = current.filter((t) => t.kind === 'shell').length
          for (const t of existing as TerminalTabRecord[]) {
            const key = tabKey(t)
            if (seen.has(key)) continue
            additions.push({
              kind: t.kind,
              id: t.id,
              label:
                t.kind === 'claude'
                  ? t.id === 'main'
                    ? primaryLabel
                    : `${primaryLabel} (${t.id})`
                  : `shell ${++shellCount}`,
            })
          }
          return additions.length > 0 ? [...current, ...additions] : current
        })
      })
      .catch(() => {
        // Non-fatal — fall back to the default claude tab. The container
        // might just not have any extra tmux sessions yet.
      })
    return () => {
      cancelled = true
    }
  }, [projectId, sessionId])

  const addShell = () => {
    const id = makeShellId()
    const shellCount = tabs.filter((t) => t.kind === 'shell').length + 1
    const newTab: LocalTab = {
      kind: 'shell',
      id,
      label: `shell ${shellCount}`,
    }
    setTabs((t) => [...t, newTab])
    setActiveKey(tabKey(newTab))
  }

  const closeTab = async (target: LocalTab) => {
    // Refuse to close the last claude tab — it's the workspace's primary
    // surface and reopening it has friction.
    if (target.kind === 'claude' && target.id === 'main') return
    setTabs((current) => {
      const next = current.filter((t) => tabKey(t) !== tabKey(target))
      if (activeKey === tabKey(target)) {
        const fallback = next[next.length - 1] ?? next[0]
        if (fallback) setActiveKey(tabKey(fallback))
      }
      return next
    })
    try {
      await deleteTerminalTab(projectId, sessionId, target.kind, target.id)
    } catch {
      // Best-effort: the tmux session might already be dead. Either way the
      // local tab is gone from the UI, which is what the user clicked for.
    }
  }

  return (
    <div className="flex h-full w-full flex-col bg-[#0b0b0f]">
      {/* Tab strip */}
      <div className="flex items-center gap-0.5 border-b border-white/5 bg-[#0b0b0f] px-1 pt-1">
        {tabs.map((t) => {
          const key = tabKey(t)
          const active = key === activeKey
          const Icon = t.kind === 'claude' ? Sparkles : TerminalIcon
          const closable = !(t.kind === 'claude' && t.id === 'main')
          const status: TerminalStatus = statuses[key] ?? 'connecting'
          // Tiny coloured dot per tab so the user can see live
          // connection state at a glance, even on inactive tabs.
          const dotColor =
            status === 'open'
              ? 'bg-emerald-500'
              : status === 'connecting'
                ? 'bg-amber-400 animate-pulse'
                : 'bg-red-500'
          const dotTitle =
            status === 'open'
              ? 'Connected'
              : status === 'connecting'
                ? 'Connecting…'
                : status === 'error'
                  ? 'Connection error — click reconnect'
                  : 'Disconnected — click reconnect'
          return (
            <div
              key={key}
              className={`group flex items-center gap-1.5 rounded-t px-2.5 py-1 text-xs font-medium transition-colors ${
                active
                  ? 'bg-[#1f1f23] text-zinc-100'
                  : 'text-zinc-500 hover:text-zinc-300'
              }`}
            >
              <button
                type="button"
                onClick={() => setActiveKey(key)}
                className="flex items-center gap-1.5"
              >
                <Icon className="h-3 w-3" />
                <span>{t.label}</span>
                <span
                  className={`h-1.5 w-1.5 rounded-full ${dotColor}`}
                  title={dotTitle}
                  aria-label={dotTitle}
                />
              </button>
              {active && status !== 'open' && status !== 'connecting' && (
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation()
                    reconnect(key)
                  }}
                  className="ml-0.5 rounded p-0.5 text-zinc-400 hover:bg-white/10 hover:text-zinc-100"
                  aria-label={`Reconnect ${t.label}`}
                  title="Reconnect websocket"
                >
                  <RotateCw className="h-3 w-3" />
                </button>
              )}
              {closable && (
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation()
                    void closeTab(t)
                  }}
                  className="ml-0.5 rounded p-0.5 text-zinc-600 opacity-0 transition-opacity hover:bg-white/10 hover:text-zinc-300 group-hover:opacity-100"
                  aria-label={`Close ${t.label}`}
                >
                  <X className="h-3 w-3" />
                </button>
              )}
            </div>
          )
        })}
        <button
          type="button"
          onClick={addShell}
          className="ml-1 flex items-center gap-1 rounded px-2 py-1 text-xs text-zinc-500 hover:bg-white/5 hover:text-zinc-300"
          aria-label="New shell tab"
        >
          <Plus className="h-3 w-3" />
        </button>
      </div>

      {/* Terminal panes — all mounted, only the active one visible. Keeping
          inactive panes mounted preserves their xterm buffer + websocket
          (which itself pauses on tab visibility), so switching back is
          instant instead of cold-starting the whole stack. */}
      <div className="relative flex-1 min-h-0 w-full">
        {tabs.map((t) => {
          const key = tabKey(t)
          const active = key === activeKey
          return (
            <div
              key={key}
              className="absolute inset-0"
              style={{
                visibility: active ? 'visible' : 'hidden',
                pointerEvents: active ? 'auto' : 'none',
              }}
            >
              <SessionTerminal
                ref={(h) => {
                  if (h) handlesRef.current.set(key, h)
                  else handlesRef.current.delete(key)
                }}
                projectId={projectId}
                sessionId={sessionId}
                kind={t.kind}
                tab={t.id}
                reconnectKey={reconnectKeys[key] ?? 0}
                onStatusChange={(s) => setStatusFor(key, s)}
              />
            </div>
          )
        })}
      </div>
    </div>
  )
})
