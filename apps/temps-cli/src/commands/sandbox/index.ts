import type { Command } from 'commander'
import { requireAuth, credentials } from '../../config/store.js'
import { normalizeApiUrl } from '../../lib/api-client.js'
import { config } from '../../config/store.js'
import { withSpinner } from '../../ui/spinner.js'
import { printTable, type TableColumn } from '../../ui/table.js'
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
  keyValue,
} from '../../ui/output.js'

// ── Types mirroring /v1/sandbox/* responses ─────────────────────────────────

interface SandboxResponse {
  id: string
  name: string
  status: string
  image: string | null
  work_dir: string
  created_at: string
  expires_at: string
  preview_password_hint?: string | null
}

interface SetPreviewPasswordResponse {
  preview_password_hint: string
}

interface ListSandboxesResponse {
  items: SandboxResponse[]
  total: number
  page: number
  page_size: number
}

interface ExecResponse {
  exit_code: number
  stdout: string
  stderr: string
}

interface ExecDetachedResponse {
  job_id: string
}

interface ReadFileResponse {
  path: string
  contents_b64: string
  size: number
}

interface StatResponse {
  path: string
  exists: boolean
  is_dir: boolean
  is_file: boolean
  size: number
}

interface DomainResponse {
  url: string
}

// ── Fetch helpers ───────────────────────────────────────────────────────────

interface SandboxApi {
  baseUrl: string
  apiKey: string
}

async function auth(): Promise<SandboxApi> {
  await requireAuth()
  const apiKey = await credentials.getApiKey()
  if (!apiKey) {
    throw new Error('Not authenticated. Run `temps login` first.')
  }
  const baseUrl = normalizeApiUrl(config.get('apiUrl'))
  return { baseUrl, apiKey }
}

function sandboxUrl(api: SandboxApi, path: string): string {
  return `${api.baseUrl}/v1/sandbox${path}`
}

async function apiRequest<T>(
  api: SandboxApi,
  path: string,
  init: RequestInit = {},
): Promise<T> {
  const headers = new Headers(init.headers)
  headers.set('Authorization', `Bearer ${api.apiKey}`)
  if (init.body && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json')
  }

  const response = await fetch(sandboxUrl(api, path), { ...init, headers })

  if (!response.ok) {
    throw await readApiError(response)
  }

  // 204 No Content — caller asked for T but there's no body.
  if (response.status === 204) {
    return undefined as T
  }

  return (await response.json()) as T
}

async function readApiError(response: Response): Promise<Error> {
  const text = await response.text().catch(() => '')
  try {
    const problem = JSON.parse(text) as { title?: string; detail?: string }
    const title = problem.title ?? `HTTP ${response.status}`
    const detail = problem.detail ? ` — ${problem.detail}` : ''
    return new Error(`${title}${detail}`)
  } catch {
    return new Error(`HTTP ${response.status}: ${text || response.statusText}`)
  }
}

/**
 * Parse repeated `--env KEY=VAL` options into an object. Values may
 * contain `=` (only the first `=` splits).
 */
function parseEnvPairs(pairs: string[] | undefined): Record<string, string> {
  const out: Record<string, string> = {}
  if (!pairs) return out
  for (const p of pairs) {
    const idx = p.indexOf('=')
    if (idx <= 0) {
      throw new Error(`Invalid --env '${p}': expected KEY=VAL`)
    }
    const key = p.slice(0, idx)
    const value = p.slice(idx + 1)
    out[key] = value
  }
  return out
}

function statusColor(status: string): string {
  const s = status.toLowerCase()
  if (s === 'running') return colors.success(status)
  if (s === 'stopped' || s === 'destroyed') return colors.error(status)
  if (s === 'pending' || s === 'creating') return colors.warning(status)
  return status
}

/**
 * Generate a URL-safe preview password on the client. Uses
 * `crypto.getRandomValues` over a 64-symbol alphabet, so each character
 * carries 6 bits of entropy — 24 chars gives ~144 bits, comfortably past
 * brute-force range. Kept in sync with `web/src/components/sandboxes/
 * SandboxPreviewPasswordCard.tsx` so UI + CLI produce the same shape.
 *
 * Clamped to the server's [8, 256] range to surface typos early instead
 * of as a 400 round-trip.
 */
