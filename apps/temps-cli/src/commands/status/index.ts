import type { Command } from 'commander'
import { requireAuth } from '../../config/store.js'
import { setupClient, client } from '../../lib/api-client.js'
import { requireProjectSlug } from '../../config/resolve-project.js'
import {
  getProjectBySlug,
  getEnvironments,
  getLastDeployment,
  listContainers,
} from '../../api/sdk.gen.js'
import { withSpinner } from '../../ui/spinner.js'
import { statusBadge } from '../../ui/table.js'
import {
  newline,
  header,
  icons,
  colors,
  keyValue,
  json as jsonOutput,
  formatRelativeTime,
} from '../../ui/output.js'

interface StatusOptions {
  project?: string
  environment?: string
  json?: boolean
}

async function status(projectArg: string | undefined, options: StatusOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const resolved = await requireProjectSlug(projectArg ?? options.project)

  const projectData = await withSpinner('Fetching project status...', async () => {
    const { data, error } = await getProjectBySlug({
      client,
      path: { slug: resolved.slug },
    })
    if (error || !data) {
      throw new Error(`Project "${resolved.slug}" not found`)
    }
    return data
  })

  // Fetch environments
  const { data: environments } = await getEnvironments({
    client,
    path: { project_id: projectData.id },
  })
  const envList = environments ?? []

  // Filter environments if specified
  const targetEnvs = options.environment
    ? envList.filter(
        e =>
          e.name.toLowerCase() === options.environment!.toLowerCase() ||
          e.slug === options.environment
      )
    : envList

  if (options.json) {
    const data: Record<string, unknown> = {
      project: {
        id: projectData.id,
        name: projectData.name,
        slug: projectData.slug,
        main_branch: projectData.main_branch,
      },
      environments: targetEnvs.map(e => ({
        id: e.id,
        name: e.name,
        slug: e.slug,
        url: e.main_url,
        is_preview: e.is_preview,
      })),
    }
    jsonOutput(data)
    return
  }

  newline()
  header(`${icons.folder} ${projectData.name}`)
  keyValue('Slug', projectData.slug)
  keyValue('Branch', projectData.main_branch ?? 'main')
  if (resolved.source !== 'flag') {
    keyValue('Source', colors.muted(resolved.source))
  }

  if (targetEnvs.length === 0) {
    newline()
    if (options.environment) {
      console.log(colors.warning(`  Environment "${options.environment}" not found`))
    } else {
      console.log(colors.muted('  No environments configured'))
    }
    newline()
    return
  }

  for (const env of targetEnvs) {
    newline()
    console.log(colors.muted('─'.repeat(50)))
    console.log(`  ${colors.bold(env.name)} ${env.is_preview ? colors.muted('(preview)') : ''}`)
    console.log(colors.muted('─'.repeat(50)))

    // Fetch last deployment for this environment
    try {
      const { data: deployment } = await getLastDeployment({
        client,
        path: { id: projectData.id },
      })

      if (deployment && deployment.environment_id === env.id) {
        keyValue('  Status', statusBadge(deployment.status))
        keyValue(
          '  Last Deploy',
          `#${deployment.id} - ${deployment.commit_hash?.substring(0, 7) ?? '-'} - ${formatRelativeTime(new Date(deployment.created_at * 1000).toISOString())}`
        )
      } else {
        keyValue('  Status', colors.muted('no deployments'))
      }
    } catch {
      keyValue('  Status', colors.muted('unable to fetch'))
    }

    if (env.main_url) {
      keyValue('  URL', colors.primary(env.main_url))
    }

    // Fetch container status
    try {
      const { data: containerData } = await listContainers({
        client,
        path: { project_id: projectData.id, environment_id: env.id },
      })
      if (containerData && containerData.containers.length > 0) {
        const running = containerData.containers.filter(
          c => c.status?.toLowerCase() === 'running'
        ).length
        keyValue('  Containers', `${running}/${containerData.containers.length} running`)
      }
    } catch {
      // Container info is optional
    }
  }

  newline()
}

export function registerStatusCommand(program: Command): void {
  program
    .command('status [project]')
    .description('Show project deployment status')
    .option('-p, --project <project>', 'Project slug')
    .option('-e, --environment <env>', 'Filter by environment')
    .option('--json', 'Output in JSON format')
    .action((projectArg, opts) => {
      if (projectArg && !opts.project) {
        opts.project = projectArg
      }
      return status(projectArg, opts)
    })
}
