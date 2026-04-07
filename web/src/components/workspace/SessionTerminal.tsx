// Raw PTY terminal attached to a workspace session's sandbox container.
//
// Replaces the chat abstraction: instead of parsing stream-json events and
// rendering chat bubbles, we connect xterm.js directly to a websocket that
// pipes to `tmux new -A -s {session} {cli}` inside the container. The CLI
// owns its own UI — slash commands, interactive prompts, MCP approvals,
// scrollback. When upstream (claude/codex/opencode) ships a new feature,
// it just works.
//
// Each tab runs its own websocket → its own tmux session (independent
// terminals, not shared views). The websocket stays open for the tab's
// entire lifetime; we deliberately do NOT disconnect on tab visibility
// change, because reconnecting forces tmux to repaint the full screen and
// some TUIs (claude) interpret that as a restart and re-run their
// onboarding. Keeping the socket alive costs nothing and keeps the
// terminal state stable.

import { forwardRef, useEffect, useImperativeHandle, useRef } from 'react'

/** Live connection state for the status indicator. */
export type TerminalStatus = 'connecting' | 'open' | 'closed' | 'error'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import '@xterm/xterm/css/xterm.css'

import { pasteTerminalImage, sessionTerminalUrl } from './api'

interface SessionTerminalProps {
  projectId: number
  sessionId: number
  /** Which kind of terminal: claude (AI CLI) or shell (raw bash). */
  kind?: 'claude' | 'shell'
  /** Stable id so reopening the same tab re-attaches to the same tmux session. */
  tab?: string
  /** Bump this to force a fresh websocket + terminal. */
  reconnectKey?: number
  /** Called on every websocket state transition so the parent can render
   *  a status indicator without threading refs through the component tree. */
  onStatusChange?: (status: TerminalStatus) => void
}

/** Imperative handle exposed to parents for the mobile key picker. */
export interface SessionTerminalHandle {
  /** Send raw bytes (e.g. an escape sequence) to the PTY as if typed. */
  sendKeys: (data: string) => void
  /** Focus the xterm textarea so the soft keyboard stays open. */
  focus: () => void
}

export const SessionTerminal = forwardRef<
  SessionTerminalHandle,
  SessionTerminalProps
