import {
  getAiAgentBreakdownOptions,
  getAiPageBreakdownOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { ProjectResponse } from '@/api/client/types.gen'
import { AiAgentLogo } from '@/components/ui/ai-agent-logo'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useQuery } from '@tanstack/react-query'
import { format } from 'date-fns'
import { ArrowLeft, Bot, FileText, Search } from 'lucide-react'
import * as React from 'react'
import { useNavigate } from 'react-router-dom'

interface AiAgentsDetailProps {
  project: ProjectResponse
  startDate: Date | undefined
  endDate: Date | undefined
  environment: number | undefined
  /** Carries the active date filter back to the overview. */
  onBack: () => void
}

/**
 * Full "view all" page for AI agent traffic. Two sections:
 *  1. Every AI agent/provider that hit the site, ranked, searchable, with a
 *     by-provider / by-agent toggle.
 *  2. The pages those agents crawled most, with distinct-agent counts.
 * Both are read from the proxy-log AI breakdown endpoints and link into the
 * request log filtered to the matching AI traffic.
 */
export function AiAgentsDetail({
  project,
  startDate,
  endDate,
  environment,
  onBack,
}: AiAgentsDetailProps) {
  const navigate = useNavigate()
  const [groupBy, setGroupBy] = React.useState<'provider' | 'agent'>('agent')
  const [agentSearch, setAgentSearch] = React.useState('')
  const [pageSearch, setPageSearch] = React.useState('')

  const agentsQuery = useQuery({
    ...getAiAgentBreakdownOptions({
      query: {
        project_id: project.id,
        environment_id: environment,
        start_time: startDate ? startDate.toISOString() : undefined,
        end_time: endDate ? endDate.toISOString() : undefined,
        limit: 100,
      },
    }),
    enabled: !!startDate && !!endDate,
  })

  const pagesQuery = useQuery({
    ...getAiPageBreakdownOptions({
      query: {
        project_id: project.id,
        environment_id: environment,
        start_time: startDate ? startDate.toISOString() : undefined,
        end_time: endDate ? endDate.toISOString() : undefined,
        limit: 100,
      },
    }),
    enabled: !!startDate && !!endDate,
  })

  const agentRows = React.useMemo(() => {
    const items = agentsQuery.data?.items ?? []
    if (items.length === 0) return []
    const total = items.reduce((sum, r) => sum + r.request_count, 0)

    if (groupBy === 'agent') {
      return items
        .map((row) => ({
          provider: row.provider,
          label: row.agent,
          agent: row.agent,
          purpose: row.purpose,
          count: row.request_count,
          uniqueIps: row.unique_ips,
          percentage: total > 0 ? (row.request_count / total) * 100 : 0,
        }))
        .sort((a, b) => b.count - a.count)
    }

    const byProvider = new Map<
      string,
      { count: number; uniqueIps: number; sample: string }
    >()
    for (const row of items) {
      const prev = byProvider.get(row.provider)
      if (prev) {
        prev.count += row.request_count
        prev.uniqueIps += row.unique_ips
      } else {
        byProvider.set(row.provider, {
          count: row.request_count,
          uniqueIps: row.unique_ips,
          sample: row.agent,
        })
      }
    }
    return Array.from(byProvider.entries())
      .map(([provider, v]) => ({
        provider,
        label: provider,
        agent: v.sample,
        purpose: '',
        count: v.count,
        uniqueIps: v.uniqueIps,
        percentage: total > 0 ? (v.count / total) * 100 : 0,
      }))
      .sort((a, b) => b.count - a.count)
  }, [agentsQuery.data, groupBy])

  const filteredAgents = React.useMemo(() => {
    const q = agentSearch.trim().toLowerCase()
    if (!q) return agentRows
    return agentRows.filter(
      (r) =>
        r.label.toLowerCase().includes(q) ||
        r.provider.toLowerCase().includes(q)
    )
  }, [agentRows, agentSearch])

  const pageRows = React.useMemo(() => {
    const items = pagesQuery.data?.items ?? []
    if (items.length === 0) return []
    const max = Math.max(...items.map((p) => p.request_count), 1)
    return items.map((p) => ({
      path: p.path,
      requestCount: p.request_count,
      agentCount: p.agent_count,
      lastSeen: p.last_seen,
      percentage: (p.request_count / max) * 100,
    }))
  }, [pagesQuery.data])

  const filteredPages = React.useMemo(() => {
    const q = pageSearch.trim().toLowerCase()
    if (!q) return pageRows
    return pageRows.filter((r) => r.path.toLowerCase().includes(q))
  }, [pageRows, pageSearch])

  const totalAiRequests = React.useMemo(
    () => agentRows.reduce((sum, r) => sum + r.count, 0),
    [agentRows]
  )

  const drillToLogs = (params: URLSearchParams) => {
    params.set('is_ai_agent', 'true')
    if (startDate) params.set('start_date', startDate.toISOString())
    if (endDate) params.set('end_date', endDate.toISOString())
    params.set('filters', 'open')
    navigate(`/projects/${project.slug}/logs?${params.toString()}`)
  }

  const onAgentClick = (provider: string, agent: string) => {
    const params = new URLSearchParams()
    if (groupBy === 'agent') params.set('ai_agent', agent)
    else params.set('ai_provider', provider)
    drillToLogs(params)
  }

  const onPageClick = (path: string) => {
    const params = new URLSearchParams()
    params.set('path', path)
    drillToLogs(params)
  }

  const dateLabel =
    startDate && endDate
      ? `${format(startDate, 'LLL dd, y')} - ${format(endDate, 'LLL dd, y')}`
      : 'Select a date range'

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center gap-2">
        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7"
          onClick={onBack}
          aria-label="Back to analytics"
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div>
          <h2 className="text-xl font-semibold tracking-tight flex items-center gap-2">
            <Bot className="h-5 w-5" />
            AI Agents
          </h2>
          <p className="text-sm text-muted-foreground">
            {totalAiRequests.toLocaleString()} AI crawler requests · {dateLabel}
          </p>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        {/* Agents list */}
        <Card>
          <CardHeader>
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <CardTitle>Agents</CardTitle>
                <CardDescription>
                  Every AI crawler that hit your site
                </CardDescription>
              </div>
              <div className="flex items-center gap-1 rounded-md border p-0.5">
                <Button
                  size="sm"
                  variant={groupBy === 'provider' ? 'default' : 'ghost'}
                  className="h-6 px-2 text-xs"
                  onClick={() => setGroupBy('provider')}
                >
                  By provider
                </Button>
                <Button
                  size="sm"
                  variant={groupBy === 'agent' ? 'default' : 'ghost'}
                  className="h-6 px-2 text-xs"
                  onClick={() => setGroupBy('agent')}
                >
                  By agent
                </Button>
              </div>
            </div>
            <div className="relative mt-2">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={agentSearch}
                onChange={(e) => setAgentSearch(e.target.value)}
                placeholder="Filter agents..."
                className="pl-8"
              />
            </div>
          </CardHeader>
          <CardContent>
            {agentsQuery.isLoading ? (
              <ListSkeleton />
            ) : agentsQuery.error ? (
              <ErrorState label="AI agents" />
            ) : !filteredAgents.length ? (
              <EmptyState
                primary={
                  agentRows.length === 0
                    ? 'No AI crawlers hit your site in this period'
                    : `No agents match "${agentSearch}"`
                }
              />
            ) : (
              <div className="space-y-3">
                {filteredAgents.map((row) => (
                  <button
                    type="button"
                    key={`${row.provider}-${row.label}`}
                    className="space-y-2 w-full text-left cursor-pointer hover:bg-muted/50 rounded-lg p-1 -mx-1"
                    onClick={() => onAgentClick(row.provider, row.agent)}
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-3 min-w-0">
                        <AiAgentLogo
                          provider={row.provider}
                          agent={row.agent}
                          size={20}
                        />
                        <div className="flex items-center gap-2 min-w-0">
                          <span className="text-sm font-medium truncate">
                            {row.label}
                          </span>
                          {groupBy === 'agent' && (
                            <Badge
                              variant="outline"
                              className="text-xs px-1 py-0 h-4 shrink-0"
                            >
                              {row.provider}
                            </Badge>
                          )}
                        </div>
                      </div>
                      <div className="flex items-center gap-2 shrink-0">
                        <span className="text-xs text-muted-foreground">
                          {row.uniqueIps.toLocaleString()} IPs
                        </span>
                        <span className="text-sm font-mono text-muted-foreground">
                          {row.count.toLocaleString()}
                        </span>
                      </div>
                    </div>
                    <div className="relative h-2 bg-muted rounded-full overflow-hidden">
                      <div
                        className="absolute inset-y-0 left-0 bg-primary rounded-full transition-all duration-500"
                        style={{ width: `${row.percentage}%` }}
                      />
                    </div>
                  </button>
                ))}
              </div>
            )}
          </CardContent>
        </Card>

        {/* Pages crawled */}
        <Card>
          <CardHeader>
            <div>
              <CardTitle className="flex items-center gap-2">
                <FileText className="h-4 w-4" />
                Pages crawled by AI
              </CardTitle>
              <CardDescription>
                Which content AI agents request most, and how many distinct
                agents touched each
              </CardDescription>
            </div>
            <div className="relative mt-2">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={pageSearch}
                onChange={(e) => setPageSearch(e.target.value)}
                placeholder="Filter pages..."
                className="pl-8"
              />
            </div>
          </CardHeader>
          <CardContent className="p-0">
            {pagesQuery.isLoading ? (
              <div className="p-6">
                <ListSkeleton />
              </div>
            ) : pagesQuery.error ? (
              <div className="p-6">
                <ErrorState label="crawled pages" />
              </div>
            ) : !filteredPages.length ? (
              <div className="p-6">
                <EmptyState
                  primary={
                    pageRows.length === 0
                      ? 'No AI crawler page hits in this period'
                      : `No pages match "${pageSearch}"`
                  }
                />
              </div>
            ) : (
              <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Path</TableHead>
                      <TableHead className="text-right w-[80px]">
                        Agents
                      </TableHead>
                      <TableHead className="text-right w-[110px]">
                        Requests
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {filteredPages.map((row) => (
                      <TableRow
                        key={row.path}
                        className="cursor-pointer hover:bg-muted/50"
                        onClick={() => onPageClick(row.path)}
                      >
                        <TableCell className="font-mono text-xs max-w-[280px]">
                          <div className="truncate" title={row.path}>
                            {row.path}
                          </div>
                          <div className="relative mt-1 h-1.5 bg-muted rounded-full overflow-hidden">
                            <div
                              className="absolute inset-y-0 left-0 bg-primary/70 rounded-full"
                              style={{ width: `${row.percentage}%` }}
                            />
                          </div>
                        </TableCell>
                        <TableCell className="text-right">
                          <Badge variant="secondary" className="font-mono">
                            {row.agentCount}
                          </Badge>
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-muted-foreground">
                          {row.requestCount.toLocaleString()}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

function ListSkeleton() {
  return (
    <div className="space-y-3 py-2">
      {[...Array(8)].map((_, i) => (
        <div
          key={`skel-${i}`}
          className="flex items-center justify-between"
        >
          <div className="h-4 w-[180px] bg-muted animate-pulse rounded" />
          <div className="h-4 w-[80px] bg-muted animate-pulse rounded" />
        </div>
      ))}
    </div>
  )
}

function ErrorState({ label }: { label: string }) {
  return (
    <div className="flex flex-col items-center justify-center py-8 text-center">
      <p className="text-sm text-muted-foreground mb-2">
        Failed to load {label}
      </p>
      <Button
        variant="outline"
        size="sm"
        onClick={() => window.location.reload()}
      >
        Try again
      </Button>
    </div>
  )
}

function EmptyState({ primary }: { primary: string }) {
  return (
    <div className="flex flex-col items-center justify-center py-10 text-center">
      <p className="text-sm text-muted-foreground">{primary}</p>
    </div>
  )
}
