'use client'

import { ContainerInfoResponse, ProjectResponse } from '@/api/client'
import {
  getEnvironmentsOptions,
  listContainersOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { cn } from '@/lib/utils'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useVirtualizer } from '@tanstack/react-virtual'
import { AlertCircle, ChevronDown, ChevronUp, Search } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { FilterBar } from './filter-bar'
import { LogLine } from './log-line'

function SelectedContainerLabel({
  container,
  containerId,
}: {
  container: ContainerInfoResponse | undefined
  containerId: string
}) {
  return (
    <div className="flex items-center gap-2 text-left">
      <span className="truncate">{container?.container_name}</span>
      {container?.node_name && (
        <span className="text-xs text-muted-foreground bg-muted px-1.5 py-0.5 rounded shrink-0">
          {container.node_name}
        </span>
      )}
      <span className="text-xs text-muted-foreground shrink-0">
        {containerId.substring(0, 12)}
      </span>
    </div>
  )
}

function estimateLineHeight(content: string, containerWidth: number) {
  // Assuming average character width of 8px in monospace font
  const averageCharWidth = 9
  const lineHeight = 20 // Base height for a single line
  const minHeight = 24 // Minimum height to prevent overlap

  if (!content || !containerWidth) return minHeight

  // Account for padding (py-1 = 8px vertical padding)
  const paddingHeight = 8

  // Calculate how many lines this content might wrap into
  const effectiveWidth = containerWidth - 32 // Account for container padding (p-4 = 16px each side)
  const charactersPerLine = Math.max(
    1,
    Math.floor(effectiveWidth / averageCharWidth)
  )
  const estimatedLines = Math.max(
    1,
    Math.ceil(content.length / charactersPerLine)
  )

  return Math.max(minHeight, lineHeight * estimatedLines + paddingHeight)
}

