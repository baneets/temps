import {
  deleteDnsProvider as deleteProvider,
  listDnsProviders as listProviders,
  testProviderConnection,
  type DnsProviderResponse,
} from '@/api/client'
import { EmptyPlaceholder } from '@/components/EmptyPlaceholder'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
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
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { CreateActionButton } from '@/components/ui/create-action-button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { TimeAgo } from '@/components/utils/TimeAgo'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  AlertCircle,
  CheckCircle2,
  Globe,
  Loader2,
  MoreVertical,
  Plus,
  RefreshCw,
  TestTube2,
  Trash2,
  XCircle,
} from 'lucide-react'
import { getDnsProviderIcon } from '@/components/icons/DnsProviderIcons'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'

// Helper function to get provider icon
function getProviderIcon(providerType: string) {
  return getDnsProviderIcon(providerType, 'h-4 w-4')
}

// Helper function to format provider type for display
function formatProviderType(type: string): string {
  switch (type.toLowerCase()) {
    case 'cloudflare':
      return 'Cloudflare'
    case 'namecheap':
      return 'Namecheap'
    case 'route53':
      return 'AWS Route 53'
    case 'gcp':
      return 'Google Cloud DNS'
    case 'azure':
      return 'Azure DNS'
    case 'digitalocean':
      return 'DigitalOcean'
    default:
      return type.charAt(0).toUpperCase() + type.slice(1)
  }
}

