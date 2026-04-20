import { useEffect, useMemo, useState } from 'react'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { useDashboardAnalytics } from '@/hooks/useDashboardAnalytics'
import { useDashboardHealth } from '@/hooks/useDashboardHealth'
import { usePageTitle } from '@/hooks/usePageTitle'
import { ExternalConnectivityAlert } from '@/components/alerts/ExternalConnectivityAlert'
import { MetricCard } from '@/components/dashboard/MetricCard'
import { ProjectCard } from '@/components/dashboard/ProjectCard'
import { ImprovedOnboardingDashboard } from '@/components/onboarding/ImprovedOnboardingDashboard'
import { MetricCardSkeleton } from '@/components/skeletons/MetricCardSkeleton'
import { ProjectCardSkeleton } from '@/components/skeletons/ProjectCardSkeleton'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CreateActionButton } from '@/components/ui/create-action-button'
import {
  getGeneralStatsOptions,
  getProjectsOptions,
  listGitProvidersOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { useQuery } from '@tanstack/react-query'
import { subDays } from 'date-fns'
import {
  DollarSign,
  Eye,
  FolderGit2,
  FolderPlus,
  GitBranch,
  Minus,
  TrendingDown,
  TrendingUp,
  Upload,
  Users,
} from 'lucide-react'
import { Link, useNavigate } from 'react-router-dom'

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
        className: 'text-xs text-muted-foreground flex items-center mt-1',
      },
    }
  }

  if (rounded > 0) {
    return {
      change: `+${rounded}% vs prev. period`,
      changeDisplay: {
        icon: <TrendingUp className="mr-1 h-3 w-3" />,
        className:
          'text-xs text-emerald-600 dark:text-emerald-400 flex items-center mt-1',
        isPositive: true,
      },
    }
  }

  return {
    change: `${rounded}% vs prev. period`,
    changeDisplay: {
      icon: <TrendingDown className="mr-1 h-3 w-3" />,
      className:
        'text-xs text-red-600 dark:text-red-400 flex items-center mt-1',
      isPositive: false,
    },
  }
}

const ITEMS_PER_PAGE = 8