function generatePassword(length = 24): string {
  if (length < 8 || length > 256) {
    throw new Error('Password length must be between 8 and 256 characters')
  }
  const alphabet =
    'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'
  const buf = new Uint8Array(length)
  crypto.getRandomValues(buf)
  let out = ''
  for (let i = 0; i < length; i++) {
    out += alphabet[buf[i]! % alphabet.length]
  }
  return out
}

function base64Encode(bytes: Uint8Array): string {
  // Bun/Node both support Buffer.
  return Buffer.from(bytes).toString('base64')
}

function base64Decode(b64: string): Uint8Array {
  return new Uint8Array(Buffer.from(b64, 'base64'))
}

// ── Commander wiring ────────────────────────────────────────────────────────

export function registerSandboxCommands(program: Command): void {
  const sandbox = program
    .command('sandbox')
    .description('Manage standalone sandboxes (/v1/sandbox API)')

  sandbox
    .command('create')
    .description('Create a new sandbox')
    .option('--image <image>', 'Docker image override (uses platform default when omitted)')
    .option('--name <name>', 'Display name for the sandbox')
    .option('--timeout <seconds>', 'Idle timeout in seconds (clamped to [60, 86400])')
    .option(
      '-e, --env <KEY=VAL>',
      'Env var baked into the container (repeatable)',
      (val: string, prev: string[]) => (prev ? [...prev, val] : [val]),
    )
    .option('--cpu-limit <cpu>', 'CPU limit (e.g., 0.5 for half a core)')
    .option('--memory-mb <mb>', 'Memory limit in megabytes')
    .option('--git-url <url>', 'Git repo URL to clone into the work dir')
    .option('--git-rev <revision>', 'Git revision to check out (requires --git-url)')
    .option('--git-depth <n>', 'Shallow clone depth (requires --git-url)')
    .option(
      '--git-connection <id>',
      'ID of a stored git provider connection; temps injects the token server-side',
    )
    .option('--git-username <user>', 'HTTP Basic username for private repo clone (requires --git-password)')
    .option('--git-password <token>', 'HTTP Basic password/token (paired with --git-username; injected via GIT_ASKPASS)')
    .option('--tarball-url <url>', 'Tarball URL to download and extract')
    .option(
      '--preview-password',
      'Generate a random preview-URL password and print it once on stdout',
    )
    .option(
      '--preview-password-length <n>',
      'Length of the generated preview password (8..=256, default 24)',
    )
    .option('--json', 'Output as JSON')
    .action(createAction)

  sandbox
    .command('list')
    .alias('ls')
    .description('List your sandboxes')
    .option('--page <n>', 'Page (1-indexed)')
    .option('--page-size <n>', 'Items per page (default 20, max 100)')
    .option('--json', 'Output as JSON')
    .action(listAction)

  sandbox
    .command('show <id>')
    .description('Show details for a sandbox')
    .option('--json', 'Output as JSON')
    .action(showAction)

  sandbox
    .command('stop <id>')
    .alias('rm')
    .description('Stop and destroy a sandbox (alias: destroy)')
    .option('-f, --force', 'Skip confirmation prompt')
    .action(stopAction)

  sandbox
    .command('destroy <id>')
    .description('Alias for `stop` — stop and destroy a sandbox permanently')
    .option('-f, --force', 'Skip confirmation prompt')
    .action(stopAction)

  sandbox
    .command('pause <id>')
    .description('Pause a running sandbox (non-destructive — resume later with `sandbox resume`)')
    .action(pauseAction)

  sandbox
    .command('resume <id>')
    .description('Resume a paused sandbox')
    .action(resumeAction)

  sandbox
    .command('restart <id>')
    .description('Restart a running sandbox (preserves filesystem)')
    .action(restartAction)

  sandbox
    .command('clone <id>')
    .description('Clone a git repo or extract a tarball into a running sandbox')
    .option('--git-url <url>', 'Git repo URL to clone')
    .option('--git-rev <revision>', 'Git revision (branch/tag/SHA) to check out')
    .option('--git-depth <n>', 'Shallow clone depth')
    .option(
      '--git-connection <id>',
      'ID of a stored git provider connection; temps injects the token server-side',
    )
    .option('--git-username <user>', 'HTTP Basic username (pairs with --git-password)')
    .option('--git-password <token>', 'HTTP Basic password/token (injected via GIT_ASKPASS)')
    .option('--tarball-url <url>', 'Tarball URL to download and extract')
    .action(cloneAction)

  sandbox
    .command('extend <id>')
    .description("Extend a sandbox's idle timeout")
    .requiredOption('--secs <seconds>', 'Extra seconds to add to the current expiry')
    .action(extendAction)

  sandbox
    .command('exec <id> [args...]')
    .description('Run a command inside a sandbox. Use `--` to pass flags: `exec ID -- ls -la`')
    .option('--detach', 'Start in background and print a job ID instead of waiting')
    .option('--cwd <path>', 'Working directory inside the sandbox')
    .option(
      '-e, --env <KEY=VAL>',
      'Env var for this exec (repeatable)',
      (val: string, prev: string[]) => (prev ? [...prev, val] : [val]),
    )
    .action(execAction)

  sandbox
    .command('logs <id> <jobId>')
    .description('Stream logs from a detached job (SSE)')
    .action(logsAction)

  sandbox
    .command('domain <id>')
    .description('Resolve the preview URL for a port inside a sandbox')
    .requiredOption('--port <port>', 'Port inside the sandbox (1..=65535)')
    .action(domainAction)

  sandbox
    .command('password <id>')
    .description(
      'Generate, rotate, or clear the preview-URL password for a sandbox',
    )
    .option(
      '--rotate',
      'Generate a new random password and set it (default when no flag is given)',
    )
    .option('--length <n>', 'Length of the generated password (8..=256, default 24)')
    .option('--clear', 'Remove the preview password — preview URLs become open again')
    .action(passwordAction)

  // ── Filesystem subgroup ──
  const fs = sandbox.command('fs').description('Filesystem operations inside a sandbox')

  fs.command('read <id>')
    .description('Read a file from the sandbox')
    .requiredOption('--path <path>', 'Absolute file path inside the sandbox')
    .option('--out <localPath>', 'Write to this local file (stdout when omitted)')
    .action(fsReadAction)

  fs.command('write <id>')
    .description('Write a file to the sandbox')
    .requiredOption('--path <path>', 'Absolute target path inside the sandbox')
    .option('--file <localPath>', 'Local source file to upload (mutually exclusive with --content)')
    .option('--content <string>', 'Inline string content to write')
    .option('--mode <octal>', 'Unix permission mask (default: 0644)')
    .action(fsWriteAction)

  fs.command('stat <id>')
    .description('Stat a path inside the sandbox')
    .requiredOption('--path <path>', 'Absolute path inside the sandbox')
    .option('--json', 'Output as JSON')
    .action(fsStatAction)

  fs.command('mkdir <id>')
    .description('Create a directory inside the sandbox (mkdir -p)')
    .requiredOption('--path <path>', 'Absolute path inside the sandbox')
    .action(fsMkdirAction)
}

