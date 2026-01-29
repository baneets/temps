import { requireAuth, config, credentials } from '../../config/store.js'
import { setupClient, client } from '../../lib/api-client.js'
import { watchDeployment } from '../../lib/deployment-watcher.jsx'
import { getProjectBySlug, getProject, getEnvironments } from '../../api/sdk.gen.js'
import type { EnvironmentResponse } from '../../api/types.gen.js'
import { promptSelect } from '../../ui/prompts.js'
import {
  startSpinner,
  succeedSpinner,
  failSpinner,
  updateSpinner,
} from '../../ui/spinner.js'
import {
  success,
  info,
  warning,
  newline,
  icons,
  colors,
  box,
} from '../../ui/output.js'
import { spawn } from 'node:child_process'
import { existsSync, createWriteStream } from 'node:fs'
import { unlink } from 'node:fs/promises'
import { resolve, basename, join } from 'node:path'
import { tmpdir } from 'node:os'

interface DeployLocalImageOptions {
  image?: string
  dockerfile?: string
  context?: string
  buildArg?: string[]
  noBuild?: boolean
  project?: string
  environment?: string
  environmentId?: string
  tag?: string
  wait?: boolean
  yes?: boolean
  metadata?: string
  timeout?: string
}

export async function deployLocalImage(options: DeployLocalImageOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  newline()

  let imageName: string
  let didBuild = false

  // Determine if we should build or use existing image
  const shouldBuild = !options.noBuild && !options.image

  if (shouldBuild) {
    // Build mode: build from Dockerfile
    const dockerfilePath = options.dockerfile || 'Dockerfile'
    const contextPath = options.context || '.'
    const resolvedDockerfile = resolve(dockerfilePath)
    const resolvedContext = resolve(contextPath)

    // Check if Dockerfile exists
    if (!existsSync(resolvedDockerfile)) {
      warning(`Dockerfile not found: ${resolvedDockerfile}`)
      info('Options:')
      info('  1. Create a Dockerfile in the current directory')
      info('  2. Specify a Dockerfile path: --dockerfile <path>')
      info('  3. Skip build and use existing image: --image <image-name>')
      return
    }

    // Generate image tag if not provided
    const projectName = options.project ?? config.get('defaultProject')
    const dirName = basename(process.cwd())
    const timestamp = Date.now()
    imageName = options.tag || `${projectName || dirName}:local-${timestamp}`

    // Show build preview
    newline()
    box(
      `Dockerfile: ${colors.bold(resolvedDockerfile)}\n` +
        `Context: ${colors.bold(resolvedContext)}\n` +
        `Image Tag: ${colors.bold(imageName)}` +
        (options.buildArg?.length ? `\nBuild Args: ${colors.bold(options.buildArg.join(', '))}` : ''),
      `${icons.package} Docker Build`
    )
    newline()

    // Build the image
    startSpinner('Building Docker image...')

    try {
      await dockerBuild({
        dockerfile: resolvedDockerfile,
        context: resolvedContext,
        tag: imageName,
        buildArgs: options.buildArg || [],
        onOutput: (line) => {
          // Show build progress
          const trimmed = line.trim()
          if (trimmed) {
            updateSpinner(`Building: ${trimmed.substring(0, 60)}${trimmed.length > 60 ? '...' : ''}`)
          }
        },
      })
      succeedSpinner(`Image built: ${imageName}`)
      didBuild = true
    } catch (err) {
      failSpinner('Docker build failed')
      if (err instanceof Error) {
        warning(err.message)
      }
      return
    }
  } else if (options.image) {
    // Use existing image
    imageName = options.image

    // Verify image exists locally
    startSpinner('Verifying local Docker image...')

    const imageExists = await checkImageExists(imageName)
    if (!imageExists) {
      failSpinner(`Image "${imageName}" not found locally`)
      info('Make sure the image exists by running: docker images')
      info('Or remove --image to build from Dockerfile')
      return
    }

    succeedSpinner(`Found local image: ${imageName}`)
  } else {
    // No image and --no-build specified
    warning('No image specified and build is disabled')
    info('Use: temps deploy:local-image --image <image-name>')
    info('Or remove --no-build to build from Dockerfile')
    return
  }

  // Get image size for display
  const imageSize = await getImageSize(imageName)
  info(`Image size: ${formatFileSize(imageSize)}`)

  // Get project name
  const projectName = options.project ?? config.get('defaultProject')

  if (!projectName) {
    warning('No project specified')
    info(
      'Use: temps deploy:local-image --project <project> or set a default with temps configure'
    )
    return
  }

  // Fetch project details
  startSpinner('Fetching project details...')

  let projectData: { id: number; name: string; slug: string }
  let environments: EnvironmentResponse[] = []

  try {
    // Check if projectName is a numeric ID
    const isNumericId = /^\d+$/.test(projectName)

    if (isNumericId) {
      // Fetch by numeric ID
      const result = await getProject({
        client,
        path: { id: parseInt(projectName, 10) },
      })

      if (result.error || !result.data) {
        failSpinner(`Project with ID "${projectName}" not found`)
        info(`Debug: ${JSON.stringify(result)}`)
        return
      }

      // Handle potential wrapped response
      const responseData = result.data as Record<string, unknown>
      if (responseData.id !== undefined) {
        projectData = result.data as { id: number; name: string; slug: string }
      } else if (responseData.data && typeof responseData.data === 'object') {
        projectData = responseData.data as { id: number; name: string; slug: string }
      } else {
        failSpinner(`Unexpected project response format`)
        info(`Debug: ${JSON.stringify(result.data)}`)
        return
      }
    } else {
      // Fetch by slug
      const { data, error } = await getProjectBySlug({
        client,
        path: { slug: projectName },
      })

      if (error || !data) {
        failSpinner(`Project "${projectName}" not found`)
        return
      }

      projectData = data
    }

    succeedSpinner(`Found project: ${projectData.name || projectData.slug}`)

    // Fetch environments
    const { data: envData } = await getEnvironments({
      client,
      path: { project_id: projectData.id },
    })

    // Handle different response formats - could be array directly or wrapped in object
    if (Array.isArray(envData)) {
      environments = envData
    } else if (envData && typeof envData === 'object') {
      // Try common wrapper properties
      const wrapped = envData as Record<string, unknown>
      if (Array.isArray(wrapped.data)) {
        environments = wrapped.data as EnvironmentResponse[]
      } else if (Array.isArray(wrapped.environments)) {
        environments = wrapped.environments as EnvironmentResponse[]
      }
    }
  } catch (err) {
    failSpinner('Failed to fetch project')
    throw err
  }

  // Get environment
  let environmentId: number | undefined
  let environmentName = options.environment || 'production'

  // If environment ID is specified directly, use it without lookup
  if (options.environmentId) {
    environmentId = parseInt(options.environmentId, 10)
    // Try to find the name from environments list if available
    if (environments.length > 0) {
      const env = environments.find((e) => e.id === environmentId)
      if (env) {
        environmentName = env.name
      }
    } else {
      environmentName = `Environment #${environmentId}`
    }
  } else if (environments.length > 0) {
    if (options.environment) {
      // Find by name
      const env = environments.find((e) => e.name === options.environment)
      if (env) {
        environmentId = env.id
        environmentName = env.name
      }
    } else if (!options.yes) {
      // Interactive: prompt for environment selection
      const selectedEnv = await promptSelect({
        message: 'Select environment',
        choices: environments.map((env) => ({
          name: env.name,
          value: String(env.id),
          description: env.is_preview ? 'Preview environment' : undefined,
        })),
        default: String(
          environments.find((e) => e.name === 'production')?.id ??
            environments[0]?.id ??
            ''
        ),
      })
      environmentId = parseInt(selectedEnv, 10)
      environmentName =
        environments.find((e) => e.id === environmentId)?.name ?? 'production'
    } else {
      // Non-interactive: use production or first environment
      const prodEnv = environments.find((e) => e.name === 'production')
      if (prodEnv) {
        environmentId = prodEnv.id
        environmentName = prodEnv.name
      } else if (environments[0]) {
        environmentId = environments[0].id
        environmentName = environments[0].name
      }
    }
  } else if (!options.environmentId) {
    warning('No environments found for this project')
    info('Create an environment first or specify --environment-id directly')
    return
  }

  // Show deployment preview
  newline()
  box(
    `Project: ${colors.bold(projectName)}\n` +
      `Environment: ${colors.bold(environmentName)}\n` +
      `Image: ${colors.bold(imageName)}\n` +
      `Size: ${colors.bold(formatFileSize(imageSize))}` +
      (didBuild ? `\n${colors.dim('(freshly built)')}` : ''),
    `${icons.rocket} Deploy Local Image`
  )
  newline()

  // Export image using docker save to a temp file
  const tempFilename = `temps-image-${Date.now()}.tar`
  const tempFilePath = join(tmpdir(), tempFilename)

  startSpinner('Exporting image with docker save...')

  let exportedSize: number
  try {
    exportedSize = await dockerSaveToFile(imageName, tempFilePath, (progress) => {
      updateSpinner(`Exporting image... ${formatFileSize(progress)} saved`)
    })
    succeedSpinner(`Image exported: ${formatFileSize(exportedSize)}`)
  } catch (err) {
    failSpinner('Failed to export image')
    if (err instanceof Error) {
      warning(err.message)
    }
    return
  }

  // Check size limit (1GB)
  const MAX_SIZE = 1024 * 1024 * 1024
  if (exportedSize > MAX_SIZE) {
    warning(`Image size (${formatFileSize(exportedSize)}) exceeds maximum allowed (1 GB)`)
    await unlink(tempFilePath).catch(() => {})
    return
  }

  // Upload to server
  startSpinner('Uploading image to server...')

  const apiUrl = config.get('apiUrl')
  const apiKey = await credentials.getApiKey()

  try {
    const uploadUrl = `${apiUrl}/projects/${projectData.id}/environments/${environmentId}/deploy/image-upload`

    // Build query string for tag
    const queryParams = new URLSearchParams()
    if (options.tag) {
      queryParams.set('tag', options.tag)
    }
    const finalUrl = queryParams.toString()
      ? `${uploadUrl}?${queryParams.toString()}`
      : uploadUrl

    // Stream file upload using Bun's native file streaming
    const file = Bun.file(tempFilePath)
    const filename = `${imageName.replace(/[/:]/g, '-')}.tar`

    // Create multipart boundary
    const boundary = `----BunFormBoundary${Date.now().toString(16)}`

    // Build multipart body as a stream
    const fileStream = file.stream()

    // Multipart header and footer
    const header = [
      `--${boundary}`,
      `Content-Disposition: form-data; name="file"; filename="${filename}"`,
      'Content-Type: application/x-tar',
      '',
      '',
    ].join('\r\n')

    const footer = `\r\n--${boundary}--\r\n`

    const headerBytes = new TextEncoder().encode(header)
    const footerBytes = new TextEncoder().encode(footer)

    // Create a combined stream: header + file + footer
    let uploadedBytes = 0

    const combinedStream = new ReadableStream({
      async start(controller) {
        // Send header
        controller.enqueue(headerBytes)

        // Stream file chunks
        const reader = fileStream.getReader()
        try {
          while (true) {
            const { done, value } = await reader.read()
            if (done) break
            uploadedBytes += value.byteLength
            updateSpinner(`Uploading... ${formatFileSize(uploadedBytes)} / ${formatFileSize(exportedSize)}`)
            controller.enqueue(value)
          }
        } finally {
          reader.releaseLock()
        }

        // Send footer
        controller.enqueue(footerBytes)
        controller.close()
      },
    })

    const uploadResponse = await fetch(finalUrl, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${apiKey}`,
        'Content-Type': `multipart/form-data; boundary=${boundary}`,
      },
      body: combinedStream,
      duplex: 'half',
    } as RequestInit)

    // Clean up temp file
    await unlink(tempFilePath).catch(() => {})

    if (!uploadResponse.ok) {
      const errorText = await uploadResponse.text()
      failSpinner(`Upload failed: ${uploadResponse.status}`)
      info(`URL: ${finalUrl}`)
      warning(`Response: ${errorText}`)
      return
    }

    const responseText = await uploadResponse.text()
    let deployment: { id: number; slug: string }
    try {
      deployment = JSON.parse(responseText) as { id: number; slug: string }
    } catch (parseErr) {
      failSpinner('Failed to parse deployment response')
      info(`URL: ${finalUrl}`)
      warning(`Response: ${responseText}`)
      return
    }

    succeedSpinner(`Deployment started: ${deployment.slug}`)

    // Wait for completion if requested
    if (options.wait !== false) {
      const result = await watchDeployment({
        projectId: projectData.id,
        deploymentId: deployment.id,
        timeoutSecs: parseInt(options.timeout || '600', 10), // Longer default for image uploads
        projectName,
      })

      if (!result.success) {
        // Exit with error code for CI/CD
        process.exitCode = 1
      }
    } else {
      newline()
      info('Deployment running in background')
      info(`Check status with: temps deployments list --project ${projectName}`)
      newline()
      success('Local image deployment initiated successfully!')
      newline()
    }
  } catch (err) {
    failSpinner('Deployment failed')
    throw err
  }
}

interface DockerBuildOptions {
  dockerfile: string
  context: string
  tag: string
  buildArgs: string[]
  onOutput?: (line: string) => void
}

async function dockerBuild(options: DockerBuildOptions): Promise<void> {
  return new Promise((resolve, reject) => {
    const args = [
      'build',
      '-f', options.dockerfile,
      '-t', options.tag,
    ]

    // Add build args
    for (const arg of options.buildArgs) {
      args.push('--build-arg', arg)
    }

    // Add context path
    args.push(options.context)

    const docker = spawn('docker', args, {
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    let stderr = ''

    docker.stdout.on('data', (chunk: Buffer) => {
      const lines = chunk.toString().split('\n')
      for (const line of lines) {
        if (line.trim() && options.onOutput) {
          options.onOutput(line)
        }
      }
    })

    docker.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString()
      // Docker build outputs progress to stderr
      const lines = chunk.toString().split('\n')
      for (const line of lines) {
        if (line.trim() && options.onOutput) {
          options.onOutput(line)
        }
      }
    })

    docker.on('close', (code) => {
      if (code === 0) {
        resolve()
      } else {
        reject(new Error(`docker build failed with code ${code}:\n${stderr}`))
      }
    })

    docker.on('error', (err) => {
      reject(new Error(`Failed to spawn docker: ${err.message}`))
    })
  })
}

async function checkImageExists(imageName: string): Promise<boolean> {
  return new Promise((resolve) => {
    const docker = spawn('docker', ['image', 'inspect', imageName], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    docker.on('close', (code) => {
      resolve(code === 0)
    })

    docker.on('error', () => {
      resolve(false)
    })
  })
}

async function getImageSize(imageName: string): Promise<number> {
  return new Promise((resolve) => {
    const docker = spawn('docker', ['image', 'inspect', '--format', '{{.Size}}', imageName], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    let output = ''

    docker.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString()
    })

    docker.on('close', (code) => {
      if (code === 0) {
        const size = parseInt(output.trim(), 10)
        resolve(isNaN(size) ? 0 : size)
      } else {
        resolve(0)
      }
    })

    docker.on('error', () => {
      resolve(0)
    })
  })
}

async function dockerSaveToFile(
  imageName: string,
  outputPath: string,
  onProgress?: (bytesWritten: number) => void
): Promise<number> {
  return new Promise((resolve, reject) => {
    let totalBytes = 0

    const docker = spawn('docker', ['save', imageName], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    const writeStream = createWriteStream(outputPath)

    docker.stdout.on('data', (chunk: Buffer) => {
      writeStream.write(chunk)
      totalBytes += chunk.length
      if (onProgress) {
        onProgress(totalBytes)
      }
    })

    let stderr = ''
    docker.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString()
    })

    docker.on('close', (code) => {
      writeStream.end()
      if (code === 0) {
        resolve(totalBytes)
      } else {
        reject(new Error(`docker save failed with code ${code}: ${stderr}`))
      }
    })

    docker.on('error', (err) => {
      writeStream.end()
      reject(new Error(`Failed to spawn docker: ${err.message}`))
    })

    writeStream.on('error', (err) => {
      reject(new Error(`Failed to write to file: ${err.message}`))
    })
  })
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
}
