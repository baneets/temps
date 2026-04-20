import { useState } from 'react'
import { ConnectionResponse, ProviderResponse } from '@/api/client/types.gen'
import { deleteConnectionMutation } from '@/api/client/@tanstack/react-query.gen'
import { isGitHubApp } from '@/lib/provider'
import { UpdateTokenDialog } from '@/components/git/UpdateTokenDialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { TimeAgo } from '@/components/utils/TimeAgo'
import {
  CheckCircle2,
  Clock,
  EllipsisVertical,
  Key,
  RefreshCw,
  Trash2,
  Users,
  XCircle,
} from 'lucide-react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'

type Variant = 'single-line' | 'two-line' | 'avatar'

interface ConnectionsCompactListProps {
  connections: ConnectionResponse[]
  provider?: ProviderResponse
  onSyncRepository: (connectionId: number) => void
  isSyncing: boolean
  onConnectionDeleted?: () => void
  variant: Variant
}

export function ConnectionsCompactList({
  connections,
  provider,
  onSyncRepository,
  isSyncing,
  onConnectionDeleted,
  variant,
}: ConnectionsCompactListProps) {
  const queryClient = useQueryClient()
  const [updateTokenDialog, setUpdateTokenDialog] = useState<{
    open: boolean
    connectionId: number
    connectionName: string
  }>({ open: false, connectionId: 0, connectionName: '' })
  const [deleteDialog, setDeleteDialog] = useState<{
    open: boolean
    connectionId: number
    connectionName: string
  }>({ open: false, connectionId: 0, connectionName: '' })

  const deleteConnectionMut = useMutation({
    ...deleteConnectionMutation(),
    onSuccess: () => {
      toast.success('Connection deleted successfully')
      queryClient.invalidateQueries({ queryKey: ['listConnections'] })
      setDeleteDialog({ open: false, connectionId: 0, connectionName: '' })
      onConnectionDeleted?.()
    },
    onError: () => toast.error('Failed to delete connection'),
  })

  const isPATProvider =
    provider &&
    ((provider.provider_type === 'github' &&
      (provider.auth_method === 'pat' ||
        provider.auth_method === 'github_pat')) ||
      (provider.provider_type === 'gitlab' &&
        (provider.auth_method === 'pat' ||
          provider.auth_method === 'gitlab_pat')))

  const ActionsMenu = ({ c }: { c: ConnectionResponse }) => (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" className="h-7 w-7">
          <EllipsisVertical className="h-3.5 w-3.5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem
          onSelect={(e) => {
            e.preventDefault()
            onSyncRepository(c.id)
          }}
          disabled={isSyncing}
        >
          <RefreshCw
            className={`mr-2 h-4 w-4 ${isSyncing ? 'animate-spin' : ''}`}
          />
          Sync repositories
        </DropdownMenuItem>
        {isPATProvider && (
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault()
              setUpdateTokenDialog({
                open: true,
                connectionId: c.id,
                connectionName: c.account_name,
              })
            }}
          >
            <Key className="mr-2 h-4 w-4" />
            Update token
          </DropdownMenuItem>
        )}
        {provider && isGitHubApp(provider) && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className="text-destructive"
              onSelect={(e) => {
                e.preventDefault()
                setDeleteDialog({
                  open: true,
                  connectionId: c.id,
                  connectionName: c.account_name,
                })
              }}
            >
              <Trash2 className="mr-2 h-4 w-4" />
              Delete connection
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )

  const StatusBadge = ({ c }: { c: ConnectionResponse }) =>
    c.is_active ? (
      <Badge variant="secondary" className="h-5 gap-0.5 px-1.5 text-[10px]">
        <CheckCircle2 className="h-2.5 w-2.5" />
        Active
      </Badge>
    ) : (
      <Badge variant="destructive" className="h-5 gap-0.5 px-1.5 text-[10px]">
        <XCircle className="h-2.5 w-2.5" />
        Inactive
      </Badge>
    )

  const SyncBadge = ({ c }: { c: ConnectionResponse }) =>
    c.syncing ? (
      <Badge variant="outline" className="h-5 gap-0.5 px-1.5 text-[10px]">
        <RefreshCw className="h-2.5 w-2.5 animate-spin" />
        Syncing
      </Badge>
    ) : null

  const renderRow = (c: ConnectionResponse) => {
    if (variant === 'single-line') {
      return (
        <li
          key={c.id}
          className="flex flex-col gap-2 px-3 py-2.5 sm:flex-row sm:items-center sm:gap-3"
        >
          {/* Primary: icon + account + type + status */}
          <div className="flex min-w-0 items-center gap-2 sm:shrink-0">
            <Users className="h-4 w-4 text-muted-foreground shrink-0" />
            <span className="truncate text-sm font-medium">
              {c.account_name}
            </span>
            <Badge variant="outline" className="h-5 px-1.5 text-[10px]">
              {c.account_type || 'unknown'}
            </Badge>
            <StatusBadge c={c} />
            <SyncBadge c={c} />
          </div>

          {/* Meta: installation id + last synced */}
          <div className="flex min-w-0 flex-1 items-center gap-3 text-xs text-muted-foreground">
            {c.installation_id && (
              <span className="font-mono truncate">
                id:{c.installation_id}
              </span>
            )}
            <span className="flex items-center gap-1 shrink-0">
              <Clock className="h-3 w-3" />
              {c.last_synced_at ? (
                <>
                  synced <TimeAgo date={c.last_synced_at} />
                </>
              ) : (
                'never synced'
              )}
            </span>
          </div>

          {/* Right: time + menu */}
          <div className="flex items-center gap-2 sm:shrink-0">
            <span className="whitespace-nowrap text-xs text-muted-foreground">
              <TimeAgo date={c.created_at} />
            </span>
            <ActionsMenu c={c} />
          </div>
        </li>
      )
    }

    if (variant === 'two-line') {
      return (
        <li
          key={c.id}
          className="flex items-start gap-3 px-3 py-3 sm:px-4"
        >
          <div className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md bg-muted">
            <Users className="h-4 w-4 text-muted-foreground" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="truncate text-sm font-medium">
                {c.account_name}
              </span>
              <Badge variant="outline" className="h-5 px-1.5 text-[10px]">
                {c.account_type || 'unknown'}
              </Badge>
              <StatusBadge c={c} />
              <SyncBadge c={c} />
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
              {c.installation_id && (
                <span className="font-mono">id:{c.installation_id}</span>
              )}
              <span className="flex items-center gap-1">
                <Clock className="h-3 w-3" />
                {c.last_synced_at ? (
                  <>
                    synced <TimeAgo date={c.last_synced_at} />
                  </>
                ) : (
                  'never synced'
                )}
              </span>
              <span>
                added <TimeAgo date={c.created_at} />
              </span>
            </div>
          </div>
          <ActionsMenu c={c} />
        </li>
      )
    }

    // variant === 'avatar'
    const initials = c.account_name
      .split(/[\s-]+/)
      .map((s) => s[0])
      .slice(0, 2)
      .join('')
      .toUpperCase()

    return (
      <li
        key={c.id}
        className="flex items-center gap-3 px-3 py-2.5 sm:px-4"
      >
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full border bg-muted text-xs font-semibold text-muted-foreground">
          {initials || '?'}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm font-medium">
              {c.account_name}
            </span>
            <Badge variant="outline" className="h-5 px-1.5 text-[10px]">
              {c.account_type || 'unknown'}
            </Badge>
          </div>
          <div className="mt-0.5 flex items-center gap-3 text-xs text-muted-foreground">
            <span className="flex items-center gap-1">
              <Clock className="h-3 w-3" />
              {c.last_synced_at ? (
                <>
                  synced <TimeAgo date={c.last_synced_at} />
                </>
              ) : (
                'never synced'
              )}
            </span>
            {c.installation_id && (
              <span className="font-mono truncate">
                id:{c.installation_id}
              </span>
            )}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <StatusBadge c={c} />
          <SyncBadge c={c} />
          <ActionsMenu c={c} />
        </div>
      </li>
    )
  }

  return (
    <>
      <ul role="list" className="divide-y rounded-md border">
        {connections.map(renderRow)}
      </ul>

      <UpdateTokenDialog
        connectionId={updateTokenDialog.connectionId}
        connectionName={updateTokenDialog.connectionName}
        open={updateTokenDialog.open}
        onOpenChange={(open) =>
          setUpdateTokenDialog({ ...updateTokenDialog, open })
        }
      />

      <AlertDialog
        open={deleteDialog.open}
        onOpenChange={(open) => setDeleteDialog({ ...deleteDialog, open })}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete Connection</AlertDialogTitle>
            <AlertDialogDescription>
              Are you sure you want to delete the connection for{' '}
              <strong>{deleteDialog.connectionName}</strong>? This action
              cannot be undone and will remove all associated repositories.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteConnectionMut.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                deleteConnectionMut.mutate({
                  path: { connection_id: deleteDialog.connectionId },
                })
              }
              disabled={deleteConnectionMut.isPending}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {deleteConnectionMut.isPending ? (
                <>
                  <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                  Deleting...
                </>
              ) : (
                <>
                  <Trash2 className="mr-2 h-4 w-4" />
                  Delete
                </>
              )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
