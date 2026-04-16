import { useEffect, useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import {
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query'
import {
  Box,
  ChevronDown,
  ExternalLink,
  Loader2,
  Play,
  RefreshCw,
  RotateCw,
  Square,
  Timer,
  Trash2,
} from 'lucide-react'
import { toast } from 'sonner'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { CopyButton } from '@/components/ui/copy-button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
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
import {
  extendTimeout,
  listSandboxes,
  pauseSandbox,
  restartSandbox,
  resumeSandbox,
  stopSandbox,
  type SandboxResponse,
} from '@/components/sandboxes/api'

function statusVariant(
  status: string,
): 'default' | 'secondary' | 'success' | 'warning' | 'destructive' | 'outline' {
  switch (status) {
    case 'running':
      return 'success'
    case 'stopped':
      return 'warning'
    case 'destroyed':
      return 'destructive'
    default:
      return 'outline'
  }
}

// Dev-server defaults the Open Preview dropdown surfaces as quick picks.
// Kept in sync with SandboxDetail — if a port family is added there, add
// it here so the list row and the detail page stay consistent.
const DEFAULT_PORTS: { port: number; label: string }[] = [
  { port: 3000, label: 'Next.js · Node' },
  { port: 5173, label: 'Vite' },
  { port: 8080, label: 'Generic HTTP' },
  { port: 8000, label: 'Django · FastAPI' },
  { port: 4000, label: 'Phoenix · Keystone' },
  { port: 4200, label: 'Angular' },
  { port: 3001, label: 'Alt Node' },
]

function formatCountdown(iso: string, now: number): string {
  const diffMs = new Date(iso).getTime() - now
  if (diffMs <= 0) return 'expired'
  const secs = Math.floor(diffMs / 1000)
  const d = Math.floor(secs / 86400)
  const h = Math.floor((secs % 86400) / 3600)
  const m = Math.floor((secs % 3600) / 60)
  const s = secs % 60
  if (d >= 1) return `${d}d ${h}h`
  if (h >= 1) return `${h}h ${m}m`
  if (m >= 1) return `${m}m ${s}s`
  return `${s}s`
}

function formatAge(iso: string, now: number): string {
  const diffMs = now - new Date(iso).getTime()
  if (diffMs < 0) {
    try {
      return new Date(iso).toLocaleString()
    } catch {
      return iso
    }
  }
  const secs = Math.floor(diffMs / 1000)
  if (secs < 60) return `${secs}s ago`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  return `${days}d ago`
}

/// Tick every second so the per-row countdown feels live. A single
/// top-level tick drives every row — cheaper than each row owning its
/// own interval, and all rows stay in visual sync.
function useNow(enabled: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!enabled) return
    const id = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [enabled])
  return now
}

const PAGE_SIZE = 20

