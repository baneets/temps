import { useEffect, useState } from 'react'

export interface LiveMetrics {
  cpu_percent: number
  memory_bytes: number
  memory_limit_bytes: number
  memory_percent: number
  network_rx_bytes: number
  network_tx_bytes: number
  network_rx_rate: number
  network_tx_rate: number
  timestamp: string
}

interface RawMetric {
  cpu_percent: number
  memory_bytes: number
  memory_limit_bytes: number
  memory_percent: number
  network_rx_bytes: number
  network_tx_bytes: number
  timestamp: string
}

export function useContainerMetricsStream(
  projectId: string,
  environmentId: string,
  containerId: string,
  enabled: boolean = true
): { metrics: LiveMetrics | null; connected: boolean; error: string | null } {
  const [metrics, setMetrics] = useState<LiveMetrics | null>(null)
  const [connected, setConnected] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!enabled || !projectId || !environmentId || !containerId) return

    const eventSource = new EventSource(
      `/api/projects/${projectId}/environments/${environmentId}/containers/${containerId}/metrics/stream`
    )

    let prev: RawMetric | null = null

    eventSource.onopen = () => {
      setConnected(true)
      setError(null)
    }

    eventSource.onmessage = (event) => {
      try {
        const next = JSON.parse(event.data) as RawMetric
        let rxRate = 0
        let txRate = 0
        if (prev) {
          const dt = Math.max(
            (new Date(next.timestamp).getTime() -
              new Date(prev.timestamp).getTime()) /
              1000,
            1
          )
          rxRate = Math.max(next.network_rx_bytes - prev.network_rx_bytes, 0) / dt
          txRate = Math.max(next.network_tx_bytes - prev.network_tx_bytes, 0) / dt
        }
        prev = next
        setMetrics({
          ...next,
          network_rx_rate: rxRate,
          network_tx_rate: txRate,
        })
      } catch (e) {
        console.error('Failed to parse metrics:', e)
      }
    }

    eventSource.onerror = () => {
      setConnected(false)
      setError('Failed to connect to metrics stream')
      eventSource.close()
    }

    return () => eventSource.close()
  }, [projectId, environmentId, containerId, enabled])

  return { metrics, connected, error }
}
