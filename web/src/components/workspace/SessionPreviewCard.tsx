import { useEffect, useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import {
  ChevronDown,
  ChevronRight,
  Clock,
  Cpu,
  ExternalLink,
  Globe,
  KeyRound,
  Loader2,
  Pause,
  Play,
  RefreshCw,
  RotateCcw,
} from 'lucide-react'
import { toast } from 'sonner'

import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { CopyButton } from '@/components/ui/copy-button'
import { Input } from '@/components/ui/input'

import {
  regeneratePreviewPassword,
  restartSandbox,
  refreshSandbox,
  startSandbox,
  stopSandbox,
  updateSession,
  type WorkspaceSession,
} from './api'

const SERVER_DEFAULT_IDLE_MINUTES = 120
const DEFAULT_CPU = 2
const DEFAULT_MEM_MB = 4096
const DEFAULT_PIDS = 512

interface SessionPreviewCardProps {
  projectId: number
  session: WorkspaceSession
  defaultExpanded?: boolean
}

/**
 * Renders preview-URL chips, sandbox lifecycle controls (stop/start/restart),
 * and the show-once preview password card.
 *
 * The plaintext password is only present in `session.preview_password` after a
 * fresh create or regenerate response. Once the user navigates away or
 * acknowledges it, only the 4-char hint remains.
 */
export function SessionPreviewCard({
  projectId,
  session,
  defaultExpanded = false,
}: SessionPreviewCardProps) {
  const queryClient = useQueryClient()

  // Card stays collapsed by default — opens automatically the first time we
  // see a fresh plaintext password so the user doesn't miss the show-once reveal.
  const [expanded, setExpanded] = useState<boolean>(
    defaultExpanded || !!session.preview_password,
  )

  // The plaintext password lives in component state so the show-once reveal
  // survives a refetch (which would null it out on the server side).
  const [revealedPassword, setRevealedPassword] = useState<string | null>(
    session.preview_password ?? null,
  )

  // When a new session arrives with a fresh password, surface it and pop
  // the card open so the user can copy it before it disappears.
  useEffect(() => {
    if (session.preview_password) {
      setRevealedPassword(session.preview_password)
      setExpanded(true)
    }
  }, [session.preview_password])

  const invalidate = () =>
    queryClient.invalidateQueries({
      queryKey: ['workspace', projectId, 'session', session.id],
    })

  const regenerate = useMutation({
    mutationFn: () => regeneratePreviewPassword(projectId, session.id),
    onSuccess: (updated) => {
      if (updated.preview_password) {
        setRevealedPassword(updated.preview_password)
      }
      toast.success('Preview password regenerated')
      invalidate()
    },
    onError: (e: Error) => toast.error(e.message),
  })

  const stop = useMutation({
    mutationFn: () => stopSandbox(projectId, session.id),
    onSuccess: () => {
      toast.success('Sandbox stopped')
      invalidate()
    },
    onError: (e: Error) => toast.error(e.message),
  })

  const start = useMutation({
    mutationFn: () => startSandbox(projectId, session.id),
    onSuccess: () => {
      toast.success('Sandbox started')
      invalidate()
    },
    onError: (e: Error) => toast.error(e.message),
  })

  const restart = useMutation({
    mutationFn: () => restartSandbox(projectId, session.id),
    onSuccess: () => {
      toast.success('Sandbox restarted')
      invalidate()
    },
    onError: (e: Error) => toast.error(e.message),
  })

  // Per-session idle timeout control. Empty input = inherit server default
  // (`null` on the wire). We prime the field from the session so clearing it
  // explicitly is distinguishable from "never touched it".
  const [idleInput, setIdleInput] = useState<string>(
    session.idle_timeout_minutes != null
      ? String(session.idle_timeout_minutes)
      : '',
  )
  useEffect(() => {
    setIdleInput(
      session.idle_timeout_minutes != null
        ? String(session.idle_timeout_minutes)
        : '',
    )
  }, [session.idle_timeout_minutes])

  const saveIdle = useMutation({
    mutationFn: (minutes: number | null) =>
      updateSession(projectId, session.id, { idle_timeout_minutes: minutes }),
    onSuccess: () => {
      toast.success('Idle timeout updated')
      invalidate()
    },
    onError: (e: Error) => toast.error(e.message),
  })

  // Per-session resource overrides — empty input falls back to server default.
  const [cpuInput, setCpuInput] = useState<string>(
    session.cpu_limit != null ? String(session.cpu_limit) : '',
  )
  const [memInput, setMemInput] = useState<string>(
    session.memory_limit_mb != null ? String(session.memory_limit_mb) : '',
  )
  const [pidsInput, setPidsInput] = useState<string>(
    session.pids_limit != null ? String(session.pids_limit) : '',
  )
  useEffect(() => {
    setCpuInput(session.cpu_limit != null ? String(session.cpu_limit) : '')
    setMemInput(
      session.memory_limit_mb != null ? String(session.memory_limit_mb) : '',
    )
    setPidsInput(session.pids_limit != null ? String(session.pids_limit) : '')
  }, [session.cpu_limit, session.memory_limit_mb, session.pids_limit])

  const saveResources = useMutation({
    mutationFn: (body: {
      cpu_limit: number | null
      memory_limit_mb: number | null
      pids_limit: number | null
    }) => updateSession(projectId, session.id, body),
    onSuccess: () => {
      toast.success('Resource limits updated. Restart sandbox to apply.')
      invalidate()
    },
    onError: (e: Error) => toast.error(e.message),
  })

  const refresh = useMutation({
    mutationFn: () => refreshSandbox(projectId, session.id),
    onSuccess: () => {
      toast.success('Sandbox refreshed (skill, env, token reloaded)')
      invalidate()
    },
    onError: (e: Error) => toast.error(e.message),
  })

  const lifecycleBusy =
    stop.isPending || start.isPending || restart.isPending || refresh.isPending
  const sandboxAttached = !!session.sandbox_container_id
  const sessionClosed = session.status === 'closed'
  const passwordRevealed = revealedPassword !== null

  // Free-form port input — lets users open arbitrary ports beyond the
  // common ones we surface as chips.
  const [customPort, setCustomPort] = useState('')
  const customUrl = customPort.trim()
    ? session.preview_url_template.replace('{port}', customPort.trim())
    : null

  return (
    <Card className="p-3 space-y-3">
      {/* Collapsible header — keeps the chat compact until the user asks for it */}
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="flex w-full items-center justify-between gap-2 text-left"
        aria-expanded={expanded}
      >
        <div className="flex items-center gap-2 text-xs font-medium">
          {expanded ? (
            <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />
          )}
          <Globe className="h-3.5 w-3.5 text-muted-foreground" />
          Preview &amp; sandbox
        </div>
        <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
          {sandboxAttached ? (
            <span className="font-mono">
              {session.sandbox_container_id?.slice(0, 12)}
            </span>
          ) : (
            <span>not started</span>
          )}
          {session.preview_password_hint && !expanded && (
            <span>· pwd …{session.preview_password_hint}</span>
          )}
        </div>
      </button>

      {expanded && (
      <>
      {/* Sandbox lifecycle controls */}
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="text-xs text-muted-foreground">
          Sandbox{' '}
          <span className="font-mono">
            {session.sandbox_container_id?.slice(0, 12) ?? 'not started'}
          </span>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={() => stop.mutate()}
            disabled={!sandboxAttached || sessionClosed || lifecycleBusy}
            title="Stop the sandbox container"
          >
            {stop.isPending ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Pause className="h-3.5 w-3.5" />
            )}
            Stop
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => start.mutate()}
            disabled={!sandboxAttached || sessionClosed || lifecycleBusy}
            title="Start the sandbox container"
          >
            {start.isPending ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Play className="h-3.5 w-3.5" />
            )}
            Start
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => restart.mutate()}
            disabled={!sandboxAttached || sessionClosed || lifecycleBusy}
            title="Restart the sandbox container in place"
          >
            {restart.isPending ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <RotateCcw className="h-3.5 w-3.5" />
            )}
            Restart
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => refresh.mutate()}
            disabled={!sandboxAttached || sessionClosed || lifecycleBusy}
            title="Reload skill, env vars, and re-issue the deployment token without restarting the container"
          >
            {refresh.isPending ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <RefreshCw className="h-3.5 w-3.5" />
            )}
            Refresh
          </Button>
        </div>
      </div>

      {/* Preview URLs */}
      <div className="space-y-2">
        <div className="text-xs font-medium text-muted-foreground">
          Preview URLs
        </div>
        <div className="flex flex-wrap gap-2">
          {session.preview_urls.map((p) => (
            <a
              key={p.port}
              href={p.url}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs hover:bg-accent"
            >
              <ExternalLink className="h-3 w-3" />
              {p.port}
            </a>
          ))}
        </div>
        <div className="flex gap-2">
          <Input
            value={customPort}
            onChange={(e) => setCustomPort(e.target.value)}
            placeholder="Custom port"
            inputMode="numeric"
            className="h-8 max-w-[140px] text-xs"
          />
          {customUrl && (
            <a
              href={customUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs hover:bg-accent"
            >
              <ExternalLink className="h-3 w-3" />
              Open :{customPort.trim()}
            </a>
          )}
        </div>
      </div>

      {/* Idle timeout (per-session) */}
      <div className="space-y-2 border-t pt-3">
        <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Clock className="h-3.5 w-3.5" />
          Idle timeout
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            type="number"
            min={0}
            max={10080}
            inputMode="numeric"
            value={idleInput}
            onChange={(e) => setIdleInput(e.target.value)}
            placeholder={`${SERVER_DEFAULT_IDLE_MINUTES} (default)`}
            className="h-8 max-w-[140px] text-xs"
            disabled={sessionClosed || saveIdle.isPending}
          />
          <span className="text-[11px] text-muted-foreground">
            minutes (0 = never)
          </span>
          <Button
            size="sm"
            variant="outline"
            disabled={sessionClosed || saveIdle.isPending}
            onClick={() => {
              const trimmed = idleInput.trim()
              if (trimmed === '') {
                saveIdle.mutate(null)
                return
              }
              const n = Number(trimmed)
              if (!Number.isFinite(n) || n < 0 || n > 10080) {
                toast.error(
                  'Enter 0–10080 minutes (0 = never), or leave empty for default',
                )
                return
              }
              saveIdle.mutate(Math.floor(n))
            }}
          >
            {saveIdle.isPending && (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            )}
            Save
          </Button>
          {session.idle_timeout_minutes != null && (
            <Button
              size="sm"
              variant="ghost"
              disabled={sessionClosed || saveIdle.isPending}
              onClick={() => {
                setIdleInput('')
                saveIdle.mutate(null)
              }}
              title="Clear override and fall back to server default"
            >
              Reset
            </Button>
          )}
        </div>
        <p className="text-[11px] text-muted-foreground">
          Sessions idle longer than this are automatically closed and their
          sandbox torn down. Leave empty to use the server default (
          {SERVER_DEFAULT_IDLE_MINUTES} min).
        </p>
      </div>

      {/* Resource limits (per-session) */}
      <div className="space-y-2 border-t pt-3">
        <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Cpu className="h-3.5 w-3.5" />
          Resource limits
        </div>
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
          <label className="space-y-1">
            <span className="text-[11px] text-muted-foreground">
              CPU (vCPUs)
            </span>
            <Input
              type="number"
              min={0.25}
              max={16}
              step={0.25}
              inputMode="decimal"
              value={cpuInput}
              onChange={(e) => setCpuInput(e.target.value)}
              placeholder={`${DEFAULT_CPU} (default)`}
              className="h-8 text-xs"
              disabled={sessionClosed || saveResources.isPending}
            />
          </label>
          <label className="space-y-1">
            <span className="text-[11px] text-muted-foreground">
              Memory (MB)
            </span>
            <Input
              type="number"
              min={256}
              max={32768}
              step={256}
              inputMode="numeric"
              value={memInput}
              onChange={(e) => setMemInput(e.target.value)}
              placeholder={`${DEFAULT_MEM_MB} (default)`}
              className="h-8 text-xs"
              disabled={sessionClosed || saveResources.isPending}
            />
          </label>
          <label className="space-y-1">
            <span className="text-[11px] text-muted-foreground">PIDs</span>
            <Input
              type="number"
              min={64}
              max={8192}
              step={64}
              inputMode="numeric"
              value={pidsInput}
              onChange={(e) => setPidsInput(e.target.value)}
              placeholder={`${DEFAULT_PIDS} (default)`}
              className="h-8 text-xs"
              disabled={sessionClosed || saveResources.isPending}
            />
          </label>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={sessionClosed || saveResources.isPending}
            onClick={() => {
              const parse = (s: string, min: number, max: number, label: string): number | null | 'err' => {
                const t = s.trim()
                if (t === '') return null
                const n = Number(t)
                if (!Number.isFinite(n) || n < min || n > max) {
                  toast.error(`${label} must be between ${min} and ${max}`)
                  return 'err'
                }
                return n
              }
              const cpu = parse(cpuInput, 0.25, 16, 'CPU')
              if (cpu === 'err') return
              const mem = parse(memInput, 256, 32768, 'Memory')
              if (mem === 'err') return
              const pids = parse(pidsInput, 64, 8192, 'PIDs')
              if (pids === 'err') return
              saveResources.mutate({
                cpu_limit: cpu,
                memory_limit_mb: mem == null ? null : Math.floor(mem),
                pids_limit: pids == null ? null : Math.floor(pids),
              })
            }}
          >
            {saveResources.isPending && (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            )}
            Save
          </Button>
          {(session.cpu_limit != null ||
            session.memory_limit_mb != null ||
            session.pids_limit != null) && (
            <Button
              size="sm"
              variant="ghost"
              disabled={sessionClosed || saveResources.isPending}
              onClick={() => {
                setCpuInput('')
                setMemInput('')
                setPidsInput('')
                saveResources.mutate({
                  cpu_limit: null,
                  memory_limit_mb: null,
                  pids_limit: null,
                })
              }}
            >
              Reset
            </Button>
          )}
        </div>
        <p className="text-[11px] text-muted-foreground">
          Changes apply on next sandbox restart. Defaults: {DEFAULT_CPU} vCPU /
          {' '}{DEFAULT_MEM_MB} MB / {DEFAULT_PIDS} PIDs.
        </p>
      </div>

      {/* Show-once preview password */}
      <div className="space-y-2 border-t pt-3">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <KeyRound className="h-3.5 w-3.5" />
            Preview password
          </div>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => regenerate.mutate()}
            disabled={regenerate.isPending || sessionClosed}
          >
            {regenerate.isPending ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <RotateCcw className="h-3.5 w-3.5" />
            )}
            Regenerate
          </Button>
        </div>

        {passwordRevealed ? (
          <div className="space-y-1.5">
            <div className="flex items-center gap-2">
              <code className="flex-1 rounded bg-muted px-2 py-1 text-xs font-mono break-all">
                {revealedPassword}
              </code>
              <CopyButton value={revealedPassword!} />
              <Button
                size="sm"
                variant="ghost"
                onClick={() => setRevealedPassword(null)}
              >
                Hide
              </Button>
            </div>
            <p className="text-[11px] text-amber-600 dark:text-amber-400">
              Save this password — it will not be shown again. Enter it on the
              login page shown when you first open a preview URL.
            </p>
          </div>
        ) : (
          <div className="text-xs text-muted-foreground">
            {session.preview_password_hint ? (
              <>
                Active password ends in{' '}
                <span className="font-mono">
                  …{session.preview_password_hint}
                </span>
                . Click <span className="font-medium">Regenerate</span> to
                issue a new one.
              </>
            ) : (
              <>No preview password set yet. Click Regenerate to issue one.</>
            )}
          </div>
        )}
      </div>
      </>
      )}
    </Card>
  )
}
