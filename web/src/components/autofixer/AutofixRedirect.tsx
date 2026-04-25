import { ProjectResponse } from '@/api/client'
import { latestRunForSourceOptions } from '@/api/client/@tanstack/react-query.gen'
import { startAnalysis } from '@/api/client/sdk.gen'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Skeleton } from '@/components/ui/skeleton'
import { useQuery } from '@tanstack/react-query'
import { AlertTriangle } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Navigate, useParams } from 'react-router-dom'

interface AutofixRedirectProps {
  project: ProjectResponse
}

/**
 * Redirects `/errors/:errorGroupId/autofix` to the unified agent run viewer at
 * `/agents/:runId`. If a latest run exists for this error group, we navigate
 * there; otherwise, we kick off a fresh analysis and navigate to the new run.
 */
export function AutofixRedirect({ project }: AutofixRedirectProps) {
  const { errorGroupId } = useParams<{ errorGroupId: string }>()
  const groupId = Number(errorGroupId) || 0
  const [newRunId, setNewRunId] = useState<number | null>(null)
  const [kickoffError, setKickoffError] = useState<string | null>(null)
  const kickoffStartedRef = useRef(false)

  const {
    data: latestRun,
    isLoading,
    isError,
  } = useQuery({
    ...latestRunForSourceOptions({
      path: { project_id: project.id },
      query: {
        trigger_source_type: 'error_group',
        trigger_source_id: groupId,
      },
    }),
    enabled: groupId > 0,
    retry: false,
  })

  // If there's no existing run, kick one off. Guard with ref so React's
  // double-render in dev doesn't fire the mutation twice.
  useEffect(() => {
    if (isLoading) return
    if (latestRun || newRunId || kickoffStartedRef.current) return
    if (groupId <= 0) return

    // latestRunForSource returns 404 when no run exists — that's our cue to
    // start analysis. A 5xx lands here too; we surface it below.
    if (isError || !latestRun) {
      kickoffStartedRef.current = true
      startAnalysis({
        path: { project_id: project.id },
        body: { error_group_id: groupId },
        throwOnError: true,
      })
        .then(({ data }) => setNewRunId(data.id))
        .catch((e: unknown) => {
          setKickoffError(e instanceof Error ? e.message : 'Failed to start analysis')
        })
    }
  }, [isLoading, isError, latestRun, newRunId, groupId, project.id])

  if (groupId <= 0) {
    return <Navigate to="../errors" replace />
  }

  const targetRunId = latestRun?.id ?? newRunId
  if (targetRunId) {
    return <Navigate to={`../agents/${targetRunId}`} replace />
  }

  if (kickoffError) {
    return (
      <Alert variant="destructive">
        <AlertTriangle className="h-4 w-4" />
        <AlertTitle>Could not start analysis</AlertTitle>
        <AlertDescription>{kickoffError}</AlertDescription>
      </Alert>
    )
  }

  return (
    <div className="space-y-4">
      <Skeleton className="h-8 w-48" />
      <Skeleton className="h-40 w-full" />
    </div>
  )
}