export default function Sandboxes() {
  const [page, setPage] = useState(1)
  const [stopTarget, setStopTarget] = useState<SandboxResponse | null>(null)
  const queryClient = useQueryClient()

  const { data, isLoading, isError, error, refetch, isFetching } = useQuery({
    queryKey: ['sandboxes', page],
    queryFn: () => listSandboxes(page, PAGE_SIZE),
    refetchInterval: 15_000,
  })

  const items = data?.items ?? []
  const total = data?.total ?? 0
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE))

  // Only tick when at least one row has a live countdown to render. Avoids
  // pointless re-renders on an all-destroyed page.
  const needsTick = items.some((s) => s.status !== 'destroyed')
  const now = useNow(needsTick)

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ['sandboxes'] })

  const deleteMutation = useMutation({
    mutationFn: (id: string) => stopSandbox(id),
    meta: { errorTitle: 'Failed to delete sandbox' },
    onSuccess: () => {
      invalidate()
      setStopTarget(null)
      toast.success('Sandbox deleted')
    },
  })

  return (
    <div className="container mx-auto py-6 space-y-5">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="text-3xl font-bold flex items-center gap-2">
            <Box className="h-7 w-7" />
            Sandboxes
            {total > 0 && (
              <span className="text-muted-foreground font-normal text-lg">
                · {total}
              </span>
            )}
          </h1>
          <p className="text-muted-foreground mt-2">
            Standalone sandboxes ({'/v1/sandbox'}). Run isolated containers
            for one-off commands, tests, or agent work.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => refetch()}
            disabled={isFetching}
          >
            <RefreshCw
              className={`mr-2 h-4 w-4 ${isFetching ? 'animate-spin' : ''}`}
            />
            Refresh
          </Button>
        </div>
      </div>

      {isLoading ? (
        <div className="flex items-center justify-center py-16">
          <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      ) : isError ? (
        <Card>
          <CardContent className="py-12 text-center text-sm text-destructive">
            {(error as Error)?.message ?? 'Failed to load sandboxes'}
          </CardContent>
        </Card>
      ) : items.length === 0 ? (
        <Card>
          <CardContent className="py-16 text-center space-y-2">
            <Box className="mx-auto h-10 w-10 text-muted-foreground" />
            <p className="text-sm text-muted-foreground">No sandboxes yet.</p>
            <p className="text-xs text-muted-foreground">
              Create one via the{' '}
              <code className="font-mono">POST /v1/sandbox</code> API or the{' '}
              <code className="font-mono">temps sandbox</code> CLI.
            </p>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-3">
          {items.map((sbx) => (
            <SandboxRow
              key={sbx.id}
              sandbox={sbx}
              now={now}
              onDeleteRequest={setStopTarget}
            />
          ))}
        </div>
      )}

      {total > PAGE_SIZE && (
        <div className="flex items-center justify-between">
          <div className="text-sm text-muted-foreground">
            <span className="hidden sm:inline">
              Showing {(page - 1) * PAGE_SIZE + 1}–
              {Math.min(page * PAGE_SIZE, total)} of {total}
            </span>
            <span className="sm:hidden">
              {page} / {totalPages}
            </span>
          </div>
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={page <= 1}
              onClick={() => setPage((p) => Math.max(1, p - 1))}
            >
              Previous
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={page >= totalPages}
              onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
            >
              Next
            </Button>
          </div>
        </div>
      )}

      <AlertDialog
        open={stopTarget !== null}
        onOpenChange={(open) => {
          if (!open) setStopTarget(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete sandbox?</AlertDialogTitle>
            <AlertDialogDescription>
              This tears down the container for{' '}
              <span className="font-mono">{stopTarget?.id}</span>. The row is
              kept for audit but cannot be restarted. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (stopTarget) deleteMutation.mutate(stopTarget.id)
              }}
              disabled={deleteMutation.isPending}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {deleteMutation.isPending ? 'Deleting…' : 'Delete'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

/**
 * One sandbox row-card. Each row owns its own mutations so a slow
 * operation on one sandbox doesn't block actions on another (a pending
 * `Stop` on row A keeps row B's buttons live). The per-row mutation
 * objects are lightweight — React Query dedupes internally.
 *
 * Kept structurally parallel to SandboxDetail's header + status strip:
 * identity on the left, action cluster on the right, live countdown +
 * extend chips below. Clicking anywhere outside an interactive control
 * navigates into the detail page.
 */
function SandboxRow({
  sandbox,
  now,
  onDeleteRequest,
}: {
  sandbox: SandboxResponse
  now: number
  onDeleteRequest: (s: SandboxResponse) => void
}) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [customPort, setCustomPort] = useState('')

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ['sandboxes'] })

  const pauseMutation = useMutation({
    mutationFn: () => pauseSandbox(sandbox.id),
    meta: { errorTitle: 'Failed to stop sandbox' },
    onSuccess: () => {
      invalidate()
      toast.success('Sandbox stopped')
    },
  })

  const resumeMutation = useMutation({
    mutationFn: () => resumeSandbox(sandbox.id),
    meta: { errorTitle: 'Failed to resume sandbox' },
    onSuccess: () => {
      invalidate()
      toast.success('Sandbox resumed')
    },
  })

  const restartMutation = useMutation({
    mutationFn: () => restartSandbox(sandbox.id),
    meta: { errorTitle: 'Failed to restart sandbox' },
    onSuccess: () => {
      invalidate()
      toast.success('Sandbox restarted')
    },
  })

  const extendMutation = useMutation({
    mutationFn: (secs: number) => extendTimeout(sandbox.id, secs),
    meta: { errorTitle: 'Failed to extend timeout' },
    onSuccess: (_data, secs) => {
      invalidate()
      toast.success(
        `Timeout extended by ${secs >= 3600 ? `${secs / 3600}h` : `${secs / 60}m`}`,
      )
    },
  })

  const running = sandbox.status === 'running'
  const stopped = sandbox.status === 'stopped'
  const destroyed = sandbox.status === 'destroyed'
  const hasPreview = Boolean(sandbox.preview_url_template) && running

  const timeLeft = !destroyed ? formatCountdown(sandbox.expires_at, now) : '—'
  const expired = !destroyed && new Date(sandbox.expires_at).getTime() <= now

  const openPort = (port: number) => {
    if (!sandbox.preview_url_template || port < 1 || port > 65535) return
    const url = sandbox.preview_url_template.replace('{port}', String(port))
    window.open(url, '_blank', 'noopener,noreferrer')
  }

  const customPortValid = useMemo(() => {
    if (!/^\d+$/.test(customPort)) return false
    const n = Number(customPort)
    return n >= 1 && n <= 65535
  }, [customPort])

  // Whole card is clickable so rows behave like links, but we stop the
  // click propagation on every interactive control below. Background
  // click → detail; button click → that button's action only.
  const goToDetail = () => navigate(`/sandboxes/${sandbox.id}`)
  const stop = (e: React.MouseEvent) => e.stopPropagation()

  return (
    <Card
      className={`cursor-pointer transition-colors hover:border-foreground/20 ${
        expired ? 'border-destructive/40' : ''
      }`}
      onClick={goToDetail}
    >
      <CardContent className="py-4 space-y-3">
        {/* Identity row + action cluster */}
        <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
          <div className="min-w-0 space-y-1">
            <div className="flex items-center gap-2 flex-wrap">
              <Link
                to={`/sandboxes/${sandbox.id}`}
                onClick={stop}
                className="font-medium hover:underline truncate"
              >
                {sandbox.name}
              </Link>
              <Badge variant={statusVariant(sandbox.status)}>
                {sandbox.status}
              </Badge>
              {sandbox.image && (
                <span className="font-mono text-[11px] text-muted-foreground truncate">
                  {sandbox.image}
                </span>
              )}
            </div>
            <div
              className="flex items-center gap-2 text-xs font-mono text-muted-foreground"
              onClick={stop}
            >
              <span className="truncate">{sandbox.id}</span>
              <CopyButton
                value={sandbox.id}
                minimal
                className="h-5 w-5 shrink-0"
              />
            </div>
          </div>

          <div
            className="flex flex-wrap items-center gap-2"
            onClick={stop}
          >
            {hasPreview && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button size="sm" className="gap-1">
                    <ExternalLink className="h-4 w-4" />
                    Open preview
                    <ChevronDown className="h-3 w-3 opacity-70" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-56">
                  {DEFAULT_PORTS.map((p) => (
                    <DropdownMenuItem
                      key={p.port}
                      onClick={() => openPort(p.port)}
                      className="justify-between"
                    >
                      <span className="font-mono text-xs">:{p.port}</span>
                      <span className="text-muted-foreground text-xs">
                        {p.label}
                      </span>
                    </DropdownMenuItem>
                  ))}
                  <DropdownMenuSeparator />
                  <div className="p-2 space-y-1.5">
                    <Label
                      htmlFor={`port-${sandbox.id}`}
                      className="text-[11px] text-muted-foreground"
                    >
                      Custom port
                    </Label>
                    <form
                      className="flex items-center gap-1.5"
                      onSubmit={(e) => {
                        e.preventDefault()
                        if (customPortValid) openPort(Number(customPort))
                      }}
                    >
                      <Input
                        id={`port-${sandbox.id}`}
                        type="number"
                        min={1}
                        max={65535}
                        placeholder="4321"
                        value={customPort}
                        onChange={(e) => setCustomPort(e.target.value)}
                        className="h-8 text-xs"
                      />
                      <Button
                        type="submit"
                        size="sm"
                        variant="outline"
                        className="h-8"
                        disabled={!customPortValid}
                      >
                        Go
                      </Button>
                    </form>
                  </div>
                </DropdownMenuContent>
              </DropdownMenu>
            )}

            {running && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => pauseMutation.mutate()}
                disabled={pauseMutation.isPending}
              >
                <Square className="mr-1 h-4 w-4" />
                Stop
              </Button>
            )}
            {stopped && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => resumeMutation.mutate()}
                disabled={resumeMutation.isPending}
              >
                <Play className="mr-1 h-4 w-4" />
                Resume
              </Button>
            )}
            {running && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => restartMutation.mutate()}
                disabled={restartMutation.isPending}
                title="Restart"
              >
                <RotateCw className="h-4 w-4" />
              </Button>
            )}
            {!destroyed && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => onDeleteRequest(sandbox)}
                className="text-destructive hover:text-destructive"
                title="Delete"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>

        {/* Live countdown + inline extend — hidden for destroyed rows */}
        {!destroyed && (
          <div
            className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between pt-1 border-t"
            onClick={stop}
          >
            <div className="flex items-center gap-4 flex-wrap pt-2">
              <div className="flex items-center gap-2">
                <Timer
                  className={`h-4 w-4 ${
                    expired ? 'text-destructive' : 'text-muted-foreground'
                  }`}
                />
                <div className="leading-tight">
                  <div
                    className={`font-mono text-xs ${
                      expired ? 'text-destructive' : ''
                    }`}
                  >
                    {expired ? 'expired' : `${timeLeft} left`}
                  </div>
                  <div className="text-[10px] text-muted-foreground">
                    created {formatAge(sandbox.created_at, now)}
                  </div>
                </div>
              </div>
            </div>
            <div className="flex items-center gap-1.5 pt-2 sm:pt-0">
              <span className="text-[11px] text-muted-foreground mr-1">
                Extend:
              </span>
              <Button
                variant="outline"
                size="sm"
                className="h-7 text-xs"
                onClick={() => extendMutation.mutate(900)}
                disabled={extendMutation.isPending}
              >
                +15m
              </Button>
              <Button
                variant="outline"
                size="sm"
                className="h-7 text-xs"
                onClick={() => extendMutation.mutate(3600)}
                disabled={extendMutation.isPending}
              >
                +1h
              </Button>
              <Button
                variant="outline"
                size="sm"
                className="h-7 text-xs"
                onClick={() => extendMutation.mutate(14400)}
                disabled={extendMutation.isPending}
              >
                +4h
              </Button>
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
