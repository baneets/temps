import chalk from 'chalk'
import { requireAuth } from '../../config/store.js'
import { setupClient, client, getErrorMessage } from '../../lib/api-client.js'
import { requireProjectSlug } from '../../config/resolve-project.js'
import {
  getProjectBySlug,
  getUniqueCounts,
  getPagePaths,
  getEventsCount,
  getVisitors,
  getHourlyVisits,
} from '../../api/sdk.gen.js'
import { withSpinner } from '../../ui/spinner.js'
import { newline, json as jsonOut, colors, info } from '../../ui/output.js'
import { parsePeriod } from './period.js'

interface OverviewOptions {
  project?: string
  period?: string
  json?: boolean
}

interface LocationCount {
  country: string
  count: number
  percentage: number
}


function formatNumber(n: number): string {
  return n.toLocaleString('en-US')
}

function renderSparkline(data: { count: number }[]): string {
  const blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']
  const max = Math.max(...data.map((d) => d.count), 1)

  // Take last 48 points to fit terminal
  const points = data.length > 48 ? data.slice(-48) : data

  return points
    .map((d) => {
      const idx = Math.min(Math.floor((d.count / max) * 7), 7)
      return blocks[idx]
    })
    .join('')
}

export async function overview(options: OverviewOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const period = options.period ?? '24h'
  const { startDate, endDate, label } = parsePeriod(period)

  const resolved = await requireProjectSlug(options.project)

  if (resolved.source !== 'flag') {
    info(`Using project ${colors.bold(resolved.slug)} (from ${resolved.source})`)
  }

  // Resolve project ID from slug
  const { data: projectData, error: projectError } = await getProjectBySlug({
    client,
    path: { slug: resolved.slug },
  })

  if (projectError || !projectData) {
    throw new Error(`Project "${resolved.slug}" not found`)
  }

  const projectId = projectData.id

  // Fetch all data concurrently
  const data = await withSpinner('Fetching analytics...', async () => {
    const [visitorsRes, sessionsRes, pageViewsRes, pagesRes, eventsRes, visitorsListRes, hourlyRes] =
      await Promise.all([
        getUniqueCounts({
          client,
          path: { project_id: projectId },
          query: { start_date: startDate, end_date: endDate, metric: 'visitors' },
        }),
        getUniqueCounts({
          client,
          path: { project_id: projectId },
          query: { start_date: startDate, end_date: endDate, metric: 'sessions' },
        }),
        getUniqueCounts({
          client,
          path: { project_id: projectId },
          query: { start_date: startDate, end_date: endDate, metric: 'page_views' },
        }),
        getPagePaths({
          client,
          query: { project_id: projectId, start_date: startDate, end_date: endDate, limit: 10 },
        }),
        getEventsCount({
          client,
          query: {
            project_id: projectId,
            start_date: startDate,
            end_date: endDate,
            limit: 10,
            custom_events_only: true,
          },
        }),
        getVisitors({
          client,
          query: {
            project_id: projectId,
            start_date: startDate,
            end_date: endDate,
            limit: 200,
          },
        }),
        getHourlyVisits({
          client,
          path: { project_id: projectId },
          query: { start_date: startDate, end_date: endDate, aggregation_level: 'visitors' },
        }),
      ])

    if (visitorsRes.error) throw new Error(getErrorMessage(visitorsRes.error))
    if (sessionsRes.error) throw new Error(getErrorMessage(sessionsRes.error))
    if (pageViewsRes.error) throw new Error(getErrorMessage(pageViewsRes.error))
    if (pagesRes.error) throw new Error(getErrorMessage(pagesRes.error))
    if (eventsRes.error) throw new Error(getErrorMessage(eventsRes.error))
    if (visitorsListRes.error) throw new Error(getErrorMessage(visitorsListRes.error))

    // Aggregate locations from visitors
    const locationMap = new Map<string, number>()
    let totalWithCountry = 0
    const visitors = (visitorsListRes.data as any)?.visitors ?? []
    for (const v of visitors) {
      if (v.country) {
        locationMap.set(v.country, (locationMap.get(v.country) ?? 0) + 1)
        totalWithCountry++
      }
    }

    const topLocations: LocationCount[] = [...locationMap.entries()]
      .map(([country, count]) => ({
        country,
        count,
        percentage: totalWithCountry > 0 ? (count / totalWithCountry) * 100 : 0,
      }))
      .sort((a, b) => b.count - a.count)
      .slice(0, 10)

    return {
      uniqueVisitors: (visitorsRes.data as any)?.count ?? 0,
      totalSessions: (sessionsRes.data as any)?.count ?? 0,
      pageViews: (pageViewsRes.data as any)?.count ?? 0,
      topPages: (pagesRes.data as any)?.page_paths ?? [],
      topEvents: (eventsRes.data as any) ?? [],
      topLocations,
      hourly: (hourlyRes.data as any) ?? [],
    }
  })

  if (options.json) {
    jsonOut({
      project: resolved.slug,
      period,
      ...data,
    })
    return
  }

  // Pretty output
  const line = chalk.cyan('━'.repeat(64))

  newline()
  console.log(line)
  console.log(
    `   ${chalk.bold.white('Analytics:')} ${chalk.bold.cyan(resolved.slug)} ${chalk.gray(`(${label})`)}`
  )
  console.log(line)
  newline()

  // Key metrics
  console.log(`  ${chalk.white('Unique Visitors')}${' '.repeat(7)}${chalk.bold.green(formatNumber(data.uniqueVisitors))}`)
  console.log(`  ${chalk.white('Total Sessions')}${' '.repeat(8)}${chalk.bold.green(formatNumber(data.totalSessions))}`)
  console.log(`  ${chalk.white('Page Views')}${' '.repeat(12)}${chalk.bold.green(formatNumber(data.pageViews))}`)

  // Sparkline
  if (data.hourly.length > 0) {
    const max = Math.max(...data.hourly.map((d: any) => d.count), 1)
    newline()
    console.log(`  ${chalk.bold.white('Hourly Visitors')}`)
    console.log(`  ${chalk.cyan(renderSparkline(data.hourly))} ${chalk.gray(`(max: ${formatNumber(max)})`)}`)
  }

  // Top Pages
  if (data.topPages.length > 0) {
    newline()
    console.log(`  ${chalk.bold.white('Top Pages')}`)
    console.log(`  ${chalk.gray('─'.repeat(60))}`)
    console.log(
      `  ${chalk.gray('#'.padEnd(4))}${chalk.gray('Path'.padEnd(40))}${chalk.gray('Sessions'.padStart(10))}${chalk.gray('Views'.padStart(8))}`
    )

    data.topPages.forEach((page: any, i: number) => {
      const path = page.page_path.length > 38 ? page.page_path.slice(0, 35) + '...' : page.page_path
      console.log(
        `  ${chalk.gray(String(i + 1).padEnd(4))}${chalk.white(path.padEnd(40))}${formatNumber(page.session_count).padStart(10)}${formatNumber(page.page_view_count).padStart(8)}`
      )
    })
  }

  // Top Events
  if (data.topEvents.length > 0) {
    newline()
    console.log(`  ${chalk.bold.white('Top Events')}`)
    console.log(`  ${chalk.gray('─'.repeat(60))}`)
    console.log(
      `  ${chalk.gray('#'.padEnd(4))}${chalk.gray('Event'.padEnd(40))}${chalk.gray('Count'.padStart(10))}${chalk.gray('%'.padStart(8))}`
    )

    data.topEvents.forEach((event: any, i: number) => {
      const name = event.event_name.length > 38 ? event.event_name.slice(0, 35) + '...' : event.event_name
      console.log(
        `  ${chalk.gray(String(i + 1).padEnd(4))}${chalk.white(name.padEnd(40))}${formatNumber(event.count).padStart(10)}${(event.percentage.toFixed(1) + '%').padStart(8)}`
      )
    })
  }

  // Top Locations
  if (data.topLocations.length > 0) {
    newline()
    console.log(`  ${chalk.bold.white('Top Locations')}`)
    console.log(`  ${chalk.gray('─'.repeat(60))}`)
    console.log(
      `  ${chalk.gray('#'.padEnd(4))}${chalk.gray('Country'.padEnd(40))}${chalk.gray('Visitors'.padStart(10))}${chalk.gray('%'.padStart(8))}`
    )

    data.topLocations.forEach((loc, i) => {
      console.log(
        `  ${chalk.gray(String(i + 1).padEnd(4))}${chalk.white(loc.country.padEnd(40))}${formatNumber(loc.count).padStart(10)}${(loc.percentage.toFixed(1) + '%').padStart(8)}`
      )
    })
  }

  newline()
  console.log(line)
  newline()
}
