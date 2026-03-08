import { Badge } from '@/components/ui/badge'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import {
  adminListNodesOptions,
  getJoinTokenStatusOptions,
  generateJoinTokenMutation,
  revokeJoinTokenMutation,
} from '@/api/client/@tanstack/react-query.gen'
import type { NodeInfoResponse } from '@/api/client/types.gen'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  AlertCircle,
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  Copy,
  ExternalLink,
  Key,
  Loader2,
  RefreshCw,
  Server,
  Shield,
  Trash2,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { toast } from 'sonner'

function StatusBadge({ status }: { status: string }) {
  switch (status) {
    case 'active':
      return (
        <Badge
          variant="default"
          className="bg-green-500/15 text-green-700 dark:text-green-400 border-green-500/20 text-xs"
        >
          Active
        </Badge>
      )
    case 'offline':
      return (
        <Badge
          variant="default"
          className="bg-red-500/15 text-red-700 dark:text-red-400 border-red-500/20 text-xs"
        >
          Offline
        </Badge>
      )
    case 'draining':
      return (
        <Badge
          variant="default"
          className="bg-orange-500/15 text-orange-700 dark:text-orange-400 border-orange-500/20 text-xs"
        >
          Draining
        </Badge>
      )
    case 'pending':
      return (
        <Badge
          variant="default"
          className="bg-yellow-500/15 text-yellow-700 dark:text-yellow-400 border-yellow-500/20 text-xs"
        >
          Pending
        </Badge>
      )
    default:
      return (
        <Badge variant="secondary" className="text-xs">
          {status}
        </Badge>
      )
  }
}

function formatRelativeTime(dateStr: string | null | undefined): string {
  if (!dateStr) return 'Never'
  const date = new Date(dateStr)
  const now = new Date()
  const diffMs = now.getTime() - date.getTime()
  const diffSecs = Math.floor(diffMs / 1000)

  if (diffSecs < 60) return `${diffSecs}s ago`
  const diffMins = Math.floor(diffSecs / 60)
  if (diffMins < 60) return `${diffMins}m ago`
  const diffHours = Math.floor(diffMins / 60)
  if (diffHours < 24) return `${diffHours}h ago`
  const diffDays = Math.floor(diffHours / 24)
  return `${diffDays}d ago`
}

function CopyButton({ text }: { text: string }) {
  const handleCopy = () => {
    navigator.clipboard.writeText(text)
    toast.success('Copied to clipboard')
  }

  return (
    <Button
      variant="ghost"
      size="icon"
      className="h-6 w-6 shrink-0"
      onClick={handleCopy}
    >
      <Copy className="h-3 w-3" />
    </Button>
  )
}

