// Workspace API types and fetch functions

export interface PreviewPortUrl {
  port: number
  url: string
}

export interface WorkspaceSession {
  id: number
  project_id: number
  user_id: number
  status: string // "active" | "idle" | "closed"
  ai_provider: string
  ai_model: string | null
  tokens_input: number
  tokens_output: number
  estimated_cost_cents: number
  files_changed: number
  branch_name: string | null
  base_branch_name: string | null
  started_at: string
  last_activity_at: string
  closed_at: string | null
  sandbox_container_id: string | null
  /** Last 4 chars of the current preview password (UI disambiguation only). */
  preview_password_hint: string | null
  /** Plaintext password — populated ONLY on session create + password regenerate. */
  preview_password: string | null
  preview_urls: PreviewPortUrl[]
  preview_url_template: string
  /** Per-session idle timeout in minutes. Null = use server default (120).
   *  0 = disabled (never reap — use for long-running background agent runs). */
  idle_timeout_minutes: number | null
  /** User-provided session title. Null = UI shows "Session #{id}". */
  title: string | null
  /** CPU limit in vCPU cores. Null = server default. */
  cpu_limit: number | null
  /** Memory limit in MB. Null = server default. */
  memory_limit_mb: number | null
  /** PID limit. Null = server default. */
  pids_limit: number | null
  /** Skill slugs attached to this session (resolved from project/global definitions). */
  skills: string[]
  /** MCP-server slugs attached to this session. */
  mcp_servers: string[]
}

export interface WorkspaceMessage {
  id: number
  session_id: number
  role: string // "user" | "assistant" | "system" | "ai_event" | "tool_call" | "tool_result"
  content: string
  metadata: Record<string, unknown> | null
  created_at: string
}

export interface SessionWithMessages {
  session: WorkspaceSession
  messages: WorkspaceMessage[]
}

export interface SessionListResponse {
  sessions: WorkspaceSession[]
  total: number
  page: number
  page_size: number
}

export interface StartSessionRequest {
  ai_provider?: string
  /** Per-session model override. Takes precedence over the provider's
   *  configured default_model for this session only. Omit to use the
   *  platform default. */
  ai_model?: string
  /** Branch the workspace should check out. Defaults to project main branch.
   *  When `base_branch_name` is also set, this is the *new* branch to be
   *  created locally off `base_branch_name`. */
  branch_name?: string
  /** When set, the sandbox clones `base_branch_name` and creates
   *  `branch_name` as a new local branch off it. Use this to start a
   *  session "off main" without touching the remote. */
  base_branch_name?: string
  metadata?: Record<string, unknown>
  /** When resuming from an agent run, pass the run ID so the backend
   *  can inject Claude session files into the workspace sandbox. */
  agent_run_id?: number
  /** Skill slugs to inject into the sandbox. Resolved from project-scoped
   *  and global `skill_definitions` tables. */
  skills?: string[]
  /** MCP-server slugs to inject into the sandbox. */
  mcp_servers?: string[]
}

export interface SendMessageRequest {
  content: string
  metadata?: Record<string, unknown>
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const text = await response.text()
    throw new Error(`Workspace API error ${response.status}: ${text}`)
  }
  return response.json() as Promise<T>
}

export async function startSession(
  projectId: number,
  request: StartSessionRequest = {},
): Promise<WorkspaceSession> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    },
  )
  return handleResponse<WorkspaceSession>(response)
}

export async function listSessions(
  projectId: number,
  page = 1,
  pageSize = 20,
): Promise<SessionListResponse> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions?page=${page}&page_size=${pageSize}`,
  )
  return handleResponse<SessionListResponse>(response)
}

export async function getSession(
  projectId: number,
  sessionId: number,
): Promise<SessionWithMessages> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}`,
  )
  return handleResponse<SessionWithMessages>(response)
}

export async function sendMessage(
  projectId: number,
  sessionId: number,
  request: SendMessageRequest,
): Promise<WorkspaceMessage> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/messages`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    },
  )
  return handleResponse<WorkspaceMessage>(response)
}

export async function closeSession(
  projectId: number,
  sessionId: number,
): Promise<void> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/close`,
    { method: 'POST' },
  )
  if (!response.ok) {
    throw new Error(`Failed to close session: ${response.status}`)
  }
}

export async function deleteSession(
  projectId: number,
  sessionId: number,
): Promise<void> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}`,
    { method: 'DELETE' },
  )
  if (!response.ok) {
    throw new Error(`Failed to delete session: ${response.status}`)
  }
}

export async function reopenSession(
  projectId: number,
  sessionId: number,
): Promise<WorkspaceSession> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/reopen`,
    { method: 'POST' },
  )
  return handleResponse<WorkspaceSession>(response)
}

export async function updateSession(
  projectId: number,
  sessionId: number,
  body: {
    idle_timeout_minutes?: number | null
    title?: string | null
    cpu_limit?: number | null
    memory_limit_mb?: number | null
    pids_limit?: number | null
  },
): Promise<WorkspaceSession> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}`,
    {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    },
  )
  return handleResponse<WorkspaceSession>(response)
}

export async function regeneratePreviewPassword(
  projectId: number,
  sessionId: number,
): Promise<WorkspaceSession> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/preview-password/regenerate`,
    { method: 'POST' },
  )
  return handleResponse<WorkspaceSession>(response)
}