>(function SessionTerminal(
  {
    projectId,
    sessionId,
    kind = 'claude',
    tab = 'main',
    reconnectKey = 0,
    onStatusChange,
  },
  ref,
) {
  // Stash the latest callback in a ref so the effect doesn't need to
  // re-run (and tear down the websocket) every time the parent re-renders
  // with a new inline function.
  const onStatusChangeRef = useRef(onStatusChange)
  onStatusChangeRef.current = onStatusChange
  const containerRef = useRef<HTMLDivElement>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const termRef = useRef<Terminal | null>(null)

  useImperativeHandle(ref, () => ({
    sendKeys: (data: string) => {
      const ws = wsRef.current
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(new TextEncoder().encode(data))
      }
    },
    focus: () => {
      termRef.current?.focus()
    },
  }))

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    const term = new Terminal({
      cursorBlink: true,
      fontFamily:
        'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      theme: {
        background: '#0b0b0f',
        foreground: '#e4e4e7',
        cursor: '#e4e4e7',
        black: '#1f1f23',
        brightBlack: '#52525b',
      },
      scrollback: 10000,
      convertEol: true,
    })
    termRef.current = term

    const fit = new FitAddon()
    term.loadAddon(fit)
    term.loadAddon(new WebLinksAddon())

    term.open(container)
    // CRITICAL: do NOT call fit.fit() here. If the parent is `display: none`
    // when this component mounts (e.g. user starts on the Chat tab, or the
    // terminal pane was hidden when the session was opened), the container
    // is 0×0 and FitAddon snaps to its minimum (~10 cols). xterm then ships
    // 10 cols to the PTY in the very first resize message, tmux/claude
    // render their welcome banner wrapped at 10 cols, and those wrapped
    // bytes land in the scrollback buffer where no later resize can
    // un-wrap them. We defer the first fit until ResizeObserver reports a
    // container with real dimensions (see below).

    // Single long-lived websocket for the life of this component. No
    // visibility-driven reconnects — see file header for why.
    onStatusChangeRef.current?.('connecting')
    const ws = new WebSocket(
      sessionTerminalUrl(projectId, sessionId, { kind, tab }),
    )
    ws.binaryType = 'arraybuffer'
    wsRef.current = ws

    // Track whether we've shipped a real (non-degenerate) size to the PTY.
    // Until this is true, we suppress resize messages so a 0×0 → ~10×N
    // transient never reaches tmux.
    let hasRealSize = false
    // Last cols/rows actually shipped to the PTY. We dedupe against this so
    // that no-op resizes — which happen *constantly* when the page reflows
    // by a few pixels around the terminal (status bar height changing,
    // composer growing, "thinking…" indicator appearing) — never reach
    // tmux. Each real resize sends SIGWINCH and Claude's TUI repaints its
    // entire alternate-screen frame, which paints over whatever was on
    // screen and produces the "Welcome back!" overlap glitch.
    let lastCols = -1
    let lastRows = -1

    const sendResize = () => {
      if (ws.readyState !== WebSocket.OPEN) return
      // Guardrail: 20 cols is the smallest reasonable workspace width. If
      // we measure smaller than that the container clearly isn't laid out
      // yet — drop the resize on the floor and wait for the next RO tick.
      if (term.cols < 20 || term.rows < 5) return
      // Drop no-op resizes. Without this, the surrounding layout reflowing
      // by even one pixel triggers a fit that yields the same cols/rows but
      // we still ship it, tmux SIGWINCHes the TUI, and Claude redraws.
      if (term.cols === lastCols && term.rows === lastRows) return
      hasRealSize = true
      lastCols = term.cols
      lastRows = term.rows
      ws.send(
        JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }),
      )
    }

    ws.onopen = () => {
      onStatusChangeRef.current?.('open')
      term.writeln('\x1b[90m[temps] connected — attaching to sandbox…\x1b[0m')
      // If we already measured a real size before the socket opened, ship
      // it now. Otherwise the first ResizeObserver tick will handle it.
      if (hasRealSize) sendResize()
    }

    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') {
        try {
          const parsed = JSON.parse(ev.data)
          if (parsed?.type === 'exit') {
            term.writeln(
              `\r\n\x1b[90m[temps] session ended (exit ${parsed.code ?? '?'})\x1b[0m`,
            )
          }
        } catch {
          term.write(ev.data)
        }
      } else {
        const bytes =
          ev.data instanceof ArrayBuffer ? new Uint8Array(ev.data) : ev.data
        term.write(bytes as Uint8Array)
      }
    }

    ws.onclose = () => {
      onStatusChangeRef.current?.('closed')
      term.writeln('\r\n\x1b[90m[temps] disconnected\x1b[0m')
    }

    ws.onerror = () => {
      onStatusChangeRef.current?.('error')
      term.writeln('\r\n\x1b[31m[temps] websocket error\x1b[0m')
    }

    // Pipe keystrokes to the PTY.
    const keyDisposable = term.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(new TextEncoder().encode(data))
      }
    })

    // Debounced fit on container resize. Skips degenerate (0×0 / hidden)
    // measurements so we never ship a tiny size to the PTY.
    let resizeTimer: ReturnType<typeof setTimeout> | null = null
    const refit = () => {
      // Container not laid out yet — bail and wait for the next tick.
      if (container.clientWidth < 20 || container.clientHeight < 20) return
      try {
        fit.fit()
        sendResize()
      } catch {
        /* fit can throw if container is detached mid-resize */
      }
    }
    const ro = new ResizeObserver(() => {
      if (resizeTimer) clearTimeout(resizeTimer)
      resizeTimer = setTimeout(refit, 60)
    })
    ro.observe(container)

    // The parent toggles between Terminal and Chat by flipping a `hidden`
    // class on the wrapper. ResizeObserver does NOT reliably fire for the
    // `display: none → block` transition, so on every flip back we re-fit
    // and let `sendResize` push the new dims to the PTY *only if they
    // actually changed*. We deliberately do NOT clear xterm's buffer or
    // send Ctrl-L here — Claude's TUI redrawing on top of existing
    // scrollback is what produces the "Welcome back!" overlap glitch. If
    // the size really did change, tmux's SIGWINCH will trigger the repaint
    // on its own; if it didn't, leaving the screen alone is correct.
    const onShow = () => {
      // Two-frame delay: one for layout to settle after `hidden` flips off,
      // one more for the browser to compute the new container size before
      // FitAddon measures it.
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          refit()
        })
      })
    }
    window.addEventListener('temps:terminal-show', onShow)

    // Kick an initial fit on the next paint frame in case the container is
    // already visible at mount (the common case when the user lands
    // directly on the Terminal view).
    requestAnimationFrame(() => {
      requestAnimationFrame(refit)
    })

    // Image paste: xterm.js only forwards text from the clipboard, so binary
    // image data on Cmd+V is dropped on the floor. We intercept paste events
    // at the document capture phase (before xterm's own textarea handler runs)
    // so we can stopPropagation() on image pastes and prevent xterm from
    // swallowing them as empty text. For non-image pastes we do nothing and
    // xterm handles them normally.
    //
    // The uploaded path is sent wrapped in bracketed-paste escape sequences
    // (ESC [ 200 ~ … ESC [ 201 ~). Claude CLI uses the bracketed-paste
    // boundary to detect pasted file paths and treat them as image
    // attachments rather than literal text.
    const onPaste = async (ev: ClipboardEvent) => {
      // Only handle pastes targeting our terminal. Without this check every
      // paste anywhere in the app would run through this handler.
      if (!container.contains(ev.target as Node | null)) return
      const items = ev.clipboardData?.items
      if (!items) return
      let imageItem: DataTransferItem | null = null
      for (let i = 0; i < items.length; i++) {
        const item = items[i]
        if (item.kind === 'file' && item.type.startsWith('image/')) {
          imageItem = item
          break
        }
      }
      if (!imageItem) return // let xterm handle normal text paste
      // Capture phase + stopImmediatePropagation: xterm's paste listener
      // lives on its inner <textarea>, which is a descendant of `container`.
      // We must preempt it entirely or it'll receive an empty-text paste
      // event and write nothing to the PTY while we're uploading.
      ev.preventDefault()
      ev.stopPropagation()
      ev.stopImmediatePropagation?.()
      const file = imageItem.getAsFile()
      if (!file) return
      // Do NOT write status lines to xterm here. Claude's TUI owns the
      // alternate screen and any bytes we inject get painted over its input
      // box, corrupting its cursor model (symptom: "[Imagea#1] test⎵sions"
      // garble as Claude redraws on top of our line). The bind-mount write
      // is fast enough that a spinner isn't needed; on error we surface a
      // browser alert instead of scribbling into the TUI.
      try {
        const buf = new Uint8Array(await file.arrayBuffer())
        const { path } = await pasteTerminalImage(
          projectId,
          sessionId,
          buf,
          imageItem.type,
        )
        if (ws.readyState === WebSocket.OPEN) {
          // Bracketed paste wrapper — Claude CLI reads the boundary and
          // recognizes a bare existing file path as an image attachment.
          const payload = `\x1b[200~${path}\x1b[201~`
          ws.send(new TextEncoder().encode(payload))
        }
      } catch (err) {
        console.error('[temps] image paste failed', err)
        window.alert(
          `Image paste failed: ${
            err instanceof Error ? err.message : String(err)
          }`,
        )
      }
    }
    document.addEventListener('paste', onPaste, { capture: true })

    // Focus on mount so the user can type immediately.
    term.focus()

    return () => {
      document.removeEventListener('paste', onPaste, { capture: true })
      window.removeEventListener('temps:terminal-show', onShow)
      keyDisposable.dispose()
      ro.disconnect()
      if (resizeTimer) clearTimeout(resizeTimer)
      try {
        ws.close()
      } catch {
        /* ignore */
      }
      term.dispose()
      if (wsRef.current === ws) wsRef.current = null
      if (termRef.current === term) termRef.current = null
    }
  }, [projectId, sessionId, kind, tab, reconnectKey])

  return (
    <div
      ref={containerRef}
      className="h-full w-full bg-[#0b0b0f] p-2"
      style={{ minHeight: 0 }}
    />
  )
})