// ── Actions ─────────────────────────────────────────────────────────────────

interface CreateOptions {
  image?: string
  name?: string
  timeout?: string
  env?: string[]
  cpuLimit?: string
  memoryMb?: string
  gitUrl?: string
  gitRev?: string
  gitDepth?: string
  gitConnection?: string
  gitUsername?: string
  gitPassword?: string
  tarballUrl?: string
  previewPassword?: boolean
  previewPasswordLength?: string
  json?: boolean
}

/**
 * Shared between `create --git-*` and `clone --git-*`. Returns the
 * `source` body field or `null` when no source was requested, and
 * throws on validation errors the server would also reject (mutual
 * exclusion, missing --git-url, etc.).
 */
function buildSource(options: {
  gitUrl?: string
  gitRev?: string
  gitDepth?: string
  gitConnection?: string
  gitUsername?: string
  gitPassword?: string
  tarballUrl?: string
}): Record<string, unknown> | null {
  const gitFlags = [
    options.gitRev,
    options.gitDepth,
    options.gitConnection,
    options.gitUsername,
    options.gitPassword,
  ].filter((v) => v !== undefined)
  if (gitFlags.length > 0 && !options.gitUrl) {
    throw new Error('--git-* flags require --git-url')
  }
  if (options.gitUrl && options.tarballUrl) {
    throw new Error('--git-url and --tarball-url are mutually exclusive')
  }
  if (
    (options.gitUsername || options.gitPassword) &&
    options.gitConnection !== undefined
  ) {
    throw new Error('--git-connection is mutually exclusive with --git-username/--git-password')
  }
  if (
    (options.gitUsername && !options.gitPassword) ||
    (!options.gitUsername && options.gitPassword)
  ) {
    throw new Error('--git-username and --git-password must be provided together')
  }

  if (options.gitUrl) {
    const src: Record<string, unknown> = { type: 'git', url: options.gitUrl }
    if (options.gitRev) src.revision = options.gitRev
    if (options.gitDepth !== undefined) {
      const d = Number(options.gitDepth)
      if (!Number.isInteger(d) || d <= 0) {
        throw new Error('--git-depth must be a positive integer')
      }
      src.depth = d
    }
    if (options.gitConnection !== undefined) {
      const id = Number(options.gitConnection)
      if (!Number.isInteger(id) || id <= 0) {
        throw new Error('--git-connection must be a positive integer')
      }
      src.git_connection_id = id
    }
    if (options.gitUsername) src.username = options.gitUsername
    if (options.gitPassword) src.password = options.gitPassword
    return src
  }

  if (options.tarballUrl) {
    return { type: 'tarball', url: options.tarballUrl }
  }

  return null
}

