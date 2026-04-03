// Autofixer API functions

export interface AutofixerRun {
  id: number
  project_id: number
  status: string
  phase: string | null
  analysis: string | null
  user_context: string | null
  trigger_source_id: number | null
  ai_output: string | null
  ai_model: string | null
  tokens_input: number
  tokens_output: number
  estimated_cost_cents: number
  files_changed: number
  pr_url: string | null
  pr_number: number | null
  branch_name: string | null
  error_message: string | null
  started_at: string | null
  completed_at: string | null
  created_at: string
}

export interface AutofixerRunLog {
  id: number
  run_id: number
  level: string
  message: string
  metadata: Record<string, unknown> | null
  created_at: string
}

export interface AutofixerRunWithLogs {
  run: AutofixerRun
  logs: AutofixerRunLog[]
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const text = await response.text().catch(() => '')
    throw new Error(text || `Request failed with status ${response.status}`)
  }
  return response.json()
}

export async function startAnalysis(
  projectId: number,
  errorGroupId: number,
  userContext?: string
): Promise<AutofixerRun> {
  const response = await fetch(`/api/projects/${projectId}/autofixer/analyze`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ error_group_id: errorGroupId, user_context: userContext }),
  })
  return handleResponse<AutofixerRun>(response)
}

export async function getAutofixerRun(
  projectId: number,
  runId: number
): Promise<AutofixerRunWithLogs> {
  const response = await fetch(`/api/projects/${projectId}/autofixer/runs/${runId}`)
  return handleResponse<AutofixerRunWithLogs>(response)
}

export async function addContext(
  projectId: number,
  runId: number,
  message: string
): Promise<void> {
  await fetch(`/api/projects/${projectId}/autofixer/runs/${runId}/add-context`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ message }),
  })
}

export async function startFix(projectId: number, runId: number): Promise<void> {
  const response = await fetch(`/api/projects/${projectId}/autofixer/runs/${runId}/fix`, {
    method: 'POST',
  })
  if (!response.ok) {
    const text = await response.text().catch(() => '')
    throw new Error(text || 'Failed to start fix')
  }
}

export async function createPR(projectId: number, runId: number): Promise<AutofixerRun> {
  const response = await fetch(`/api/projects/${projectId}/autofixer/runs/${runId}/create-pr`, {
    method: 'POST',
  })
  return handleResponse<AutofixerRun>(response)
}

export async function getLatestRunForError(
  projectId: number,
  errorGroupId: number
): Promise<AutofixerRun | null> {
  // Fetch recent autofixer runs and find the latest for this error group
  const response = await fetch(`/api/projects/${projectId}/agents/runs?page=1&page_size=50`)
  if (!response.ok) return null
  const data = await response.json()
  const runs = data.items || data || []
  const match = runs.find(
    (r: any) =>
      r.trigger_type === 'autofixer' &&
      r.trigger_source_id === errorGroupId
  )
  return match || null
}

export async function reAnalyze(projectId: number, runId: number): Promise<void> {
  const response = await fetch(`/api/projects/${projectId}/autofixer/runs/${runId}/re-analyze`, {
    method: 'POST',
  })
  if (!response.ok) {
    const text = await response.text().catch(() => '')
    throw new Error(text || 'Failed to start re-analysis')
  }
}

export async function cancelRun(projectId: number, runId: number): Promise<void> {
  await fetch(`/api/projects/${projectId}/autofixer/runs/${runId}/cancel`, {
    method: 'POST',
  })
}
