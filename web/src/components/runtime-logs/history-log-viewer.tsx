'use client'

import { ProjectResponse } from '@/api/client'
import { getEnvironmentsOptions } from '@/api/client/@tanstack/react-query.gen'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { cn } from '@/lib/utils'
import {
  LogLevel,
  LogSearchLine,
  useLogHistory,
} from '@/hooks/useLogHistory'
import { useQuery } from '@tanstack/react-query'
import { useVirtualizer } from '@tanstack/react-virtual'
import {
  AlertCircle,
  ChevronLeft,
  ChevronRight,
  Clock,
  Loader2,
  Search,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

const LOG_LEVEL_OPTIONS: LogLevel[] = ['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE']

const LEVEL_COLORS: Record<LogLevel, string> = {
  ERROR: 'bg-red-500/15 text-red-700 dark:text-red-400 border-red-500/20',
  WARN: 'bg-yellow-500/15 text-yellow-700 dark:text-yellow-400 border-yellow-500/20',
  INFO: 'bg-blue-500/15 text-blue-700 dark:text-blue-400 border-blue-500/20',
  DEBUG: 'bg-zinc-500/15 text-zinc-700 dark:text-zinc-400 border-zinc-500/20',
  TRACE: 'bg-zinc-400/15 text-zinc-500 dark:text-zinc-500 border-zinc-400/20',
}

function TimeRangeSelect({
  value,
  onChange,
}: {
  value: string
  onChange: (value: string) => void
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className="w-[160px]">
        <Clock className="h-3.5 w-3.5 mr-2 text-muted-foreground" />
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="15m">Last 15 min</SelectItem>
        <SelectItem value="1h">Last 1 hour</SelectItem>
        <SelectItem value="6h">Last 6 hours</SelectItem>
        <SelectItem value="24h">Last 24 hours</SelectItem>
      </SelectContent>
    </Select>
  )
}

function getTimeRange(range: string): { start: string; end: string } {
  const now = new Date()
  const end = now.toISOString()
  const ms: Record<string, number> = {
    '15m': 15 * 60 * 1000,
    '1h': 60 * 60 * 1000,
    '6h': 6 * 60 * 60 * 1000,
    '24h': 24 * 60 * 60 * 1000,
  }
  const start = new Date(now.getTime() - (ms[range] ?? ms['1h'])).toISOString()
  return { start, end }
}

function HistoryLogLine({ line }: { line: LogSearchLine }) {
  const ts = new Date(line.timestamp).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  })

  return (
    <div className="flex items-start gap-2 py-0.5 px-2 font-mono text-xs hover:bg-muted/50">
      <span className="text-muted-foreground shrink-0 tabular-nums w-[85px]">
        {ts}
      </span>
      <Badge
        variant="outline"
        className={cn(
          'shrink-0 text-[10px] font-medium px-1.5 py-0 h-[18px] leading-[18px] rounded-sm',
          LEVEL_COLORS[line.level] ?? LEVEL_COLORS.INFO
        )}
      >
        {line.level}
      </Badge>
      <span className="text-muted-foreground shrink-0 w-[70px] truncate">
        {line.service}
      </span>
      <span className="whitespace-pre-wrap break-all min-w-0 flex-1">
        {line.message}
      </span>
    </div>
  )
}