async function createAction(options: CreateOptions): Promise<void> {
  const api = await auth()
  const env = parseEnvPairs(options.env)

  const body: Record<string, unknown> = {}
  if (options.image) body.image = options.image
  if (options.name) body.name = options.name
  if (options.timeout !== undefined) body.timeout_secs = Number(options.timeout)
  if (Object.keys(env).length > 0) body.env = env
  if (options.cpuLimit !== undefined) body.cpu_limit = Number(options.cpuLimit)
  if (options.memoryMb !== undefined) body.memory_limit_mb = Number(options.memoryMb)

  const source = buildSource(options)
  if (source) body.source = source

  // Preview-password generation happens client-side so the plaintext
  // exists only on this machine: the server stores just an argon2 hash
  // and the 4-char hint. Printed once below — never retrievable later.
  let generatedPassword: string | undefined
  if (options.previewPassword) {
    const len =
      options.previewPasswordLength !== undefined
        ? Number(options.previewPasswordLength)
        : 24
    if (!Number.isInteger(len)) {
      throw new Error('--preview-password-length must be an integer')
    }
    generatedPassword = generatePassword(len)
    body.preview_password = generatedPassword
  }

  const sbx = await withSpinner('Creating sandbox...', () =>
    apiRequest<SandboxResponse>(api, '', {
      method: 'POST',
      body: JSON.stringify(body),
    }),
  )

  if (options.json) {
    // In JSON mode the generated plaintext is part of the payload so
    // scripts can capture it in one call. Caller is responsible for
    // handling it safely.
    json(generatedPassword ? { ...sbx, preview_password: generatedPassword } : sbx)
    return
  }

  success(`Sandbox ${colors.primary(sbx.id)} created`)
  keyValue('Name', sbx.name)
  keyValue('Status', statusColor(sbx.status))
  keyValue('Image', sbx.image ?? '(default)')
  keyValue('Work dir', sbx.work_dir)
  keyValue('Expires', sbx.expires_at)
  if (generatedPassword) {
    newline()
    warning('Preview password (shown once — copy it now):')
    console.log(`  ${colors.primary(generatedPassword)}`)
    if (sbx.preview_password_hint) {
      keyValue('Hint', `ends in …${sbx.preview_password_hint}`)
    }
  } else if (sbx.preview_password_hint) {
    keyValue('Preview password', `ends in …${sbx.preview_password_hint}`)
  }
  newline()
}

interface PasswordOptions {
  rotate?: boolean
  length?: string
  clear?: boolean
}

/**
 * Rotate or clear a sandbox's preview-URL password. The CLI generates
 * the new plaintext locally and sends it to
 * `PUT /v1/sandbox/{id}/preview-password`; the server only ever sees —
 * and persists — the argon2 hash plus the 4-char hint.
 *
 * The new password is printed exactly once. There is no retrieval path:
 * rotating again replaces it, and losing it before it's copied means
 * rotating a second time.
 */
