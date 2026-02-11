import { config } from './store.js'
import { readProjectConfig } from './project-config.js'

export type ProjectSource = 'flag' | 'local-config' | 'env-var' | 'global-config'

export interface ResolvedProject {
  slug: string
  source: ProjectSource
}

/**
 * Resolve the project slug using priority chain:
 * CLI flag > .temps/config.json > TEMPS_PROJECT env var > global defaultProject
 */
export async function resolveProjectSlug(flagValue?: string): Promise<ResolvedProject | null> {
  // 1. CLI flag takes highest priority
  if (flagValue) {
    return { slug: flagValue, source: 'flag' }
  }

  // 2. Local .temps/config.json
  const localConfig = await readProjectConfig()
  if (localConfig?.projectSlug) {
    return { slug: localConfig.projectSlug, source: 'local-config' }
  }

  // 3. TEMPS_PROJECT environment variable
  const envProject = process.env.TEMPS_PROJECT
  if (envProject) {
    return { slug: envProject, source: 'env-var' }
  }

  // 4. Global default project
  const defaultProject = config.get('defaultProject')
  if (defaultProject) {
    return { slug: defaultProject, source: 'global-config' }
  }

  return null
}

/**
 * Resolve project slug or throw with a helpful message
 */
export async function requireProjectSlug(flagValue?: string): Promise<ResolvedProject> {
  const resolved = await resolveProjectSlug(flagValue)
  if (resolved) return resolved

  throw new Error(
    'No project specified. You can:\n' +
    '  • Pass --project <slug> flag\n' +
    '  • Run "temps link" to link this directory to a project\n' +
    '  • Set TEMPS_PROJECT environment variable\n' +
    '  • Run "temps configure" to set a default project'
  )
}
