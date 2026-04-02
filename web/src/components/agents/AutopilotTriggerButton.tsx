import { Button } from '@/components/ui/button'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Sparkles } from 'lucide-react'
import { toast } from 'sonner'
import { triggerAutopilotRun } from './api'

interface AutopilotTriggerButtonProps {
  projectId: number
  errorGroupId: number
  /** If false, the button is hidden (no git write access) */
  hasGitConnection?: boolean
}

export function AutopilotTriggerButton({
  projectId,
  errorGroupId,
  hasGitConnection = true,
}: AutopilotTriggerButtonProps) {
  if (!hasGitConnection) return null

  const queryClient = useQueryClient()

  const trigger = useMutation({
    mutationFn: () =>
      triggerAutopilotRun(projectId, {
        trigger_source_type: 'error_group',
        trigger_source_id: errorGroupId,
      }),
    onSuccess: () => {
      toast.success('Autopilot run triggered')
      queryClient.invalidateQueries({
        queryKey: ['agent-runs', projectId],
      })
    },
    onError: (error: Error) => {
      toast.error(error.message || 'Failed to trigger agent run')
    },
  })

  return (
    <Button
      variant="ghost"
      size="icon"
      onClick={(e) => {
        e.stopPropagation()
        trigger.mutate()
      }}
      disabled={trigger.isPending}
      title="Fix with Autopilot"
    >
      <Sparkles className="h-4 w-4" />
    </Button>
  )
}
