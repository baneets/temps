import type { Command } from 'commander'
import { readFileSync, statSync, existsSync } from 'node:fs'
import { execSync } from 'node:child_process'
import { resolve } from 'node:path'
import { requireAuth, credentials, config } from '../../config/store.js'
import {
  setupClient,
  client,
  getErrorMessage,
  normalizeApiUrl,
} from '../../lib/api-client.js'
import { withSpinner } from '../../ui/spinner.js'
import { promptConfirm } from '../../ui/prompts.js'
import {
  newline,
  header,
  icons,
  json,
  colors,
  success,
  info,
  warning,
  truncate,
} from '../../ui/output.js'

// --- Types ---

interface SkillDefinition {
  id: number
  slug: string
  name: string
  description?: string | null
  content: string
  has_archive?: boolean
  project_id?: number | null
}

interface ListResponse {
  items: SkillDefinition[]
  total: number
}

// --- Helpers ---

/** Resolve value: if prefixed with @, read from file path */
function resolveValue(value: string): string {
  if (value.startsWith('@')) {
    const filePath = value.slice(1)
    try {
      return readFileSync(filePath, 'utf-8')
    } catch (e) {
      throw new Error(`Failed to read file '${filePath}': ${e}`)
    }
  }
  return value
}

/**
 * Check if the given path (after stripping @) is a directory or tar.gz.
 * Returns { type: 'directory' | 'tarball' | 'file', path: string }
 */
function classifyContent(
  value: string,
): { type: 'directory' | 'tarball' | 'file'; path: string } | null {
  if (!value.startsWith('@')) return null
  const filePath = resolve(value.slice(1))
  if (!existsSync(filePath)) return null

  const stat = statSync(filePath)
  if (stat.isDirectory()) return { type: 'directory', path: filePath }
  if (filePath.endsWith('.tar.gz') || filePath.endsWith('.tgz'))
    return { type: 'tarball', path: filePath }
  return { type: 'file', path: filePath }
}

/**
 * Create a tar.gz from a directory and return the buffer.
 * Also extracts SKILL.md content if present.
 */
function packDirectory(dirPath: string): {
  archive: Buffer
  content: string
} {
  // Read SKILL.md if it exists
  const skillMdPath = resolve(dirPath, 'SKILL.md')
  let content = ''
  if (existsSync(skillMdPath)) {
    content = readFileSync(skillMdPath, 'utf-8')
  }

  // Create tar.gz using system tar (available on macOS + Linux)
  const tarData = execSync(`tar -czf - -C "${dirPath}" .`, {
    maxBuffer: 50 * 1024 * 1024, // 50MB
  })

  return { archive: Buffer.from(tarData), content }
}

/**
 * Upload a skill via multipart form (for directory/archive skills).
 */
