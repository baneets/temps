// Raw PTY terminal attached to a workspace session's sandbox container.
//
// Renders via ghostty-web (Ghostty's VT parser compiled to WASM + canvas),
// not xterm.js. The underlying transport is unchanged — we still pipe a
// websocket to `tmux new -A -s {session} {cli}` inside the container — but
// ghostty-web parses more sequences correctly (Kitty graphics, advanced
// mouse, modern color) and ships a native scrollback buffer with a proper
// wheel handler, which xterm.js did not.
//
// Each tab runs its own websocket → its own tmux session (independent
// terminals, not shared views). The websocket stays open for the tab's
// entire lifetime; we deliberately do NOT disconnect on tab visibility
// change, because reconnecting forces tmux to repaint the full screen and
// some TUIs (claude) interpret that as a restart and re-run their
// onboarding. Keeping the socket alive costs nothing and keeps the
// terminal state stable.

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
} from 'react'

/** Live connection state for the status indicator. */
export type TerminalStatus = 'connecting' | 'open' | 'closed' | 'error'
import { FitAddon, Ghostty, Terminal } from 'ghostty-web'

import { pasteTerminalImage, sessionTerminalUrl } from './api'

// Ghostty is a singleton WASM module — load it once per page and reuse
// across every Terminal instance. The .wasm file is served from /public
// by rsbuild and embedded into the Rust binary via include_dir!.
let ghosttyPromise: Promise<Ghostty> | null = null
function ensureGhostty(): Promise<Ghostty> {
  if (!ghosttyPromise) ghosttyPromise = Ghostty.load('/ghostty-vt.wasm')
  return ghosttyPromise
}

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
  /** Focus the terminal input so the soft keyboard stays open. */
  focus: () => void
  /** Scroll the viewport by N lines (negative = up, positive = down). */
  scrollLines: (lines: number) => void
  /** Snap the viewport to the bottom of the scrollback (the prompt). */
  scrollToBottom: () => void
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
  ref
) {
  // Stash the latest callback in a ref so the effect doesn't need to
  // re-run (and tear down the websocket) every time the parent re-renders
  // with a new inline function. We update it in a layout effect because
  // React forbids mutating ref.current during render — which is fine, the
  // only consumers are the websocket callbacks that fire after mount.
  const onStatusChangeRef = useRef(onStatusChange)
  useLayoutEffect(() => {
    onStatusChangeRef.current = onStatusChange
  })
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
    scrollLines: (lines: number) => {
      termRef.current?.scrollLines(lines)
    },
    scrollToBottom: () => {
      termRef.current?.scrollToBottom()
    },
  }))

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    // Track disposal so the async Ghostty.load() path can bail out if the
    // effect was torn down before the WASM module finished loading.
    let disposed = false
    let term: Terminal | null = null
    let fit: FitAddon | null = null
    let ws: WebSocket | null = null
    let ro: ResizeObserver | null = null
    let resizeTimer: ReturnType<typeof setTimeout> | null = null
    let detachListeners: (() => void) | null = null

    onStatusChangeRef.current?.('connecting')

    void (async () => {
      const ghostty = await ensureGhostty()
      if (disposed) return

      term = new Terminal({
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
        ghostty,
      })
      termRef.current = term

      // Force the scrollbar to stay visible. ghostty-web's default is
      // to fade it in on wheel/scroll and fade it out after 1500ms;
      // users have explicitly asked for a persistent bar.
      //
      // There's no public option for this, so we reach into the private
      // API: `hideScrollbar` is the fade-out trigger — stubbing it to
      // a no-op keeps the bar pinned. We also call `showScrollbar()`
      // once after open() so the first paint has it visible instead of
      // waiting for the first wheel event.
      //
      // Fragile if ghostty-web renames these methods — guarded with
      // `typeof` checks so any rename just reverts to the default
      // auto-hide behavior rather than crashing.
      const termAny = term as unknown as {
        hideScrollbar?: () => void
        showScrollbar?: () => void
      }
      if (typeof termAny.hideScrollbar === 'function') {
        termAny.hideScrollbar = () => {}
      }

      fit = new FitAddon()
      term.loadAddon(fit)

      term.open(container)
      // Show the (now-pinned) scrollbar on first paint. See the
      // `hideScrollbar` override above — without this the bar only
      // becomes visible on the first wheel event.
      if (typeof termAny.showScrollbar === 'function') {
        termAny.showScrollbar()
      }
      // CRITICAL: do NOT call fit.fit() here. If the parent is `display: none`
      // when this component mounts the container is 0×0 and FitAddon snaps
      // to its minimum. The PTY would then be sized to that tiny value and
      // tmux/claude would render their welcome banner wrapped at the wrong
      // width, landing in scrollback where no later resize can un-wrap them.
      // Defer the first fit until the RO reports real dimensions (see below).

      // Single long-lived websocket for the life of this component. No
      // visibility-driven reconnects — see file header for why.
      ws = new WebSocket(
        sessionTerminalUrl(projectId, sessionId, { kind, tab })
      )
      ws.binaryType = 'arraybuffer'
      wsRef.current = ws

      // Track whether we've shipped a real (non-degenerate) size to the PTY.
      // Until this is true, we suppress resize messages so a 0×0 → ~10×N
      // transient never reaches tmux.
      let hasRealSize = false
      // Last cols/rows actually shipped to the PTY. We dedupe against this
      // so that no-op resizes — which happen *constantly* as the page
      // reflows by a few pixels around the terminal — never reach tmux.
      // Each real resize sends SIGWINCH and the TUI repaints its whole
      // alt-screen frame, which paints over whatever was on screen.
      let lastCols = -1
      let lastRows = -1

      const sendResize = () => {
        if (!term || !ws || ws.readyState !== WebSocket.OPEN) return
        if (term.cols < 20 || term.rows < 5) return
        if (term.cols === lastCols && term.rows === lastRows) return
        hasRealSize = true
        lastCols = term.cols
        lastRows = term.rows
        ws.send(
          JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows })
        )
      }

      ws.onopen = () => {
        onStatusChangeRef.current?.('open')
        term?.write(
          '\x1b[90m[temps] connected — attaching to sandbox…\x1b[0m\r\n'
        )
        if (hasRealSize) sendResize()
      }

      ws.onmessage = (ev) => {
        if (!term) return
        if (typeof ev.data === 'string') {
          try {
            const parsed = JSON.parse(ev.data)
            if (parsed?.type === 'exit') {
              term.write(
                `\r\n\x1b[90m[temps] session ended (exit ${parsed.code ?? '?'})\x1b[0m\r\n`
              )
              return
            }
          } catch {
            /* fall through to raw write */
          }
          term.write(ev.data)
        } else {
          const bytes =
            ev.data instanceof ArrayBuffer ? new Uint8Array(ev.data) : ev.data
          term.write(bytes as Uint8Array)
        }
      }

      ws.onclose = () => {
        onStatusChangeRef.current?.('closed')
        term?.write('\r\n\x1b[90m[temps] disconnected\x1b[0m\r\n')
      }

      ws.onerror = () => {
        onStatusChangeRef.current?.('error')
        term?.write('\r\n\x1b[31m[temps] websocket error\x1b[0m\r\n')
      }

      // Pipe keystrokes to the PTY.
      const dataDisposable = term.onData((data) => {
        if (ws && ws.readyState === WebSocket.OPEN) {
          ws.send(new TextEncoder().encode(data))
        }
      })

      // Debounced fit on container resize. Skips degenerate (hidden)
      // measurements so we never ship a tiny size to the PTY.
      const refit = () => {
        if (!fit || !term) return
        if (container.clientWidth < 20 || container.clientHeight < 20) return
        try {
          fit.fit()
          sendResize()
        } catch {
          /* fit can throw if container is detached mid-resize */
        }
      }
      ro = new ResizeObserver(() => {
        if (resizeTimer) clearTimeout(resizeTimer)
        resizeTimer = setTimeout(refit, 60)
      })
      ro.observe(container)

      // The parent toggles between Terminal and Chat by flipping a `hidden`
      // class on the wrapper. ResizeObserver does NOT reliably fire for the
      // `display: none → block` transition, so on every flip back we re-fit
      // and let `sendResize` push the new dims to the PTY only if they
      // actually changed. tmux's SIGWINCH will trigger a repaint if the
      // size really did change; otherwise leaving the screen alone is
      // correct.
      const onShow = () => {
        requestAnimationFrame(() => {
          requestAnimationFrame(refit)
        })
      }
      window.addEventListener('temps:terminal-show', onShow)

      // Mobile soft keyboard: when it opens, the visual viewport height
      // drops (keyboard sits above the page). Without intervention, the
      // terminal's last line (the prompt) is hidden behind the keyboard
      // and the user has to hunt-and-peck blind. Snap to the bottom of
      // the scrollback whenever the viewport shrinks meaningfully so the
      // prompt stays visible. On desktop the visualViewport resize events
      // don't fire for regular window resizes, so this is effectively
      // mobile-only.
      let lastViewportH = window.visualViewport?.height ?? window.innerHeight
      const onViewportResize = () => {
        const vv = window.visualViewport
        if (!vv || !term) return
        const h = vv.height
        // Shrink of >=120px = keyboard opened (typical iOS/Android
        // keyboards are 250-350px; using 120 as a safe threshold that
        // won't trigger on toolbar hide/show animations ≈70-90px).
        if (h < lastViewportH - 120) {
          // Two rAFs: first for the layout to settle, second for ghostty
          // to have a valid row count to scroll to.
          requestAnimationFrame(() => {
            requestAnimationFrame(() => {
              term?.scrollToBottom()
            })
          })
        }
        lastViewportH = h
      }
      window.visualViewport?.addEventListener('resize', onViewportResize)

      // Wheel & focus containment. Without this, scrolling over the terminal
      // while focus is in a sibling input (e.g. the command-palette search
      // box, or the composer) sends the wheel event bubbling up to the
      // React root, where React's delegated synthetic onWheel fires on the
      // sibling input and interprets scroll-up as "history up."
      //
      // The fix has two parts:
      //   1) Steal focus to the terminal on pointer-enter / mousedown so the
      //      canvas becomes the natural scroll target.
      //   2) Install a CAPTURE-phase wheel listener at the document level.
      //      Capture runs top-down before bubble, and critically before
      //      React's root-level synthetic delegation (which listens in
      //      bubble at the document). If the wheel originated inside our
      //      container we stopImmediatePropagation — ghostty-web's own
      //      handler on the canvas runs first (native DOM dispatch hits the
      //      canvas target before capture bubbles back out), then we squash
      //      the event so no ancestor ever sees it.
      const onPointerEnter = () => {
        const active = document.activeElement as HTMLElement | null
        if (active && active.closest('[role="dialog"]')) return
        term?.focus()
      }
      const onMouseDown = () => {
        term?.focus()
      }
      // Bubble-phase document listener. We must run *after* ghostty-web's
      // own canvas wheel handler has seen and processed the event (so
      // scrollback on the primary screen actually scrolls), then swallow
      // the event before it bubbles up to React's synthetic delegation
      // at the document level — that's where sibling inputs would
      // otherwise interpret the wheel as their own scroll/nav.
      //
      // Earlier versions of this listener used `capture: true` +
      // `stopImmediatePropagation`, which *preceded* the canvas handler
      // and killed it. Symptom: cursor-drag scrolled, wheel did not.
      const onDocWheel = (e: WheelEvent) => {
        const target = e.target as Node | null
        if (target && container.contains(target)) {
          e.stopPropagation()
        }
      }
      container.addEventListener('pointerenter', onPointerEnter)
      container.addEventListener('mousedown', onMouseDown)
      document.addEventListener('wheel', onDocWheel, {
        capture: false,
        passive: true,
      })

      // Touch scroll + tap-to-focus for mobile. ghostty-web handles `wheel`
      // on the canvas but has no built-in touch scrolling, so on phones
      // the terminal can't scroll at all — even though the scrollbar is
      // visible, there's no way to move the viewport. We also have to
      // decide deterministically whether a given gesture is a *scroll*
      // or a *tap-to-open-keyboard*, because the earlier "scroll once we
      // exceed one row of movement" heuristic meant ambiguous gestures
      // sometimes did both (finger moves 25px, scrolls a row, then on
      // release the browser still dispatches the synthesized click and
      // the textarea focuses → keyboard pops open).
      //
      // Rule (matches iOS/Android native):
      //   - A gesture is a TAP iff total movement stayed under TAP_SLOP
      //     px AND total duration stayed under TAP_MAX_MS ms. Only taps
      //     trigger focus() (and thus the soft keyboard).
      //   - Any gesture that crosses the SCROLL_SLOP threshold becomes a
      //     scroll. Once committed, preventDefault() on every touchmove
      //     so the browser cannot synthesize a click on release.
      //   - In between (movement > TAP_SLOP but < SCROLL_SLOP) the
      //     gesture is discarded — no focus, no scroll. This is the
      //     "user slightly jittered and lifted" case; do nothing rather
      //     than guess.
      //
      // One terminal row at font-size 13 is ~18px. Thresholds:
      const TAP_SLOP = 10 // ≤10px total movement = tap
      const TAP_MAX_MS = 300 // and ≤300ms duration
      const SCROLL_SLOP = 14 // ≥14px movement commits to scroll
      const PX_PER_LINE = 18 // ghostty row height at our font size

      type TouchState = {
        startX: number
        startY: number
        startT: number
        // Anchor Y for line-increment calculation (resets per committed row).
        anchorY: number
        // Max displacement observed so far (px); used for the tap check.
        maxDist: number
        // Once true, this gesture is a scroll — keep preventDefaulting so
        // the synthesized click on release never fires.
        scrolling: boolean
      }
      let touchState: TouchState | null = null

      const onTouchStart = (e: TouchEvent) => {
        if (e.touches.length !== 1) {
          touchState = null
          return
        }
        const t = e.touches[0]
        touchState = {
          startX: t.clientX,
          startY: t.clientY,
          startT: performance.now(),
          anchorY: t.clientY,
          maxDist: 0,
          scrolling: false,
        }
      }
      const onTouchMove = (e: TouchEvent) => {
        if (!touchState || !term) return
        if (e.touches.length !== 1) return
        const t = e.touches[0]
        const dx = t.clientX - touchState.startX
        const dy = t.clientY - touchState.startY
        const dist = Math.hypot(dx, dy)
        if (dist > touchState.maxDist) touchState.maxDist = dist

        // Commit to scroll once we cross the threshold. After that every
        // touchmove preventDefaults so no synthesized click fires on release.
        if (!touchState.scrolling && dist >= SCROLL_SLOP) {
          touchState.scrolling = true
          // Rebase the anchor to here so the first scroll step is smooth
          // (we don't want to count the pre-commit movement as lines).
          touchState.anchorY = t.clientY
        }

        if (touchState.scrolling) {
          if (e.cancelable) e.preventDefault()
          const deltaY = touchState.anchorY - t.clientY
          const lines = Math.trunc(deltaY / PX_PER_LINE)
          if (Math.abs(lines) >= 1) {
            // Drag up (positive deltaY) → newer content; drag down →
            // older. Matches iOS/Android content-follows-finger.
            term.scrollLines(lines)
            touchState.anchorY = t.clientY
          }
        }
      }
      const onTouchEnd = (e: TouchEvent) => {
        if (!touchState || !term) {
          touchState = null
          return
        }
        const duration = performance.now() - touchState.startT
        const wasTap =
          !touchState.scrolling &&
          touchState.maxDist <= TAP_SLOP &&
          duration <= TAP_MAX_MS
        if (wasTap) {
          // Explicit focus opens the soft keyboard via the xterm textarea.
          // Without this, taps on the terminal body don't reliably focus
          // it on iOS — the browser only synthesizes click→focus for
          // elements it considers clickable, and our canvas isn't one.
          term.focus()
        } else if (touchState.scrolling) {
          // Scroll gestures: swallow the synthesized click so it doesn't
          // refocus the textarea and dismiss/re-open the keyboard.
          if (e.cancelable) e.preventDefault()
        }
        // Middle ground (moved > TAP_SLOP but < SCROLL_SLOP): do nothing.
        touchState = null
      }
      const onTouchCancel = () => {
        touchState = null
      }
      // passive: false on touchmove AND touchend so preventDefault() is
      // respected (Chrome/Safari default to passive otherwise).
      container.addEventListener('touchstart', onTouchStart, { passive: true })
      container.addEventListener('touchmove', onTouchMove, { passive: false })
      container.addEventListener('touchend', onTouchEnd, { passive: false })
      container.addEventListener('touchcancel', onTouchCancel, { passive: true })

      // Kick an initial fit on the next paint frame in case the container is
      // already visible at mount.
      requestAnimationFrame(() => {
        requestAnimationFrame(refit)
      })

      // Image paste: ghostty-web forwards text from the clipboard, but not
      // binary image data. We intercept paste events at the document capture
      // phase so we can stopPropagation() on image pastes and upload them
      // via the sandbox bind-mount, then feed the resulting file path back
      // into the PTY wrapped in bracketed-paste so Claude/Codex recognize
      // it as an image attachment rather than literal text.
      const onPaste = async (ev: ClipboardEvent) => {
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
        if (!imageItem) return // let ghostty-web handle normal text paste
        ev.preventDefault()
        ev.stopPropagation()
        ev.stopImmediatePropagation?.()
        const file = imageItem.getAsFile()
        if (!file) return
        try {
          const buf = new Uint8Array(await file.arrayBuffer())
          const { path } = await pasteTerminalImage(
            projectId,
            sessionId,
            buf,
            imageItem.type
          )
          if (ws && ws.readyState === WebSocket.OPEN) {
            const payload = `\x1b[200~${path}\x1b[201~`
            ws.send(new TextEncoder().encode(payload))
          }
        } catch (err) {
          console.error('[temps] image paste failed', err)
          window.alert(
            `Image paste failed: ${
              err instanceof Error ? err.message : String(err)
            }`
          )
        }
      }
      document.addEventListener('paste', onPaste, { capture: true })

      // Focus on mount so the user can type immediately.
      term.focus()

      detachListeners = () => {
        document.removeEventListener('paste', onPaste, { capture: true })
        window.removeEventListener('temps:terminal-show', onShow)
        window.visualViewport?.removeEventListener('resize', onViewportResize)
        container.removeEventListener('pointerenter', onPointerEnter)
        container.removeEventListener('mousedown', onMouseDown)
        document.removeEventListener('wheel', onDocWheel, {
          capture: false,
        } as EventListenerOptions)
        container.removeEventListener('touchstart', onTouchStart)
        container.removeEventListener('touchmove', onTouchMove)
        container.removeEventListener('touchend', onTouchEnd)
        container.removeEventListener('touchcancel', onTouchEnd)
        dataDisposable.dispose()
      }
    })()

    return () => {
      disposed = true
      detachListeners?.()
      ro?.disconnect()
      if (resizeTimer) clearTimeout(resizeTimer)
      try {
        ws?.close()
      } catch {
        /* ignore */
      }
      term?.dispose()
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
