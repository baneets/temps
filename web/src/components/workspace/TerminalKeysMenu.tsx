// Floating dropdown that injects special keys and exposes terminal
// actions (scroll, fullscreen, restart) through a single trigger. We
// funnel everything through one popover so the corner toolbar stays
// tidy on mobile — five loose buttons was too busy.
//
// xterm.js renders an off-screen <textarea> for input, and on mobile that
// textarea is what holds the soft keyboard open. Tapping any button that
// blurs it would dismiss the keyboard, so every interactive control uses
// onMouseDown + preventDefault and refocuses the terminal after sending.
// The trigger sits at the bottom-right of the terminal pane (anchored
// from the parent) — when tapped it opens upward as a small grid of
// actions + key groups missing from mobile soft keyboards.

import { ChevronDown, ChevronUp, Keyboard } from 'lucide-react'
import { useEffect, useRef } from 'react'

import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'

import type { TerminalTabsHandle } from './TerminalTabs'

interface TerminalKeysMenuProps {
  /** Imperative handle from the terminal we control. May be null until mounted. */
  terminalRef: React.RefObject<TerminalTabsHandle | null>
}

interface KeySpec {
  label: string
  /** Raw bytes to ship to the PTY. */
  data: string
  /** Optional tooltip / hint shown on hover. */
  title?: string
}

interface KeySection {
  title: string
  keys: KeySpec[]
}

// Sections mirror the shortcuts power users actually reach for in Termius,
// Claude Code, Codex, OpenCode, and tmux. Grouped so the popover scans
// quickly on mobile without becoming a wall of buttons.
const KEY_SECTIONS: KeySection[] = [
  {
    title: 'Navigation',
    keys: [
      { label: 'Esc', data: '\x1b' },
      { label: 'Tab', data: '\t' },
      { label: '⇧Tab', data: '\x1b[Z', title: 'Shift+Tab — reverse complete / Claude plan mode' },
      { label: '↑', data: '\x1b[A' },
      { label: '↓', data: '\x1b[B' },
      { label: '←', data: '\x1b[D' },
      { label: '→', data: '\x1b[C' },
      // Home/End: send both xterm ("OH"/"OF", application-keypad mode) and
      //   legacy "H"/"F" is unnecessary — the tilde variants "[1~"/"[4~"
      //   are what readline/vim/claude actually bind to across terminfo
      //   entries (xterm, xterm-256color, tmux-256color). "[H"/"[F" was
      //   the ANSI cursor-position form and many CLIs ignore it.
      { label: 'Home', data: '\x1b[1~' },
      { label: 'End', data: '\x1b[4~' },
      { label: 'PgUp', data: '\x1b[5~' },
      { label: 'PgDn', data: '\x1b[6~' },
      { label: 'Del', data: '\x1b[3~' },
    ],
  },
  {
    title: 'Signals & screen',
    keys: [
      { label: '^C', data: '\x03', title: 'Interrupt (SIGINT)' },
      { label: '^D', data: '\x04', title: 'EOF / exit shell' },
      { label: '^Z', data: '\x1a', title: 'Suspend (SIGTSTP)' },
      { label: '^\\', data: '\x1c', title: 'Quit (SIGQUIT)' },
      { label: '^L', data: '\x0c', title: 'Clear screen' },
      { label: '^S', data: '\x13', title: 'Stop output (XOFF)' },
      { label: '^Q', data: '\x11', title: 'Resume output (XON)' },
    ],
  },
  {
    title: 'Line editing (readline)',
    keys: [
      { label: '^A', data: '\x01', title: 'Beginning of line' },
      { label: '^E', data: '\x05', title: 'End of line' },
      { label: '^B', data: '\x02', title: 'Back one char' },
      { label: '^F', data: '\x06', title: 'Forward one char' },
      { label: '^U', data: '\x15', title: 'Cut to start of line' },
      { label: '^K', data: '\x0b', title: 'Cut to end of line' },
      { label: '^W', data: '\x17', title: 'Cut previous word' },
      { label: '^Y', data: '\x19', title: 'Yank (paste cut)' },
      { label: '^T', data: '\x14', title: 'Transpose chars' },
      { label: '⌥B', data: '\x1bb', title: 'Back one word' },
      { label: '⌥F', data: '\x1bf', title: 'Forward one word' },
      { label: '⌥D', data: '\x1bd', title: 'Delete next word' },
      { label: '⌥.', data: '\x1b.', title: 'Insert last argument' },
    ],
  },
  {
    title: 'History & search',
    keys: [
      { label: '^R', data: '\x12', title: 'Reverse history search' },
      { label: '^P', data: '\x10', title: 'Previous history' },
      { label: '^N', data: '\x0e', title: 'Next history' },
      { label: '^G', data: '\x07', title: 'Cancel search' },
    ],
  },
  {
    title: 'AI agents (Claude / Codex / OpenCode)',
    keys: [
      { label: '⇧⏎', data: '\x1b\r', title: 'Newline without submit' },
      { label: 'Esc Esc', data: '\x1b\x1b', title: 'Cancel / interrupt turn' },
    ],
  },
  {
    title: 'tmux prefix (Ctrl-B)',
    keys: [
      { label: 'Prefix', data: '\x02', title: 'Ctrl-B (raw prefix)' },
      { label: 'P · c', data: '\x02c', title: 'New window' },
      { label: 'P · n', data: '\x02n', title: 'Next window' },
      { label: 'P · p', data: '\x02p', title: 'Previous window' },
      { label: 'P · d', data: '\x02d', title: 'Detach session' },
      { label: 'P · [', data: '\x02[', title: 'Copy / scroll mode' },
      { label: 'P · "', data: '\x02"', title: 'Split horizontal' },
      { label: 'P · %', data: '\x02%', title: 'Split vertical' },
      { label: 'P · z', data: '\x02z', title: 'Toggle zoom' },
    ],
  },
]

