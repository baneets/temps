import type { Command } from 'commander'
import { requireAuth } from '../../config/store.js'
import { setupClient, client, getErrorMessage } from '../../lib/api-client.js'
import {
  discoverWorkloads,
  createPlan,
  executeImport,
  getImportStatus,
  listSources,
} from '../../api/sdk.gen.js'
import type { ImportSource } from '../../api/types.gen.js'
import { withSpinner } from '../../ui/spinner.js'
import { printTable, statusBadge, type TableColumn } from '../../ui/table.js'
import { promptText, promptSelect, promptConfirm } from '../../ui/prompts.js'
import { newline, header, icons, json, colors, success, info, warning, keyValue } from '../../ui/output.js'

interface SourcesOptions {
  json?: boolean
}

interface DiscoverOptions {
  source?: string
  json?: boolean
}

interface PlanOptions {
  source?: string
  workload?: string
}

interface ExecuteOptions {
  source?: string
  workload?: string
  yes?: boolean
}

interface StatusOptions {
  sessionId: string
  json?: boolean
}

export function registerImportsCommands(program: Command): void {
  const imports = program
    .command('imports')
    .alias('import')
    .description('Import workloads from external sources')

  imports
    .command('sources')
    .alias('ls')
    .description('List available import sources')
    .option('--json', 'Output in JSON format')
    .action(listSourcesAction)

  imports
    .command('discover')
    .description('Discover workloads from a source')
    .option('-s, --source <source>', 'Import source')
    .option('--json', 'Output in JSON format')
    .action(discoverAction)

  imports
    .command('plan')
    .description('Create an import plan')
    .option('-s, --source <source>', 'Import source')
    .option('-w, --workload <workload>', 'Workload ID to import')
    .action(planAction)

  imports
    .command('execute')
    .description('Execute an import')
    .option('-s, --source <source>', 'Import source')
    .option('-w, --workload <workload>', 'Workload ID to import')
    .option('-y, --yes', 'Skip confirmation prompts (for automation)')
    .action(executeAction)

  imports
    .command('status')
    .description('Get import status')
    .requiredOption('--session-id <id>', 'Import session ID')
    .option('--json', 'Output in JSON format')
    .action(statusAction)
}

async function listSourcesAction(options: SourcesOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const sources = await withSpinner('Fetching import sources...', async () => {
    const { data, error } = await listSources({ client })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data ?? []
  })

  if (options.json) {
    json(sources)
    return
  }

  newline()
  header(`${icons.package} Import Sources (${sources.length})`)

  if (sources.length === 0) {
    info('No import sources available')
    newline()
    return
  }

  const columns: TableColumn<Record<string, unknown>>[] = [
    { header: 'Name', key: 'name', color: (v) => colors.bold(v) },
    { header: 'Type', key: 'type' },
    { header: 'Description', key: 'description', color: (v) => colors.muted(v) },
    { header: 'Status', key: 'status', color: (v) => statusBadge(v === 'available' || v === 'active' ? 'active' : 'inactive') },
  ]

  printTable(sources as Record<string, unknown>[], columns, { style: 'minimal' })
  newline()
}

