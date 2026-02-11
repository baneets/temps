import type { Command } from 'commander'
import { execSync } from 'node:child_process'
import { requireAuth } from '../../config/store.js'
import { setupClient } from '../../lib/api-client.js'
import { resolveProjectSlug } from '../../config/resolve-project.js'
import { hasProjectConfig, writeProjectConfig } from '../../config/project-config.js'
import { deploy } from '../deploy/deploy.js'
import { promptConfirm } from '../../ui/prompts.js'
import { info, warning, newline, colors } from '../../ui/output.js'

interface UpOptions {
  project?: string
  environment?: string
  branch?: string
  wait?: boolean
  yes?: boolean
}

/**
 * Detect the current git branch from the working directory
 */
function detectGitBranch(): string | null {
  try {
    return execSync('git rev-parse --abbrev-ref HEAD', {
      encoding: 'utf-8',
      stdio: ['pipe', 'pipe', 'pipe'],
    }).trim()
  } catch {
    return null
  }
}

async function up(projectArg: string | undefined, options: UpOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  // Resolve project
  const resolved = await resolveProjectSlug(projectArg ?? options.project)

  if (!resolved) {
    newline()
    warning('No project linked to this directory')
    info('Run "temps init" or "temps link" to connect a project')
    return
  }

  // Auto-detect git branch if not specified
  let branch = options.branch
  if (!branch) {
    const detectedBranch = detectGitBranch()
    if (detectedBranch && detectedBranch !== 'HEAD') {
      branch = detectedBranch
    }
  }

  if (resolved.source !== 'flag') {
    info(`Using project ${colors.bold(resolved.slug)} (from ${resolved.source})`)
  }

  // Delegate to existing deploy function
  await deploy({
    project: resolved.slug,
    environment: options.environment,
    branch,
    wait: options.wait,
    yes: options.yes,
  })

  // Offer to save config if it doesn't exist
  if (!hasProjectConfig() && !options.yes) {
    newline()
    const save = await promptConfirm({
      message: 'Save this project link for future use?',
      default: true,
    })
    if (save) {
      await writeProjectConfig({ projectSlug: resolved.slug })
      info('Saved to .temps/config.json')
    }
  }
}

export function registerUpCommand(program: Command): void {
  program
    .command('up [project]')
    .description('Deploy the current project (shortcut for deploy)')
    .option('-p, --project <project>', 'Project slug or ID')
    .option('-e, --environment <env>', 'Target environment name')
    .option('-b, --branch <branch>', 'Git branch to deploy (auto-detected from cwd)')
    .option('--no-wait', 'Do not wait for deployment to complete')
    .option('-y, --yes', 'Skip confirmation prompts')
    .action((projectArg, opts) => {
      if (projectArg && !opts.project) {
        opts.project = projectArg
      }
      return up(projectArg, opts)
    })
}
