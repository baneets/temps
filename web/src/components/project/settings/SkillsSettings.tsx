import { ProjectResponse } from '@/api/client'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { useMutation, useQuery } from '@tanstack/react-query'
import {
  EllipsisVertical,
  FileCode,
  Loader2,
  Plus,
  Wand2,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import {
  type SkillDefinition,
  listSkillDefinitions,
  createSkillDefinition,
  updateSkillDefinition,
  deleteSkillDefinition,
} from '@/components/agents/api'

interface SkillsSettingsProps {
  project: ProjectResponse
}

export function SkillsSettings({ project }: SkillsSettingsProps) {
  const [skillToDelete, setSkillToDelete] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editingSkill, setEditingSkill] = useState<SkillDefinition | null>(null)

  const {
    data: skills,
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['skills', project.id],
    queryFn: () => listSkillDefinitions(project.id),
  })

  const deleteMutation = useMutation({
    mutationFn: (slug: string) => deleteSkillDefinition(project.id, slug),
    onSuccess: () => {
      toast.success('Skill deleted')
      refetch()
      setSkillToDelete(null)
    },
    onError: () => toast.error('Failed to delete skill'),
  })

  const openCreate = () => {
    setEditingSkill(null)
    setDialogOpen(true)
  }

  const openEdit = (skill: SkillDefinition) => {
    setEditingSkill(skill)
    setDialogOpen(true)
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-lg font-semibold">Skills</h2>
          <p className="text-sm text-muted-foreground mt-1">
            Define reusable skill definitions that can be assigned to AI
            workflows. Skills are injected as{' '}
            <code className="text-xs bg-muted px-1 rounded">
              .claude/skills/
            </code>{' '}
            files in the sandbox.
          </p>
        </div>
        <Button onClick={openCreate} disabled={isLoading}>
          <Plus className="h-4 w-4 mr-2" />
          Add Skill
        </Button>
      </div>

      {error && (
        <Card>
          <CardContent className="py-6">
            <p className="text-sm text-destructive">
              Failed to load skills.{' '}
              <button
                onClick={() => refetch()}
                className="underline hover:no-underline"
              >
                Retry
              </button>
            </p>
          </CardContent>
        </Card>
      )}

      {isLoading ? (
        <div className="flex items-center justify-center py-12">
          <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      ) : !error && skills && skills.length > 0 ? (
        <div className="space-y-3">
          {skills.map((skill) => (
            <Card key={skill.id}>
              <CardHeader className="pb-2">
                <div className="flex items-start justify-between">
                  <div className="flex items-start gap-3 flex-1">
                    <div className="mt-1">
                      <Wand2 className="h-5 w-5 text-muted-foreground" />
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 mb-1">
                        <CardTitle className="text-base">
                          {skill.name}
                        </CardTitle>
                        <Badge variant="secondary" className="font-mono text-xs">
                          {skill.slug}
                        </Badge>
                      </div>
                      {skill.description && (
                        <CardDescription className="text-xs">
                          {skill.description}
                        </CardDescription>
                      )}
                    </div>
                  </div>
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="ghost" size="icon" className="h-8 w-8">
                        <EllipsisVertical className="h-4 w-4" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onClick={() => openEdit(skill)}>
                        Edit
                      </DropdownMenuItem>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem
                        className="text-destructive"
                        onClick={() => setSkillToDelete(skill.slug)}
                      >
                        Delete
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              </CardHeader>
              <CardContent>
                <div className="rounded-md border bg-muted/50 p-3">
                  <pre className="text-xs text-muted-foreground whitespace-pre-wrap line-clamp-4 font-mono">
                    {skill.content}
                  </pre>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      ) : !error ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-12">
            <FileCode className="h-12 w-12 text-muted-foreground/50 mb-4" />
            <h3 className="text-lg font-semibold mb-2">No skills defined</h3>
            <p className="text-sm text-muted-foreground text-center mb-4 max-w-md">
              Skills are reusable instruction sets for AI workflows. Define
              skills here and assign them to workflows to customize their
              behavior.
            </p>
            <Button onClick={openCreate}>
              <Plus className="h-4 w-4 mr-2" />
              Create Your First Skill
            </Button>
          </CardContent>
        </Card>
      ) : null}

      <SkillDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        projectId={project.id}
        skill={editingSkill}
        onSuccess={() => {
          refetch()
          setDialogOpen(false)
        }}
      />

      <AlertDialog
        open={skillToDelete !== null}
        onOpenChange={() => setSkillToDelete(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete skill?</AlertDialogTitle>
            <AlertDialogDescription>
              This will permanently delete this skill definition. Workflows
              referencing it will no longer have access to it.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (skillToDelete) deleteMutation.mutate(skillToDelete)
              }}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

// ── Create / Edit Dialog ──

interface SkillDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  projectId: number
  skill: SkillDefinition | null
  onSuccess: () => void
}

function SkillDialog({
  open,
  onOpenChange,
  projectId,
  skill,
  onSuccess,
}: SkillDialogProps) {
  const isEdit = !!skill
  const [slug, setSlug] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [content, setContent] = useState('')
  const [isPending, setIsPending] = useState(false)

  useEffect(() => {
    if (open) {
      if (skill) {
        setSlug(skill.slug)
        setName(skill.name)
        setDescription(skill.description || '')
        setContent(skill.content)
      } else {
        setSlug('')
        setName('')
        setDescription('')
        setContent('')
      }
    }
  }, [open, skill])

  // Auto-generate slug from name
  const handleNameChange = (value: string) => {
    setName(value)
    if (!isEdit) {
      setSlug(
        value
          .toLowerCase()
          .replace(/[^a-z0-9]+/g, '-')
          .replace(/^-|-$/g, '')
      )
    }
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!name.trim() || !slug.trim() || !content.trim()) return

    setIsPending(true)
    try {
      if (isEdit) {
        await updateSkillDefinition(projectId, skill!.slug, {
          name: name.trim(),
          description: description.trim() || undefined,
          content: content,
        })
        toast.success('Skill updated')
      } else {
        await createSkillDefinition(projectId, {
          slug: slug.trim(),
          name: name.trim(),
          description: description.trim() || undefined,
          content: content,
        })
        toast.success('Skill created')
      }
      onSuccess()
    } catch (err) {
      toast.error(
        isEdit ? 'Failed to update skill' : 'Failed to create skill'
      )
    } finally {
      setIsPending(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{isEdit ? 'Edit Skill' : 'Create Skill'}</DialogTitle>
          <DialogDescription>
            {isEdit
              ? 'Update this skill definition.'
              : 'Define a new skill that can be assigned to AI workflows.'}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <div className="space-y-2">
              <Label htmlFor="skill-name">Name</Label>
              <Input
                id="skill-name"
                value={name}
                onChange={(e) => handleNameChange(e.target.value)}
                placeholder="e.g. Blog Writer"
                required
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="skill-slug">Slug</Label>
              <Input
                id="skill-slug"
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                placeholder="e.g. blog-writer"
                disabled={isEdit}
                required
                className="font-mono"
              />
              {isEdit && (
                <p className="text-xs text-muted-foreground">
                  Slug cannot be changed after creation.
                </p>
              )}
            </div>
          </div>
          <div className="space-y-2">
            <Label htmlFor="skill-description">Description</Label>
            <Input
              id="skill-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Brief description of what this skill does"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="skill-content">Content</Label>
            <p className="text-xs text-muted-foreground">
              The skill instructions in markdown. This becomes the SKILL.md
              file content.
            </p>
            <Textarea
              id="skill-content"
              value={content}
              onChange={(e) => setContent(e.target.value)}
              placeholder={`---\nname: my-skill\ndescription: What this skill does\n---\n\nInstructions for the AI when this skill is active...`}
              required
              className="font-mono text-sm min-h-[200px]"
              rows={12}
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={isPending}>
              {isPending ? (
                <Loader2 className="h-4 w-4 animate-spin mr-2" />
              ) : null}
              {isPending
                ? isEdit
                  ? 'Saving...'
                  : 'Creating...'
                : isEdit
                  ? 'Save'
                  : 'Create Skill'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
