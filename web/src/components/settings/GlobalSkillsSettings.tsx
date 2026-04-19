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
import { usePageTitle } from '@/hooks/usePageTitle'
import { useMutation, useQuery } from '@tanstack/react-query'
import {
  ChevronRight,
  EllipsisVertical,
  FileCode,
  Loader2,
  Plus,
  Search,
  Wand2,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import {
  type SkillDefinition,
  listGlobalSkillDefinitions,
  createGlobalSkillDefinition,
  updateGlobalSkillDefinition,
  deleteGlobalSkillDefinition,
} from '@/components/agents/api'

export function GlobalSkillsSettings() {
  usePageTitle('Skills')
  const navigate = useNavigate()

  const [skillToDelete, setSkillToDelete] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editingSkill, setEditingSkill] = useState<SkillDefinition | null>(null)

  const {
    data: skills,
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['global-skills'],
    queryFn: () => listGlobalSkillDefinitions(),
  })

  const deleteMutation = useMutation({
    mutationFn: (slug: string) => deleteGlobalSkillDefinition(slug),
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
          <h2 className="text-lg font-semibold">Global Skills</h2>
          <p className="text-sm text-muted-foreground mt-1">
            Platform-wide skill definitions available to all projects. Skills
            are injected as{' '}
            <code className="text-xs bg-muted px-1 rounded">
              .claude/skills/
            </code>{' '}
            files in workflow sandboxes.
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
        <div data-uidotsh-pick="Skills layout" className="contents">
          <div data-uidotsh-option="Cards stack (current)" className="contents">
            <div className="space-y-3">
              {skills.map((skill) => (
                <Card
                  key={skill.id}
                  className="cursor-pointer hover:bg-muted/30 transition-colors"
                  onClick={() => navigate(`/settings/skills/${skill.slug}`)}
                >
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
                            <Badge
                              variant="secondary"
                              className="font-mono text-xs"
                            >
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
                      <SkillRowMenu
                        skill={skill}
                        onView={() =>
                          navigate(`/settings/skills/${skill.slug}`)
                        }
                        onEdit={() => openEdit(skill)}
                        onDelete={() => setSkillToDelete(skill.slug)}
                      />
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
          </div>

          <div data-uidotsh-option="Compact rows" className="contents" hidden>
            <SkillsCompactRows
              skills={skills}
              onOpen={(slug) => navigate(`/settings/skills/${slug}`)}
              onEdit={openEdit}
              onDelete={setSkillToDelete}
            />
          </div>

          <div data-uidotsh-option="Grid tiles" className="contents" hidden>
            <SkillsGridTiles
              skills={skills}
              onOpen={(slug) => navigate(`/settings/skills/${slug}`)}
              onEdit={openEdit}
              onDelete={setSkillToDelete}
            />
          </div>

          <div data-uidotsh-option="Master / detail" className="contents" hidden>
            <SkillsMasterDetail
              skills={skills}
              onOpen={(slug) => navigate(`/settings/skills/${slug}`)}
              onEdit={openEdit}
              onDelete={setSkillToDelete}
            />
          </div>
        </div>
      ) : !error ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-12">
            <FileCode className="h-12 w-12 text-muted-foreground/50 mb-4" />
            <h3 className="text-lg font-semibold mb-2">No global skills</h3>
            <p className="text-sm text-muted-foreground text-center mb-4 max-w-md">
              Global skills are available to all projects. Define common
              patterns, coding standards, or reusable instructions here.
            </p>
            <Button onClick={openCreate}>
              <Plus className="h-4 w-4 mr-2" />
              Create Your First Skill
            </Button>
          </CardContent>
        </Card>
      ) : null}

      <GlobalSkillDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
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
            <AlertDialogTitle>Delete global skill?</AlertDialogTitle>
            <AlertDialogDescription>
              This will permanently delete this skill. All projects that
              reference it will lose access.
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

// ── Shared row menu ──

interface SkillRowMenuProps {
  skill: SkillDefinition
  onView: () => void
  onEdit: () => void
  onDelete: () => void
}

function SkillRowMenu({ onView, onEdit, onDelete }: SkillRowMenuProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild onClick={(e) => e.stopPropagation()}>
        <Button variant="ghost" size="icon" className="h-8 w-8">
          <EllipsisVertical className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
        <DropdownMenuItem onClick={onView}>View details</DropdownMenuItem>
        <DropdownMenuItem onClick={onEdit}>Edit</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive" onClick={onDelete}>
          Delete
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

// ── Variant: Compact rows ──

interface SkillsListProps {
  skills: SkillDefinition[]
  onOpen: (slug: string) => void
  onEdit: (skill: SkillDefinition) => void
  onDelete: (slug: string) => void
}

function SkillsCompactRows({
  skills,
  onOpen,
  onEdit,
  onDelete,
}: SkillsListProps) {
  return (
    <div className="overflow-hidden rounded-lg border">
      <ul role="list" className="divide-y">
        {skills.map((skill) => (
          <li
            key={skill.id}
            onClick={() => onOpen(skill.slug)}
            className="flex cursor-pointer items-center gap-4 px-4 py-3 hover:bg-muted/40 transition-colors"
          >
            <div className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted">
              <Wand2 className="size-4 text-muted-foreground" />
            </div>
            <div className="flex min-w-0 flex-1 items-center gap-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <p className="truncate text-sm font-medium">{skill.name}</p>
                  <Badge variant="secondary" className="font-mono text-xs">
                    {skill.slug}
                  </Badge>
                </div>
                {skill.description && (
                  <p className="mt-0.5 truncate text-xs text-muted-foreground">
                    {skill.description}
                  </p>
                )}
              </div>
            </div>
            <SkillRowMenu
              skill={skill}
              onView={() => onOpen(skill.slug)}
              onEdit={() => onEdit(skill)}
              onDelete={() => onDelete(skill.slug)}
            />
            <ChevronRight className="size-4 shrink-0 text-muted-foreground/50" />
          </li>
        ))}
      </ul>
    </div>
  )
}

// ── Variant: Grid tiles ──

function SkillsGridTiles({
  skills,
  onOpen,
  onEdit,
  onDelete,
}: SkillsListProps) {
  return (
    <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
      {skills.map((skill) => (
        <Card
          key={skill.id}
          onClick={() => onOpen(skill.slug)}
          className="group cursor-pointer transition-colors hover:bg-muted/30"
        >
          <CardHeader className="pb-3">
            <div className="flex items-start justify-between gap-2">
              <div className="flex size-9 items-center justify-center rounded-md bg-muted">
                <Wand2 className="size-4 text-muted-foreground" />
              </div>
              <SkillRowMenu
                skill={skill}
                onView={() => onOpen(skill.slug)}
                onEdit={() => onEdit(skill)}
                onDelete={() => onDelete(skill.slug)}
              />
            </div>
            <div className="mt-3 space-y-1">
              <CardTitle className="truncate text-base">{skill.name}</CardTitle>
              <Badge variant="secondary" className="font-mono text-xs">
                {skill.slug}
              </Badge>
            </div>
          </CardHeader>
          <CardContent>
            {skill.description ? (
              <p className="line-clamp-3 text-xs text-muted-foreground">
                {skill.description}
              </p>
            ) : (
              <p className="text-xs italic text-muted-foreground/60">
                No description
              </p>
            )}
          </CardContent>
        </Card>
      ))}
    </div>
  )
}

// ── Variant: Master / detail ──

function SkillsMasterDetail({
  skills,
  onOpen,
  onEdit,
  onDelete,
}: SkillsListProps) {
  const [selectedSlug, setSelectedSlug] = useState<string | null>(
    skills[0]?.slug ?? null
  )
  const [query, setQuery] = useState('')

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return skills
    return skills.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.slug.toLowerCase().includes(q) ||
        (s.description ?? '').toLowerCase().includes(q)
    )
  }, [skills, query])

  const selected =
    skills.find((s) => s.slug === selectedSlug) ?? filtered[0] ?? skills[0]

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-[320px_1fr]">
      <div className="overflow-hidden rounded-lg border">
        <div className="border-b p-2">
          <div className="relative">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter skills"
              className="h-8 pl-8 text-sm"
            />
          </div>
        </div>
        <ul
          role="list"
          className="max-h-[480px] divide-y overflow-y-auto"
        >
          {filtered.map((skill) => {
            const isActive = selected?.slug === skill.slug
            return (
              <li
                key={skill.id}
                onClick={() => setSelectedSlug(skill.slug)}
                className={`cursor-pointer px-3 py-2.5 transition-colors ${
                  isActive ? 'bg-muted' : 'hover:bg-muted/40'
                }`}
              >
                <div className="flex items-center gap-2">
                  <Wand2 className="size-3.5 shrink-0 text-muted-foreground" />
                  <p className="truncate text-sm font-medium">{skill.name}</p>
                </div>
                <p className="mt-0.5 truncate pl-5 font-mono text-xs text-muted-foreground">
                  {skill.slug}
                </p>
              </li>
            )
          })}
          {filtered.length === 0 && (
            <li className="px-3 py-6 text-center text-xs text-muted-foreground">
              No skills match "{query}"
            </li>
          )}
        </ul>
      </div>

      {selected ? (
        <Card>
          <CardHeader>
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <CardTitle className="truncate text-base">
                    {selected.name}
                  </CardTitle>
                  <Badge variant="secondary" className="font-mono text-xs">
                    {selected.slug}
                  </Badge>
                </div>
                {selected.description && (
                  <CardDescription className="mt-1 text-xs">
                    {selected.description}
                  </CardDescription>
                )}
              </div>
              <div className="flex items-center gap-1">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => onOpen(selected.slug)}
                >
                  Open
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => onEdit(selected)}
                >
                  Edit
                </Button>
                <SkillRowMenu
                  skill={selected}
                  onView={() => onOpen(selected.slug)}
                  onEdit={() => onEdit(selected)}
                  onDelete={() => onDelete(selected.slug)}
                />
              </div>
            </div>
          </CardHeader>
          <CardContent>
            <div className="rounded-md border bg-muted/50 p-3">
              <pre className="max-h-[420px] overflow-y-auto whitespace-pre-wrap font-mono text-xs text-muted-foreground">
                {selected.content}
              </pre>
            </div>
          </CardContent>
        </Card>
      ) : null}
    </div>
  )
}

// ── Create / Edit Dialog ──

interface GlobalSkillDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  skill: SkillDefinition | null
  onSuccess: () => void
}

function GlobalSkillDialog({
  open,
  onOpenChange,
  skill,
  onSuccess,
}: GlobalSkillDialogProps) {
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
        await updateGlobalSkillDefinition(skill!.slug, {
          name: name.trim(),
          description: description.trim() || undefined,
          content: content,
        })
        toast.success('Skill updated')
      } else {
        await createGlobalSkillDefinition({
          slug: slug.trim(),
          name: name.trim(),
          description: description.trim() || undefined,
          content: content,
        })
        toast.success('Skill created')
      }
      onSuccess()
    } catch {
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
          <DialogTitle>
            {isEdit ? 'Edit Global Skill' : 'Create Global Skill'}
          </DialogTitle>
          <DialogDescription>
            {isEdit
              ? 'Update this global skill definition.'
              : 'Define a new global skill available to all projects.'}
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
                placeholder="e.g. Code Review"
                required
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="skill-slug">Slug</Label>
              <Input
                id="skill-slug"
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                placeholder="e.g. code-review"
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