export function Projects() {
  const { setBreadcrumbs } = useBreadcrumbs()
  const navigate = useNavigate()
  const [page, setPage] = useState(1)

  const { data: projectsData, isLoading } = useQuery({
    ...getProjectsOptions({
      query: {
        page,
        per_page: ITEMS_PER_PAGE,
      },
    }),
  })

  const { data: gitProviders, isLoading: gitProvidersLoading } = useQuery({
    ...listGitProvidersOptions({}),
    retry: false,
  })

  useEffect(() => {
    setBreadcrumbs([{ label: 'Projects' }])
  }, [setBreadcrumbs])

  // Keyboard shortcut: N to create new project

  // Keyboard shortcuts: Ctrl+1 through Ctrl+9 to navigate to projects
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Check if user is typing in an input field
      const target = e.target as HTMLElement
      const isTyping =
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable

      // Only trigger if Ctrl (or Cmd on Mac) is pressed with a number key
      if (
        !isTyping &&
        (e.ctrlKey || e.metaKey) &&
        !e.altKey &&
        !e.shiftKey &&
        e.key >= '1' &&
        e.key <= '9'
      ) {
        const index = parseInt(e.key, 10) - 1
        const projects = projectsData?.projects || []

        if (projects[index]) {
          e.preventDefault()
          navigate(`/projects/${projects[index].slug}`)
        }
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [projectsData?.projects, navigate])

  usePageTitle('Projects')

  // Batch fetch analytics for all visible projects
  const { startDate, endDate } = useMemo(() => {
    return {
      startDate: subDays(new Date(), 1).toISOString(),
      endDate: new Date().toISOString(),
    }
  }, [])

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

  // Stats for the header metric cards (merged in from the former Dashboard
  // page). Only fetched when at least one project exists — the onboarding
  // view replaces the metrics otherwise.
  const hasProjects = (projectsData?.projects?.length || 0) > 0
  const generalStatsQuery = useQuery({
    ...getGeneralStatsOptions({
      query: { start_date: startDate, end_date: endDate },
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

  if (!isLoading && !hasProjects) {
    return (
      <div className="sm:p-8">
        <ImprovedOnboardingDashboard />
      </div>
    )
  }

  return (
    <div className="p-4 sm:p-8 space-y-6">
      <ExternalConnectivityAlert showInDashboard dismissible />

      {/* Metric cards (merged from former Dashboard page). */}
      <div className="grid gap-3 grid-cols-2 sm:gap-4 md:grid-cols-4 md:gap-6">
        {generalStatsQuery.isLoading ? (
          <MetricCardSkeleton />
        ) : (
          <MetricCard
            title="Projects"
            value={
              generalStatsQuery.data?.total_projects ??
              projectsData?.total ??
              0
            }
            change=""
            icon={<FolderGit2 className="h-5 w-5" />}
          />
        )}

        {generalStatsQuery.isLoading ? (
          <MetricCardSkeleton />
        ) : (
          <MetricCard
            title="Visitors"
            value={
              generalStatsQuery.data?.total_unique_visitors?.toLocaleString() ??
              (generalStatsQuery.error ? 'N/A' : 0)
            }
            change={visitorsTrend?.change ?? 'vs prev. period'}
            changeDisplay={visitorsTrend?.changeDisplay}
            icon={<Users className="h-5 w-5" />}
          />
        )}

        {generalStatsQuery.isLoading ? (
          <MetricCardSkeleton />
        ) : (
          <MetricCard
            title="Page Views"
            value={
              generalStatsQuery.data?.total_page_views?.toLocaleString() ??
              (generalStatsQuery.error ? 'N/A' : 0)
            }
            change={pageViewsTrend?.change ?? 'vs prev. period'}
            changeDisplay={pageViewsTrend?.changeDisplay}
            icon={<Eye className="h-5 w-5" />}
          />
        )}

        <div className="relative">
          {generalStatsQuery.isLoading ? (
            <MetricCardSkeleton />
          ) : (
            <MetricCard
              title="Revenue"
              value="$0"
              change=""
              icon={<DollarSign className="h-5 w-5" />}
            />
          )}
          <div className="absolute inset-0 bg-background/80 rounded-lg flex items-center justify-center">
            <Badge variant="secondary" className="text-xs">
              Coming Soon
            </Badge>
          </div>
        </div>
      </div>

      {/* Header */}
      <div className="flex flex-col gap-3 sm:flex-row sm:justify-between sm:items-center">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Projects</h1>
          <p className="text-sm text-muted-foreground">
            Manage your projects and their settings
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button asChild variant="outline">
            <Link
              to="/projects/import-wizard"
              className="flex items-center gap-2"
            >
              <Upload className="h-4 w-4" />
              Import Project
            </Link>
          </Button>
          <CreateActionButton to="/projects/new" label="New Project" />
        </div>
      </div>

      {/* Projects Grid */}
      <div className="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
        {isLoading || gitProvidersLoading ? (
          <>
            {Array.from({ length: ITEMS_PER_PAGE }).map((_, i) => (
              <ProjectCardSkeleton key={i} />
            ))}
          </>
        ) : projectsData?.projects.length === 0 ? (
          !gitProviders || gitProviders.length === 0 ? (
            <div className="col-span-full flex flex-col items-center justify-center rounded-lg border border-dashed p-8 text-center animate-in fade-in-50">
              <div className="flex h-20 w-20 items-center justify-center rounded-full bg-muted">
                <GitBranch className="h-10 w-10 text-muted-foreground" />
              </div>
              <h2 className="mt-6 text-xl font-semibold">
                No Git providers configured
              </h2>
              <p className="mt-2 text-center text-sm text-muted-foreground max-w-md">
                Before creating projects, you need to set up a Git provider like
                GitHub or GitLab to connect your repositories.
              </p>
              <div className="flex gap-3 mt-6">
                <Button asChild>
                  <Link
                    to="/settings/git-providers/add"
                    className="flex items-center gap-2"
                  >
                    <GitBranch className="h-4 w-4" />
                    Add Git Provider
                  </Link>
                </Button>
                <Button asChild variant="outline">
                  <Link to="/settings/git-providers" className="flex items-center gap-2">
                    View Providers
                  </Link>
                </Button>
              </div>
            </div>
          ) : (
            <div className="col-span-full flex flex-col items-center justify-center rounded-lg border border-dashed p-8 text-center animate-in fade-in-50">
              <div className="flex h-20 w-20 items-center justify-center rounded-full bg-muted">
                <FolderPlus className="h-10 w-10 text-muted-foreground" />
              </div>
              <h2 className="mt-6 text-xl font-semibold">
                No projects created
              </h2>
              <p className="mt-2 text-center text-sm text-muted-foreground">
                Get started by creating or importing your first project
              </p>
              <div className="flex gap-3 mt-6">
                <CreateActionButton to="/projects/new" label="New Project" />
                <Button asChild variant="outline">
                  <Link
                    to="/projects/import-wizard"
                    className="flex items-center gap-2"
                  >
                    <Upload className="h-4 w-4" />
                    Import Project
                  </Link>
                </Button>
              </div>
            </div>
          )
        ) : (
          <>
            {projectsData?.projects.map((project, index) => (
              <ProjectCard
                key={project.id}
                project={project}
                shortcutNumber={index < 9 ? index + 1 : undefined}
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
          </>
        )}
      </div>

      {/* Pagination - Only show if there are projects */}
      {projectsData && projectsData.projects.length > 0 && (
        <div className="flex items-center justify-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setPage((p) => Math.max(1, p - 1))}
            disabled={page === 1}
          >
            Previous
          </Button>
          <span className="text-sm text-muted-foreground">
            Page {page} of {Math.ceil(projectsData.total / ITEMS_PER_PAGE)}
          </span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setPage((p) => p + 1)}
            disabled={page >= Math.ceil(projectsData.total / ITEMS_PER_PAGE)}
          >
            Next
          </Button>
        </div>
      )}
    </div>
  )
}
