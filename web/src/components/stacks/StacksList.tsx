import {
  createStack,
  deleteStack,
  deployStack,
  discoverComposeFiles,
  listStacks,
  restartStack,
  stopStack,
  updateStack,
  type CreateStackRequest,
  type Stack,
  type UpdateStackRequest,
} from '@/api/stacks'
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  FileText,
  GitBranch,
  Layers,
  Loader2,
  MoreHorizontal,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Search,
  Trash2,
} from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'

function stateVariant(state: string) {
  switch (state) {
    case 'running':
      return 'default'
    case 'stopped':
      return 'secondary'
    case 'error':
      return 'destructive'
    default:
      return 'outline'
  }
}

function CreateStackDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const queryClient = useQueryClient()
  const [mode, setMode] = useState<'compose' | 'repo'>('compose')
  const [pathMode, setPathMode] = useState<'auto' | 'manual'>('auto')
  const [discoveredFiles, setDiscoveredFiles] = useState<string[]>([])
  const [form, setForm] = useState<CreateStackRequest>({
    name: '',
    compose_content: '',
  })

  const discoverMutation = useMutation({
    mutationFn: () =>
      discoverComposeFiles({
        repo_url: form.repo_url!,
        repo_branch: form.repo_branch,
        repo_access_token: form.repo_access_token,
      }),
    meta: { errorTitle: 'Failed to scan repository' },
    onSuccess: (res) => {
      const files = res.data?.files ?? []
      setDiscoveredFiles(files)
      if (files.length > 0) {
        setForm((f) => ({ ...f, repo_compose_path: files[0] }))
        setPathMode('auto')
      } else {
        setPathMode('manual')
        toast.info('No compose files found. Enter the path manually.')
      }
    },
  })

  const createMutation = useMutation({
    mutationFn: () => createStack(form),
    meta: { errorTitle: 'Failed to create stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      onOpenChange(false)
      setForm({ name: '', compose_content: '' })
      setMode('compose')
      setPathMode('auto')
      setDiscoveredFiles([])
      toast.success('Stack created successfully')
    },
  })

  const isValid =
    form.name &&
    (mode === 'compose' ? !!form.compose_content : !!form.repo_url)

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (!v) {
          setForm({ name: '', compose_content: '' })
          setMode('compose')
          setPathMode('auto')
          setDiscoveredFiles([])
        }
        onOpenChange(v)
      }}
    >
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Create Stack</DialogTitle>
          <DialogDescription>
            Create a new Docker Compose stack from a compose file or a git
            repository.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="name">Name</Label>
            <Input
              id="name"
              placeholder="my-stack"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="description">Description</Label>
            <Input
              id="description"
              placeholder="Optional description"
              value={form.description ?? ''}
              onChange={(e) =>
                setForm({
                  ...form,
                  description: e.target.value || undefined,
                })
              }
            />
          </div>

          <Tabs
            value={mode}
            onValueChange={(v) => setMode(v as 'compose' | 'repo')}
          >
            <TabsList className="w-full">
              <TabsTrigger value="compose" className="flex-1">
                Compose File
              </TabsTrigger>
              <TabsTrigger value="repo" className="flex-1">
                <GitBranch className="mr-2 h-4 w-4" />
                From Repository
              </TabsTrigger>
            </TabsList>

            <TabsContent value="compose" className="space-y-4 mt-4">
              <div className="space-y-2">
                <Label htmlFor="compose">Compose File</Label>
                <Textarea
                  id="compose"
                  placeholder={`version: '3'\nservices:\n  web:\n    image: nginx:latest\n    ports:\n      - "8080:80"`}
                  className="font-mono text-sm min-h-[200px]"
                  value={form.compose_content ?? ''}
                  onChange={(e) =>
                    setForm({ ...form, compose_content: e.target.value })
                  }
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="env">Environment Variables (.env)</Label>
                <Textarea
                  id="env"
                  placeholder={`# Optional .env content\nDB_HOST=localhost\nDB_PORT=5432`}
                  className="font-mono text-sm min-h-[80px]"
                  value={form.env_content ?? ''}
                  onChange={(e) =>
                    setForm({
                      ...form,
                      env_content: e.target.value || undefined,
                    })
                  }
                />
              </div>
            </TabsContent>

            <TabsContent value="repo" className="space-y-4 mt-4">
              <div className="space-y-2">
                <Label htmlFor="repo-url">Repository URL</Label>
                <div className="flex gap-2">
                  <Input
                    id="repo-url"
                    placeholder="https://github.com/user/repo.git"
                    value={form.repo_url ?? ''}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        repo_url: e.target.value || undefined,
                      })
                    }
                  />
                  <Button
                    type="button"
                    variant="outline"
                    disabled={!form.repo_url || discoverMutation.isPending}
                    onClick={() => discoverMutation.mutate()}
                  >
                    {discoverMutation.isPending ? (
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    ) : (
                      <Search className="mr-2 h-4 w-4" />
                    )}
                    Scan
                  </Button>
                </div>
              </div>
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-2">
                  <Label htmlFor="repo-branch">Branch</Label>
                  <Input
                    id="repo-branch"
                    placeholder="main (default)"
                    value={form.repo_branch ?? ''}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        repo_branch: e.target.value || undefined,
                      })
                    }
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="repo-token">
                    Access Token (private repos)
                  </Label>
                  <Input
                    id="repo-token"
                    type="password"
                    placeholder="Optional access token"
                    value={form.repo_access_token ?? ''}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        repo_access_token: e.target.value || undefined,
                      })
                    }
                  />
                </div>
              </div>

              {discoveredFiles.length > 0 && (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <Label>Compose File</Label>
                    <div className="flex gap-1 ml-auto">
                      <Button
                        type="button"
                        variant={pathMode === 'auto' ? 'default' : 'ghost'}
                        size="sm"
                        onClick={() => {
                          setPathMode('auto')
                          setForm((f) => ({
                            ...f,
                            repo_compose_path: discoveredFiles[0],
                          }))
                        }}
                      >
                        Auto
                      </Button>
                      <Button
                        type="button"
                        variant={pathMode === 'manual' ? 'default' : 'ghost'}
                        size="sm"
                        onClick={() => setPathMode('manual')}
                      >
                        Manual
                      </Button>
                    </div>
                  </div>
                  {pathMode === 'auto' ? (
                    <div className="space-y-1.5">
                      {discoveredFiles.map((file) => (
                        <button
                          key={file}
                          type="button"
                          className={`flex items-center gap-2 w-full rounded-md border px-3 py-2 text-sm text-left transition-colors ${
                            form.repo_compose_path === file
                              ? 'border-primary bg-primary/5'
                              : 'border-border hover:bg-muted/50'
                          }`}
                          onClick={() =>
                            setForm({ ...form, repo_compose_path: file })
                          }
                        >
                          <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
                          <span className="font-mono text-xs">{file}</span>
                        </button>
                      ))}
                    </div>
                  ) : (
                    <Input
                      placeholder="path/to/docker-compose.yml"
                      value={form.repo_compose_path ?? ''}
                      onChange={(e) =>
                        setForm({
                          ...form,
                          repo_compose_path: e.target.value || undefined,
                        })
                      }
                    />
                  )}
                </div>
              )}

              {discoveredFiles.length === 0 && !discoverMutation.isPending && (
                <div className="space-y-2">
                  <Label htmlFor="repo-path">Compose File Path</Label>
                  <Input
                    id="repo-path"
                    placeholder="docker-compose.yml"
                    value={form.repo_compose_path ?? ''}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        repo_compose_path: e.target.value || undefined,
                      })
                    }
                  />
                  <p className="text-xs text-muted-foreground">
                    Click Scan to auto-discover compose files, or enter the path
                    manually.
                  </p>
                </div>
              )}
            </TabsContent>
          </Tabs>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            onClick={() => createMutation.mutate()}
            disabled={createMutation.isPending || !isValid}
          >
            {createMutation.isPending && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function EditStackDialog({
  stack,
  open,
  onOpenChange,
}: {
  stack: Stack | null
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const queryClient = useQueryClient()
  const [form, setForm] = useState<UpdateStackRequest>({})

  const updateMutation = useMutation({
    mutationFn: () => {
      if (!stack) throw new Error('No stack')
      return updateStack(stack.id, form)
    },
    meta: { errorTitle: 'Failed to update stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      onOpenChange(false)
      toast.success('Stack updated successfully')
    },
  })

  // Initialize form when stack changes
  if (stack && !form.name && !form.compose_content) {
    setForm({
      name: stack.name,
      compose_content: stack.compose_content,
      description: stack.description,
      env_content: stack.env_content,
    })
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (!v) setForm({})
        onOpenChange(v)
      }}
    >
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Edit Stack</DialogTitle>
          <DialogDescription>
            Update the stack configuration. Changes will apply on next deploy.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="edit-name">Name</Label>
            <Input
              id="edit-name"
              value={form.name ?? ''}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="edit-description">Description</Label>
            <Input
              id="edit-description"
              value={form.description ?? ''}
              onChange={(e) =>
                setForm({
                  ...form,
                  description: e.target.value || null,
                })
              }
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="edit-compose">Compose File</Label>
            <Textarea
              id="edit-compose"
              className="font-mono text-sm min-h-[200px]"
              value={form.compose_content ?? ''}
              onChange={(e) =>
                setForm({ ...form, compose_content: e.target.value })
              }
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="edit-env">Environment Variables (.env)</Label>
            <Textarea
              id="edit-env"
              className="font-mono text-sm min-h-[80px]"
              value={form.env_content ?? ''}
              onChange={(e) =>
                setForm({
                  ...form,
                  env_content: e.target.value || null,
                })
              }
            />
          </div>
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => {
              setForm({})
              onOpenChange(false)
            }}
          >
            Cancel
          </Button>
          <Button
            onClick={() => updateMutation.mutate()}
            disabled={updateMutation.isPending}
          >
            {updateMutation.isPending && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function StacksList() {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [isCreateOpen, setIsCreateOpen] = useState(false)
  const [editStack, setEditStack] = useState<Stack | null>(null)

  const { data, isLoading } = useQuery({
    queryKey: ['stacks'],
    queryFn: async () => {
      const { data } = await listStacks()
      return data
    },
  })

  const deployMutation = useMutation({
    mutationFn: (id: number) => deployStack(id),
    meta: { errorTitle: 'Failed to deploy stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack deployed')
    },
  })

  const stopMutation = useMutation({
    mutationFn: (id: number) => stopStack(id),
    meta: { errorTitle: 'Failed to stop stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack stopped')
    },
  })

  const restartMutation = useMutation({
    mutationFn: (id: number) => restartStack(id),
    meta: { errorTitle: 'Failed to restart stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack restarted')
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (id: number) => deleteStack(id),
    meta: { errorTitle: 'Failed to delete stack' },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stacks'] })
      toast.success('Stack deleted')
    },
  })

  const stacks = data?.items ?? []

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-[200px]">
        <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
      </div>
    )
  }

  return (
    <>
      <Card>
        <CardHeader>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <CardTitle>Docker Compose Stacks</CardTitle>
              <CardDescription>
                Manage standalone Docker Compose stacks separate from your
                projects.
              </CardDescription>
            </div>
            <Button onClick={() => setIsCreateOpen(true)}>
              <Plus className="mr-2 h-4 w-4" />
              <span className="hidden sm:inline">New Stack</span>
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {stacks.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-12 text-center">
              <Layers className="h-12 w-12 text-muted-foreground mb-4" />
              <h3 className="text-lg font-medium">No stacks yet</h3>
              <p className="text-sm text-muted-foreground mt-1 mb-4">
                Create your first Docker Compose stack to get started.
              </p>
              <Button onClick={() => setIsCreateOpen(true)}>
                <Plus className="mr-2 h-4 w-4" />
                Create Stack
              </Button>
            </div>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead className="hidden md:table-cell">
                      Description
                    </TableHead>
                    <TableHead>State</TableHead>
                    <TableHead className="hidden md:table-cell">
                      Source
                    </TableHead>
                    <TableHead className="hidden md:table-cell">
                      Updated
                    </TableHead>
                    <TableHead className="w-[50px]" />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {stacks.map((stack) => (
                    <TableRow
                      key={stack.id}
                      className="cursor-pointer"
                      onClick={() => navigate(`/stacks/${stack.id}`)}
                    >
                      <TableCell className="font-medium">
                        {stack.name}
                      </TableCell>
                      <TableCell className="hidden md:table-cell text-muted-foreground">
                        {stack.description || '-'}
                      </TableCell>
                      <TableCell>
                        <Badge variant={stateVariant(stack.state)}>
                          {stack.state}
                        </Badge>
                      </TableCell>
                      <TableCell className="hidden md:table-cell text-muted-foreground text-sm">
                        {stack.repo_url ? (
                          <span className="flex items-center gap-1 text-xs">
                            <GitBranch className="h-3 w-3" />
                            {stack.repo_branch ?? 'default'}
                          </span>
                        ) : (
                          <span className="text-xs">Compose</span>
                        )}
                      </TableCell>
                      <TableCell className="hidden md:table-cell text-muted-foreground text-sm">
                        {new Date(stack.updated_at).toLocaleDateString()}
                      </TableCell>
                      <TableCell>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              variant="ghost"
                              size="icon"
                              onClick={(e) => e.stopPropagation()}
                            >
                              <MoreHorizontal className="h-4 w-4" />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end">
                            <DropdownMenuItem
                              onClick={() => setEditStack(stack)}
                            >
                              Edit
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            {stack.state !== 'running' ? (
                              <DropdownMenuItem
                                onClick={() =>
                                  deployMutation.mutate(stack.id)
                                }
                              >
                                <Play className="mr-2 h-4 w-4" />
                                Deploy
                              </DropdownMenuItem>
                            ) : (
                              <>
                                <DropdownMenuItem
                                  onClick={() =>
                                    stopMutation.mutate(stack.id)
                                  }
                                >
                                  <Pause className="mr-2 h-4 w-4" />
                                  Stop
                                </DropdownMenuItem>
                                <DropdownMenuItem
                                  onClick={() =>
                                    restartMutation.mutate(stack.id)
                                  }
                                >
                                  <RefreshCw className="mr-2 h-4 w-4" />
                                  Restart
                                </DropdownMenuItem>
                              </>
                            )}
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              className="text-destructive"
                              onClick={() =>
                                deleteMutation.mutate(stack.id)
                              }
                              disabled={stack.state === 'running'}
                            >
                              <Trash2 className="mr-2 h-4 w-4" />
                              Delete
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>

      <CreateStackDialog open={isCreateOpen} onOpenChange={setIsCreateOpen} />
      <EditStackDialog
        stack={editStack}
        open={!!editStack}
        onOpenChange={(open) => {
          if (!open) setEditStack(null)
        }}
      />
    </>
  )
}
