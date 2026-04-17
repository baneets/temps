import {
  getGeneralStatsOptions,
  getProjectsOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { ExternalConnectivityAlert } from '@/components/alerts/ExternalConnectivityAlert'
import { MetricCard } from '@/components/dashboard/MetricCard'
import { ProjectCard } from '@/components/dashboard/ProjectCard'
import { EmptyPlaceholder } from '@/components/EmptyPlaceholder'
import { ImprovedOnboardingDashboard } from '@/components/onboarding/ImprovedOnboardingDashboard'
import { MetricCardSkeleton } from '@/components/skeletons/MetricCardSkeleton'
import { ProjectCardSkeleton } from '@/components/skeletons/ProjectCardSkeleton'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { useDashboardAnalytics } from '@/hooks/useDashboardAnalytics'
import { useDashboardHealth } from '@/hooks/useDashboardHealth'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useQuery } from '@tanstack/react-query'
import { subDays, subHours } from 'date-fns'
import {
  ChevronLeft,
  ChevronRight,
  DollarSign,
  Eye,
  FolderGit2,
  Minus,
  Plus,
  TrendingDown,
  TrendingUp,
  Users,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router-dom'

type RangeKey = '24h' | '7d' | '30d'

const RANGE_OPTIONS: { value: RangeKey; label: string }[] = [
  { value: '24h', label: 'Last 24 hours' },
  { value: '7d', label: 'Last 7 days' },
  { value: '30d', label: 'Last 30 days' },
]

function rangeToDates(range: RangeKey) {
  const end = new Date()
  let start: Date
  switch (range) {
    case '7d':
      start = subDays(end, 7)
      break
    case '30d':
      start = subDays(end, 30)
      break
    case '24h':
    default:
      start = subHours(end, 24)
  }
  return { startDate: start.toISOString(), endDate: end.toISOString() }
}

function formatTrendChange(trendPercentage: number | null | undefined): {
  change: string
  changeDisplay: {
    icon: React.ReactNode
    className: string
    isPositive?: boolean
  }
} | null {
  if (trendPercentage == null) return null

  const rounded = Math.round(trendPercentage)

  if (rounded === 0) {
    return {
      change: '0% vs prev. period',
      changeDisplay: {
        icon: <Minus className="mr-1 h-3 w-3" />,
        className: 'text-muted-foreground',
      },
    }
  }

  if (rounded > 0) {
    return {
      change: `+${rounded}% vs prev. period`,
      changeDisplay: {
        icon: <TrendingUp className="mr-1 h-3 w-3" />,
        className: 'text-emerald-600 dark:text-emerald-400',
        isPositive: true,
      },
    }
  }

  return {
    change: `${rounded}% vs prev. period`,
    changeDisplay: {
      icon: <TrendingDown className="mr-1 h-3 w-3" />,
      className: 'text-red-600 dark:text-red-400',
      isPositive: false,
    },
  }
}

const ITEMS_PER_PAGE = 8

export function Dashboard() {
  const { setBreadcrumbs } = useBreadcrumbs()
  const [page, setPage] = useState(1)
  const [range, setRange] = useState<RangeKey>('24h')

  const { data: projectsData, isLoading } = useQuery({
    ...getProjectsOptions({
      query: {
        page,
        per_page: ITEMS_PER_PAGE,
      },
    }),
  })

  useEffect(() => {
    setBreadcrumbs([{ label: 'Dashboard' }])
  }, [setBreadcrumbs])

  usePageTitle('Dashboard')

  const hasProjects = (projectsData?.projects?.length || 0) > 0

  const { startDate, endDate } = useMemo(() => rangeToDates(range), [range])

  const projectIds = useMemo(
    () => projectsData?.projects?.map((p: { id: number }) => p.id) ?? [],
    [projectsData?.projects]
  )

  const dashboardAnalytics = useDashboardAnalytics(
    projectIds,
    startDate,
    endDate
  )

  const dashboardHealth = useDashboardHealth(projectIds)

  const generalStatsQuery = useQuery({
    ...getGeneralStatsOptions({
      query: {
        start_date: startDate,
        end_date: endDate,
      },
    }),
    enabled: hasProjects,
  })

  const statsData = generalStatsQuery.data as
    | (typeof generalStatsQuery.data & {
        visitors_trend_percentage?: number | null
        page_views_trend_percentage?: number | null
      })
    | undefined
  const visitorsTrend = formatTrendChange(statsData?.visitors_trend_percentage)
  const pageViewsTrend = formatTrendChange(
    statsData?.page_views_trend_percentage
  )

  const totalPages = projectsData
    ? Math.max(1, Math.ceil(projectsData.total / ITEMS_PER_PAGE))
    : 1
  const rangeFromIndex =
    projectsData && projectsData.total > 0
      ? (page - 1) * ITEMS_PER_PAGE + 1
      : 0
  const rangeToIndex = projectsData
    ? Math.min(page * ITEMS_PER_PAGE, projectsData.total)
    : 0

  if (isLoading) {
    return (
      <div className="mx-auto w-full max-w-7xl p-4 sm:p-6 lg:p-8">
        <div className="mb-8 flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
          <div className="space-y-2">
            <div className="h-7 w-40 animate-pulse rounded bg-muted" />
            <div className="h-4 w-64 animate-pulse rounded bg-muted" />
          </div>
          <div className="h-9 w-40 animate-pulse rounded bg-muted" />
        </div>
        <div className="mb-8 grid gap-4 grid-cols-2 lg:grid-cols-4">
          <MetricCardSkeleton />
          <MetricCardSkeleton />
          <MetricCardSkeleton />
          <MetricCardSkeleton />
        </div>
        <div className="space-y-6">
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {Array.from({ length: ITEMS_PER_PAGE }).map((_, i) => (
              <ProjectCardSkeleton key={i} />
            ))}
          </div>
        </div>
      </div>
    )
  }

  const shouldShowOnboarding = !hasProjects

  if (shouldShowOnboarding) {
    return (
      <div className="mx-auto w-full max-w-7xl p-4 sm:p-6 lg:p-8">
        <ImprovedOnboardingDashboard />
      </div>
    )
  }

  return (
    <div className="mx-auto w-full max-w-7xl p-4 sm:p-6 lg:p-8">
      <ExternalConnectivityAlert showInDashboard dismissible />

      {/* Page header */}
      <div className="mb-6 flex flex-col gap-4 sm:mb-8 sm:flex-row sm:items-end sm:justify-between">
        <div className="space-y-1">
          <h1 className="text-2xl font-semibold tracking-tight">Dashboard</h1>
          <p className="text-sm text-muted-foreground">
            Overview of your projects, traffic, and health.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Select
            value={range}
            onValueChange={(v) => setRange(v as RangeKey)}
          >
            <SelectTrigger className="w-full sm:w-[170px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {RANGE_OPTIONS.map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button asChild size="sm">
            <Link to="/projects/new">
              <Plus className="mr-1.5 h-4 w-4" />
              <span className="hidden sm:inline">New project</span>
              <span className="sm:hidden">New</span>
            </Link>
          </Button>
        </div>
      </div>

      {/* Metric cards */}
      <div className="mb-8 grid gap-4 grid-cols-2 lg:grid-cols-4">
        {generalStatsQuery.isLoading ? (
          <MetricCardSkeleton />
        ) : (
          <MetricCard
            title="Projects"
            value={
              generalStatsQuery.error
                ? projectsData?.total || 0
                : generalStatsQuery.data?.total_projects || 0
            }
            change=""
            icon={<FolderGit2 />}
          />
        )}

        {generalStatsQuery.isLoading ? (
          <MetricCardSkeleton />
        ) : (
          <MetricCard
            title="Visitors"
            value={
              generalStatsQuery.error
                ? 'N/A'
                : generalStatsQuery.data?.total_unique_visitors?.toLocaleString() ||
                  0
            }
            change={visitorsTrend?.change ?? ''}
            changeDisplay={visitorsTrend?.changeDisplay}
            icon={<Users />}
            error={!!generalStatsQuery.error}
          />
        )}

        {generalStatsQuery.isLoading ? (
          <MetricCardSkeleton />
        ) : (
          <MetricCard
            title="Page views"
            value={
              generalStatsQuery.error
                ? 'N/A'
                : generalStatsQuery.data?.total_page_views?.toLocaleString() ||
                  0
            }
            change={pageViewsTrend?.change ?? ''}
            changeDisplay={pageViewsTrend?.changeDisplay}
            icon={<Eye />}
            error={!!generalStatsQuery.error}
          />
        )}

        <MetricCard
          title="Revenue"
          value="—"
          change=""
          icon={<DollarSign />}
          locked
        />
      </div>

      {/* Projects section */}
      <div className="space-y-4">
        <div className="flex items-end justify-between gap-2">
          <div className="flex items-baseline gap-2">
            <h2 className="text-base font-semibold tracking-tight">
              Projects
            </h2>
            {projectsData && projectsData.total > 0 && (
              <span className="text-xs text-muted-foreground tabular-nums">
                {projectsData.total}
              </span>
            )}
          </div>
        </div>

        {projectsData?.projects.length === 0 ? (
          <EmptyPlaceholder
            title="No projects found"
            description="You haven't created any projects yet. Start by creating your first project."
            icon={FolderGit2}
          >
            <Button asChild>
              <Link to="/projects/new">
                <Plus className="mr-2 h-4 w-4" />
                Create Project
              </Link>
            </Button>
          </EmptyPlaceholder>
        ) : (
          <>
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
              {projectsData?.projects.map((project: any) => (
                <ProjectCard
                  key={project.id}
                  project={project}
                  analytics={
                    dashboardAnalytics.data?.projects?.[String(project.id)]
                  }
                  analyticsLoading={dashboardAnalytics.isLoading}
                  analyticsError={dashboardAnalytics.isError}
                  health={
                    dashboardHealth.data?.projects?.[String(project.id)]
                  }
                />
              ))}
            </div>

            {/* Pagination */}
            {projectsData && projectsData.total > ITEMS_PER_PAGE && (
              <div className="flex flex-col gap-2 border-t pt-4 sm:flex-row sm:items-center sm:justify-between">
                <p className="text-xs text-muted-foreground tabular-nums">
                  <span className="hidden sm:inline">
                    Showing {rangeFromIndex}–{rangeToIndex} of{' '}
                    {projectsData.total}
                  </span>
                  <span className="sm:hidden">
                    Page {page} / {totalPages}
                  </span>
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setPage((p) => Math.max(1, p - 1))}
                    disabled={page === 1}
                  >
                    <ChevronLeft className="mr-1 h-4 w-4" />
                    Previous
                  </Button>
                  <span className="hidden text-xs text-muted-foreground tabular-nums sm:inline">
                    Page {page} / {totalPages}
                  </span>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setPage((p) => p + 1)}
                    disabled={page >= totalPages}
                  >
                    Next
                    <ChevronRight className="ml-1 h-4 w-4" />
                  </Button>
                </div>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}