async function discoverAction(options: DiscoverOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  let source: string

  if (options.source) {
    source = options.source
  } else {
    // Fetch available sources for selection
    const sources = await withSpinner('Fetching sources...', async () => {
      const { data, error } = await listSources({ client })
      if (error) {
        throw new Error(getErrorMessage(error))
      }
      return data ?? []
    })

    if (sources.length === 0) {
      warning('No import sources available')
      return
    }

    const sourceItems = sources as Record<string, unknown>[]
    source = await promptSelect({
      message: 'Select import source',
      choices: sourceItems.map((s) => ({
        name: (s.name as string) || (s.type as string) || 'Unknown',
        value: (s.name as string) || (s.type as string) || '',
      })),
    })
  }

  const workloads = await withSpinner(`Discovering workloads from ${source}...`, async () => {
    const { data, error } = await discoverWorkloads({
      client,
      body: { source: source as ImportSource },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  if (options.json) {
    json(workloads)
    return
  }

  const workloadsData = workloads as Record<string, unknown>
  const items = (workloadsData.workloads ?? workloadsData.items ?? []) as Record<string, unknown>[]

  newline()
  header(`${icons.package} Discovered Workloads from ${source} (${items.length})`)

  if (items.length === 0) {
    info('No workloads discovered from this source')
    newline()
    return
  }

  const columns: TableColumn<Record<string, unknown>>[] = [
    { header: 'Name', key: 'name', color: (v) => colors.bold(v) },
    { header: 'Type', key: 'type' },
    { header: 'Status', key: 'status', color: (v) => statusBadge(v === 'running' || v === 'active' ? 'active' : 'inactive') },
    { header: 'Image', key: 'image', color: (v) => colors.muted(v) },
  ]

  printTable(items, columns, { style: 'minimal' })
  newline()
}

async function planAction(options: PlanOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  let source: string

  if (options.source) {
    source = options.source
  } else {
    // Fetch available sources for selection
    const sources = await withSpinner('Fetching sources...', async () => {
      const { data, error } = await listSources({ client })
      if (error) {
        throw new Error(getErrorMessage(error))
      }
      return data ?? []
    })

    if (sources.length === 0) {
      warning('No import sources available')
      return
    }

    const sourceItems = sources as Record<string, unknown>[]
    source = await promptSelect({
      message: 'Select import source',
      choices: sourceItems.map((s) => ({
        name: (s.name as string) || (s.type as string) || 'Unknown',
        value: (s.name as string) || (s.type as string) || '',
      })),
    })
  }

  // Discover workloads and select one
  let workloadId: string
  if (options.workload) {
    workloadId = options.workload
  } else {
    const discovered = await withSpinner(`Discovering workloads from ${source}...`, async () => {
      const { data, error } = await discoverWorkloads({
        client,
        body: { source: source as ImportSource },
      })
      if (error) {
        throw new Error(getErrorMessage(error))
      }
      return data
    })

    const discoveredData = discovered as Record<string, unknown>
    const workloadItems = (discoveredData.workloads ?? []) as Record<string, unknown>[]

    if (workloadItems.length === 0) {
      warning('No workloads discovered from this source')
      return
    }

    workloadId = await promptSelect({
      message: 'Select workload to import',
      choices: workloadItems.map((w) => ({
        name: (w.name as string) || (w.id as string) || 'Unknown',
        value: (w.id as string) || (w.name as string) || '',
      })),
    })
  }

  const plan = await withSpinner(`Creating import plan for ${source}...`, async () => {
    const { data, error } = await createPlan({
      client,
      body: { source: source as ImportSource, workload_id: workloadId },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  const planData = plan as Record<string, unknown>

  newline()
  header(`${icons.info} Import Plan for ${source}`)

  if (planData.session_id) {
    keyValue('Session ID', planData.session_id as string)
  }
  if (planData.source) {
    keyValue('Source', planData.source as string)
  }
  if (planData.status) {
    keyValue('Status', planData.status as string)
  }

  const actions = (planData.actions ?? planData.steps ?? planData.workloads ?? []) as Record<string, unknown>[]

  if (actions.length > 0) {
    newline()
    header('Planned Actions')
    for (const action of actions) {
      const name = (action.name ?? action.workload ?? action.description ?? 'Unknown') as string
      const actionType = (action.action ?? action.type ?? 'import') as string
      console.log(`  ${colors.muted(actionType.toUpperCase())} ${colors.bold(name)}`)
      if (action.details) {
        console.log(`    ${colors.muted(action.details as string)}`)
      }
    }
  }

  if (planData.session_id) {
    newline()
    info('To execute this import plan, run:')
    info(`  temps imports execute --source ${source} -y`)
  }
  newline()
}

async function executeAction(options: ExecuteOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  let source: string

  if (options.source) {
    source = options.source
  } else {
    // Fetch available sources for selection
    const sources = await withSpinner('Fetching sources...', async () => {
      const { data, error } = await listSources({ client })
      if (error) {
        throw new Error(getErrorMessage(error))
      }
      return data ?? []
    })

    if (sources.length === 0) {
      warning('No import sources available')
      return
    }

    const sourceItems = sources as Record<string, unknown>[]
    source = await promptSelect({
      message: 'Select import source',
      choices: sourceItems.map((s) => ({
        name: (s.name as string) || (s.type as string) || 'Unknown',
        value: (s.name as string) || (s.type as string) || '',
      })),
    })
  }

  // Discover workloads and select one
  let workloadId: string
  if (options.workload) {
    workloadId = options.workload
  } else {
    const discovered = await withSpinner(`Discovering workloads from ${source}...`, async () => {
      const { data, error } = await discoverWorkloads({
        client,
        body: { source: source as ImportSource },
      })
      if (error) {
        throw new Error(getErrorMessage(error))
      }
      return data
    })

    const discoveredData = discovered as Record<string, unknown>
    const workloadItems = (discoveredData.workloads ?? []) as Record<string, unknown>[]

    if (workloadItems.length === 0) {
      warning('No workloads discovered from this source')
      return
    }

    workloadId = await promptSelect({
      message: 'Select workload to import',
      choices: workloadItems.map((w) => ({
        name: (w.name as string) || (w.id as string) || 'Unknown',
        value: (w.id as string) || (w.name as string) || '',
      })),
    })
  }

  // Create a plan first to get session_id and plan details
  const planResult = await withSpinner(`Creating import plan for ${source}...`, async () => {
    const { data, error } = await createPlan({
      client,
      body: { source: source as ImportSource, workload_id: workloadId },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  const planData = planResult as Record<string, unknown>
  const sessionId = planData.session_id as string
  const plan = planData.plan as Record<string, unknown> | undefined
  const project = (plan?.project ?? {}) as Record<string, unknown>
  const projectName = (project.name as string) || source

  if (!options.yes) {
    warning('This will import workloads from the selected source into your environment.')
    const confirmed = await promptConfirm({
      message: `Execute import from ${source}?`,
      default: false,
    })
    if (!confirmed) {
      info('Cancelled')
      return
    }
  }

  const result = await withSpinner(`Executing import from ${source}...`, async () => {
    const { data, error } = await executeImport({
      client,
      body: {
        session_id: sessionId,
        project_name: projectName,
        directory: '.',
        main_branch: 'main',
        preset: 'docker',
      },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  const resultData = result as Record<string, unknown>

  newline()
  success(`Import from ${source} initiated`)

  if (resultData.session_id) {
    keyValue('Session ID', resultData.session_id as string)
    newline()
    info('Track progress with:')
    info(`  temps imports status --session-id ${resultData.session_id}`)
  }
  if (resultData.status) {
    keyValue('Status', resultData.status as string)
  }
  if (resultData.imported_count !== undefined) {
    keyValue('Imported', String(resultData.imported_count))
  }
  newline()
}

async function statusAction(options: StatusOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const sessionId = options.sessionId

  const status = await withSpinner('Fetching import status...', async () => {
    const { data, error } = await getImportStatus({
      client,
      path: { session_id: sessionId },
    })
    if (error || !data) {
      throw new Error(getErrorMessage(error) ?? `Import session ${sessionId} not found`)
    }
    return data
  })

  if (options.json) {
    json(status)
    return
  }

  const statusData = status as Record<string, unknown>

  newline()
  header(`${icons.info} Import Status: ${sessionId}`)
  keyValue('Session ID', sessionId)
  if (statusData.source) {
    keyValue('Source', statusData.source as string)
  }
  if (statusData.status) {
    const statusValue = statusData.status as string
    keyValue('Status', statusBadge(
      statusValue === 'completed' || statusValue === 'success' ? 'active' :
      statusValue === 'failed' || statusValue === 'error' ? 'error' :
      statusValue === 'running' || statusValue === 'in_progress' ? 'pending' : 'inactive'
    ))
  }
  if (statusData.progress !== undefined) {
    keyValue('Progress', `${statusData.progress}%`)
  }
  if (statusData.total_workloads !== undefined) {
    keyValue('Total Workloads', String(statusData.total_workloads))
  }
  if (statusData.imported_count !== undefined) {
    keyValue('Imported', colors.success(String(statusData.imported_count)))
  }
  if (statusData.failed_count !== undefined && (statusData.failed_count as number) > 0) {
    keyValue('Failed', colors.error(String(statusData.failed_count)))
  }
  if (statusData.skipped_count !== undefined && (statusData.skipped_count as number) > 0) {
    keyValue('Skipped', colors.warning(String(statusData.skipped_count)))
  }
  if (statusData.started_at) {
    keyValue('Started', new Date(statusData.started_at as string).toLocaleString())
  }
  if (statusData.completed_at) {
    keyValue('Completed', new Date(statusData.completed_at as string).toLocaleString())
  }

  const errors = (statusData.errors ?? []) as Record<string, unknown>[]
  if (errors.length > 0) {
    newline()
    header('Errors')
    for (const err of errors) {
      const workload = (err.workload ?? err.name ?? 'Unknown') as string
      const message = (err.message ?? err.error ?? 'Unknown error') as string
      console.log(`  ${colors.error('x')} ${colors.bold(workload)}: ${message}`)
    }
  }

  newline()
}