export default function HistoryLogViewer({
  project,
}: {
  project: ProjectResponse
}) {
  const [selectedEnv, setSelectedEnv] = useState<string | undefined>()
  const [selectedLevels, setSelectedLevels] = useState<LogLevel[]>([])
  const [searchText, setSearchText] = useState('')
  const [debouncedText, setDebouncedText] = useState('')
  const [timeRange, setTimeRange] = useState('1h')
  const [cursor, setCursor] = useState<string | undefined>()
  const [cursorStack, setCursorStack] = useState<string[]>([])
  const parentRef = useRef<HTMLDivElement>(null)
  // Track filter key to reset pagination on filter changes
  const filterKey = `${selectedEnv}-${selectedLevels.join(',')}-${debouncedText}-${timeRange}`
  const prevFilterKeyRef = useRef(filterKey)

  // Debounce search text
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedText(searchText), 400)
    return () => clearTimeout(timer)
  }, [searchText])

  // Reset pagination when filters change
  if (filterKey !== prevFilterKeyRef.current) {
    prevFilterKeyRef.current = filterKey
    if (cursor !== undefined || cursorStack.length > 0) {
      setCursor(undefined)
      setCursorStack([])
    }
  }

  // Environments
  const { data: environments } = useQuery({
    ...getEnvironmentsOptions({
      path: { project_id: project.id },
    }),
  })

  // Auto-select first environment (store ID as string)
  useEffect(() => {
    if (environments?.length && !selectedEnv) {
      setSelectedEnv(String(environments[0].id))
    }
  }, [environments, selectedEnv])

  // Memoize time range so it only recalculates when timeRange selection changes,
  // not on every render (getTimeRange calls new Date() which would create new
  // strings each render, causing infinite query invalidation).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const { start, end } = useMemo(() => getTimeRange(timeRange), [timeRange])
  const { data, isLoading, isFetching, error } = useLogHistory(
    {
      projectId: project.id,
      startTime: start,
      endTime: end,
      levels: selectedLevels.length > 0 ? selectedLevels : undefined,
      envs: selectedEnv ? [selectedEnv] : undefined, // sends env ID as string
      text: debouncedText || undefined,
      cursor,
      pageSize: 200,
    },
    !!project.id
  )

  const lines = data?.lines ?? []

  const virtualizer = useVirtualizer({
    count: lines.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 22,
    overscan: 20,
  })

  // Scroll to bottom when new data loads (logs read oldest→newest, bottom is most recent)
  useEffect(() => {
    if (lines.length > 0) {
      virtualizer.scrollToIndex(lines.length - 1, { align: 'end' })
    }
  }, [lines.length, virtualizer])

  const toggleLevel = useCallback((level: LogLevel) => {
    setSelectedLevels((prev) =>
      prev.includes(level) ? prev.filter((l) => l !== level) : [...prev, level]
    )
  }, [])

  const handleNextPage = useCallback(() => {
    if (data?.next_cursor) {
      if (cursor) {
        setCursorStack((prev) => [...prev, cursor])
      }
      setCursor(data.next_cursor)
    }
  }, [data?.next_cursor, cursor])

  const handlePrevPage = useCallback(() => {
    setCursorStack((prev) => {
      const newStack = [...prev]
      const prevCursor = newStack.pop()
      setCursor(prevCursor)
      return newStack
    })
  }, [])

  return (
    <div className="p-4 space-y-4">
      {/* Filters */}
      <div className="flex flex-col sm:flex-row gap-3 items-start sm:items-center">
        <Select
          value={selectedEnv}
          onValueChange={setSelectedEnv}
        >
          <SelectTrigger className="w-full sm:w-[200px]">
            <SelectValue placeholder="All environments" />
          </SelectTrigger>
          <SelectContent>
            {environments?.map((env) => (
              <SelectItem key={env.id} value={String(env.id)}>
                {env.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <TimeRangeSelect value={timeRange} onChange={setTimeRange} />

        <div className="relative flex-1 min-w-[200px]">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            placeholder="Search log messages..."
            value={searchText}
            onChange={(e) => setSearchText(e.target.value)}
            className="pl-9 w-full"
          />
        </div>
      </div>

      {/* Level filter chips */}
      <div className="flex gap-1.5 flex-wrap">
        {LOG_LEVEL_OPTIONS.map((level) => (
          <button
            type="button"
            key={level}
            onClick={() => toggleLevel(level)}
            className={cn(
              'px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors',
              selectedLevels.includes(level)
                ? LEVEL_COLORS[level]
                : 'bg-muted/50 text-muted-foreground border-border hover:bg-muted'
            )}
          >
            {level}
          </button>
        ))}
        {selectedLevels.length > 0 && (
          <button
            type="button"
            onClick={() => setSelectedLevels([])}
            className="px-2.5 py-0.5 text-xs text-muted-foreground hover:text-foreground"
          >
            Clear
          </button>
        )}
      </div>

      {/* Error state */}
      {error && (
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>
            Failed to load logs: {error.message}
          </AlertDescription>
        </Alert>
      )}

      {/* Results header */}
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <div className="flex items-center gap-2">
          {isFetching && <Loader2 className="h-3 w-3 animate-spin" />}
          <span>
            {lines.length} log{lines.length !== 1 ? 's' : ''}
            {data?.total_scanned
              ? ` (${data.total_scanned.toLocaleString()} scanned)`
              : ''}
          </span>
          {data?.search_mode && (
            <Badge variant="outline" className="text-[10px] h-4">
              {data.search_mode === 'archive' ? 'Full scan' : 'Indexed'}
            </Badge>
          )}
        </div>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            disabled={cursorStack.length === 0 && !cursor}
            onClick={handlePrevPage}
          >
            <ChevronLeft className="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            disabled={!data?.next_cursor}
            onClick={handleNextPage}
          >
            <ChevronRight className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      {/* Log lines */}
      <div className="border rounded-lg bg-background">
        {isLoading && !data ? (
          <div className="h-[500px] flex items-center justify-center">
            <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
          </div>
        ) : lines.length === 0 ? (
          <div className="h-[500px] flex items-center justify-center text-muted-foreground">
            <div className="text-center">
              <AlertCircle className="h-10 w-10 mx-auto mb-3 opacity-40" />
              <p className="text-sm">No logs found for the selected filters</p>
              <p className="text-xs mt-1">
                Try adjusting the time range or clearing filters
              </p>
            </div>
          </div>
        ) : (
          <div
            ref={parentRef}
            className="h-[500px] overflow-auto select-text"
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
                  <HistoryLogLine line={lines[virtualRow.index]} />
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
