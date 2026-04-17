// Sandbox API types and fetch functions.
// Backend surface: `/v1/sandbox/*`.
// Base URL through the app is `/api/v1/sandbox/*`.

export interface SandboxResponse {
  id: string // opaque "sbx_…" public id
  name: string
  status: string // "running" | "stopped" | "destroyed" | "paused"
  image: string | null
  work_dir: string
  created_at: string
  expires_at: string
  /**
   * URL template with a literal `{port}` placeholder. Substitute any
   * port the sandbox binds (e.g. 3000, 5173) to get the public preview
   * URL for that port. Empty string when preview URLs aren't configured
   * on this install.
   */
  preview_url_template: string
  /**
   * Last 4 characters of the active preview password. Present only when
   * a password has been set — omitted otherwise. The plaintext password
   * is never returned by the API (the user chooses it).
   */
  preview_password_hint?: string
}

export interface ListSandboxesResponse {
  items: SandboxResponse[]
  total: number
  page: number
  page_size: number
}

export interface ExecResponse {
  exit_code: number
  stdout: string
  stderr: string
}

export interface ExecBody {
  cmd: string[]
  env?: Record<string, string>
  cwd?: string
}

export interface CreateSandboxBody {
  image?: string
  name?: string
  timeout_secs?: number
  env?: Record<string, string>
  cpu_limit?: number
  memory_limit_mb?: number
  pids_limit?: number
  /**
   * Optional preview-URL password. When set, every preview URL served for
   * this sandbox is gated behind a login form. 8–256 characters. Omit to
   * leave preview URLs open (the sandbox ID is then the only gate). The
   * plaintext is never returned — only the last-4 hint surfaces on the
   * response as `preview_password_hint`.
   */
  preview_password?: string
}

const BASE = '/api/v1/sandbox'

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const text = await response.text()
    throw new Error(`Sandbox API error ${response.status}: ${text}`)
  }
  return response.json() as Promise<T>
}

async function handleEmpty(response: Response, op: string): Promise<void> {
  if (!response.ok) {
    const text = await response.text()
    throw new Error(`Failed to ${op}: ${response.status} ${text}`)
  }
}

export async function listSandboxes(
  page = 1,
  pageSize = 20,
): Promise<ListSandboxesResponse> {
  const response = await fetch(
    `${BASE}?page=${page}&page_size=${pageSize}`,
  )
  return handleResponse<ListSandboxesResponse>(response)
}

export async function getSandbox(id: string): Promise<SandboxResponse> {
  const response = await fetch(`${BASE}/${encodeURIComponent(id)}`)
  return handleResponse<SandboxResponse>(response)
}

export async function createSandbox(
  body: CreateSandboxBody,
): Promise<SandboxResponse> {
  const response = await fetch(BASE, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return handleResponse<SandboxResponse>(response)
}

export async function stopSandbox(id: string): Promise<void> {
  const response = await fetch(`${BASE}/${encodeURIComponent(id)}/stop`, {
    method: 'POST',
  })
  return handleEmpty(response, 'stop sandbox')
}

export async function pauseSandbox(id: string): Promise<SandboxResponse> {
  const response = await fetch(`${BASE}/${encodeURIComponent(id)}/pause`, {
    method: 'POST',
  })
  return handleResponse<SandboxResponse>(response)
}

export async function resumeSandbox(id: string): Promise<SandboxResponse> {
  const response = await fetch(`${BASE}/${encodeURIComponent(id)}/resume`, {
    method: 'POST',
  })
  return handleResponse<SandboxResponse>(response)
}

export async function restartSandbox(id: string): Promise<SandboxResponse> {
  const response = await fetch(`${BASE}/${encodeURIComponent(id)}/restart`, {
    method: 'POST',
  })
  return handleResponse<SandboxResponse>(response)
}

export async function extendTimeout(
  id: string,
  extraSecs: number,
): Promise<SandboxResponse> {
  const response = await fetch(
    `${BASE}/${encodeURIComponent(id)}/extend-timeout`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ extra_secs: extraSecs }),
    },
  )
  return handleResponse<SandboxResponse>(response)
}

export async function execCommand(
  id: string,
  body: ExecBody,
): Promise<ExecResponse> {
  const response = await fetch(`${BASE}/${encodeURIComponent(id)}/exec`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return handleResponse<ExecResponse>(response)
}

/**
 * A single detached job, as returned by the jobs list. Excludes stdout
 * and stderr bodies — drill into `getJob()` for the full buffer.
 */
export interface JobSummary {
  id: string
  /** `"running" | "exited" | "failed"`. */
  status: string
  /** Populated only when `status === "exited"`. */
  exit_code: number | null
  /** Populated only when `status === "failed"`. */
  reason: string | null
  /** Human-readable command (argv joined by space). */
  cmd: string
  /** RFC3339 start timestamp. */
  started_at: string
}

export interface ListJobsResponse {
  items: JobSummary[]
}

export async function listJobs(id: string): Promise<JobSummary[]> {
  const response = await fetch(`${BASE}/${encodeURIComponent(id)}/jobs`)
  const data = await handleResponse<ListJobsResponse>(response)
  return data.items
}

/**
 * Full snapshot of a detached job. Includes accumulated stdout/stderr
 * from when the job started — callers combine this with the SSE stream
 * from `jobLogsUrl()` to get history plus live tail in one view.
 */
export interface JobStatus {
  status: string
  exit_code: number | null
  reason: string | null
  stdout: string
  stderr: string
}

export async function getJob(
  sandboxId: string,
  jobId: string,
): Promise<JobStatus> {
  const response = await fetch(
    `${BASE}/${encodeURIComponent(sandboxId)}/jobs/${encodeURIComponent(jobId)}`,
  )
  return handleResponse<JobStatus>(response)
}

/**
 * SSE URL for a job's live log tail. Use with `new EventSource(url)`.
 * Server emits `log` events shaped `{ stream: "stdout" | "stderr",
 * data: string }`, plus a final `done` event when the command exits.
 * Late subscribers only see lines produced after connect — call
 * `getJob()` first to fetch the history.
 */
export function jobLogsUrl(sandboxId: string, jobId: string): string {
  return `${BASE}/${encodeURIComponent(sandboxId)}/jobs/${encodeURIComponent(jobId)}/logs`
}

export interface SetPreviewPasswordResponse {
  preview_password_hint: string
}

/**
 * Set or rotate the sandbox's preview-URL password. The user supplies the
 * plaintext; the server hashes it with argon2 and returns only the last-4
 * hint so the UI can confirm which password is active. Plaintext is never
 * persisted and never echoed back.
 *
 * Backend validation: 8–256 characters. HTTP 400 on length violations.
 */
export async function setSandboxPreviewPassword(
  id: string,
  password: string,
): Promise<SetPreviewPasswordResponse> {
  const response = await fetch(
    `${BASE}/${encodeURIComponent(id)}/preview-password`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password }),
    },
  )
  return handleResponse<SetPreviewPasswordResponse>(response)
}

/**
 * Remove the password on the sandbox's preview URLs. After this call the
 * sandbox's 16-hex public ID is the only gate on preview traffic. 204
 * No Content on success.
 */
export async function clearSandboxPreviewPassword(id: string): Promise<void> {
  const response = await fetch(
    `${BASE}/${encodeURIComponent(id)}/preview-password`,
    { method: 'DELETE' },
  )
  return handleEmpty(response, 'clear preview password')
}
