'use client'

import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import { AlertCircle, Search, SlidersHorizontal, X } from 'lucide-react'
import { useState } from 'react'
import { useLogStream } from '@/hooks/useLogStream'
import { LogLine } from '@/components/runtime-logs/log-line'

interface ContainerLogsViewerProps {
  fetchUrl: string
  containerId: string
}

export function ContainerLogsViewer({ fetchUrl }: ContainerLogsViewerProps) {
  // Convert HTTP URL to WebSocket URL
  const wsUrl = (() => {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const path = fetchUrl.replace(/^https?:\/\/[^/]+/, '')
    return `${protocol}//${window.location.host}${path}`
  })()

  const {
    logs,
    filteredLogs,
    connectionStatus,
    errorMessage,
    searchTerm,
    currentMatchIndex,
    autoScroll,
    showTimestamps,
    parentRef,
    virtualizer,
    setSearchTerm,
    setAutoScroll,
    setShowTimestamps,
    handleScroll,
    handleNextMatch,
    handlePrevMatch,
  } = useLogStream({ wsUrl })

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSearchTerm(e.target.value)
  }

  const [searchOpen, setSearchOpen] = useState(false)

  return (
    <div className="flex flex-col h-full bg-background">
      {/* Toolbar */}
      <div className="flex-shrink-0 flex items-center gap-2 px-4 sm:px-6 lg:px-8 py-2 border-b">
        <div
          className={cn(
            'flex items-center gap-1.5 text-xs',
            connectionStatus === 'connected'
              ? 'text-emerald-600 dark:text-emerald-400'
              : connectionStatus === 'connecting'
                ? 'text-yellow-600 dark:text-yellow-400'
                : 'text-red-600 dark:text-red-400'
          )}
        >
          <span
            className={cn(
              'h-1.5 w-1.5 rounded-full',
              connectionStatus === 'connected'
                ? 'bg-emerald-500'
                : connectionStatus === 'connecting'
                  ? 'bg-yellow-500'
                  : 'bg-red-500'
            )}
          />
          <span>
            {connectionStatus === 'connected' && 'Connected'}
            {connectionStatus === 'connecting' && 'Connecting...'}
            {connectionStatus === 'error' && 'Disconnected'}
          </span>
        </div>

        <div className="ml-auto flex items-center gap-1.5">
          {searchOpen ? (
            <div className="flex items-center gap-1.5">
              <div className="relative">
                <Search className="absolute left-2 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground" />
                <Input
                  autoFocus
                  placeholder="Search logs..."
                  value={searchTerm}
                  onChange={handleSearch}
                  className="h-8 pl-7 pr-2 text-sm w-56"
                />
              </div>
              {searchTerm && (
                <span className="text-xs text-muted-foreground tabular-nums">
                  {currentMatchIndex >= 0
                    ? `${currentMatchIndex + 1}/${filteredLogs.length}`
                    : `${filteredLogs.length}`}
                </span>
              )}
              <Button
                variant="outline"
                size="sm"
                className="h-8 w-8 p-0"
                onClick={handlePrevMatch}
                disabled={!searchTerm || filteredLogs.length === 0}
              >
                ↑
              </Button>
              <Button
                variant="outline"
                size="sm"
                className="h-8 w-8 p-0"
                onClick={handleNextMatch}
                disabled={!searchTerm || filteredLogs.length === 0}
              >
                ↓
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="h-8 w-8 p-0"
                onClick={() => {
                  setSearchTerm('')
                  setSearchOpen(false)
                }}
                aria-label="Close search"
              >
                <X className="h-3.5 w-3.5" />
              </Button>
            </div>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              className="h-8 px-2"
              onClick={() => setSearchOpen(true)}
            >
              <Search className="h-3.5 w-3.5 sm:mr-1.5" />
              <span className="hidden sm:inline text-xs">Search</span>
            </Button>
          )}

          <Popover>
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                size="sm"
                className="h-8 px-2"
                aria-label="Log options"
              >
                <SlidersHorizontal className="h-3.5 w-3.5 sm:mr-1.5" />
                <span className="hidden sm:inline text-xs">Options</span>
              </Button>
            </PopoverTrigger>
            <PopoverContent align="end" className="w-52 p-3 space-y-2.5">
              <div className="flex items-center gap-2">
                <Checkbox
                  id="auto-scroll"
                  checked={autoScroll}
                  onCheckedChange={(checked) =>
                    setAutoScroll(checked as boolean)
                  }
                />
                <Label
                  htmlFor="auto-scroll"
                  className="text-sm cursor-pointer"
                >
                  Auto-scroll
                </Label>
              </div>
              <div className="flex items-center gap-2">
                <Checkbox
                  id="timestamps"
                  checked={showTimestamps}
                  onCheckedChange={(checked) =>
                    setShowTimestamps(checked as boolean)
                  }
                />
                <Label
                  htmlFor="timestamps"
                  className="text-sm cursor-pointer"
                >
                  Show timestamps
                </Label>
              </div>
            </PopoverContent>
          </Popover>
        </div>
      </div>

      {/* Error Alert */}
      {errorMessage && (
        <Alert className="flex-shrink-0 mx-4 mt-2 border-destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>{errorMessage}</AlertDescription>
        </Alert>
      )}

      {/* Logs Container */}
      <div
        ref={parentRef}
        className={cn(
          'flex-1 overflow-auto bg-background text-foreground p-4 font-mono text-xs select-text min-h-0',
          connectionStatus === 'connecting' && 'opacity-50'
        )}
        onScroll={handleScroll}
      >
        {logs.length === 0 && connectionStatus === 'connecting' && (
          <div className="text-muted-foreground">Connecting to logs...</div>
        )}

        {logs.length === 0 && connectionStatus !== 'connecting' && (
          <div className="text-muted-foreground">No logs available</div>
        )}

        <div
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            width: '100%',
            position: 'relative',
          }}
        >
          {virtualizer.getVirtualItems().map((virtualItem) => {
            const log = filteredLogs[virtualItem.index]

            return (
              <div
                key={virtualItem.key}
                data-index={virtualItem.index}
                ref={virtualizer.measureElement}
                style={{
                  position: 'absolute',
                  top: `${virtualItem.start}px`,
                  left: 0,
                  width: '100%',
                }}
              >
                <LogLine content={log} searchTerm={searchTerm} />
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}
