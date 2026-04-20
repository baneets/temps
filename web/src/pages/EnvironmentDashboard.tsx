import { getEnvironmentOptions } from '@/api/client/@tanstack/react-query.gen'
import { Skeleton } from '@/components/ui/skeleton'
import { ErrorAlert } from '@/components/utils/ErrorAlert'
import { ContainerManagement } from '@/components/containers/ContainerManagement'
import { EnvironmentSettingsContent } from '@/components/environments/EnvironmentSettingsContent'
import { EnvironmentHeaderBar } from '@/components/environments/EnvironmentHeaderBar'
import { useQuery } from '@tanstack/react-query'
import { useSearchParams } from 'react-router-dom'
import { EnvironmentResponse, ProjectResponse } from '@/api/client'
import { useCallback } from 'react'

interface EnvironmentDashboardProps {
  project: ProjectResponse
  environmentId: number
  environments?: EnvironmentResponse[]
  onEnvironmentChange?: (id: number) => void
  onCreateEnvironment?: () => void
  onDelete?: () => void
}

export function EnvironmentDashboard({
  project,
  environmentId,
  environments,
  onEnvironmentChange,
  onCreateEnvironment,
  onDelete,
}: EnvironmentDashboardProps) {
  const [searchParams, setSearchParams] = useSearchParams()
  const activeView = (searchParams.get('view') || 'containers') as string

  const handleViewChange = useCallback(
    (view: string) => {
      searchParams.set('view', view)
      setSearchParams(searchParams)
    },
    [searchParams, setSearchParams]
  )

  const {
    data: environment,
    isLoading: isEnvironmentLoading,
    error: environmentError,
    refetch,
  } = useQuery({
    ...getEnvironmentOptions({
      path: {
        project_id: project?.id || 0,
        env_id: environmentId,
      },
    }),
    enabled: !!project?.id && !!environmentId,
  })

  if (environmentError) {
    return (
      <div className="p-4 sm:p-6">
        <ErrorAlert
          title="Failed to load environment"
          description={
            environmentError instanceof Error
              ? environmentError.message
              : 'An unexpected error occurred'
          }
          retry={() => refetch()}
        />
      </div>
    )
  }

  if (isEnvironmentLoading) {
    return (
      <div className="flex flex-col h-full">
        <div className="p-6 border-b bg-background">
          <div className="flex items-center justify-between">
            <div>
              <Skeleton className="h-8 w-48 mb-2" />
              <Skeleton className="h-4 w-64" />
            </div>
            <div className="flex items-center gap-3">
              <Skeleton className="h-6 w-24" />
              <Skeleton className="h-9 w-24" />
            </div>
          </div>
        </div>
        <div className="flex-1 p-6">
          <Skeleton className="h-96 w-full" />
        </div>
      </div>
    )
  }

  if (!environment) {
    return (
      <div className="p-4 sm:p-6">
        <ErrorAlert
          title="Environment not found"
          description="The environment you're looking for does not exist"
          retry={() => refetch()}
        />
      </div>
    )
  }

  const isStatic = project?.preset === 'custom'

  const containersContent = isStatic ? (
    <div className="flex flex-col items-center justify-center h-96 text-center">
      <p className="text-muted-foreground">
        This static site does not have running containers to manage.
      </p>
    </div>
  ) : (
    <ContainerManagement
      project={project}
      environmentId={environmentId.toString()}
    />
  )

  const settingsContent = (
    <EnvironmentSettingsContent
      environment={environment}
      project={project}
      environmentId={environmentId.toString()}
      onDelete={onDelete}
    />
  )

  const content =
    activeView === 'settings' ? settingsContent : containersContent

  return (
    <div className="flex flex-col h-full bg-white dark:bg-neutral-950">
      <EnvironmentHeaderBar
        environment={environment}
        project={project}
        activeView={activeView}
        onViewChange={handleViewChange}
        isStatic={isStatic}
        environments={environments}
        onEnvironmentChange={onEnvironmentChange}
        onCreateEnvironment={onCreateEnvironment}
      />
      <div className="flex-1 overflow-auto">
        <div className="w-full px-4 py-6 sm:px-6 sm:py-8 lg:px-8">
          {content}
        </div>
      </div>
    </div>
  )
}
