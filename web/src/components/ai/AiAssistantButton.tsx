import { useAiAssistant } from '@/components/ai/AiAssistantContext'
import { getProjectBySlugOptions } from '@/api/client/@tanstack/react-query.gen'
import { Button } from '@/components/ui/button'
import { useQuery } from '@tanstack/react-query'
import { Sparkles } from 'lucide-react'

/**
 * Top-bar entry point to the persistent AI assistant dock (ADR-023). Renders
 * only inside a project that has AI debug chat enabled. Toggles the dock open on
 * its conversation list, so any past chat can be resumed — and because the dock
 * lives in the app shell, it stays open while you navigate the project.
 */
export function AiAssistantButton({ projectSlug }: { projectSlug: string }) {
  const { open, close, isOpen, projectId } = useAiAssistant()
  const { data: project } = useQuery({
    ...getProjectBySlugOptions({ path: { slug: projectSlug } }),
  })

  if (!project || project.ai_debug_chat_enabled !== true) return null

  const showingThisProject = isOpen && projectId === project.id
  const onClick = () =>
    showingThisProject ? close() : open({ projectId: project.id })

  return (
    <Button
      variant={showingThisProject ? 'secondary' : 'outline'}
      size="icon"
      onClick={onClick}
      title="AI assistant"
      aria-pressed={showingThisProject}
    >
      <Sparkles className="h-4 w-4" />
      <span className="sr-only">AI assistant</span>
    </Button>
  )
}