async function passwordAction(
  id: string,
  options: PasswordOptions,
): Promise<void> {
  if (options.clear && (options.rotate || options.length)) {
    throw new Error('--clear is mutually exclusive with --rotate/--length')
  }

  const api = await auth()

  if (options.clear) {
    await withSpinner('Clearing preview password...', () =>
      apiRequest<void>(
        api,
        `/${encodeURIComponent(id)}/preview-password`,
        { method: 'DELETE' },
      ),
    )
    success(`Preview password cleared for ${colors.primary(id)}`)
    info('Preview URLs are now open — the sandbox ID is the only gate.')
    return
  }

  // Default behavior when no flag is given: rotate. Matches what the
  // command description says and avoids a silent no-op.
  const len = options.length !== undefined ? Number(options.length) : 24
  if (!Number.isInteger(len)) {
    throw new Error('--length must be an integer')
  }
  const password = generatePassword(len)

  const res = await withSpinner('Setting preview password...', () =>
    apiRequest<SetPreviewPasswordResponse>(
      api,
      `/${encodeURIComponent(id)}/preview-password`,
      {
        method: 'PUT',
        body: JSON.stringify({ password }),
      },
    ),
  )

  success(`Preview password set for ${colors.primary(id)}`)
  newline()
  warning('Preview password (shown once — copy it now):')
  console.log(`  ${colors.primary(password)}`)
  keyValue('Hint', `ends in …${res.preview_password_hint}`)
  newline()
}

interface ListOptions {
  page?: string
  pageSize?: string
  json?: boolean
}

async function listAction(options: ListOptions): Promise<void> {
  const api = await auth()

  const qs: string[] = []
  if (options.page) qs.push(`page=${encodeURIComponent(options.page)}`)
  if (options.pageSize) qs.push(`page_size=${encodeURIComponent(options.pageSize)}`)
  const path = qs.length ? `?${qs.join('&')}` : ''

  const data = await withSpinner('Fetching sandboxes...', () =>
    apiRequest<ListSandboxesResponse>(api, path),
  )

  if (options.json) {
    json(data)
    return
  }

  newline()
  header(`${icons.info} Sandboxes (${data.total})`)

  if (data.items.length === 0) {
    info('No sandboxes found. Create one with `temps sandbox create`.')
    newline()
    return
  }

  const columns: TableColumn<SandboxResponse>[] = [
    { header: 'ID', key: 'id', color: (v) => colors.primary(v) },
    { header: 'Name', key: 'name', color: (v) => colors.bold(v) },
    { header: 'Status', key: 'status', color: (v) => statusColor(v) },
    {
      header: 'Image',
      accessor: (s) => s.image ?? '(default)',
      color: (v) => colors.muted(v.length > 30 ? v.slice(0, 30) + '...' : v),
    },
    { header: 'Expires', key: 'expires_at', color: (v) => colors.muted(v) },
  ]

  printTable(data.items, columns, { style: 'minimal' })
  newline()
}

async function showAction(id: string, options: { json?: boolean }): Promise<void> {
  const api = await auth()
  const sbx = await withSpinner('Fetching sandbox...', () =>
    apiRequest<SandboxResponse>(api, `/${encodeURIComponent(id)}`),
  )

  if (options.json) {
    json(sbx)
    return
  }

  newline()
  header(`${icons.info} ${sbx.id}`)
  keyValue('Name', sbx.name)
  keyValue('Status', statusColor(sbx.status))
  keyValue('Image', sbx.image ?? '(default)')
  keyValue('Work dir', sbx.work_dir)
  keyValue('Created', sbx.created_at)
  keyValue('Expires', sbx.expires_at)
  if (sbx.preview_password_hint) {
    keyValue('Preview password', `ends in …${sbx.preview_password_hint}`)
  }
  newline()
}

async function stopAction(id: string, options: { force?: boolean }): Promise<void> {
  const api = await auth()

  if (!options.force) {
    const confirmed = await promptConfirm({
      message: `Stop and destroy sandbox ${id}?`,
      default: false,
    })
    if (!confirmed) {
      info('Cancelled')
      return
    }
  }

  await withSpinner('Stopping sandbox...', () =>
    apiRequest<void>(api, `/${encodeURIComponent(id)}/stop`, { method: 'POST' }),
  )
  success(`Sandbox ${colors.primary(id)} stopped`)
}

async function pauseAction(id: string): Promise<void> {
  const api = await auth()
  const sbx = await withSpinner('Pausing sandbox...', () =>
    apiRequest<SandboxResponse>(api, `/${encodeURIComponent(id)}/pause`, { method: 'POST' }),
  )
  success(`Sandbox ${colors.primary(id)} paused — status: ${statusColor(sbx.status)}`)
  info(`Resume with: temps sandbox resume ${id}`)
}