export default function LogViewer({ project }: { project: ProjectResponse }) {
  const [logs, setLogs] = useState<string[]>([])
  const [connectionStatus, setConnectionStatus] = useState<
    'connecting' | 'connected' | 'error' | 'permanent_error'
  >('connecting')
  const [retryCount, setRetryCount] = useState(0)
  const [errorMessage, setErrorMessage] = useState('')
  const [searchTerm, setSearchTerm] = useState('')
  const [currentMatchIndex, setCurrentMatchIndex] = useState(-1)
  const [startDate, setStartDate] = useState<Date>()
  const [endDate, setEndDate] = useState<Date>()
  const [selectedTarget, setSelectedTarget] = useState<number>()
  const [selectedContainer, setSelectedContainer] = useState<string>()
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [tail, setTail] = useState<number>(1000)
  const [autoScroll, setAutoScroll] = useState(true)
  const [showTimestamps, setShowTimestamps] = useState(false)
  const parentRef = useRef<HTMLDivElement>(null)
  const matchRefs = useRef<HTMLSpanElement[]>([])
  const wsRef = useRef<WebSocket | null>(null)
  const containerWidth = useRef<number>(0)
  const isConnectingRef = useRef(false)
  const retryTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const virtualizer = useVirtualizer({
    count: logs.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (index) => {
      return estimateLineHeight(logs[index], containerWidth.current)
    },
    overscan: 20,
    measureElement: (element) => {
      return element?.getBoundingClientRect().height ?? 0
    },
  })
  // Poll the environments list. `current_deployment_id` on the entry whose
  // id == selectedTarget is the per-environment "what's running right now"
  // pointer; we watch it below to detect redeploys for the env the user
  // is actually looking at. Polling here is fine because it's a single
  // small query and avoids a separate per-env endpoint.
  const { data: environments } = useQuery({
    ...getEnvironmentsOptions({
      path: { project_id: project.id },
    }),
    refetchInterval: 3000,
    refetchOnWindowFocus: true,
  })

  const queryClient = useQueryClient()

  // Fetch containers for the selected environment. The list itself doesn't
  // need polling — instead we watch the project's last-deployment id below
  // and invalidate this query when it flips, which is a far cheaper and
  // more deterministic signal than blind polling.
  const containersQueryKey = useMemo(
    () =>
      listContainersOptions({
        path: {
          project_id: project.id,
          environment_id: selectedTarget || 0,
        },
      }).queryKey,
    [project.id, selectedTarget],
  )

  const { data: containersData } = useQuery({
    ...listContainersOptions({
      path: {
        project_id: project.id,
        environment_id: selectedTarget || 0,
      },
    }),
    enabled: !!selectedTarget,
    staleTime: 0,
    gcTime: 0,
    refetchOnMount: 'always',
    refetchOnWindowFocus: true,
  })

  // The per-environment `current_deployment_id` is the canonical "what is
  // running right now" pointer. Watching this (rather than the project-wide
  // last deployment) means a redeploy in *another* environment won't make
  // us drop logs we're tailing here.
  const currentEnv = environments?.find((e) => e.id === selectedTarget)
  const currentDeploymentId = currentEnv?.current_deployment_id ?? null

  // When the env's current deployment id flips, refresh the container list
  // so the reconciliation effect can pick up the new containers. Crucially
  // we DO NOT clear `selectedContainer` here — that creates a window where
  // the WS effect bails out (no container) and then races with the new
  // list arriving. Instead we let the reconciliation effect atomically
  // swap selectedContainer once the new container is visible in the list,
  // which keeps the WS lifecycle to a single clean reconnect.
  const previousDeploymentIdRef = useRef<number | null>(null)
  useEffect(() => {
    if (currentDeploymentId == null) return
    const prev = previousDeploymentIdRef.current
    previousDeploymentIdRef.current = currentDeploymentId
    if (prev != null && prev !== currentDeploymentId) {
      queryClient.invalidateQueries({ queryKey: containersQueryKey })
    }
  }, [currentDeploymentId, queryClient, containersQueryKey])

  // Auto-select first environment when environments are loaded
  useEffect(() => {
    if (environments && environments.length > 0 && !selectedTarget) {
      setSelectedTarget(environments[0].id)
    }
  }, [environments, selectedTarget])

  // Reconcile selectedContainer with the latest container list:
  //   - if nothing is selected, pick the first container
  //   - if the previously-selected container is no longer present (e.g. it
  //     was destroyed by a redeploy), fall back to the first available
  // Only acts when the list has at least one container, so a transient
  // empty-list response during the redeploy window won't yank an
  // already-good selection out from under the WS.
  useEffect(() => {
    const containers = containersData?.containers ?? []
    if (containers.length === 0) return

    const stillExists = containers.some(
      (c) => c.container_id === selectedContainer,
    )
    if (!selectedContainer || !stillExists) {
      setSelectedContainer(containers[0].container_id)
    }
  }, [containersData, selectedContainer])

  // WebSocket connection effect
  useEffect(() => {
    if (!selectedTarget) return

    // Wait for container to be selected - don't connect without a specific container
    if (!selectedContainer) return

    // Capture the container this effect-instance is tailing. Used by the
    // socket handlers below to reject any late frames from a previous
    // socket whose handlers may still fire while React is unwinding.
    const targetContainer = selectedContainer
    setLogs([])
    setRetryCount(0)
    setErrorMessage('')
    isConnectingRef.current = false

    let isCleaningUp = false
    let currentRetryCount = 0

    const connectWS = () => {
      if (isCleaningUp) {
        return
      }

      isConnectingRef.current = true
      const params = new URLSearchParams()
      if (startDate) {
        params.append(
          'start_date',
          Math.floor(startDate.getTime() / 1000).toString()
        )
      }
      if (endDate) {
        params.append(
          'end_date',
          Math.floor(endDate.getTime() / 1000).toString()
        )
      }
      if (tail) {
        params.append('tail', tail.toString())
      }
      // Add timestamps parameter
      params.append('timestamps', showTimestamps.toString())

      // Use container-specific endpoint (selectedContainer is guaranteed by the guard above)
      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
      const wsUrl = `${protocol}//${window.location.host}/api/projects/${project.id}/environments/${selectedTarget}/containers/${targetContainer}/logs?${params.toString()}`

      // Close any prior socket and detach its handlers so its in-flight
      // frames or close events can't bleed into this connection's state.
      if (wsRef.current) {
        const prev = wsRef.current
        prev.onopen = null
        prev.onmessage = null
        prev.onerror = null
        prev.onclose = null
        try {
          prev.close(1000, 'Reconnecting')
        } catch {
          // best-effort
        }
        wsRef.current = null
      }

      try {
        const ws = new WebSocket(wsUrl)
        wsRef.current = ws
        setConnectionStatus('connecting')

        ws.onopen = () => {
          // Stale-socket guard: if React already swapped us out before the
          // open event fired, do nothing.
          if (ws !== wsRef.current || isCleaningUp) return
          setConnectionStatus('connected')
          currentRetryCount = 0
          setRetryCount(0)
          setErrorMessage('')
          isConnectingRef.current = false

          // Clear any pending retry timeouts
          if (retryTimeoutRef.current) {
            clearTimeout(retryTimeoutRef.current)
            retryTimeoutRef.current = null
          }
        }

        ws.onmessage = (event) => {
          // Drop frames from any socket that's no longer the active one —
          // prevents old-deployment "Deployment not found" frames from
          // bleeding into the freshly-connected container's log buffer.
          if (ws !== wsRef.current || isCleaningUp) return
          try {
            // Try to parse as JSON first
            const parsed = JSON.parse(event.data)

            // If it's an error object with stack, format it nicely
            if (parsed.error && parsed.stack) {
              const formattedLog = `ERROR: ${parsed.error}\n${parsed.stack}`
              setLogs((prevLogs) => [...prevLogs, formattedLog])
            }
            // If it's a log object with a message field
            else if (parsed.message) {
              setLogs((prevLogs) => [...prevLogs, parsed.message])
            }
            // If it's a log object with a log field
            else if (parsed.log) {
              setLogs((prevLogs) => [...prevLogs, parsed.log])
            }
            // Otherwise stringify it
            else {
              setLogs((prevLogs) => [
                ...prevLogs,
                JSON.stringify(parsed, null, 2),
              ])
            }
          } catch {
            // If it's not JSON, just use it as-is
            setLogs((prevLogs) => [...prevLogs, event.data])
          }
        }

        ws.onerror = (error) => {
          if (ws !== wsRef.current || isCleaningUp) return
          console.error('WebSocket error:', error)
          setErrorMessage('Connection failed')
          isConnectingRef.current = false
        }

        ws.onclose = (event) => {
          // Late close from a replaced socket — ignore.
          if (ws !== wsRef.current) return
          isConnectingRef.current = false

          // Don't reconnect if cleaning up or normal closure
          if (isCleaningUp || event.code === 1000) {
            return
          }

          // Increment retry count
          currentRetryCount++
          setRetryCount(currentRetryCount)

          if (currentRetryCount >= 3) {
            setConnectionStatus('permanent_error')
            setErrorMessage('Connection failed after multiple attempts')
            return
          }

          // Temporary error - attempt to reconnect
          setConnectionStatus('error')
          const delay = Math.pow(2, currentRetryCount) * 1000

          // Clear any existing retry timeout
          if (retryTimeoutRef.current) {
            clearTimeout(retryTimeoutRef.current)
          }

          retryTimeoutRef.current = setTimeout(() => {
            retryTimeoutRef.current = null
            connectWS()
          }, delay)
        }
      } catch (error) {
        console.error('Failed to create WebSocket:', error)
        setConnectionStatus('permanent_error')
        setErrorMessage('Failed to establish connection')
        isConnectingRef.current = false
      }
    }

    connectWS()

    return () => {
      isCleaningUp = true
      isConnectingRef.current = false

      // Clear any pending retry timeout
      if (retryTimeoutRef.current) {
        clearTimeout(retryTimeoutRef.current)
        retryTimeoutRef.current = null
      }

      // Detach handlers before closing so any late events from the closing
      // socket can't write to React state on the next mounted instance.
      const ws = wsRef.current
      if (ws) {
        ws.onopen = null
        ws.onmessage = null
        ws.onerror = null
        ws.onclose = null
        try {
          ws.close(1000, 'Component unmounting')
        } catch {
          // best-effort
        }
        wsRef.current = null
      }
    }
    // `containersData` is intentionally NOT in this dep array. The polling
    // refetch (every 3s) updates that object on every tick; if it were a
    // dep, we would tear down and re-open the WebSocket every 3s and the
    // user would see the "Connection lost" banner flap forever. The
    // reconciliation effect above already swaps `selectedContainer` when
    // the active container disappears, which *does* re-trigger this effect
    // via the selectedContainer dep — that's the only legitimate reason
    // to reconnect.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    project.id,
    project.slug,
    selectedTarget,
    selectedContainer,
    startDate,
    endDate,
    tail,
    showTimestamps,
  ])

  // Shared connectWS function for retry
  const handleRetryConnection = useCallback(() => {
    setRetryCount(0)
    setConnectionStatus('connecting')
    setErrorMessage('')

    const params = new URLSearchParams()
    if (startDate) {
      params.append(
        'start_date',
        Math.floor(startDate.getTime() / 1000).toString()
      )
    }
    if (endDate) {
      params.append('end_date', Math.floor(endDate.getTime() / 1000).toString())
    }
    if (tail) {
      params.append('tail', tail.toString())
    }
    // Add timestamps parameter
    params.append('timestamps', showTimestamps.toString())

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const wsUrl = `${protocol}//${window.location.host}/api/projects/${project.id}/environments/${selectedTarget}/containers/${selectedContainer}/logs?${params.toString()}`

    // Close existing connection if any
    if (wsRef.current) {
      wsRef.current.close()
    }

    try {
      wsRef.current = new WebSocket(wsUrl)
      setConnectionStatus('connecting')

      wsRef.current.onopen = () => {
        setConnectionStatus('connected')
        setRetryCount(0)
        setErrorMessage('')
      }

      wsRef.current.onmessage = (event) => {
        try {
          // Try to parse as JSON first
          const parsed = JSON.parse(event.data)

          // If it's an error object with stack, format it nicely
          if (parsed.error && parsed.stack) {
            const formattedLog = `ERROR: ${parsed.error}\n${parsed.stack}`
            setLogs((prevLogs) => [...prevLogs, formattedLog])
          }
          // If it's a log object with a message field
          else if (parsed.message) {
            setLogs((prevLogs) => [...prevLogs, parsed.message])
          }
          // If it's a log object with a log field
          else if (parsed.log) {
            setLogs((prevLogs) => [...prevLogs, parsed.log])
          }
          // Otherwise stringify it
          else {
            setLogs((prevLogs) => [
              ...prevLogs,
              JSON.stringify(parsed, null, 2),
            ])
          }
        } catch {
          // If it's not JSON, just use it as-is
          setLogs((prevLogs) => [...prevLogs, event.data])
        }
      }

      wsRef.current.onerror = () => {
        // Try to extract more details from the error event
        let errorMessage = 'Connection failed'
        setErrorMessage(errorMessage)
        wsRef.current?.close()

        setRetryCount((prev) => {
          const newRetryCount = prev + 1
          if (newRetryCount >= 3) {
            setConnectionStatus('permanent_error')
            setErrorMessage('Connection failed after multiple attempts')
            return newRetryCount
          }

          setConnectionStatus('error')

          setTimeout(
            () => {
              if (wsRef.current !== null) {
                handleRetryConnection()
              }
            },
            Math.pow(2, newRetryCount) * 1000
          )

          return newRetryCount
        })
      }
    } catch {
      setConnectionStatus('permanent_error')
      setErrorMessage('Failed to establish connection')
    }
  }, [
    project.id,
    selectedTarget,
    selectedContainer,
    startDate,
    endDate,
    tail,
    showTimestamps,
  ])

  // Update search functionality
  const scrollToMatch = (index: number, matches: number) => {
    if (matches === 0) return

    const wrappedIndex = ((index % matches) + matches) % matches
    setCurrentMatchIndex(wrappedIndex)

    const element = document.getElementById(`search-match-${wrappedIndex}`)
    element?.scrollIntoView({
      behavior: 'smooth',
      block: 'center',
    })
  }

  // Update search refs effect
  useEffect(() => {
    if (!searchTerm) {
      setCurrentMatchIndex(-1)
      return
    }

    const elements = document.querySelectorAll('[id^="search-match-"]')
    matchRefs.current = Array.from(elements) as HTMLSpanElement[]

    if (matchRefs.current.length > 0 && currentMatchIndex === -1) {
      scrollToMatch(0, matchRefs.current.length)
    }
  }, [logs, searchTerm, currentMatchIndex])

  useEffect(() => {
    if (autoScroll && parentRef.current) {
      parentRef.current.scrollTop = parentRef.current.scrollHeight
    }
  }, [logs, autoScroll])

  const handleScroll = (event: React.UIEvent<HTMLDivElement>) => {
    const { scrollTop, scrollHeight, clientHeight } = event.currentTarget
    const isAtBottom = scrollHeight - scrollTop - clientHeight < 1
    setAutoScroll(isAtBottom)
  }

  const handleSearch = useCallback((value: string) => {
    setSearchTerm(value)
    setCurrentMatchIndex(0)
  }, [])

  const handleRetry = () => {
    handleRetryConnection()
  }

  // Add this effect to measure container width
  useEffect(() => {
    if (parentRef.current) {
      const resizeObserver = new ResizeObserver((entries) => {
        containerWidth.current = entries[0].contentRect.width
        virtualizer.measure()
      })

      resizeObserver.observe(parentRef.current)
      return () => resizeObserver.disconnect()
    }
  }, [virtualizer])

  return (
    <div className="w-full">
      <div className="rounded-lg border bg-background shadow-sm">
        {/* Add connection status alerts */}
        {connectionStatus === 'connecting' && (
          <Alert className="m-4">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>Connecting to log stream...</AlertDescription>
          </Alert>
        )}

        {connectionStatus === 'error' && (
          <Alert variant="destructive" className="m-4">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>
              Connection lost. Attempting to reconnect... (Attempt {retryCount}
              /3)
            </AlertDescription>
          </Alert>
        )}

        {connectionStatus === 'permanent_error' && (
          <Alert variant="destructive" className="m-4">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription className="flex items-center justify-between">
              <span>{errorMessage || 'Connection failed permanently'}</span>
              <Button
                variant="outline"
                size="sm"
                onClick={handleRetry}
                className="ml-4"
              >
                Retry Connection
              </Button>
            </AlertDescription>
          </Alert>
        )}

        {/* Main Filters */}
        <div className="p-4 space-y-4">
          <div className="flex flex-col sm:flex-row gap-4">
            <Select
              value={selectedTarget?.toString()}
              onValueChange={(value) => {
                setSelectedTarget(Number(value))
                setSelectedContainer(undefined)
              }}
            >
              <SelectTrigger className="w-full sm:w-[250px]">
                <SelectValue placeholder="Select environment" />
              </SelectTrigger>
              <SelectContent>
                {environments?.map((environment) => (
                  <SelectItem
                    key={environment.id}
                    value={environment.id.toString()}
                  >
                    {environment.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            <Select
              value={selectedContainer}
              onValueChange={(value) => setSelectedContainer(value)}
            >
              <SelectTrigger className="w-full sm:w-auto sm:max-w-[400px] text-left">
                <SelectValue placeholder="Select container">
                  {selectedContainer && (
                    <SelectedContainerLabel
                      container={containersData?.containers?.find(
                        (x) => x.container_id === selectedContainer
                      )}
                      containerId={selectedContainer}
                    />
                  )}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                {containersData?.containers?.map((container) => (
                  <SelectItem
                    key={container.container_id}
                    value={container.container_id}
                  >
                    <div className="flex flex-col items-start text-left">
                      <div className="flex items-center gap-2">
                        <span>{container.container_name}</span>
                        {container.node_name && (
                          <span className="text-xs text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                            {container.node_name}
                          </span>
                        )}
                      </div>
                      <span className="text-xs text-muted-foreground">
                        {container.container_id.substring(0, 12)}
                      </span>
                    </div>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            <div className="relative flex-1">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
              <Input
                placeholder="Search logs..."
                value={searchTerm}
                onChange={(e) => handleSearch(e.target.value)}
                className="pl-9 w-full"
              />
            </div>

            <div className="flex items-center gap-2">
              <Checkbox
                id="show-timestamps"
                checked={showTimestamps}
                onCheckedChange={(checked) =>
                  setShowTimestamps(checked === true)
                }
              />
              <Label
                htmlFor="show-timestamps"
                className="text-sm font-normal cursor-pointer"
              >
                Show timestamps
              </Label>
            </div>
          </div>

          <div className="flex items-center justify-between">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setShowAdvanced(!showAdvanced)}
              className="text-muted-foreground hover:text-foreground"
            >
              Advanced Options
              {showAdvanced ? (
                <ChevronUp className="ml-2 h-4 w-4" />
              ) : (
                <ChevronDown className="ml-2 h-4 w-4" />
              )}
            </Button>
          </div>

          {showAdvanced && (
            <div className="pt-4 border-t border-border">
              <FilterBar
                onStartDateChange={setStartDate}
                onEndDateChange={setEndDate}
                onTailLinesChange={(lines) => setTail(lines)}
                startDate={startDate}
                endDate={endDate}
                tailLines={tail}
              />
            </div>
          )}
        </div>
        {/* Logs Display */}
        <div className="border-t border-border">
          {!selectedTarget ? (
            <div className="h-[600px] flex items-center justify-center text-muted-foreground">
              <div className="text-center">
                <AlertCircle className="h-12 w-12 mx-auto mb-3 opacity-50" />
                <p className="text-sm">Select an environment to view logs</p>
              </div>
            </div>
          ) : (
            <div
              ref={parentRef}
              className={cn(
                'h-[600px] overflow-auto p-4 font-mono text-xs bg-background text-foreground select-text',
                connectionStatus === 'connecting' && 'opacity-50'
              )}
              onScroll={handleScroll}
            >
              <div
                style={{
                  height: `${virtualizer.getTotalSize()}px`,
                  width: '100%',
                  position: 'relative',
                }}
              >
                {virtualizer.getVirtualItems().map((virtualRow) => (
                  <div
                    key={virtualRow.key}
                    data-index={virtualRow.index}
                    ref={virtualizer.measureElement}
                    style={{
                      position: 'absolute',
                      top: `${virtualRow.start}px`,
                      left: 0,
                      width: '100%',
                    }}
                  >
                    <LogLine
                      content={logs[virtualRow.index]}
                      isHighlighted={virtualRow.index === currentMatchIndex}
                      searchTerm={searchTerm}
                    />
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