async function postSandboxAction(
  projectId: number,
  sessionId: number,
  action: 'stop' | 'start' | 'restart' | 'refresh',
): Promise<void> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/sandbox/${action}`,
    { method: 'POST' },
  )
  if (!response.ok) {
    const text = await response.text()
    throw new Error(`Failed to ${action} sandbox: ${response.status} ${text}`)
  }
}

export interface SandboxStats {
  container_id: string
  /** CPU cores currently consumed (0 → cpu_limit_cores). */
  cpu_used_cores: number
  /** CPU limit in vCPU cores. */
  cpu_limit_cores: number
  /** Percent of CPU budget in use (0–100). */
  cpu_percent: number
  /** RAM currently consumed, in bytes. */
  memory_used_bytes: number
  /** Hard memory limit, in bytes. */
  memory_limit_bytes: number
  /** Percent of RAM budget in use (0–100). */
  memory_percent: number
}

/** Fetch an instantaneous resource-usage snapshot for a session's sandbox.
 *  Cheap one-shot cgroup read — safe to poll on a few-second interval. */
export async function getSandboxStats(
  projectId: number,
  sessionId: number,
): Promise<SandboxStats> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/sandbox/stats`,
  )
  return handleResponse<SandboxStats>(response)
}

export const stopSandbox = (p: number, s: number) =>
  postSandboxAction(p, s, 'stop')
export const startSandbox = (p: number, s: number) =>
  postSandboxAction(p, s, 'start')
export const restartSandbox = (p: number, s: number) =>
  postSandboxAction(p, s, 'restart')
export const refreshSandbox = (p: number, s: number) =>
  postSandboxAction(p, s, 'refresh')

/**
 * Cancel an in-flight assistant run for a session.
 *
 * Writes a synthetic terminal assistant message so the UI's "Thinking…"
 * indicator clears immediately. The user's escape hatch when something
 * appears stuck — works regardless of whether the underlying executor is
 * actually wedged or not.
 */
export async function cancelRun(
  projectId: number,
  sessionId: number,
): Promise<void> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/cancel`,
    { method: 'POST' },
  )
  if (!response.ok) {
    const text = await response.text()
    throw new Error(`Failed to cancel run: ${response.status} ${text}`)
  }
}

/**
 * Build the SSE stream URL for a workspace session.
 * Use with `new EventSource(url)`.
 */
export function sessionStreamUrl(
  projectId: number,
  sessionId: number,
  afterId = 0,
): string {
  return `/api/projects/${projectId}/workspace/sessions/${sessionId}/stream?after_id=${afterId}`
}

/**
 * Upload a pasted image into the session's sandbox container. Returns the
 * path inside the sandbox where the file was written. The frontend then types
 * that path into the PTY so Claude CLI picks it up as an image attachment.
 */
export async function pasteTerminalImage(
  projectId: number,
  sessionId: number,
  bytes: Uint8Array,
  mime: string,
): Promise<{ path: string }> {
  // Convert to base64 in chunks to avoid blowing the call stack on large images.
  let binary = ''
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
  }
  const data = btoa(binary)
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/terminal/paste-image`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data, mime }),
    },
  )
  return handleResponse<{ path: string }>(response)
}

/**
 * One terminal tab inside a workspace session. Each tab corresponds to one
 * tmux session (`temps-{kind}-{id}`) inside the sandbox container.
 */
export interface TerminalTab {
  /** `claude` runs the AI CLI; `shell` runs raw bash. */
  kind: 'claude' | 'shell'
  /** Stable identifier the client picks. Combined with kind = tmux session. */
  id: string
  /** Live count of attached tmux clients (browser tabs viewing this). */
  attached_clients: number
}

export async function listTerminalTabs(
  projectId: number,
  sessionId: number,
): Promise<TerminalTab[]> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/terminal/tabs`,
  )
  const data = await handleResponse<{ tabs: TerminalTab[] }>(response)
  return data.tabs
}

export async function deleteTerminalTab(
  projectId: number,
  sessionId: number,
  kind: 'claude' | 'shell',
  id: string,
): Promise<void> {
  const response = await fetch(
    `/api/projects/${projectId}/workspace/sessions/${sessionId}/terminal/tabs/${kind}-${id}`,
    { method: 'DELETE' },
  )
  if (!response.ok) {
    throw new Error(`Failed to delete tab: ${response.status}`)
  }
}

/**
 * Build the WebSocket URL for a workspace session terminal tab.
 *
 * Each (kind, tab) pair attaches to its own tmux session inside the sandbox,
 * so a workspace can have multiple independent terminals — one running
 * claude, others running raw shells. Reusing the same {kind,tab} re-attaches
 * to the same tmux session so refreshes don't lose state.
 *
 * Protocol:
 * - Binary frames → raw PTY bytes in both directions (xterm.js compatible)
 * - Text `{"type":"resize","cols":N,"rows":N}` → resize the remote PTY
 * - Text `{"type":"exit","code":N}` from server → session ended
 */
export function sessionTerminalUrl(
  projectId: number,
  sessionId: number,
  options: { kind?: 'claude' | 'shell'; tab?: string } = {},
): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  const params = new URLSearchParams()
  if (options.kind) params.set('kind', options.kind)
  if (options.tab) params.set('tab', options.tab)
  const qs = params.toString()
  return `${proto}//${window.location.host}/api/projects/${projectId}/workspace/sessions/${sessionId}/terminal${qs ? '?' + qs : ''}`
}