async function resumeAction(id: string): Promise<void> {
  const api = await auth()
  const sbx = await withSpinner('Resuming sandbox...', () =>
    apiRequest<SandboxResponse>(api, `/${encodeURIComponent(id)}/resume`, { method: 'POST' }),
  )
  success(`Sandbox ${colors.primary(id)} resumed — expires: ${sbx.expires_at}`)
}

async function restartAction(id: string): Promise<void> {
  const api = await auth()
  const sbx = await withSpinner('Restarting sandbox...', () =>
    apiRequest<SandboxResponse>(api, `/${encodeURIComponent(id)}/restart`, { method: 'POST' }),
  )
  success(`Sandbox ${colors.primary(id)} restarted — status: ${statusColor(sbx.status)}`)
}

interface CloneOptions {
  gitUrl?: string
  gitRev?: string
  gitDepth?: string
  gitConnection?: string
  gitUsername?: string
  gitPassword?: string
  tarballUrl?: string
}

async function cloneAction(id: string, options: CloneOptions): Promise<void> {
  const source = buildSource(options)
  if (!source) {
    throw new Error('Provide --git-url or --tarball-url')
  }
  const api = await auth()
  const sbx = await withSpinner('Seeding source...', () =>
    apiRequest<SandboxResponse>(api, `/${encodeURIComponent(id)}/source`, {
      method: 'POST',
      body: JSON.stringify(source),
    }),
  )
  success(`Source seeded into ${colors.primary(sbx.id)}`)
  keyValue('Work dir', sbx.work_dir)
}

async function extendAction(id: string, options: { secs: string }): Promise<void> {
  const api = await auth()
  const extra = Number(options.secs)
  if (!Number.isFinite(extra) || extra <= 0) {
    throw new Error('--secs must be a positive number')
  }

  const sbx = await withSpinner('Extending timeout...', () =>
    apiRequest<SandboxResponse>(api, `/${encodeURIComponent(id)}/extend-timeout`, {
      method: 'POST',
      body: JSON.stringify({ extra_secs: extra }),
    }),
  )
  success(`Extended by ${extra}s — new expiry: ${sbx.expires_at}`)
}

interface ExecOptions {
  detach?: boolean
  cwd?: string
  env?: string[]
}

async function execAction(
  id: string,
  args: string[],
  options: ExecOptions,
): Promise<void> {
  if (!args || args.length === 0) {
    throw new Error(
      'Provide a command to run. Example: `temps sandbox exec ID -- ls -la`',
    )
  }

  const api = await auth()
  const env = parseEnvPairs(options.env)

  const body: Record<string, unknown> = { cmd: args }
  if (Object.keys(env).length > 0) body.env = env
  if (options.cwd) body.cwd = options.cwd

  const path = options.detach
    ? `/${encodeURIComponent(id)}/exec-detached`
    : `/${encodeURIComponent(id)}/exec`

  if (options.detach) {
    const res = await withSpinner('Starting detached job...', () =>
      apiRequest<ExecDetachedResponse>(api, path, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    )
    success(`Detached job started: ${colors.primary(res.job_id)}`)
    info(`Stream logs: temps sandbox logs ${id} ${res.job_id}`)
    return
  }

  const res = await apiRequest<ExecResponse>(api, path, {
    method: 'POST',
    body: JSON.stringify(body),
  })

  if (res.stdout) process.stdout.write(res.stdout)
  if (res.stderr) process.stderr.write(res.stderr)

  if (res.exit_code !== 0) {
    process.exit(res.exit_code)
  }
}

/**
 * Stream logs via Server-Sent Events. The server emits `event: log`
 * frames with JSON data `{ stream: 'stdout'|'stderr', data: '<line>' }`,
 * `event: lagged` when the broadcast buffer overflows, and `event: done`
 * when the job finishes.
 */
async function logsAction(id: string, jobId: string): Promise<void> {
  const api = await auth()

  const response = await fetch(
    sandboxUrl(api, `/${encodeURIComponent(id)}/jobs/${encodeURIComponent(jobId)}/logs`),
    {
      headers: {
        Authorization: `Bearer ${api.apiKey}`,
        Accept: 'text/event-stream',
      },
    },
  )

  if (!response.ok) {
    throw await readApiError(response)
  }
  if (!response.body) {
    throw new Error('Server returned no response body for log stream')
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let eventName = ''

  try {
    while (true) {
      const { value, done } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })

      let idx: number
      while ((idx = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, idx).replace(/\r$/, '')
        buffer = buffer.slice(idx + 1)

        if (line === '') {
          // End of event frame
          eventName = ''
          continue
        }

        if (line.startsWith('event:')) {
          eventName = line.slice(6).trim()
          continue
        }

        if (!line.startsWith('data:')) continue
        const data = line.slice(5).replace(/^\s/, '')

        if (eventName === 'log') {
          try {
            const payload = JSON.parse(data) as { stream?: string; data?: string }
            const text = (payload.data ?? '') + '\n'
            if (payload.stream === 'stderr') {
              process.stderr.write(text)
            } else {
              process.stdout.write(text)
            }
          } catch {
            process.stdout.write(data + '\n')
          }
        } else if (eventName === 'lagged') {
          warning('Log subscriber fell behind; some lines were dropped')
        } else if (eventName === 'done') {
          return
        }
      }
    }
  } finally {
    reader.releaseLock()
  }
}

