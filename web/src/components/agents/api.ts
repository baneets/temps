// Agents API types and fetch functions

export interface Agent {
  id: number
  project_id: number
  slug: string
  name: string
  description: string | null
  source: string // 'yaml' | 'dashboard'
  enabled: boolean
  trigger_config: {
    error?: { new_issue?: boolean; regression?: boolean }
    deploy?: { production?: boolean; preview?: boolean }
    monitoring?: { downtime?: boolean; latency_spike?: boolean }
    schedule?: { cron?: string | null }
    manual?: boolean
  }
  prompt: string | null
  ai_provider: string
  api_key_set: boolean
  ai_provider_key_id: number | null
  max_turns: number
  timeout_seconds: number
  daily_budget_cents: number
  cooldown_minutes: number
  branch_prefix: string
  deliverable: string
  sandbox_enabled: boolean | null
  config_repo_url: string | null
  config_repo_branch: string | null
  /** Slug references to project-level MCP server definitions */
  mcp_servers_config: string[] | null
  /** Slug references to project-level skill definitions */
  skills_config: string[] | null
  tools_config: Record<string, unknown> | null
  webhook_url: string | null
  webhook_token: string | null
  created_at: string
  updated_at: string
}

export interface AgentRun {
  id: number
  project_id: number
  config_id: number
  agent_id: number | null
  agent_slug: string | null
  agent_name: string | null
  trigger_type: string
  trigger_source_id: number | null
  trigger_source_type: string | null
  status: string
  branch_name: string | null
  commit_sha: string | null
  pr_url: string | null
  pr_number: number | null
  preview_url: string | null
  error_message: string | null
  ai_output: string | null
  ai_reasoning: string | null
  ai_model: string | null
  tokens_input: number
  tokens_output: number
  estimated_cost_cents: number
  files_changed: number
  analysis: string | null
  started_at: string | null
  completed_at: string | null
  created_at: string
  sandbox_enabled: boolean
  user_context: string | null
  ai_session_id: string | null
}

export interface AgentRunLog {
  id: number
  run_id: number
  level: string
  message: string
  metadata: Record<string, unknown> | null
  created_at: string
}

export interface AgentRunWithLogs {
  run: AgentRun
  logs: AgentRunLog[]
}

export interface CreateAgentRequest {
  slug: string
  name: string
  description?: string
  enabled?: boolean
  ai_provider?: string
  api_key?: string
  trigger_config?: Record<string, unknown>
  prompt?: string
  max_turns?: number
  timeout_seconds?: number
  daily_budget_cents?: number
  cooldown_minutes?: number
  branch_prefix?: string
  deliverable?: string
  sandbox_enabled?: boolean
  config_repo_url?: string
  config_repo_branch?: string
  mcp_servers_config?: string[] | null
  skills_config?: string[] | null
  tools_config?: Record<string, unknown> | null
}

export interface UpdateAgentRequest {
  name?: string
  description?: string
  enabled?: boolean
  ai_provider?: string
  api_key?: string
  trigger_config?: Record<string, unknown>
  prompt?: string
  max_turns?: number
  timeout_seconds?: number
  daily_budget_cents?: number
  cooldown_minutes?: number
  branch_prefix?: string
  deliverable?: string
  sandbox_enabled?: boolean
  config_repo_url?: string | null
  config_repo_branch?: string | null
  mcp_servers_config?: string[] | null
  skills_config?: string[] | null
  tools_config?: Record<string, unknown> | null
}

export interface PaginatedRuns {
  items: AgentRun[]
  total: number
  page: number
  page_size: number
}

export interface AiCliStatus {
  provider: string
  installed: boolean
  version: string | null
  authenticated: boolean
  auth_method: string | null
  email: string | null
  subscription_type: string | null
  setup_hint: string | null
}

// ── Fetch helpers ──

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const text = await response.text().catch(() => '')
    throw new Error(text || `Request failed with status ${response.status}`)
  }
  return response.json()
}

// ── Agent CRUD ──

export async function listAgents(projectId: number): Promise<Agent[]> {
  const response = await fetch(`/api/projects/${projectId}/agents`)
  const data = await handleResponse<{ items: Agent[] } | Agent[]>(response)
  // API may return { items: [...] } or plain array
  return Array.isArray(data) ? data : data.items
}

