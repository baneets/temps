import { requireAuth, config } from '../../config/store.js'
import { promptText, promptConfirm, promptSelect, type SelectOption } from '../../ui/prompts.js'
import { withSpinner } from '../../ui/spinner.js'
import {
  success,
  error,
  newline,
  icons,
  colors,
  keyValue,
  header,
  info,
  warning,
} from '../../ui/output.js'
import { setupClient, client, getErrorMessage } from '../../lib/api-client.js'
import { createProject } from '../../api/sdk.gen.js'
import type { RepositoryResponse } from '../../api/types.gen.js'
import { readEnvFile, findEnvFiles } from '../../lib/env-file.js'

// Shared utilities (extracted to avoid duplication with setup wizard)
import {
  selectGitConnection,
  selectRepository,
  selectBranch,
  detectAndSelectPreset,
  findRepositoryByName,
  fetchGitConnections,
} from '../../lib/git-connection.js'
import { selectStorageServices } from '../../lib/service-setup.js'

interface CreateOptions {
  name?: string
  branch?: string
  directory?: string
  preset?: string
  connection?: string
  repo?: string
  yes?: boolean
}

export async function create(options: CreateOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const skipPrompts = options.yes ?? false

  newline()
  console.log(colors.bold(`${icons.sparkles} Create New Project`))
  console.log(colors.muted('─'.repeat(40)))
  newline()

  try {
    // Step 1: Select Git Connection
    let connection
    if (options.connection) {
      // Resolve connection by ID
      const connections = await fetchGitConnections()
      const connId = parseInt(options.connection, 10)
      connection = connections.find((c) => c.id === connId)
      if (!connection) {
        error(`Git connection with ID ${options.connection} not found.`)
        return
      }
      info(`Using git connection: ${connection.account_name}`)
    } else if (options.repo && skipPrompts) {
      // Auto-find the connection that has this repo
      const parts = options.repo.split('/')
      if (parts.length !== 2 || !parts[0] || !parts[1]) {
        error('Repository must be in owner/name format (e.g., myorg/myrepo)')
        return
      }
      const connections = await fetchGitConnections()
      for (const conn of connections) {
        const repo = await findRepositoryByName(conn.id, parts[0], parts[1])
        if (repo) {
          connection = conn
          info(`Auto-selected git connection: ${conn.account_name}`)
          break
        }
      }
      if (!connection) {
        error(`Repository "${options.repo}" not found in any git connection.`)
        return
      }
    } else {
      connection = await selectGitConnection()
    }
    if (!connection) {
      error('No git connection selected. Please set up a git provider first.')
      return
    }

    // Step 2: Select Repository
    let repository
    if (options.repo) {
      // Parse owner/name format
      const parts = options.repo.split('/')
      if (parts.length !== 2 || !parts[0] || !parts[1]) {
        error('Repository must be in owner/name format (e.g., myorg/myrepo)')
        return
      }
      repository = await findRepositoryByName(connection.id, parts[0], parts[1])
      if (!repository) {
        error(`Repository "${options.repo}" not found in connection "${connection.account_name}".`)
        return
      }
      info(`Using repository: ${repository.owner}/${repository.name}`)
    } else {
      repository = await selectRepository(connection.id)
    }
    if (!repository) {
      error('No repository selected.')
      return
    }

    // Step 3: Select Branch
    let branch: string
    if (options.branch) {
      branch = options.branch
      info(`Using branch: ${branch}`)
    } else {
      branch = await selectBranch(connection.id, repository)
    }

    // Step 4: Detect and Select Preset
    let preset: string
    let directory: string
    if (options.preset) {
      preset = options.preset
      directory = options.directory || './'
      info(`Using preset: ${preset}, directory: ${directory}`)
    } else {
      const detected = await detectAndSelectPreset(repository.id, branch)
      preset = detected.preset
      directory = detected.directory
    }

    // Step 5: Configure Project Name
    let projectName: string
    if (options.name) {
      projectName = options.name
      info(`Using project name: ${projectName}`)
    } else {
      projectName = await configureProjectName(repository, directory)
    }

    // Step 6: Select Storage Services (skip with --yes)
    const serviceIds = skipPrompts ? [] : await selectStorageServices()

    // Step 7: Configure Environment Variables (skip with --yes)
    const envVars = skipPrompts ? [] : await configureEnvironmentVariables()

    // Step 8: Create the Project
    const project = await withSpinner('Creating project...', async () => {
      const { data, error: apiError } = await createProject({
        client,
        body: {
          name: projectName,
          main_branch: branch,
          directory: directory,
          preset: preset,
          repo_name: repository.name,
          repo_owner: repository.owner,
          git_url: repository.clone_url || repository.ssh_url || '',
          git_provider_connection_id: connection.id,
          automatic_deploy: true,
          source_type: 'git',
          storage_service_ids: serviceIds,
          environment_variables: envVars.length > 0 ? envVars : undefined,
        },
      })

      if (apiError || !data) {
        throw new Error(getErrorMessage(apiError) || 'Failed to create project')
      }

      return data
    })

    // Display success
    newline()
    header(`${icons.check} Project Created Successfully`)
    newline()

    keyValue('ID', project.id)
    keyValue('Name', project.name)
    keyValue('Slug', project.slug)
    keyValue('Repository', `${repository.owner}/${repository.name}`)
    keyValue('Branch', project.main_branch)
    keyValue('Directory', project.directory)
    keyValue('Preset', preset)
    if (serviceIds.length > 0) {
      keyValue('Services', `${serviceIds.length} linked`)
    }
    if (envVars.length > 0) {
      keyValue('Environment Variables', `${envVars.length} configured`)
    }

    newline()

    // Ask if user wants to set as default (auto-set with --yes)
    if (skipPrompts) {
      config.set('defaultProject', project.slug)
      success(`Default project set to "${project.slug}"`)
    } else {
      const setDefault = await promptConfirm({
        message: 'Set as default project?',
        default: true,
      })

      if (setDefault) {
        config.set('defaultProject', project.slug)
        success(`Default project set to "${project.slug}"`)
      }
    }

    newline()
    info(`View your project: temps projects show ${project.slug}`)
    info(`Deploy your project: temps deploy -p ${project.slug}`)
  } catch (err) {
    error(getErrorMessage(err))
  }
}