export function DnsProviders() {
  const { setBreadcrumbs } = useBreadcrumbs()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [providerToDelete, setProviderToDelete] =
    useState<DnsProviderResponse | null>(null)

  const {
    data: dnsProviders,
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['dnsProviders'],
    queryFn: async () => {
      const response = await listProviders()
      return response.data
    },
    retry: false,
  })

  const deleteProviderMut = useMutation({
    mutationFn: (id: number) => deleteProvider({ path: { id } }),
    onSuccess: () => {
      toast.success('DNS provider deleted successfully')
      queryClient.invalidateQueries({ queryKey: ['dnsProviders'] })
      setProviderToDelete(null)
    },
    onError: (error: Error) => {
      toast.error('Failed to delete DNS provider', {
        description: error.message,
      })
    },
  })

  const testConnectionMut = useMutation({
    mutationFn: async (id: number) => {
      const response = await testProviderConnection({ path: { id } })
      return response.data
    },
    onSuccess: (result) => {
      if (result?.success) {
        toast.success('Connection test successful', {
          description: result.message,
        })
      } else {
        toast.error('Connection test failed', {
          description: result?.message,
        })
      }
    },
    onError: (error: Error) => {
      toast.error('Connection test failed', {
        description: error.message,
      })
    },
  })

  useEffect(() => {
    setBreadcrumbs([{ label: 'DNS Providers' }])
  }, [setBreadcrumbs])

  usePageTitle('DNS Providers')

  const handleDeleteProvider = async () => {
    if (!providerToDelete) return
    deleteProviderMut.mutate(providerToDelete.id)
  }

  const handleTestConnection = (provider: DnsProviderResponse) => {
    testConnectionMut.mutate(provider.id)
  }

  return (
    <div className="flex-1 overflow-auto">
      <div className="space-y-6 p-4 sm:p-6">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <h1 className="text-2xl font-bold">DNS Providers</h1>
            <p className="text-muted-foreground">
              Manage your DNS providers for automatic DNS record configuration
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => refetch()}>
              <RefreshCw className="mr-2 h-4 w-4" />
              Refresh
            </Button>
            <CreateActionButton
              onClick={() => navigate('/dns-providers/add')}
              label="Add DNS Provider"
            />
          </div>
        </div>

        {error ? (
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertTitle>Error</AlertTitle>
            <AlertDescription>
              Failed to load DNS providers. Please try again later or contact
              support if the issue persists.
            </AlertDescription>
          </Alert>
        ) : (
          <Card>
            <CardHeader>
              <CardTitle>Active Providers</CardTitle>
            </CardHeader>
            <CardContent>
              {isLoading ? (
                <div className="grid gap-4">
                  {Array.from({ length: 3 }).map((_, i) => (
                    <div
                      key={i}
                      className="p-4 border rounded-lg space-y-3 animate-pulse"
                    >
                      <div className="flex items-center justify-between">
                        <div className="h-5 w-48 bg-muted rounded" />
                        <div className="h-6 w-20 bg-muted rounded" />
                      </div>
                      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                        <div className="space-y-2">
                          <div className="h-4 w-24 bg-muted rounded" />
                          <div className="h-4 w-32 bg-muted rounded" />
                        </div>
                        <div className="space-y-2">
                          <div className="h-4 w-24 bg-muted rounded" />
                          <div className="h-4 w-32 bg-muted rounded" />
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              ) : !dnsProviders?.length ? (
                <EmptyPlaceholder
                  icon={Globe}
                  title="No DNS providers found"
                  description="Get started by adding a DNS provider like Cloudflare or Namecheap to enable automatic DNS record management"
                >
                  <Button onClick={() => navigate('/dns-providers/add')}>
                    <Plus className="mr-2 h-4 w-4" />
                    Add DNS Provider
                  </Button>
                </EmptyPlaceholder>
              ) : (
                <div className="grid gap-4">
                  {dnsProviders.map((provider) => (
                    <div
                      key={provider.id}
                      className="group relative p-4 border rounded-lg transition-colors hover:bg-muted/50 cursor-pointer"
                      onClick={() =>
                        navigate(`/dns-providers/${provider.id}`)
                      }
                    >
                      <div className="flex flex-col sm:flex-row sm:items-center gap-4">
                        <div className="flex-1 min-w-0 space-y-1">
                          <div className="flex items-center gap-3">
                            {getProviderIcon(provider.provider_type)}
                            <span className="font-medium truncate">
                              {provider.name}
                            </span>
                            <Badge variant="outline">
                              {formatProviderType(provider.provider_type)}
                            </Badge>
                            {provider.is_active ? (
                              <Badge
                                variant="secondary"
                                className="flex items-center gap-1"
                              >
                                <CheckCircle2 className="h-3 w-3" />
                                Active
                              </Badge>
                            ) : (
                              <Badge
                                variant="destructive"
                                className="flex items-center gap-1"
                              >
                                <XCircle className="h-3 w-3" />
                                Inactive
                              </Badge>
                            )}
                          </div>
                          <div className="grid grid-cols-1 sm:flex sm:items-center gap-x-6 gap-y-1 text-sm text-muted-foreground">
                            {provider.description && (
                              <div className="flex items-center gap-2">
                                <span className="truncate max-w-[300px]">
                                  {provider.description}
                                </span>
                              </div>
                            )}
                            {provider.last_error && (
                              <div className="flex items-center gap-2 text-destructive">
                                <AlertCircle className="h-4 w-4" />
                                <span className="truncate max-w-[200px]">
                                  {provider.last_error}
                                </span>
                              </div>
                            )}
                            <div className="flex items-center gap-2">
                              <span>Created </span>
                              <TimeAgo
                                date={provider.created_at}
                                className=""
                              />
                            </div>
                          </div>
                        </div>
                        <div
                          className="flex items-center gap-2"
                          onClick={(e) => e.stopPropagation()}
                        >
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => handleTestConnection(provider)}
                            disabled={testConnectionMut.isPending}
                            className="gap-2"
                          >
                            {testConnectionMut.isPending ? (
                              <Loader2 className="h-4 w-4 animate-spin" />
                            ) : (
                              <TestTube2 className="h-4 w-4" />
                            )}
                            Test
                          </Button>
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button variant="ghost" size="icon">
                                <MoreVertical className="h-4 w-4" />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="end">
                              <DropdownMenuItem
                                onClick={() =>
                                  navigate(
                                    `/dns-providers/${provider.id}`
                                  )
                                }
                              >
                                View Details
                              </DropdownMenuItem>
                              <DropdownMenuItem
                                onClick={() => handleTestConnection(provider)}
                              >
                                <TestTube2 className="h-4 w-4 mr-2" />
                                Test Connection
                              </DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem
                                className="text-destructive cursor-pointer"
                                onSelect={(e) => {
                                  e.preventDefault()
                                  setProviderToDelete(provider)
                                }}
                              >
                                <Trash2 className="h-4 w-4 mr-2" />
                                Delete Provider
                              </DropdownMenuItem>
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
        )}
      </div>

      {/* Delete Confirmation Dialog */}
      <AlertDialog
        open={!!providerToDelete}
        onOpenChange={(open) => !open && setProviderToDelete(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete DNS Provider</AlertDialogTitle>
            <AlertDialogDescription>
              Are you sure you want to delete &quot;{providerToDelete?.name}
              &quot;? This action cannot be undone and will remove all
              associated managed domains.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel onClick={() => setProviderToDelete(null)}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              disabled={deleteProviderMut.isPending}
              onClick={handleDeleteProvider}
            >
              {deleteProviderMut.isPending ? (
                <>
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Deleting...
                </>
              ) : (
                'Delete Provider'
              )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