function JoinTokenSection() {
  const queryClient = useQueryClient()
  const { data: tokenStatus, isLoading: statusLoading } = useQuery({
    ...getJoinTokenStatusOptions(),
  })
  const generateToken = useMutation({
    ...generateJoinTokenMutation(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getJoinTokenStatusOptions().queryKey })
    },
  })
  const revokeToken = useMutation({
    ...revokeJoinTokenMutation(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getJoinTokenStatusOptions().queryKey })
    },
  })
  const [generatedToken, setGeneratedToken] = useState<string | null>(null)

  const externalUrl = window.location.origin

  const handleGenerate = async () => {
    try {
      const result = await generateToken.mutateAsync({})
      setGeneratedToken(result.token)
      toast.success('Join token generated')
    } catch {
      toast.error('Failed to generate join token')
    }
  }

  const handleRevoke = async () => {
    try {
      await revokeToken.mutateAsync({})
      setGeneratedToken(null)
      toast.success('Join token revoked')
    } catch {
      toast.error('Failed to revoke join token')
    }
  }

  if (statusLoading) {
    return (
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading token status...
      </div>
    )
  }

  const hasToken = tokenStatus?.has_token ?? false

  // Just generated — show the plaintext token with the full command
  if (generatedToken) {
    const joinCommand = `temps join ${externalUrl} ${generatedToken} --private-address <worker-ip>`
    return (
      <div className="space-y-4">
        <Alert className="border-amber-500/30 bg-amber-500/5">
          <AlertTriangle className="h-4 w-4 text-amber-500" />
          <AlertTitle className="text-amber-700 dark:text-amber-400">
            Save this token now
          </AlertTitle>
          <AlertDescription className="text-amber-600 dark:text-amber-300">
            This is the only time the join token will be displayed. Copy the
            command below and store the token securely.
          </AlertDescription>
        </Alert>

        <JoinInstructions joinCommand={joinCommand} />

        <div className="flex items-center gap-2">
          <Button
            variant="destructive"
            size="sm"
            onClick={handleRevoke}
            disabled={revokeToken.isPending}
          >
            {revokeToken.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin mr-1" />
            ) : (
              <Trash2 className="h-4 w-4 mr-1" />
            )}
            Revoke Token
          </Button>
        </div>
      </div>
    )
  }

  // Token exists (but we don't have the plaintext)
  if (hasToken) {
    const joinCommand = `temps join ${externalUrl} <join-token> --private-address <worker-ip>`
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2 text-sm">
          <Badge
            variant="default"
            className="bg-green-500/15 text-green-700 dark:text-green-400 border-green-500/20"
          >
            <Shield className="h-3 w-3 mr-1" />
            Token configured
          </Badge>
          <span className="text-muted-foreground">
            Node registration requires a valid join token.
          </span>
        </div>

        <JoinInstructions joinCommand={joinCommand} />

        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={handleGenerate}
            disabled={generateToken.isPending}
          >
            {generateToken.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin mr-1" />
            ) : (
              <RefreshCw className="h-4 w-4 mr-1" />
            )}
            Regenerate Token
          </Button>
          <Button
            variant="destructive"
            size="sm"
            onClick={handleRevoke}
            disabled={revokeToken.isPending}
          >
            {revokeToken.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin mr-1" />
            ) : (
              <Trash2 className="h-4 w-4 mr-1" />
            )}
            Revoke Token
          </Button>
        </div>
      </div>
    )
  }

  // No token — prompt to generate one
  return (
    <div className="space-y-4">
      <Alert className="border-amber-500/30 bg-amber-500/5">
        <AlertTriangle className="h-4 w-4 text-amber-500" />
        <AlertTitle className="text-amber-700 dark:text-amber-400">
          No join token configured
        </AlertTitle>
        <AlertDescription className="text-amber-600 dark:text-amber-300">
          Without a join token, any machine that knows the endpoint can register
          as a worker node. Generate a token to secure node registration.
        </AlertDescription>
      </Alert>

      <Button
        onClick={handleGenerate}
        disabled={generateToken.isPending}
      >
        {generateToken.isPending ? (
          <Loader2 className="h-4 w-4 animate-spin mr-1" />
        ) : (
          <Key className="h-4 w-4 mr-1" />
        )}
        Generate Join Token
      </Button>
    </div>
  )
}