/**
 * Step 5: Configure Project Name
 */
async function configureProjectName(
  repository: RepositoryResponse,
  directory: string
): Promise<string> {
  // Generate default name from repo and directory
  let defaultName = repository.name

  // If directory is not root, append it
  if (directory && directory !== './' && directory !== '.' && directory !== 'root') {
    const cleanDir = directory.replace(/^\.\//, '').replace(/\//g, '-').replace(/[^a-zA-Z0-9-]/g, '')
    if (cleanDir) {
      defaultName = `${repository.name}-${cleanDir}`
    }
  }

  newline()
  return await promptText({
    message: 'Project name',
    default: defaultName,
    required: true,
    validate: (v) => (v.length >= 2 ? true : 'Name must be at least 2 characters'),
  })
}

/**
 * Step 7: Configure Environment Variables
 */
async function configureEnvironmentVariables(): Promise<[string, string][]> {
  newline()

  const addEnvVars = await promptConfirm({
    message: 'Add environment variables?',
    default: false,
  })

  if (!addEnvVars) {
    return []
  }

  const envVars: [string, string][] = []

  // Check for .env files in the current directory
  const envFiles = findEnvFiles()

  // Build method choices
  const methodChoices: SelectOption<string>[] = []

  if (envFiles.length > 0) {
    methodChoices.push({
      name: `Import from file (${envFiles.join(', ')} found)`,
      value: 'file',
      description: 'Load variables from a .env file',
    })
  }

  methodChoices.push(
    {
      name: 'Enter manually',
      value: 'manual',
      description: 'Type key-value pairs one by one',
    },
    {
      name: 'Specify file path',
      value: 'path',
      description: 'Provide a custom path to a .env file',
    },
  )

  const method = methodChoices.length === 1
    ? 'manual'
    : await promptSelect({ message: 'How to add variables?', choices: methodChoices })

  if (method === 'file' || method === 'path') {
    let filePath: string

    if (method === 'file') {
      if (envFiles.length === 1) {
        filePath = envFiles[0]!
      } else {
        filePath = await promptSelect({
          message: 'Select .env file',
          choices: envFiles.map((f) => ({ name: f, value: f })),
        })
      }
    } else {
      filePath = await promptText({
        message: 'Path to .env file',
        default: '.env',
        required: true,
      })
    }

    const parsed = readEnvFile(filePath)

    if (!parsed || Object.keys(parsed).length === 0) {
      warning(`No variables found in ${filePath}`)
    } else {
      const entries = Object.entries(parsed)
      newline()
      info(`Found ${entries.length} variable(s) in ${colors.bold(filePath)}:`)
      newline()

      for (const [key, value] of entries) {
        const masked = value.length > 30 ? `${value.substring(0, 30)}...` : value
        keyValue(`  ${key}`, colors.muted(masked))
      }

      newline()
      const confirm = await promptConfirm({
        message: `Import ${entries.length} variable(s)?`,
        default: true,
      })

      if (confirm) {
        for (const [key, value] of entries) {
          envVars.push([key, value])
        }
        success(`Imported ${entries.length} variable(s) from ${filePath}`)
      }
    }
  }

  // Manual entry (either as primary method or to add more after file import)
  if (method === 'manual' || envVars.length > 0) {
    const shouldAddManual = method === 'manual' || await promptConfirm({
      message: 'Add more variables manually?',
      default: false,
    })

    if (shouldAddManual) {
      let addMore = true
      while (addMore) {
        newline()
        const key = await promptText({
          message: 'Variable name (e.g., DATABASE_URL)',
          required: true,
          validate: (v) => {
            if (!v) return 'Variable name is required'
            if (!/^[A-Z_][A-Z0-9_]*$/i.test(v)) {
              return 'Variable name must start with a letter or underscore and contain only letters, numbers, and underscores'
            }
            return true
          },
        })

        const value = await promptText({
          message: `Value for ${key}`,
          required: true,
        })

        envVars.push([key, value])

        addMore = await promptConfirm({
          message: 'Add another variable?',
          default: false,
        })
      }
    }
  }

  return envVars
}
