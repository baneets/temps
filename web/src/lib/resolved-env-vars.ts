/**
 * Hand-written helpers for the resolved env-var view that merges manually
 * defined variables with those supplied by linked external services.
 *
 * TODO(sdk-regen): replace with generated SDK helpers for
 *   - GET /projects/{project_id}/env-vars/resolved
 * once this endpoint is included in the OpenAPI spec / generated client.
 */

export interface EnvVarIntegrationInfo {
  service_id: number
  service_name: string
  service_type: string
  service_slug?: string | null
}

export type ResolvedEnvVarSource =
  | {
      type: 'manual'
      var_id: number
      overrides_service?: EnvVarIntegrationInfo | null
    }
  | {
      type: 'integration'
      service: EnvVarIntegrationInfo
    }

export interface ResolvedEnvironmentInfo {
  id: number
  name: string
  main_url: string
  current_deployment_id?: number | null
}

export interface ResolvedEnvVar {
  key: string
  value_preview: string
  source: ResolvedEnvVarSource
  environments: ResolvedEnvironmentInfo[]
  include_in_preview: boolean
}

async function readJsonOrThrow<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let detail = response.statusText
    try {
      const body = (await response.json()) as { detail?: string; title?: string }
      detail = body.detail || body.title || detail
    } catch {
      // fall through with statusText
    }
    throw new Error(detail)
  }
  return (await response.json()) as T
}

export async function getResolvedEnvVarValue(
  projectId: number,
  key: string,
  environmentId?: number,
): Promise<string> {
  const url = new URL(
    `/api/projects/${projectId}/env-vars/resolved/${encodeURIComponent(key)}/value`,
    window.location.origin,
  )
  if (typeof environmentId === 'number') {
    url.searchParams.set('environment_id', String(environmentId))
  }
  const response = await fetch(url.pathname + url.search, {
    credentials: 'include',
  })
  const body = await readJsonOrThrow<{ value: string }>(response)
  return body.value
}

export async function getResolvedEnvVars(
  projectId: number,
  environmentId?: number,
): Promise<ResolvedEnvVar[]> {
  const url = new URL(
    `/api/projects/${projectId}/env-vars/resolved`,
    window.location.origin,
  )
  if (typeof environmentId === 'number') {
    url.searchParams.set('environment_id', String(environmentId))
  }
  const response = await fetch(url.pathname + url.search, {
    credentials: 'include',
  })
  return readJsonOrThrow<ResolvedEnvVar[]>(response)
}

/**
 * Build a Map keyed by env-var key for O(1) lookup when rendering rows.
 */
export function indexResolvedByKey(
  resolved: ResolvedEnvVar[] | undefined,
): Map<string, ResolvedEnvVar> {
  const map = new Map<string, ResolvedEnvVar>()
  if (!resolved) return map
  for (const entry of resolved) {
    map.set(entry.key, entry)
  }
  return map
}
