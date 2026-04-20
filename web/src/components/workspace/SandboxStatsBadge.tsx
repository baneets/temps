// Live CPU + memory usage badge for a workspace session's sandbox.
//
// Polls `/sandbox/stats` every few seconds and renders a compact
// `cpu 0.42/2 · mem 1.3/4 GB` pill. Polling is disabled while the tab is
// hidden so background sessions don't hammer the backend. When the
// session hasn't provisioned a sandbox yet (or the user just closed it),
// the component renders nothing — no layout shift for the common case.

import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Cpu, MemoryStick } from 'lucide-react'

import { workspaceSandboxStatsOptions } from '@/api/client/@tanstack/react-query.gen'

interface Props {
  projectId: number
  sessionId: number
  /** Skip polling when the session has no running sandbox. */
  enabled: boolean
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = bytes
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  // 1 decimal for GB/TB, whole numbers otherwise — matches how users
  // intuit container sizes.
  const digits = i >= 3 ? 1 : 0
  return `${v.toFixed(digits)} ${units[i]}`
}

function percentColor(pct: number): string {
  if (pct >= 90) return 'text-red-500'
  if (pct >= 70) return 'text-amber-500'
  return 'text-muted-foreground'
}

export function SandboxStatsBadge({ projectId, sessionId, enabled }: Props) {
  // Track document visibility so we can pause polling when the tab is
  // backgrounded. Without this, 20 open workspace tabs would each fire
  // a stats request every 3s forever.
  const [visible, setVisible] = useState(
    typeof document === 'undefined' ? true : !document.hidden,
  )
  useEffect(() => {
    const onVis = () => setVisible(!document.hidden)
    document.addEventListener('visibilitychange', onVis)
    return () => document.removeEventListener('visibilitychange', onVis)
  }, [])

  const query = useQuery({
    ...workspaceSandboxStatsOptions({
      path: { project_id: projectId, session_id: sessionId },
    }),
    enabled: enabled && visible,
    refetchInterval: enabled && visible ? 3000 : false,
    // Stats are inherently stale — don't keep the cache around long.
    staleTime: 0,
    gcTime: 10_000,
    retry: false,
  })

  if (!enabled) return null
  if (query.isError) {
    return (
      <span
        className="font-mono text-red-500"
        title={query.error instanceof Error ? query.error.message : 'error'}
      >
        stats ×
      </span>
    )
  }
  if (!query.data) {
    return <span className="font-mono text-muted-foreground/70">stats…</span>
  }

  const s = query.data
  const cpuClass = percentColor(s.cpu_percent)
  const memClass = percentColor(s.memory_percent)

  return (
    <span className="inline-flex items-center gap-2 font-mono">
      <span
        className={`inline-flex items-center gap-1 ${cpuClass}`}
        title={`CPU: ${s.cpu_used_cores.toFixed(2)} / ${s.cpu_limit_cores.toFixed(0)} vCPU (${s.cpu_percent.toFixed(0)}%)`}
      >
        <Cpu className="h-3 w-3" />
        {s.cpu_used_cores.toFixed(2)}/{s.cpu_limit_cores.toFixed(0)}
      </span>
      <span
        className={`inline-flex items-center gap-1 ${memClass}`}
        title={`Memory: ${formatBytes(s.memory_used_bytes)} / ${formatBytes(s.memory_limit_bytes)} (${s.memory_percent.toFixed(0)}%)`}
      >
        <MemoryStick className="h-3 w-3" />
        {formatBytes(s.memory_used_bytes)}/{formatBytes(s.memory_limit_bytes)}
      </span>
    </span>
  )
}
