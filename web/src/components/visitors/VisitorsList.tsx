import { getVisitorsOptions } from '@/api/client/@tanstack/react-query.gen'
import { ProjectResponse, VisitorInfo } from '@/api/client/types.gen'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Label } from '@/components/ui/label'
import { useQuery } from '@tanstack/react-query'
import { format, formatDistanceToNow } from 'date-fns'
import {
  Globe,
  Bot,
  User,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
} from 'lucide-react'
import * as React from 'react'
import { useNavigate } from 'react-router-dom'
import { Skeleton } from '@/components/ui/skeleton'

interface VisitorsListProps {
  project: ProjectResponse
}

function countryCodeToFlag(countryCode: string | null | undefined): string {
  if (!countryCode || countryCode.length !== 2) return ''
  const codePoints = countryCode
    .toUpperCase()
    .split('')
    .map((char) => 127397 + char.charCodeAt(0))
  return String.fromCodePoint(...codePoints)
}

function formatLocation(visitor: VisitorInfo): string {
  const parts: string[] = []
  if (visitor.city) parts.push(visitor.city)
  if (visitor.region && visitor.region !== visitor.city) parts.push(visitor.region)
  if (visitor.country) parts.push(visitor.country)
  return parts.join(', ') || 'Unknown'
}

function getBrowserInfo(userAgent: string): { name: string; icon: string } {
  if (userAgent.includes('Edge') || userAgent.includes('Edg/')) {
    return { name: 'Edge', icon: 'edge' }
  } else if (userAgent.includes('Chrome') && !userAgent.includes('Chromium')) {
    return { name: 'Chrome', icon: 'chrome' }
  } else if (userAgent.includes('Safari') && !userAgent.includes('Chrome')) {
    return { name: 'Safari', icon: 'safari' }
  } else if (userAgent.includes('Firefox')) {
    return { name: 'Firefox', icon: 'firefox' }
  } else if (userAgent.includes('Opera') || userAgent.includes('OPR')) {
    return { name: 'Opera', icon: 'opera' }
  } else if (userAgent.includes('bot') || userAgent.includes('Bot')) {
    return { name: 'Bot', icon: 'bot' }
  }
  return { name: 'Unknown', icon: 'unknown' }
}

function getOSName(userAgent: string): string {
  if (userAgent.includes('Windows')) return 'Windows'
  if (userAgent.includes('Mac OS')) return 'macOS'
  if (userAgent.includes('Linux') && !userAgent.includes('Android')) return 'Linux'
  if (userAgent.includes('Android')) return 'Android'
  if (userAgent.includes('iPhone') || userAgent.includes('iPad')) return 'iOS'
  if (userAgent.includes('CrOS')) return 'ChromeOS'
  return 'Unknown'
}

