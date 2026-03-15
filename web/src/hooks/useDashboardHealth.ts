import { client } from '@/api/client/client.gen'
import { useQuery } from '@tanstack/react-query'

export interface ProjectMonitorHealth {
  project_id: number
  /** "operational" | "degraded" | "down" | "no_monitors" */
  status: string
}

export interface ProjectsMonitorHealthResponse {
  projects: Record<string, ProjectMonitorHealth>
}

async function fetchProjectsHealth(
  projectIds: number[]
): Promise<ProjectsMonitorHealthResponse> {
  const response = await client.get({
    url: '/monitors-health/projects',
    query: {
      project_ids: projectIds.join(','),
    },
    security: [{ scheme: 'bearer', type: 'http' }],
  })
  return response.data as ProjectsMonitorHealthResponse
}

export function useDashboardHealth(projectIds: number[]) {
  return useQuery({
    queryKey: ['dashboard-projects-health', projectIds],
    queryFn: () => fetchProjectsHealth(projectIds),
    enabled: projectIds.length > 0,
    staleTime: 1000 * 30,
    refetchInterval: 1000 * 30,
  })
}