export async function getAgent(projectId: number, slug: string): Promise<Agent> {
  const response = await fetch(`/api/projects/${projectId}/agents/${slug}`)
  return handleResponse<Agent>(response)
}

export async function createAgent(projectId: number, data: CreateAgentRequest): Promise<Agent> {
  const response = await fetch(`/api/projects/${projectId}/agents`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<Agent>(response)
}

export async function updateAgent(projectId: number, slug: string, data: UpdateAgentRequest): Promise<Agent> {
  const response = await fetch(`/api/projects/${projectId}/agents/${slug}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<Agent>(response)
}

export async function deleteAgent(projectId: number, slug: string): Promise<void> {
  const response = await fetch(`/api/projects/${projectId}/agents/${slug}`, { method: 'DELETE' })
  if (!response.ok) {
    const text = await response.text().catch(() => '')
    throw new Error(text || `Request failed with status ${response.status}`)
  }
}

// ── Runs ──

export async function listAllRuns(projectId: number, page = 1, pageSize = 20): Promise<PaginatedRuns> {
  const response = await fetch(`/api/projects/${projectId}/agents/runs?page=${page}&page_size=${pageSize}`)
  return handleResponse<PaginatedRuns>(response)
}

export async function listRunsForAgent(projectId: number, slug: string, page = 1, pageSize = 20): Promise<PaginatedRuns> {
  const response = await fetch(`/api/projects/${projectId}/agents/${slug}/runs?page=${page}&page_size=${pageSize}`)
  return handleResponse<PaginatedRuns>(response)
}

export async function getAgentRun(projectId: number, runId: string): Promise<AgentRunWithLogs> {
  const response = await fetch(`/api/projects/${projectId}/agents/runs/${runId}`)
  return handleResponse<AgentRunWithLogs>(response)
}

// ── Triggers ──

export async function triggerAgent(projectId: number, slug: string, data?: { trigger_source_type?: string; trigger_source_id?: number; user_context?: string }): Promise<AgentRun> {
  const response = await fetch(`/api/projects/${projectId}/agents/${slug}/trigger`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data ?? {}),
  })
  return handleResponse<AgentRun>(response)
}

export async function retryRun(projectId: number, runId: number): Promise<AgentRun> {
  const response = await fetch(`/api/projects/${projectId}/agents/runs/${runId}/retry`, {
    method: 'POST',
  })
  return handleResponse<AgentRun>(response)
}

// ── CLI Status ──

export async function getCliStatus(projectId: number, provider = 'claude_cli'): Promise<AiCliStatus> {
  const response = await fetch(`/api/projects/${projectId}/agents/cli-status?provider=${provider}`)
  return handleResponse<AiCliStatus>(response)
}

// ── Secrets ──

export interface AgentSecret {
  id: number
  name: string
  secret_type: 'env' | 'file'
  mount_path: string | null
  description: string | null
  created_at: string
  updated_at: string
}

export interface CreateSecretRequest {
  name: string
  secret_type: 'env' | 'file'
  value: string
  mount_path?: string
  description?: string
}

export async function listSecrets(): Promise<AgentSecret[]> {
  const response = await fetch('/api/settings/secrets')
  const data = await handleResponse<{ items: AgentSecret[] } | AgentSecret[]>(response)
  return Array.isArray(data) ? data : data.items
}

export async function createSecret(data: CreateSecretRequest): Promise<AgentSecret> {
  const response = await fetch('/api/settings/secrets', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<AgentSecret>(response)
}

export async function deleteSecret(name: string): Promise<void> {
  const response = await fetch(`/api/settings/secrets/${encodeURIComponent(name)}`, { method: 'DELETE' })
  if (!response.ok) {
    const text = await response.text().catch(() => '')
    throw new Error(text || `Request failed with status ${response.status}`)
  }
}

// ── Skill Definitions ──

export interface SkillDefinition {
  id: number
  project_id: number | null
  slug: string
  name: string
  description: string | null
  content: string
  created_at: string
  updated_at: string
}

export interface CreateSkillDefinitionRequest {
  slug: string
  name: string
  description?: string
  content: string
}

export async function listSkillDefinitions(projectId: number): Promise<SkillDefinition[]> {
  const response = await fetch(`/api/projects/${projectId}/skills`)
  const data = await handleResponse<{ items: SkillDefinition[] }>(response)
  return data.items
}

export async function createSkillDefinition(projectId: number, data: CreateSkillDefinitionRequest): Promise<SkillDefinition> {
  const response = await fetch(`/api/projects/${projectId}/skills`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<SkillDefinition>(response)
}

export async function updateSkillDefinition(projectId: number, slug: string, data: Partial<Omit<CreateSkillDefinitionRequest, 'slug'>>): Promise<SkillDefinition> {
  const response = await fetch(`/api/projects/${projectId}/skills/${slug}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<SkillDefinition>(response)
}

export async function deleteSkillDefinition(projectId: number, slug: string): Promise<void> {
  const response = await fetch(`/api/projects/${projectId}/skills/${slug}`, { method: 'DELETE' })
  if (!response.ok) throw new Error('Failed to delete skill definition')
}

// ── MCP Server Definitions ──

export interface McpDefinition {
  id: number
  project_id: number | null
  slug: string
  name: string
  description: string | null
  config: Record<string, unknown>
  created_at: string
  updated_at: string
}

export interface CreateMcpDefinitionRequest {
  slug: string
  name: string
  description?: string
  config: Record<string, unknown>
}

export async function listMcpDefinitions(projectId: number): Promise<McpDefinition[]> {
  const response = await fetch(`/api/projects/${projectId}/mcp-servers`)
  const data = await handleResponse<{ items: McpDefinition[] }>(response)
  return data.items
}

export async function createMcpDefinition(projectId: number, data: CreateMcpDefinitionRequest): Promise<McpDefinition> {
  const response = await fetch(`/api/projects/${projectId}/mcp-servers`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<McpDefinition>(response)
}

export async function updateMcpDefinition(projectId: number, slug: string, data: Partial<Omit<CreateMcpDefinitionRequest, 'slug'>>): Promise<McpDefinition> {
  const response = await fetch(`/api/projects/${projectId}/mcp-servers/${slug}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<McpDefinition>(response)
}

export async function deleteMcpDefinition(projectId: number, slug: string): Promise<void> {
  const response = await fetch(`/api/projects/${projectId}/mcp-servers/${slug}`, { method: 'DELETE' })
  if (!response.ok) throw new Error('Failed to delete MCP server definition')
}

// ── Global Skill Definitions ──

export async function listGlobalSkillDefinitions(): Promise<SkillDefinition[]> {
  const response = await fetch('/api/settings/skills')
  const data = await handleResponse<{ items: SkillDefinition[] }>(response)
  return data.items
}

export async function createGlobalSkillDefinition(data: CreateSkillDefinitionRequest): Promise<SkillDefinition> {
  const response = await fetch('/api/settings/skills', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<SkillDefinition>(response)
}

export async function updateGlobalSkillDefinition(slug: string, data: Partial<Omit<CreateSkillDefinitionRequest, 'slug'>>): Promise<SkillDefinition> {
  const response = await fetch(`/api/settings/skills/${slug}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<SkillDefinition>(response)
}

export async function deleteGlobalSkillDefinition(slug: string): Promise<void> {
  const response = await fetch(`/api/settings/skills/${slug}`, { method: 'DELETE' })
  if (!response.ok) throw new Error('Failed to delete global skill definition')
}

// ── Global MCP Server Definitions ──

export async function listGlobalMcpDefinitions(): Promise<McpDefinition[]> {
  const response = await fetch('/api/settings/mcp-servers')
  const data = await handleResponse<{ items: McpDefinition[] }>(response)
  return data.items
}

export async function createGlobalMcpDefinition(data: CreateMcpDefinitionRequest): Promise<McpDefinition> {
  const response = await fetch('/api/settings/mcp-servers', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<McpDefinition>(response)
}

export async function updateGlobalMcpDefinition(slug: string, data: Partial<Omit<CreateMcpDefinitionRequest, 'slug'>>): Promise<McpDefinition> {
  const response = await fetch(`/api/settings/mcp-servers/${slug}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  return handleResponse<McpDefinition>(response)
}

export async function deleteGlobalMcpDefinition(slug: string): Promise<void> {
  const response = await fetch(`/api/settings/mcp-servers/${slug}`, { method: 'DELETE' })
  if (!response.ok) throw new Error('Failed to delete global MCP server definition')
}
