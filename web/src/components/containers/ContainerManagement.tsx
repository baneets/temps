import { useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { ContainerHeaderBar } from './ContainerHeaderBar'
import { ContainerDetail } from './ContainerDetail'
import { ContainerActionDialog } from './ContainerActionDialog'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { listContainersOptions } from '@/api/client/@tanstack/react-query.gen'
import { ProjectResponse } from '@/api/client'

interface ContainerManagementProps {
  project: ProjectResponse
  environmentId: string
}

export function ContainerManagement({
  project,
  environmentId,
}: ContainerManagementProps) {
  const [searchParams, setSearchParams] = useSearchParams()
  const [actionType, setActionType] = useState<
    'start' | 'stop' | 'restart' | null
  >(null)
  const queryClient = useQueryClient()

  const { data: containers, isLoading } = useQuery({
    ...listContainersOptions({
      path: {
        project_id: project.id,
        environment_id: parseInt(environmentId),
      },
    }),
    staleTime: 5000,
  })

  const list = containers?.containers ?? []
  const userSelectedId = searchParams.get('container')
  const selectedContainer =
    list.find((c) => c.container_id === userSelectedId) ?? list[0] ?? null
  const selectedContainerId = selectedContainer?.container_id ?? null

  const selectedTab =
    (searchParams.get('tab') as 'overview' | 'logs' | 'configuration' | null) ??
    'overview'

  const handleSelectContainer = (id: string) => {
    searchParams.set('container', id)
    searchParams.set('tab', 'overview')
    setSearchParams(searchParams)
  }

  const handleTabChange = (tab: 'overview' | 'logs' | 'configuration') => {
    searchParams.set('tab', tab)
    setSearchParams(searchParams)
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-96">
        <p className="text-muted-foreground">Loading containers...</p>
      </div>
    )
  }

  if (list.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-96 rounded-lg border border-neutral-950/10 bg-neutral-50 p-6 dark:border-white/10 dark:bg-white/5">
        <div className="text-center space-y-1">
          <p className="text-sm font-semibold text-neutral-900 dark:text-white">
            No containers yet
          </p>
          <p className="text-sm text-neutral-600 dark:text-neutral-400">
            This environment doesn&apos;t have any running containers
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full -mx-4 -my-6 sm:-mx-6 sm:-my-8 lg:-mx-8">
      <ContainerHeaderBar
        containers={list}
        selectedContainer={selectedContainer}
        onSelect={handleSelectContainer}
        tab={selectedTab}
        onTabChange={handleTabChange}
        onAction={setActionType}
      />

      <div className="flex-1 overflow-auto">
        <div className="w-full px-4 py-6 sm:px-6 sm:py-8 lg:px-8">
          {selectedContainerId && (
            <ContainerDetail
              projectId={project.id.toString()}
              environmentId={environmentId}
              containerId={selectedContainerId}
              tab={selectedTab}
            />
          )}
        </div>
      </div>

      <ContainerActionDialog
        projectId={project.id.toString()}
        environmentId={environmentId}
        action={actionType}
        containerId={selectedContainerId}
        onClose={() => setActionType(null)}
        onSuccess={() => {
          queryClient.invalidateQueries({
            queryKey: listContainersOptions({
              path: {
                project_id: project.id,
                environment_id: parseInt(environmentId),
              },
            }).queryKey,
          })
        }}
      />
    </div>
  )
}
