import { useEffect, useState } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'

interface ContainerMetricsProps {
  projectId: string
  environmentId: string
  containerId: string
}

interface Metrics {
  cpu_percent: number
  memory_bytes: number
  memory_limit_bytes: number
  memory_percent: number
  network_rx_bytes: number
  network_tx_bytes: number
  timestamp: string
  container_id: string
  container_name: string
}

export function ContainerMetrics({
  projectId,
  environmentId,
  containerId,
}: ContainerMetricsProps) {
  const [metrics, setMetrics] = useState<Metrics | null>(null)
  const [, setPrevMetrics] = useState<Metrics | null>(null)
  const [networkRxRate, setNetworkRxRate] = useState(0)
  const [networkTxRate, setNetworkTxRate] = useState(0)
  const [isConnected, setIsConnected] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const eventSource = new EventSource(
      `/api/projects/${projectId}/environments/${environmentId}/containers/${containerId}/metrics/stream`
    )

    eventSource.onopen = () => {
      setIsConnected(true)
      setError(null)
    }

    eventSource.onmessage = (event) => {
      try {
        const newMetric = JSON.parse(event.data)
        setMetrics((prev) => {
          if (prev) {
            // Calculate network rate from delta between samples
            const prevTime = new Date(prev.timestamp).getTime()
            const newTime = new Date(newMetric.timestamp).getTime()
            const intervalSec = Math.max((newTime - prevTime) / 1000, 1)

            const rxDelta = Math.max(newMetric.network_rx_bytes - prev.network_rx_bytes, 0)
            const txDelta = Math.max(newMetric.network_tx_bytes - prev.network_tx_bytes, 0)

            setNetworkRxRate(rxDelta / intervalSec)
            setNetworkTxRate(txDelta / intervalSec)
          }
          setPrevMetrics(prev)
          return newMetric
        })
      } catch (e) {
        console.error('Failed to parse metrics:', e)
      }
    }

    eventSource.onerror = () => {
      setIsConnected(false)
      setError('Failed to connect to metrics stream')
      eventSource.close()
    }

    return () => eventSource.close()
  }, [projectId, environmentId, containerId])

  if (!metrics) {
    return (
      <div className="flex items-center justify-center h-96 text-muted-foreground">
        {error ? (
          <div className="text-center">
            <p className="mb-2">{error}</p>
            <p className="text-sm">Retrying connection...</p>
          </div>
        ) : (
          <p>Waiting for metrics data...</p>
        )}
      </div>
    )
  }

  const cpu = metrics.cpu_percent ?? 0
  const memoryMb = (metrics.memory_bytes ?? 0) / (1024 * 1024)
  const memoryPercent = metrics.memory_percent ?? 0
  const networkIn = metrics.network_rx_bytes ?? 0
  const networkOut = metrics.network_tx_bytes ?? 0

  return (
    <div className="space-y-4">
      {/* Connection Status */}
      <div className="flex items-center gap-2">
        <div
          className={`w-2 h-2 rounded-full ${isConnected ? 'bg-green-600' : 'bg-gray-400'}`}
        />
        <p className="text-sm text-muted-foreground">
          {isConnected ? 'Live' : 'Offline'}
        </p>
        {metrics.timestamp && (
          <p className="text-xs text-muted-foreground ml-auto">
            {new Date(metrics.timestamp).toLocaleTimeString()}
          </p>
        )}
      </div>

      {/* Metrics Grid */}
      <div className="grid grid-cols-2 gap-4">
        {/* CPU */}
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">CPU Usage</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-baseline gap-2">
              <span className="text-2xl font-bold">
                {(cpu || 0).toFixed(1)}%
              </span>
            </div>
            <p className="text-xs text-muted-foreground mt-2">Current usage</p>
          </CardContent>
        </Card>

        {/* Memory */}
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">Memory Usage</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-baseline gap-2">
              <span className="text-2xl font-bold">
                {(memoryMb || 0).toFixed(0)} MB
              </span>
            </div>
            <p className="text-xs text-muted-foreground mt-2">
              {(memoryPercent || 0).toFixed(1)}%
            </p>
          </CardContent>
        </Card>

        {/* Network In */}
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">Network In</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-baseline gap-2">
              <span className="text-2xl font-bold">
                {formatBytes(networkIn || 0)}
              </span>
            </div>
            <p className="text-xs text-muted-foreground mt-2">
              {formatByteRate(networkRxRate)}
            </p>
          </CardContent>
        </Card>

        {/* Network Out */}
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">Network Out</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex items-baseline gap-2">
              <span className="text-2xl font-bold">
                {formatBytes(networkOut || 0)}
              </span>
            </div>
            <p className="text-xs text-muted-foreground mt-2">
              {formatByteRate(networkTxRate)}
            </p>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i]
}

function formatByteRate(bytesPerSec: number): string {
  if (bytesPerSec === 0) return '0 B/s'
  const k = 1024
  const sizes = ['B/s', 'KB/s', 'MB/s', 'GB/s']
  const i = Math.floor(Math.log(bytesPerSec) / Math.log(k))
  return parseFloat((bytesPerSec / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i]
}