function JoinInstructions({ joinCommand }: { joinCommand: string }) {
  const [expanded, setExpanded] = useState(true)

  return (
    <div className="rounded-lg border bg-muted/30 p-4">
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-2 text-sm font-medium w-full text-left"
      >
        {expanded ? (
          <ChevronDown className="h-4 w-4" />
        ) : (
          <ChevronRight className="h-4 w-4" />
        )}
        How to add a worker node
      </button>
      {expanded && (
        <div className="mt-3 space-y-3 text-sm text-muted-foreground">
          <div>
            <p className="font-medium text-foreground">
              1. Install Temps CLI on the worker machine
            </p>
            <div className="mt-1 flex items-center gap-2 rounded-md bg-muted px-3 py-2 font-mono text-xs">
              <span className="flex-1 overflow-x-auto">
                curl -fsSL https://temps.sh/install.sh | bash
              </span>
              <CopyButton text="curl -fsSL https://temps.sh/install.sh | bash" />
            </div>
          </div>
          <div>
            <p className="font-medium text-foreground">
              2. Join the cluster
            </p>
            <div className="mt-1 flex items-center gap-2 rounded-md bg-muted px-3 py-2 font-mono text-xs">
              <span className="flex-1 overflow-x-auto">{joinCommand}</span>
              <CopyButton text={joinCommand} />
            </div>
            <p className="mt-1 text-xs">
              Replace <code>&lt;worker-ip&gt;</code> with the worker machine's
              private IP address.
            </p>
          </div>
          <div>
            <p className="font-medium text-foreground">
              3. Start the agent
            </p>
            <div className="mt-1 flex items-center gap-2 rounded-md bg-muted px-3 py-2 font-mono text-xs">
              <span className="flex-1 overflow-x-auto">temps agent</span>
              <CopyButton text="temps agent" />
            </div>
            <p className="mt-1 text-xs">
              Reads config saved by <code>temps join</code> and starts
              the worker with heartbeats.
            </p>
          </div>
          <div>
            <a
              href="/docs/multi-node"
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-xs font-medium text-primary hover:underline"
            >
              Full documentation
              <ExternalLink className="h-3 w-3" />
            </a>
          </div>
        </div>
      )}
    </div>
  )
}

function NodeTable({ nodes }: { nodes: NodeInfoResponse[] }) {
  return (
    <div className="overflow-x-auto">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Status</TableHead>
            <TableHead className="hidden md:table-cell">Address</TableHead>
            <TableHead className="hidden md:table-cell">Role</TableHead>
            <TableHead>Last Heartbeat</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {nodes.map((node) => (
            <TableRow key={node.id}>
              <TableCell>
                <div className="flex items-center gap-2">
                  <Server className="h-4 w-4 text-muted-foreground shrink-0" />
                  <span className="font-medium truncate max-w-[200px]">
                    {node.name}
                  </span>
                </div>
              </TableCell>
              <TableCell>
                <StatusBadge status={node.status} />
              </TableCell>
              <TableCell className="hidden md:table-cell">
                <span className="font-mono text-xs text-muted-foreground truncate max-w-[200px] block">
                  {node.private_address}
                </span>
              </TableCell>
              <TableCell className="hidden md:table-cell">
                <Badge variant="outline" className="text-xs capitalize">
                  {node.role}
                </Badge>
              </TableCell>
              <TableCell>
                <span className="text-sm text-muted-foreground">
                  {formatRelativeTime(node.last_heartbeat)}
                </span>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}

export function NodesPage() {
  const { setBreadcrumbs } = useBreadcrumbs()
  const { data, isLoading, error } = useQuery({
    ...adminListNodesOptions(),
    refetchInterval: 30_000,
  })
  const nodes = data?.nodes ?? []

  useEffect(() => {
    setBreadcrumbs([
      { label: 'Settings', href: '/settings' },
      { label: 'Worker Nodes' },
    ])
  }, [setBreadcrumbs])

  usePageTitle('Worker Nodes')

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-[400px]">
        <Loader2 className="h-8 w-8 animate-spin" />
      </div>
    )
  }

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle>Error</AlertTitle>
        <AlertDescription>Failed to load worker nodes.</AlertDescription>
      </Alert>
    )
  }

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>Worker Nodes</CardTitle>
          <CardDescription>
            Distribute container deployments across multiple servers. Worker
            nodes run the Temps agent and receive containers from the control
            plane.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <JoinTokenSection />

          {nodes.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-12 text-center border-t pt-6">
              <Server className="h-12 w-12 text-muted-foreground mb-4" />
              <p className="text-sm font-medium">No worker nodes</p>
              <p className="text-sm text-muted-foreground mt-1 max-w-md">
                All deployments run on this server. Add worker nodes to
                distribute containers across multiple machines.
              </p>
            </div>
          ) : (
            <NodeTable nodes={nodes} />
          )}
        </CardContent>
      </Card>
    </div>
  )
}