export function VisitorsList({ project }: VisitorsListProps) {
  const navigate = useNavigate()
  const [page, setPage] = React.useState(1)
  const [limit, setLimit] = React.useState(25)
  const [crawlerFilter, setCrawlerFilter] = React.useState<
    'all' | 'humans' | 'crawlers'
  >('all')
  const [hideGhostVisitors, setHideGhostVisitors] = React.useState(true)

  // Default date range: last 30 days
  const endDate = React.useMemo(() => {
    const date = new Date()
    date.setHours(23, 59, 59, 999)
    return date
  }, [])

  const startDate = React.useMemo(() => {
    const date = new Date()
    date.setDate(date.getDate() - 30)
    date.setHours(0, 0, 0, 0)
    return date
  }, [])

  const { data, isLoading, error, refetch } = useQuery({
    ...getVisitorsOptions({
      query: {
        project_id: project.id,
        start_date: startDate.toISOString(),
        end_date: endDate.toISOString(),
        offset: (page - 1) * limit,
        limit,
        include_crawlers:
          crawlerFilter === 'all'
            ? undefined
            : crawlerFilter === 'crawlers'
              ? true
              : false,
        has_activity_only: hideGhostVisitors ? undefined : false,
      },
    }),
  })

  const totalPages = React.useMemo(() => {
    if (!data) return 0
    return Math.ceil(data.filtered_count / limit)
  }, [data, limit])

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <CardTitle>Visitors</CardTitle>
              <CardDescription>
                {data
                  ? `${data.filtered_count.toLocaleString()} visitors found`
                  : 'Browse and analyze visitor sessions'}
              </CardDescription>
            </div>
            <div className="flex flex-wrap items-center gap-2 sm:gap-4">
              <div className="flex items-center gap-2">
                <Switch
                  id="hide-ghost"
                  checked={hideGhostVisitors}
                  onCheckedChange={setHideGhostVisitors}
                />
                <Label
                  htmlFor="hide-ghost"
                  className="text-sm cursor-pointer whitespace-nowrap"
                >
                  Hide ghost visitors
                </Label>
              </div>
              <Select
                value={crawlerFilter}
                onValueChange={(value: 'all' | 'humans' | 'crawlers') =>
                  setCrawlerFilter(value)
                }
              >
                <SelectTrigger className="w-[120px] sm:w-[140px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All Visitors</SelectItem>
                  <SelectItem value="humans">Humans Only</SelectItem>
                  <SelectItem value="crawlers">Crawlers Only</SelectItem>
                </SelectContent>
              </Select>
              <Select
                value={limit.toString()}
                onValueChange={(value) => setLimit(parseInt(value))}
              >
                <SelectTrigger className="w-[90px] sm:w-[100px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="10">10 / page</SelectItem>
                  <SelectItem value="25">25 / page</SelectItem>
                  <SelectItem value="50">50 / page</SelectItem>
                  <SelectItem value="100">100 / page</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <div className="space-y-2">
              {[...Array(8)].map((_, i) => (
                <Skeleton key={i} className="h-12 w-full rounded" />
              ))}
            </div>
          ) : error ? (
            <div className="flex flex-col items-center justify-center py-12">
              <p className="text-muted-foreground mb-2">
                Failed to load visitors
              </p>
              <Button variant="outline" onClick={() => refetch()}>
                Try again
              </Button>
            </div>
          ) : !data || data.visitors.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-12">
              <User className="h-12 w-12 text-muted-foreground mb-4" />
              <p className="text-muted-foreground">No visitors found</p>
            </div>
          ) : (
            <>
              <TooltipProvider>
                <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className="w-[200px] sm:w-[280px]">Visitor</TableHead>
                      <TableHead>Location</TableHead>
                      <TableHead className="hidden md:table-cell">Source</TableHead>
                      <TableHead className="hidden lg:table-cell">Browser / OS</TableHead>
                      <TableHead>First Seen</TableHead>
                      <TableHead>Last Seen</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {data.visitors.map((visitor: VisitorInfo) => (
                      <VisitorRow
                        key={visitor.visitor_id}
                        visitor={visitor}
                        onClick={() =>
                          navigate(
                            `/projects/${project.slug}/analytics/visitors/${visitor.id}`
                          )
                        }
                      />
                    ))}
                  </TableBody>
                </Table>
                </div>
              </TooltipProvider>

              {/* Pagination */}
              {totalPages > 1 && (
                <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between mt-6">
                  <div className="text-sm text-muted-foreground">
                    <span className="hidden sm:inline">
                      Showing {(page - 1) * limit + 1} to{' '}
                      {Math.min(page * limit, data.filtered_count)} of{' '}
                      {data.filtered_count} visitors
                    </span>
                    <span className="sm:hidden">
                      {page} / {totalPages}
                    </span>
                  </div>
                  <div className="flex items-center gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setPage((p) => Math.max(1, p - 1))}
                      disabled={page === 1}
                    >
                      <ChevronLeft className="h-4 w-4" />
                      <span className="hidden sm:inline">Previous</span>
                    </Button>
                    <div className="hidden sm:flex items-center gap-1">
                      {[...Array(Math.min(5, totalPages))].map((_, idx) => {
                        const pageNum = page - 2 + idx
                        if (pageNum < 1 || pageNum > totalPages) return null
                        return (
                          <Button
                            key={pageNum}
                            variant={pageNum === page ? 'default' : 'outline'}
                            size="sm"
                            onClick={() => setPage(pageNum)}
                            className="w-10"
                          >
                            {pageNum}
                          </Button>
                        )
                      })}
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        setPage((p) => Math.min(totalPages, p + 1))
                      }
                      disabled={page === totalPages}
                    >
                      <span className="hidden sm:inline">Next</span>
                      <ChevronRight className="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              )}
            </>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

interface VisitorRowProps {
  visitor: VisitorInfo
  onClick: () => void
}

function VisitorRow({ visitor, onClick }: VisitorRowProps) {
  const browserInfo = visitor.user_agent
    ? getBrowserInfo(visitor.user_agent)
    : null
  const osName = visitor.user_agent ? getOSName(visitor.user_agent) : null
  const location = formatLocation(visitor)
  const flag = countryCodeToFlag(visitor.country_code)
  const lastSeenDate = new Date(visitor.last_seen)
  const firstSeenDate = new Date(visitor.first_seen)

  return (
    <TableRow
      className="cursor-pointer"
      onClick={onClick}
    >
      {/* Visitor identity */}
      <TableCell>
        <div className="flex items-center gap-3">
          <div
            className={`flex h-8 w-8 items-center justify-center rounded-full flex-shrink-0 ${
              visitor.is_crawler
                ? 'bg-amber-100 text-amber-600 dark:bg-amber-900/30 dark:text-amber-400'
                : 'bg-blue-100 text-blue-600 dark:bg-blue-900/30 dark:text-blue-400'
            }`}
          >
            {visitor.is_crawler ? (
              <Bot className="h-4 w-4" />
            ) : (
              <User className="h-4 w-4" />
            )}
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span className="font-mono text-sm truncate">
                {visitor.visitor_id?.substring(0, 12)}
              </span>
              <Badge
                variant={visitor.is_crawler ? 'warning' : 'secondary'}
                className="text-[10px] px-1.5 py-0"
              >
                {visitor.is_crawler
                  ? visitor.crawler_name || 'Bot'
                  : 'Human'}
              </Badge>
            </div>
          </div>
        </div>
      </TableCell>

      {/* Location */}
      <TableCell>
        <div className="flex items-center gap-2">
          {flag ? (
            <span className="text-base leading-none">{flag}</span>
          ) : (
            <Globe className="h-4 w-4 text-muted-foreground flex-shrink-0" />
          )}
          <span className="text-sm truncate max-w-[200px]">{location}</span>
          {visitor.is_eu && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0">
              EU
            </Badge>
          )}
        </div>
      </TableCell>

      {/* Source / Referrer */}
      <TableCell className="hidden md:table-cell">
        <VisitorSource visitor={visitor} />
      </TableCell>

      {/* Browser / OS */}
      <TableCell className="hidden lg:table-cell">
        <div className="flex items-center gap-1.5">
          <span className="text-sm">
            {browserInfo?.name || 'Unknown'}
          </span>
          {osName && osName !== 'Unknown' && (
            <span className="text-xs text-muted-foreground">/ {osName}</span>
          )}
        </div>
      </TableCell>

      {/* First Seen */}
      <TableCell>
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="text-sm text-muted-foreground cursor-default">
              {formatDistanceToNow(firstSeenDate, { addSuffix: true })}
            </span>
          </TooltipTrigger>
          <TooltipContent>
            {format(firstSeenDate, 'MMM d, yyyy HH:mm:ss')}
          </TooltipContent>
        </Tooltip>
      </TableCell>

      {/* Last Seen */}
      <TableCell>
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="text-sm text-muted-foreground cursor-default">
              {formatDistanceToNow(lastSeenDate, { addSuffix: true })}
            </span>
          </TooltipTrigger>
          <TooltipContent>
            {format(lastSeenDate, 'MMM d, yyyy HH:mm:ss')}
          </TooltipContent>
        </Tooltip>
      </TableCell>
    </TableRow>
  )
}

function VisitorSource({ visitor }: { visitor: VisitorInfo }) {
  const channel = visitor.first_channel
  const hostname = visitor.first_referrer_hostname

  if (!channel && !hostname) {
    return <span className="text-sm text-muted-foreground">Direct</span>
  }

  return (
    <div className="flex flex-col gap-0.5">
      {channel && (
        <Badge variant="outline" className="text-[10px] px-1.5 py-0 w-fit">
          {channel}
        </Badge>
      )}
      {hostname && (
        <div className="flex items-center gap-1">
          <ExternalLink className="h-3 w-3 text-muted-foreground flex-shrink-0" />
          <span className="text-xs text-muted-foreground truncate max-w-[150px]">
            {hostname}
          </span>
        </div>
      )}
    </div>
  )
}