// Scroll-hold behavior: tick cadence, and the ramp in lines/tick.
// Short tap = 2 lines. Holding ramps up to turbo for long scrollback.
const SCROLL_TICK_MS = 60
const SCROLL_STEP_INITIAL = 2
const SCROLL_STEP_FAST = 6
const SCROLL_STEP_TURBO = 14
const SCROLL_FAST_AFTER_TICKS = 5
const SCROLL_TURBO_AFTER_TICKS = 15

export function TerminalKeysMenu({ terminalRef }: TerminalKeysMenuProps) {
  const send = (data: string) => {
    terminalRef.current?.sendKeys(data)
    // Refocus immediately so the soft keyboard doesn't dismiss on mobile.
    terminalRef.current?.focus()
  }

  // Press-and-hold scroll: same ramp/refocus pattern as the old standalone
  // buttons, kept intact so users can still rip through 10k-line scrollback
  // without releasing.
  const scrollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const scrollTicksRef = useRef(0)
  const stopScroll = () => {
    if (scrollIntervalRef.current !== null) {
      clearInterval(scrollIntervalRef.current)
      scrollIntervalRef.current = null
    }
    scrollTicksRef.current = 0
  }
  useEffect(() => stopScroll, [])
  const stepScroll = (direction: -1 | 1) => {
    const t = scrollTicksRef.current
    const lines =
      t >= SCROLL_TURBO_AFTER_TICKS
        ? SCROLL_STEP_TURBO
        : t >= SCROLL_FAST_AFTER_TICKS
          ? SCROLL_STEP_FAST
          : SCROLL_STEP_INITIAL
    terminalRef.current?.scrollLines(direction * lines)
    terminalRef.current?.focus()
  }
  const startScroll = (direction: -1 | 1) => {
    stopScroll()
    scrollTicksRef.current = 0
    stepScroll(direction)
    scrollIntervalRef.current = setInterval(() => {
      scrollTicksRef.current += 1
      stepScroll(direction)
    }, SCROLL_TICK_MS)
  }
  const scrollBind = (direction: -1 | 1) => ({
    onMouseDown: (e: React.MouseEvent) => {
      e.preventDefault()
      startScroll(direction)
    },
    onMouseUp: () => stopScroll(),
    onMouseLeave: () => stopScroll(),
    onTouchStart: (e: React.TouchEvent) => {
      e.preventDefault()
      startScroll(direction)
    },
    onTouchEnd: (e: React.TouchEvent) => {
      e.preventDefault()
      stopScroll()
    },
    onTouchCancel: () => stopScroll(),
    onContextMenu: (e: React.MouseEvent) => e.preventDefault(),
  })

  const scrollBtn =
    'flex h-9 flex-1 items-center justify-center rounded border border-white/10 bg-[#1f1f23] text-zinc-200 hover:bg-[#2a2a30] active:bg-[#33333a] touch-none select-none'

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="flex h-8 w-8 items-center justify-center rounded-md border border-white/10 bg-[#1f1f23] text-zinc-300 shadow-lg hover:bg-[#2a2a30] hover:text-zinc-100"
          aria-label="Terminal actions"
          title="Terminal actions"
        >
          <Keyboard className="h-4 w-4" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="end"
        sideOffset={6}
        className="w-[min(20rem,calc(100vw-1rem))] max-h-[70vh] overflow-y-auto border-white/10 bg-[#0b0b0f] p-2"
        // Don't steal focus from the terminal when the popover opens.
        onOpenAutoFocus={(e) => e.preventDefault()}
        onCloseAutoFocus={(e) => e.preventDefault()}
        // Don't close just because xterm's textarea steals focus back after
        // every sendKeys+focus. We *do* want outside pointer-down (taps on
        // the terminal or anywhere else in the page) to dismiss it.
        onFocusOutside={(e) => e.preventDefault()}
      >
        <div className="flex flex-col gap-3">
          {/* Scroll row — press and hold to accelerate. */}
          <div className="flex flex-col gap-1">
            <div className="px-1 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
              Scroll
            </div>
            <div className="flex gap-1">
              <button
                type="button"
                aria-label="Scroll terminal up"
                title="Scroll up (hold to accelerate)"
                className={scrollBtn}
                {...scrollBind(-1)}
              >
                <ChevronUp className="h-4 w-4" />
              </button>
              <button
                type="button"
                aria-label="Scroll terminal down"
                title="Scroll down (hold to accelerate)"
                className={scrollBtn}
                {...scrollBind(1)}
              >
                <ChevronDown className="h-4 w-4" />
              </button>
            </div>
          </div>

          {KEY_SECTIONS.map((section) => (
            <div key={section.title} className="flex flex-col gap-1">
              <div className="px-1 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
                {section.title}
              </div>
              <div className="flex flex-wrap gap-1">
                {section.keys.map((k) => (
                  <button
                    key={k.label}
                    type="button"
                    title={k.title ?? k.label}
                    onMouseDown={(e) => {
                      e.preventDefault()
                      send(k.data)
                    }}
                    onTouchStart={(e) => {
                      e.preventDefault()
                      send(k.data)
                    }}
                    className="min-w-[40px] rounded border border-white/10 bg-[#1f1f23] px-2 py-1.5 font-mono text-xs text-zinc-200 hover:bg-[#2a2a30] active:bg-[#33333a]"
                  >
                    {k.label}
                  </button>
                ))}
              </div>
            </div>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  )
}