async function domainAction(id: string, options: { port: string }): Promise<void> {
  const api = await auth()
  const port = Number(options.port)
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error('--port must be an integer in 1..=65535')
  }

  const res = await apiRequest<DomainResponse>(
    api,
    `/${encodeURIComponent(id)}/domain?port=${port}`,
  )
  console.log(res.url)
}

async function fsReadAction(
  id: string,
  options: { path: string; out?: string },
): Promise<void> {
  const api = await auth()
  const res = await withSpinner('Reading file...', () =>
    apiRequest<ReadFileResponse>(
      api,
      `/${encodeURIComponent(id)}/fs/read?path=${encodeURIComponent(options.path)}`,
    ),
  )
  const bytes = base64Decode(res.contents_b64)

  if (options.out) {
    await Bun.write(options.out, bytes)
    success(`Wrote ${res.size} bytes to ${colors.primary(options.out)}`)
  } else {
    process.stdout.write(bytes)
  }
}

async function fsWriteAction(
  id: string,
  options: { path: string; file?: string; content?: string; mode?: string },
): Promise<void> {
  if (options.file && options.content !== undefined) {
    throw new Error('--file and --content are mutually exclusive')
  }

  let bytes: Uint8Array
  if (options.file) {
    const buf = await Bun.file(options.file).arrayBuffer()
    bytes = new Uint8Array(buf)
  } else if (options.content !== undefined) {
    bytes = new TextEncoder().encode(options.content)
  } else {
    throw new Error('Provide either --file <path> or --content <string>')
  }

  const body: Record<string, unknown> = {
    path: options.path,
    contents_b64: base64Encode(bytes),
  }
  if (options.mode !== undefined) {
    // Accept "0644", "644", or decimal. parseInt with base 8 if leading 0.
    const raw = options.mode
    const parsed = raw.startsWith('0') ? parseInt(raw, 8) : parseInt(raw, 10)
    if (!Number.isFinite(parsed)) {
      throw new Error(`--mode '${raw}' is not a valid permission mask`)
    }
    body.mode = parsed
  }

  const api = await auth()
  await withSpinner(`Writing ${bytes.length} bytes to ${options.path}...`, () =>
    apiRequest<void>(api, `/${encodeURIComponent(id)}/fs/write`, {
      method: 'POST',
      body: JSON.stringify(body),
    }),
  )
  success(`Wrote ${bytes.length} bytes to ${colors.primary(options.path)}`)
}

async function fsStatAction(
  id: string,
  options: { path: string; json?: boolean },
): Promise<void> {
  const api = await auth()
  const res = await apiRequest<StatResponse>(
    api,
    `/${encodeURIComponent(id)}/fs/stat?path=${encodeURIComponent(options.path)}`,
  )

  if (options.json) {
    json(res)
    return
  }

  newline()
  keyValue('Path', res.path)
  keyValue('Exists', res.exists ? colors.success('yes') : colors.error('no'))
  if (res.exists) {
    const kind = res.is_dir ? 'directory' : res.is_file ? 'file' : 'other'
    keyValue('Type', kind)
    keyValue('Size', `${res.size} bytes`)
  }
  newline()
}

async function fsMkdirAction(id: string, options: { path: string }): Promise<void> {
  const api = await auth()
  await withSpinner(`Creating ${options.path}...`, () =>
    apiRequest<void>(api, `/${encodeURIComponent(id)}/fs/mkdir`, {
      method: 'POST',
      body: JSON.stringify({ path: options.path }),
    }),
  )
  success(`Directory ${colors.primary(options.path)} created`)
}
