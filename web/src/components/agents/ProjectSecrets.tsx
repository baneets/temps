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
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { KeyRound, Loader2, Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import {
  createSecret,
  deleteSecret,
  listSecrets,
  type CreateSecretRequest,
  type AgentSecret,
} from './api'

export function AgentSecrets() {
  const queryClient = useQueryClient()
  const [dialogOpen, setDialogOpen] = useState(false)

  const { data: secrets = [], isLoading } = useQuery({
    queryKey: ['agent-secrets'],
    queryFn: () => listSecrets(),
  })

  const createMutation = useMutation({
    mutationFn: (data: CreateSecretRequest) => createSecret(data),
    onSuccess: () => {
      toast.success('Secret saved')
      queryClient.invalidateQueries({ queryKey: ['agent-secrets'] })
      setDialogOpen(false)
    },
    onError: (error: Error) => {
      toast.error(error.message || 'Failed to save secret')
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (name: string) => deleteSecret(name),
    onSuccess: () => {
      toast.success('Secret deleted')
      queryClient.invalidateQueries({ queryKey: ['agent-secrets'] })
    },
    onError: (error: Error) => {
      toast.error(error.message || 'Failed to delete secret')
    },
  })

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div>
            <CardTitle className="flex items-center gap-2 text-base">
              <KeyRound className="h-4 w-4" />
              Secrets
            </CardTitle>
            <CardDescription>
              Encrypted secrets injected into all agent sandboxes as environment variables or files.
              Reference in config with <code className="bg-muted px-1 rounded text-xs">{'${TEMPS_SECRET:name}'}</code>.
            </CardDescription>
          </div>
          <Button size="sm" onClick={() => setDialogOpen(true)}>
            <Plus className="h-3.5 w-3.5 mr-1" />
            Add Secret
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <div className="flex justify-center py-6">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        ) : secrets.length === 0 ? (
          <p className="text-sm text-muted-foreground text-center py-6">
            No secrets configured. Add secrets to inject API keys, tokens, or config files into agent sandboxes.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead className="hidden sm:table-cell">Type</TableHead>
                  <TableHead className="hidden md:table-cell">Description</TableHead>
                  <TableHead className="hidden md:table-cell">Updated</TableHead>
                  <TableHead className="w-[60px]" />
                </TableRow>
              </TableHeader>
              <TableBody>
                {secrets.map((secret: AgentSecret) => (
                  <TableRow key={secret.id}>
                    <TableCell className="font-mono text-sm">{secret.name}</TableCell>
                    <TableCell className="hidden sm:table-cell">
                      <span className="inline-flex items-center rounded-full border px-2 py-0.5 text-xs">
                        {secret.secret_type === 'env' ? 'Env Var' : 'File'}
                      </span>
                      {secret.mount_path && (
                        <span className="ml-1 text-xs text-muted-foreground font-mono">
                          {secret.mount_path}
                        </span>
                      )}
                    </TableCell>
                    <TableCell className="hidden md:table-cell text-muted-foreground text-sm">
                      {secret.description || '-'}
                    </TableCell>
                    <TableCell className="hidden md:table-cell text-muted-foreground text-sm">
                      {new Date(secret.updated_at).toLocaleDateString()}
                    </TableCell>
                    <TableCell>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7"
                        onClick={() => deleteMutation.mutate(secret.name)}
                        disabled={deleteMutation.isPending}
                      >
                        <Trash2 className="h-3.5 w-3.5 text-destructive" />
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </CardContent>

      <CreateSecretDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        onSubmit={(data) => createMutation.mutate(data)}
        isPending={createMutation.isPending}
      />
    </Card>
  )
}

function CreateSecretDialog({
  open,
  onOpenChange,
  onSubmit,
  isPending,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSubmit: (data: CreateSecretRequest) => void
  isPending: boolean
}) {
  const [name, setName] = useState('')
  const [secretType, setSecretType] = useState<'env' | 'file'>('env')
  const [value, setValue] = useState('')
  const [mountPath, setMountPath] = useState('')
  const [description, setDescription] = useState('')

  const handleSubmit = () => {
    if (!name.trim()) {
      toast.error('Name is required')
      return
    }
    if (!value.trim()) {
      toast.error('Value is required')
      return
    }
    if (secretType === 'file' && !mountPath.trim()) {
      toast.error('Mount path is required for file secrets')
      return
    }
    onSubmit({
      name: name.trim(),
      secret_type: secretType,
      value: value.trim(),
      mount_path: secretType === 'file' ? mountPath.trim() : undefined,
      description: description.trim() || undefined,
    })
    // Reset on success (dialog closes via parent)
    setName('')
    setValue('')
    setMountPath('')
    setDescription('')
    setSecretType('env')
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Add Secret</DialogTitle>
        </DialogHeader>
        <form onSubmit={(e) => { e.preventDefault(); handleSubmit() }} className="space-y-4 py-2">
          <div className="space-y-1.5">
            <Label htmlFor="secret-name">Name</Label>
            <Input
              id="secret-name"
              value={name}
              onChange={(e) => setName(e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, '_'))}
              placeholder="ANTHROPIC_API_KEY"
              className="font-mono"
            />
            <p className="text-xs text-muted-foreground">
              Use in config as <code className="bg-muted px-1 rounded">{'${TEMPS_SECRET:' + (name || 'NAME') + '}'}</code>
            </p>
          </div>

          <div className="space-y-1.5">
            <Label>Type</Label>
            <Select value={secretType} onValueChange={(v) => setSecretType(v as 'env' | 'file')}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="env">Environment Variable</SelectItem>
                <SelectItem value="file">File</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="secret-value">Value</Label>
            <Input
              id="secret-value"
              type="password"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder={secretType === 'env' ? 'sk-ant-api03-...' : 'File contents...'}
            />
            <p className="text-xs text-muted-foreground">
              Encrypted with AES-256-GCM before storage.
            </p>
          </div>

          {secretType === 'file' && (
            <div className="space-y-1.5">
              <Label htmlFor="mount-path">Mount Path</Label>
              <Input
                id="mount-path"
                value={mountPath}
                onChange={(e) => setMountPath(e.target.value)}
                placeholder="/home/temps/.config/credentials.json"
                className="font-mono"
              />
            </div>
          )}

          <div className="space-y-1.5">
            <Label htmlFor="secret-description">Description</Label>
            <Input
              id="secret-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Optional description"
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={isPending}>
              {isPending ? 'Saving...' : 'Save Secret'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
