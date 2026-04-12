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
import { EllipsisVertical, Loader2, Plus, Server } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import {
  type McpDefinition,
  listGlobalMcpDefinitions,
  createGlobalMcpDefinition,
  updateGlobalMcpDefinition,
  deleteGlobalMcpDefinition,
} from '@/components/agents/api'

export function GlobalMcpServersSettings() {
  usePageTitle('MCP Servers')

  const [mcpToDelete, setMcpToDelete] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editingMcp, setEditingMcp] = useState<McpDefinition | null>(null)

  const {
    data: mcpServers,
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['global-mcp-servers'],
    queryFn: () => listGlobalMcpDefinitions(),
  })

  const deleteMutation = useMutation({
    mutationFn: (slug: string) => deleteGlobalMcpDefinition(slug),
    onSuccess: () => {
      toast.success('MCP server deleted')
      refetch()
      setMcpToDelete(null)
    },
    onError: () => toast.error('Failed to delete MCP server'),
  })

  const openCreate = () => {
    setEditingMcp(null)
    setDialogOpen(true)
  }

  const openEdit = (mcp: McpDefinition) => {
    setEditingMcp(mcp)
    setDialogOpen(true)
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-lg font-semibold">Global MCP Servers</h2>
          <p className="text-sm text-muted-foreground mt-1">
            Platform-wide MCP server configurations available to all projects.
            Configs are merged into{' '}
            <code className="text-xs bg-muted px-1 rounded">
              .claude/settings.json
            </code>{' '}
            at workflow runtime.
          </p>
        </div>
        <Button onClick={openCreate} disabled={isLoading}>
          <Plus className="h-4 w-4 mr-2" />
          Add MCP Server
        </Button>
      </div>

      {error && (
        <Card>
          <CardContent className="py-6">
            <p className="text-sm text-destructive">
              Failed to load MCP servers.{' '}
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
      ) : !error && mcpServers && mcpServers.length > 0 ? (
        <div className="space-y-3">
          {mcpServers.map((mcp) => {
            const command = mcp.config.command as string | undefined
            const args = mcp.config.args as string[] | undefined
            return (
              <Card key={mcp.id}>
                <CardHeader className="pb-2">
                  <div className="flex items-start justify-between">
                    <div className="flex items-start gap-3 flex-1">
                      <div className="mt-1">
                        <Server className="h-5 w-5 text-muted-foreground" />
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2 mb-1">
                          <CardTitle className="text-base">
                            {mcp.name}
                          </CardTitle>
                          <Badge
                            variant="secondary"
                            className="font-mono text-xs"
                          >
                            {mcp.slug}
                          </Badge>
                        </div>
                        {mcp.description && (
                          <CardDescription className="text-xs">
                            {mcp.description}
                          </CardDescription>
                        )}
                      </div>
                    </div>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-8 w-8"
                        >
                          <EllipsisVertical className="h-4 w-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem onClick={() => openEdit(mcp)}>
                          Edit
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          className="text-destructive"
                          onClick={() => setMcpToDelete(mcp.slug)}
                        >
                          Delete
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </CardHeader>
                <CardContent>
                  {command && (
                    <div className="text-xs text-muted-foreground mb-2">
                      <span className="font-medium">Command:</span>{' '}
                      <code className="bg-muted px-1 rounded">
                        {command}
                        {args ? ` ${args.join(' ')}` : ''}
                      </code>
                    </div>
                  )}
                  <div className="rounded-md border bg-muted/50 p-3">
                    <pre className="text-xs text-muted-foreground whitespace-pre-wrap line-clamp-4 font-mono">
                      {JSON.stringify(mcp.config, null, 2)}
                    </pre>
                  </div>
                </CardContent>
              </Card>
            )
          })}
        </div>
      ) : !error ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-12">
            <Server className="h-12 w-12 text-muted-foreground/50 mb-4" />
            <h3 className="text-lg font-semibold mb-2">
              No global MCP servers
            </h3>
            <p className="text-sm text-muted-foreground text-center mb-4 max-w-md">
              Global MCP servers are available to all projects. Define common
              tools like browser automation, database access, or file system
              servers here.
            </p>
            <Button onClick={openCreate}>
              <Plus className="h-4 w-4 mr-2" />
              Create Your First MCP Server
            </Button>
          </CardContent>
        </Card>
      ) : null}

      <GlobalMcpDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        mcp={editingMcp}
        onSuccess={() => {
          refetch()
          setDialogOpen(false)
        }}
      />

      <AlertDialog
        open={mcpToDelete !== null}
        onOpenChange={() => setMcpToDelete(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete global MCP server?</AlertDialogTitle>
            <AlertDialogDescription>
              This will permanently delete this MCP server. All projects that
              reference it will lose access.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (mcpToDelete) deleteMutation.mutate(mcpToDelete)
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

const MCP_CONFIG_EXAMPLE = `{
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
  "env": {}
}`

interface GlobalMcpDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  mcp: McpDefinition | null
  onSuccess: () => void
}

function GlobalMcpDialog({
  open,
  onOpenChange,
  mcp,
  onSuccess,
}: GlobalMcpDialogProps) {
  const isEdit = !!mcp
  const [slug, setSlug] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [configText, setConfigText] = useState('')
  const [configError, setConfigError] = useState<string | null>(null)
  const [isPending, setIsPending] = useState(false)

  useEffect(() => {
    if (open) {
      if (mcp) {
        setSlug(mcp.slug)
        setName(mcp.name)
        setDescription(mcp.description || '')
        setConfigText(JSON.stringify(mcp.config, null, 2))
      } else {
        setSlug('')
        setName('')
        setDescription('')
        setConfigText(MCP_CONFIG_EXAMPLE)
      }
      setConfigError(null)
    }
  }, [open, mcp])

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

  const handleConfigChange = (value: string) => {
    setConfigText(value)
    try {
      JSON.parse(value)
      setConfigError(null)
    } catch {
      setConfigError('Invalid JSON')
    }
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!name.trim() || !slug.trim()) return

    let config: Record<string, unknown>
    try {
      config = JSON.parse(configText)
    } catch {
      setConfigError('Invalid JSON — please fix before saving')
      return
    }

    setIsPending(true)
    try {
      if (isEdit) {
        await updateGlobalMcpDefinition(mcp!.slug, {
          name: name.trim(),
          description: description.trim() || undefined,
          config,
        })
        toast.success('MCP server updated')
      } else {
        await createGlobalMcpDefinition({
          slug: slug.trim(),
          name: name.trim(),
          description: description.trim() || undefined,
          config,
        })
        toast.success('MCP server created')
      }
      onSuccess()
    } catch {
      toast.error(
        isEdit
          ? 'Failed to update MCP server'
          : 'Failed to create MCP server'
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
            {isEdit ? 'Edit Global MCP Server' : 'Create Global MCP Server'}
          </DialogTitle>
          <DialogDescription>
            {isEdit
              ? 'Update this global MCP server configuration.'
              : 'Define a new global MCP server available to all projects.'}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <div className="space-y-2">
              <Label htmlFor="mcp-name">Name</Label>
              <Input
                id="mcp-name"
                value={name}
                onChange={(e) => handleNameChange(e.target.value)}
                placeholder="e.g. Filesystem Server"
                required
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="mcp-slug">Slug</Label>
              <Input
                id="mcp-slug"
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                placeholder="e.g. filesystem"
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
            <Label htmlFor="mcp-description">Description</Label>
            <Input
              id="mcp-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Brief description of what this MCP server provides"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="mcp-config">Configuration (JSON)</Label>
            <p className="text-xs text-muted-foreground">
              MCP server config following the{' '}
              <code className="bg-muted px-1 rounded">mcpServers</code> format:
              command, args, and optional env vars.
            </p>
            <Textarea
              id="mcp-config"
              value={configText}
              onChange={(e) => handleConfigChange(e.target.value)}
              required
              className="font-mono text-sm min-h-[180px]"
              rows={10}
            />
            {configError && (
              <p className="text-xs text-destructive">{configError}</p>
            )}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              disabled={isPending || configError !== null}
            >
              {isPending ? (
                <Loader2 className="h-4 w-4 animate-spin mr-2" />
              ) : null}
              {isPending
                ? isEdit
                  ? 'Saving...'
                  : 'Creating...'
                : isEdit
                  ? 'Save'
                  : 'Create MCP Server'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
