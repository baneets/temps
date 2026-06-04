import { useQuery } from '@tanstack/react-query'
import {
  listConnectionsOptions,
  listDomainsOptions,
  getProjectsOptions,
  listNotificationProvidersOptions,
} from '@/api/client/@tanstack/react-query.gen'
import { useSettings } from './useSettings'

export interface ActivationSignals {
  /** Git provider connected and active */
  gitConnected: boolean
  /** At least one active wildcard domain in the database */
  wildcardDomainReady: boolean
  /** At least one project created */
  hasProject: boolean
  /** external_url is configured in settings */
  externalUrlSet: boolean
  /** At least one enabled notification provider */
  notificationsConfigured: boolean
  /** True once all signals are loaded (used to avoid flash of incomplete state) */
  isLoaded: boolean
  /** Number of completed items out of total */
  completedCount: number
  totalCount: number
}

const TOTAL = 5

export function useActivationSignals(): ActivationSignals {
  const { data: settings, isLoading: settingsLoading } = useSettings()

  const { data: connections, isLoading: connectionsLoading } = useQuery({
    ...listConnectionsOptions({}),
    retry: false,
  })

  const { data: domainsData, isLoading: domainsLoading } = useQuery({
    ...listDomainsOptions({ query: { page_size: 50 } }),
    retry: false,
  })

  const { data: projectsData, isLoading: projectsLoading } = useQuery({
    ...getProjectsOptions({ query: { page: 1, per_page: 1 } }),
    retry: false,
  })

  const { data: providersData, isLoading: providersLoading } = useQuery({
    ...listNotificationProvidersOptions({}),
    retry: false,
  })

  const isLoaded =
    !settingsLoading &&
    !connectionsLoading &&
    !domainsLoading &&
    !projectsLoading &&
    !providersLoading

  const gitConnected =
    (connections?.connections?.filter((c) => c.is_active).length ?? 0) > 0

  const wildcardDomainReady =
    domainsData?.domains?.some(
      (d) => d.is_wildcard && d.status === 'active'
    ) ?? false

  const hasProject = (projectsData?.projects?.length ?? 0) > 0

  const externalUrlSet = !!settings?.external_url

  const notificationsConfigured =
    Array.isArray(providersData) &&
    providersData.some((p) => p.enabled)

  const completed = [
    gitConnected,
    wildcardDomainReady,
    hasProject,
    externalUrlSet,
    notificationsConfigured,
  ].filter(Boolean).length

  return {
    gitConnected,
    wildcardDomainReady,
    hasProject,
    externalUrlSet,
    notificationsConfigured,
    isLoaded,
    completedCount: completed,
    totalCount: TOTAL,
  }
}