async function uploadSkillMultipart(
  baseUrl: string,
  apiKey: string,
  uploadUrl: string,
  fields: {
    slug: string
    name: string
    description?: string
    content: string
    archive: Buffer
  },
): Promise<SkillDefinition> {
  const form = new FormData()
  form.append('slug', fields.slug)
  form.append('name', fields.name)
  if (fields.description) form.append('description', fields.description)
  form.append('content', fields.content)
  form.append(
    'archive',
    new Blob([fields.archive], { type: 'application/gzip' }),
    `${fields.slug}.tar.gz`,
  )

  const resp = await fetch(`${baseUrl}${uploadUrl}`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${apiKey}` },
    body: form,
  })

  if (!resp.ok) {
    const errorBody = await resp.text()
    throw new Error(`Upload failed (${resp.status}): ${errorBody}`)
  }

  return (await resp.json()) as SkillDefinition
}

async function resolveProjectId(projectSlug: string): Promise<number> {
  // Try by slug first
  const { data, error } = await client.get({
    url: '/projects/by-slug/{slug}',
    path: { slug: projectSlug },
  })
  if (!error && data) {
    return (data as { id: number }).id
  }

  // Try as numeric ID
  const parsed = parseInt(projectSlug, 10)
  if (!isNaN(parsed)) return parsed

  throw new Error(`Project '${projectSlug}' not found`)
}

// --- Options ---

interface ListOptions {
  global?: boolean
  project?: string
  json?: boolean
}

interface CreateOptions {
  name: string
  slug: string
  content?: string
  description?: string
  global?: boolean
  project?: string
}

interface UpdateOptions {
  name?: string
  content?: string
  description?: string
  global?: boolean
  project?: string
}

interface DeleteOptions {
  global?: boolean
  project?: string
  force?: boolean
  yes?: boolean
}

// --- Registration ---

export function registerSkillsCommands(program: Command): void {
  const skills = program
    .command('skills')
    .alias('skill')
    .description('Manage AI skill definitions (global or project-scoped)')

  skills
    .command('list')
    .alias('ls')
    .description('List skill definitions')
    .option('--global', 'List global (platform-wide) skills')
    .option('--project <slug>', 'List skills for a specific project')
    .option('--json', 'Output in JSON format')
    .action(listAction)

  skills
    .command('create')
    .alias('add')
    .description(
      'Create a new skill definition. Use @path for content from a file, directory, or tar.gz',
    )
    .requiredOption('-n, --name <name>', 'Skill name')
    .requiredOption('-s, --slug <slug>', 'Skill slug (URL-safe identifier)')
    .option(
      '-c, --content <content>',
      'Skill content (markdown), @file, @directory, or @archive.tar.gz',
    )
    .option('-d, --description <description>', 'Skill description')
    .option('--global', 'Create as global (platform-wide) skill')
    .option('--project <slug>', 'Create skill for a specific project')
    .action(createAction)

  skills
    .command('update')
    .description('Update an existing skill definition')
    .argument('<slug>', 'Slug of the skill to update')
    .option('-n, --name <name>', 'New name')
    .option(
      '-c, --content <content>',
      'New content. Prefix with @ to read from file',
    )
    .option('-d, --description <description>', 'New description')
    .option('--global', 'Update a global skill')
    .option('--project <slug>', 'Update a project-scoped skill')
    .action(updateAction)

  skills
    .command('delete')
    .alias('rm')
    .description('Delete a skill definition')
    .argument('<slug>', 'Slug of the skill to delete')
    .option('--global', 'Delete a global skill')
    .option('--project <slug>', 'Delete a project-scoped skill')
    .option('-f, --force', 'Skip confirmation')
    .option('-y, --yes', 'Skip confirmation (alias for --force)')
    .action(deleteAction)
}

// --- Actions ---

async function listAction(options: ListOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const isProject = !!options.project

  const items = await withSpinner('Fetching skills...', async () => {
    let url: string
    let pathParams: Record<string, unknown> = {}

    if (isProject) {
      const pid = await resolveProjectId(options.project!)
      url = '/projects/{project_id}/skills'
      pathParams = { project_id: pid }
    } else {
      url = '/settings/skills'
    }

    const { data, error } = await client.get({ url, path: pathParams })
    if (error) throw new Error(getErrorMessage(error))
    return data as ListResponse
  })

  if (options.json) {
    json(items)
    return
  }

  const scopeLabel = isProject ? `Project (${options.project})` : 'Global'
  newline()
  header(`${icons.info} ${scopeLabel} Skills (${items.items.length})`)

  if (items.items.length === 0) {
    info('No skills defined yet.')
    info(
      isProject
        ? `Run: temps skills create --project ${options.project} --name "My Skill" --slug my-skill --content @./skill.md`
        : 'Run: temps skills create --global --name "My Skill" --slug my-skill --content @./skill.md',
    )
    newline()
    return
  }

  for (const skill of items.items) {
    const scopeBadge = skill.project_id
      ? colors.info('project')
      : colors.warning('global')
    const archiveBadge = skill.has_archive ? ` ${colors.muted('[archive]')}` : ''
    console.log(
      `  ${colors.primary(skill.slug)} ${colors.bold(skill.name)} [${scopeBadge}]${archiveBadge}`,
    )
    if (skill.description) {
      console.log(`    ${colors.muted(skill.description)}`)
    }
    const preview = truncate(skill.content.replace(/\n/g, ' '), 80)
    console.log(`    ${colors.muted(preview)}`)
    newline()
  }
}

async function createAction(options: CreateOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const isProject = !!options.project
  const classified = options.content ? classifyContent(options.content) : null

  // Directory or tar.gz → multipart upload
  if (classified && (classified.type === 'directory' || classified.type === 'tarball')) {
    const skill = await withSpinner('Uploading skill archive...', async () => {
      let archive: Buffer
      let content: string

      if (classified.type === 'directory') {
        const packed = packDirectory(classified.path)
        archive = packed.archive
        content = packed.content
      } else {
        archive = Buffer.from(readFileSync(classified.path))
        // Try to extract SKILL.md content from tarball for the content field
        content = ''
        try {
          const stdout = execSync(
            `tar -xzf "${classified.path}" -O ./SKILL.md 2>/dev/null || tar -xzf "${classified.path}" -O SKILL.md 2>/dev/null || true`,
            { maxBuffer: 10 * 1024 * 1024 },
          )
          content = stdout.toString('utf-8')
        } catch {
          // No SKILL.md in archive — that's ok
        }
      }

      if (!content && !options.content) {
        throw new Error(
          'No SKILL.md found in archive. Provide --content with the skill markdown text.',
        )
      }

      const apiUrl = normalizeApiUrl(config.get('apiUrl'))
      const apiKey = (await credentials.getApiKey()) || ''

      let uploadUrl: string
      if (isProject) {
        const pid = await resolveProjectId(options.project!)
        uploadUrl = `/projects/${pid}/skills/upload`
      } else {
        uploadUrl = '/settings/skills/upload'
      }

      return uploadSkillMultipart(apiUrl, apiKey, uploadUrl, {
        slug: options.slug,
        name: options.name,
        description: options.description,
        content,
        archive,
      })
    })

    success(`Skill created: ${skill.name} (${skill.slug}) [with archive]`)
    return
  }

  // Simple content-based skill (JSON)
  if (!options.content) {
    warning('Provide --content with skill markdown text, @file, @directory, or @archive.tar.gz')
    return
  }

  const content = resolveValue(options.content)

  const skill = await withSpinner('Creating skill...', async () => {
    let url: string
    let pathParams: Record<string, unknown> = {}

    if (isProject) {
      const pid = await resolveProjectId(options.project!)
      url = '/projects/{project_id}/skills'
      pathParams = { project_id: pid }
    } else {
      url = '/settings/skills'
    }

    const { data, error } = await client.post({
      url,
      path: pathParams,
      body: {
        slug: options.slug,
        name: options.name,
        description: options.description || undefined,
        content,
      },
    })
    if (error) throw new Error(getErrorMessage(error))
    return data as SkillDefinition
  })

  success(`Skill created: ${skill.name} (${skill.slug})`)
}

async function updateAction(
  slug: string,
  options: UpdateOptions,
): Promise<void> {
  await requireAuth()
  await setupClient()

  const content = options.content
    ? resolveValue(options.content)
    : undefined
  const isProject = !!options.project

  const body: Record<string, unknown> = {}
  if (options.name) body.name = options.name
  if (options.description !== undefined) body.description = options.description
  if (content) body.content = content

  if (Object.keys(body).length === 0) {
    warning(
      'No fields to update. Provide at least one of --name, --content, or --description',
    )
    return
  }

  const skill = await withSpinner('Updating skill...', async () => {
    let url: string
    let pathParams: Record<string, unknown> = {}

    if (isProject) {
      const pid = await resolveProjectId(options.project!)
      url = '/projects/{project_id}/skills/{slug}'
      pathParams = { project_id: pid, slug }
    } else {
      url = '/settings/skills/{slug}'
      pathParams = { slug }
    }

    const { data, error } = await client.put({ url, path: pathParams, body })
    if (error) throw new Error(getErrorMessage(error))
    return data as SkillDefinition
  })

  success(`Skill updated: ${skill.name} (${skill.slug})`)
}

async function deleteAction(
  slug: string,
  options: DeleteOptions,
): Promise<void> {
  await requireAuth()
  await setupClient()

  const skipConfirmation = options.force || options.yes

  if (!skipConfirmation) {
    const confirmed = await promptConfirm({
      message: `Delete skill "${slug}"? This cannot be undone.`,
      default: false,
    })
    if (!confirmed) {
      info('Cancelled')
      return
    }
  }

  const isProject = !!options.project

  await withSpinner('Deleting skill...', async () => {
    let url: string
    let pathParams: Record<string, unknown> = {}

    if (isProject) {
      const pid = await resolveProjectId(options.project!)
      url = '/projects/{project_id}/skills/{slug}'
      pathParams = { project_id: pid, slug }
    } else {
      url = '/settings/skills/{slug}'
      pathParams = { slug }
    }

    const { error } = await client.delete({ url, path: pathParams })
    if (error) throw new Error(getErrorMessage(error))
  })

  success(`Skill "${slug}" deleted`)
}
