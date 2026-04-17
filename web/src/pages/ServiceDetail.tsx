import {
  deleteServiceMutation,
  getProjectsOptions,
  getServiceOptions,
  getServicePreviewEnvironmentVariablesMaskedOptions,
  linkServiceToProjectMutation,
  listServiceProjectsOptions,
  startServiceMutation,
  stopServiceMutation,
} from '@/api/client/@tanstack/react-query.gen'
import { EditServiceDialog } from '@/components/storage/EditServiceDialog'
import { MajorUpgradeDialog } from '@/components/storage/MajorUpgradeDialog'
import { TriggerBackupDialog } from '@/components/storage/TriggerBackupDialog'
import { UpgradeServiceDialog } from '@/components/storage/UpgradeServiceDialog'
import { listPgUpgrades, phaseIndex, PG_UPGRADE_PHASES, isTerminal } from '@/lib/pg-upgrades'
import { Alert, AlertDescription } from '@/components/ui/alert'
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
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import { EnvVariablesDisplay } from '@/components/ui/env-variables-display'
import { ServiceLogo } from '@/components/ui/service-logo'
import { TimeAgo } from '@/components/utils/TimeAgo'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { maskValue, shouldMaskValue } from '@/lib/masking'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  AlertCircle,
  ArrowLeft,
  ArrowUpCircle,
  Database,
  Eye,
  EyeOff,
  HardDrive,
  Loader2,
  MoreVertical,
  Pencil,
  Plus,
  RefreshCcw,
  Server,
  Trash2,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { toast } from 'sonner'

export function ServiceDetail() {
  const { id } = useParams<{ id: string }>()
  const { setBreadcrumbs } = useBreadcrumbs()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [isDeleteDialogOpen, setIsDeleteDialogOpen] = useState(false)
  const [isUpgradeDialogOpen, setIsUpgradeDialogOpen] = useState(false)
  const [isMajorUpgradeDialogOpen, setIsMajorUpgradeDialogOpen] = useState(false)
  const [isEditDialogOpen, setIsEditDialogOpen] = useState(false)
  const [isBackupDialogOpen, setIsBackupDialogOpen] = useState(false)
  const [isStopDialogOpen, setIsStopDialogOpen] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [prevStatus, setPrevStatus] = useState<string | undefined>(undefined)
  const [visibleParameters, setVisibleParameters] = useState<Set<string>>(
    new Set()
  )

  const {
    data: service,
    isLoading,
    error: queryError,
    refetch,
  } = useQuery({
    ...getServiceOptions({
      path: { id: parseInt(id!) },
    }),
    enabled: !!id,
    refetchInterval: (query) => {
      const status = query.state.data?.service?.status
      return status === 'creating' ? 2000 : false
    },
  })

  // Query for PostgreSQL major-version upgrades. Only relevant for postgres
  // services; harmless for others (the query is enabled conditionally below).
  const isPostgres = service?.service?.service_type === 'postgres'
  const { data: pgUpgrades } = useQuery({
    queryKey: ['pg-upgrades', id],
    queryFn: () => listPgUpgrades(parseInt(id!)),
    enabled: !!id && isPostgres,
    // Poll every 3s while any upgrade is still running so the card reflects
    // phase progression without a manual refresh.
    refetchInterval: (query) => {
      const rows = query.state.data
      if (!rows) return false
      return rows.some((u) => !isTerminal(u.status)) ? 3000 : false
    },
  })

  // Query for environment variables
  const {
    data: envVars,
    isLoading: envVarsLoading,
    error: envVarsError,
  } = useQuery({
    ...getServicePreviewEnvironmentVariablesMaskedOptions({
      path: { id: parseInt(id!) },
    }),
    enabled: !!id,
    staleTime: 5 * 60 * 1000, // Cache for 5 minutes
  })

  // Query for linked projects
  const {
    data: linkedProjectsResponse,
    isLoading: linkedProjectsLoading,
    refetch: refetchLinkedProjects,
  } = useQuery({
    ...listServiceProjectsOptions({
      path: { id: parseInt(id!) },
    }),
    enabled: !!id,
  })

  // All projects for the link popover
  const { data: allProjectsData } = useQuery({
    ...getProjectsOptions({ query: { page: 1, per_page: 100 } }),
  })

  const [isLinkPopoverOpen, setIsLinkPopoverOpen] = useState(false)

  const linkService = useMutation({
    ...linkServiceToProjectMutation(),
    meta: { errorTitle: 'Failed to link project' },
    onSuccess: () => {
      toast.success('Project linked successfully')
      refetchLinkedProjects()
      setIsLinkPopoverOpen(false)
    },
  })

  useEffect(() => {
    if (service) {
      setBreadcrumbs([
        { label: 'Storage', href: '/settings/storage' },
        {
          label: service.service.name || 'Service Details',
          href: `/settings/storage/${id}`,
        },
      ])
    } else {
      setBreadcrumbs([
        { label: 'Storage', href: '/settings/storage' },
        { label: 'Service Details', href: `/settings/storage/${id}` },
      ])
    }
  }, [setBreadcrumbs, id, service])

  usePageTitle(service?.service?.name || 'Service Details')

  // Notify when cluster creation completes or fails
  useEffect(() => {
    const currentStatus = service?.service?.status
    if (prevStatus === 'creating' && currentStatus === 'running') {
      toast.success('Cluster created successfully')
    } else if (prevStatus === 'creating' && currentStatus === 'failed') {
      toast.error('Cluster creation failed')
    }
    if (currentStatus) {
      setPrevStatus(currentStatus)
    }
  }, [service?.service?.status, prevStatus])

  const startService = useMutation({
    ...startServiceMutation(),
    meta: {
      errorTitle: 'Failed to start service',
    },
    onSuccess: () => {
      refetch()
      setError(null)
    },
  })

  const stopService = useMutation({
    ...stopServiceMutation(),
    meta: {
      errorTitle: 'Failed to stop service',
    },
    onSuccess: () => {
      toast.success('Service stopped successfully')
      refetch()
    },
  })

  const retryCluster = useMutation({
    mutationFn: async (options: {
      path: { id: number }
      body: { members: { role: string; node_id?: number }[] }
    }) => {
      const response = await fetch(
        `/api/external-services/${options.path.id}/retry`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'include',
          body: JSON.stringify(options.body),
        }
      )
      if (!response.ok) {
        const error = await response.json().catch(() => ({}))
        throw new Error(error.detail || 'Retry failed')
      }
      return response.json()
    },
    onSuccess: () => {
      toast.success('Cluster retry initiated')
      refetch()
    },
    onError: (error: Error) => {
      toast.error('Failed to retry cluster', {
        description: error.message,
      })
    },
  })

  const deleteService = useMutation({
    ...deleteServiceMutation(),
    meta: {
      errorTitle: 'Failed to delete service',
    },
    onSuccess: () => {
      toast.success('Service deleted successfully')
      navigate('/settings/storage')
    },
    onError: (error: any) => {
      toast.error('Failed to delete service', {
        description:
          error.detail || error.message || 'An unexpected error occurred',
      })
      setIsDeleteDialogOpen(false)
    },
  })

  const handleServiceAction = async () => {
    if (!service) return

    if (service.service.status === 'running') {
      setIsStopDialogOpen(true)
    } else if (service.service.status === 'stopped') {
      startService.mutate({ path: { id: parseInt(id!) } })
    }
  }

  const handleStop = async () => {
    stopService.mutate(
      { path: { id: parseInt(id!) } },
      { onSettled: () => setIsStopDialogOpen(false) }
    )
  }

  const handleDelete = async () => {
    deleteService.mutate({ path: { id: parseInt(id!) } })
  }

  if (isLoading) {
    return (
      <div className="flex-1 overflow-auto">
        <div className="sm:p-4 space-y-6 md:p-6">
          <div className="h-8 w-32 bg-muted rounded animate-pulse" />
          <Card>
            <CardHeader>
              <div className="space-y-2">
                <div className="h-5 w-40 bg-muted rounded animate-pulse" />
                <div className="h-4 w-24 bg-muted rounded animate-pulse" />
              </div>
            </CardHeader>
            <CardContent>
              <div className="space-y-4">
                <div className="h-4 w-full bg-muted rounded animate-pulse" />
                <div className="h-4 w-3/4 bg-muted rounded animate-pulse" />
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    )
  }

  if (queryError || !service) {
    return (
      <div className="flex-1 overflow-auto">
        <div className="sm:p-4 space-y-6 md:p-6">
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <p className="text-sm text-muted-foreground mb-4">
              Failed to load service details
            </p>
            <Button
              variant="outline"
              onClick={() => refetch()}
              className="gap-2"
            >
              <RefreshCcw className="h-4 w-4" />
              Try again
            </Button>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className="flex-1 overflow-auto">
      <div className="sm:p-4 space-y-6 md:p-6">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <Link to="/settings/storage">
              <Button variant="ghost" size="icon">
                <ArrowLeft className="h-4 w-4" />
              </Button>
            </Link>
            <ServiceLogo
              service={service.service.service_type}
              className="h-8 w-8"
            />
            <div className="flex flex-col gap-2">
              <div className="flex items-center gap-2 flex-wrap">
                <h1 className="text-xl font-semibold sm:text-2xl">
                  {service.service.name}
                </h1>
                <Badge
                  variant={
                    service.service.status === 'running'
                      ? 'default'
                      : service.service.status === 'stopped'
                        ? 'secondary'
                        : 'outline'
                  }
                  className="capitalize"
                >
                  {service.service.status}
                </Badge>
                <Badge variant="outline" className="gap-1.5">
                  <ServiceLogo
                    service={service.service.service_type}
                    className="h-3 w-3"
                  />
                  {service.service.service_type}
                </Badge>
                {service.service.topology === 'cluster' && (
                  <Badge variant="outline" className="gap-1.5">
                    <Server className="h-3 w-3" />
                    Cluster
                  </Badge>
                )}
              </div>
              <p className="text-sm text-muted-foreground">
                Created <TimeAgo date={service.service.created_at} />
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2 self-start sm:self-auto">
            <Link to={`/settings/storage/${id}/browse`}>
              <Button variant="outline" size="sm" className="gap-2">
                <Database className="h-4 w-4" />
                Browse Data
              </Button>
            </Link>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="icon" className="h-8 w-8">
                  <MoreVertical className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={() => setIsBackupDialogOpen(true)}>
                  <HardDrive className="h-4 w-4 mr-2" />
                  Backup
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => setIsEditDialogOpen(true)}>
                  <Pencil className="h-4 w-4 mr-2" />
                  Edit
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => setIsUpgradeDialogOpen(true)}>
                  <ArrowUpCircle className="h-4 w-4 mr-2" />
                  Upgrade
                </DropdownMenuItem>
                {service.service.service_type === 'postgres' ? (
                  <DropdownMenuItem
                    onClick={() => setIsMajorUpgradeDialogOpen(true)}
                  >
                    <ArrowUpCircle className="h-4 w-4 mr-2" />
                    Major Version Upgrade…
                  </DropdownMenuItem>
                ) : null}
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={handleServiceAction}
                  disabled={
                    service.service.status === 'creating' ||
                    startService.isPending ||
                    stopService.isPending
                  }
                  className={
                    service.service.status === 'running'
                      ? 'text-destructive focus:text-destructive'
                      : ''
                  }
                >
                  {(startService.isPending || stopService.isPending) ? (
                    <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  ) : service.service.status === 'running' ? (
                    <AlertCircle className="h-4 w-4 mr-2" />
                  ) : (
                    <RefreshCcw className="h-4 w-4 mr-2" />
                  )}
                  {service.service.status === 'running'
                    ? 'Stop'
                    : service.service.status === 'creating'
                      ? 'Creating...'
                      : 'Start'}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={() => setIsDeleteDialogOpen(true)}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="h-4 w-4 mr-2" />
                  Delete
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>

        {error && (
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        <div className="grid gap-6">
          {/* Linked Projects Section */}
          <Card>
            <CardHeader>
              <div className="flex items-center justify-between">
                <div className="space-y-1.5">
                  <CardTitle className="flex items-center gap-2">
                    <span>Linked Projects</span>
                    <Badge variant="outline">
                      {linkedProjectsLoading ? (
                        <Loader2 className="h-3 w-3 animate-spin" />
                      ) : (
                        linkedProjectsResponse?.length || 0
                      )}
                    </Badge>
                  </CardTitle>
                  <CardDescription>
                    Projects that are using this service
                  </CardDescription>
                </div>
                <Popover
                  open={isLinkPopoverOpen}
                  onOpenChange={setIsLinkPopoverOpen}
                >
                  <PopoverTrigger asChild>
                    <Button variant="outline" size="sm" className="gap-2">
                      <Plus className="h-4 w-4" />
                      Link Project
                    </Button>
                  </PopoverTrigger>
                  <PopoverContent className="w-[250px] p-0" align="end">
                    <Command>
                      <CommandInput placeholder="Search projects..." />
                      <CommandList>
                        <CommandEmpty>No projects found.</CommandEmpty>
                        <CommandGroup>
                          {allProjectsData?.projects
                            ?.filter(
                              (p) =>
                                !linkedProjectsResponse?.some(
                                  (lp) => lp.project.id === p.id
                                )
                            )
                            .map((project) => (
                              <CommandItem
                                key={project.id}
                                value={project.slug}
                                onSelect={() => {
                                  linkService.mutate({
                                    path: { id: parseInt(id!) },
                                    body: { project_id: project.id },
                                  })
                                }}
                              >
                                {project.slug}
                              </CommandItem>
                            ))}
                        </CommandGroup>
                      </CommandList>
                    </Command>
                  </PopoverContent>
                </Popover>
              </div>
            </CardHeader>
            <CardContent>
              {linkedProjectsLoading ? (
                <div className="flex items-center justify-center py-8">
                  <Loader2 className="h-4 w-4 animate-spin mr-2" />
                  <span className="text-sm text-muted-foreground">
                    Loading projects...
                  </span>
                </div>
              ) : linkedProjectsResponse &&
                linkedProjectsResponse.length > 0 ? (
                <div className="space-y-2">
                  {linkedProjectsResponse.map((link) => (
                    <div
                      key={link.id}
                      className="flex items-center justify-between p-3 rounded-md border border-border hover:bg-muted/50 transition-colors"
                    >
                      <div className="flex flex-col">
                        <p className="font-medium text-sm">
                          {link.project.slug}
                        </p>
                        <p className="text-xs text-muted-foreground">
                          Linked <TimeAgo date={link.service.created_at} />
                        </p>
                      </div>
                      <Link to={`/projects/${link.project.slug}`}>
                        <Button variant="ghost" size="sm" className="gap-2">
                          <ArrowLeft className="h-4 w-4 rotate-180" />
                          View Project
                        </Button>
                      </Link>
                    </div>
                  ))}
                </div>
              ) : (
                <div className="text-sm text-muted-foreground text-center py-8">
                  No projects are currently using this service
                </div>
              )}
            </CardContent>
          </Card>

          {/* Cluster Creation Progress */}
          {service.service.topology === 'cluster' &&
            service.service.status === 'creating' && (
              <Alert>
                <Loader2 className="h-4 w-4 animate-spin" />
                <AlertDescription>
                  <span className="font-medium">
                    Creating cluster members...
                  </span>{' '}
                  This may take a minute. Members will appear below as they are
                  provisioned.
                </AlertDescription>
              </Alert>
            )}

          {/* Cluster Creation Failed */}
          {service.service.topology === 'cluster' &&
            service.service.status === 'failed' && (
              <Alert variant="destructive">
                <AlertCircle className="h-4 w-4" />
                <AlertDescription className="flex items-center justify-between gap-4">
                  <div>
                    <span className="font-medium">
                      Cluster creation failed.
                    </span>{' '}
                    {(service.service as Record<string, unknown>).error_message
                      ? String(
                          (service.service as Record<string, unknown>)
                            .error_message
                        )
                      : 'An unknown error occurred.'}
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={retryCluster.isPending}
                    onClick={() => {
                      // Reconstruct members from preserved service_members records,
                      // or send empty array to let the backend reconstruct.
                      const members =
                        service.service.members &&
                        service.service.members.length > 0
                          ? service.service.members.map(
                              (m: { role: string; node_id?: number | null }) => ({
                                role: m.role,
                                node_id: m.node_id ?? undefined,
                              })
                            )
                          : []
                      retryCluster.mutate({
                        path: { id: parseInt(id!) },
                        body: { members },
                      })
                    }}
                  >
                    {retryCluster.isPending ? (
                      <Loader2 className="h-4 w-4 animate-spin mr-1" />
                    ) : (
                      <RefreshCcw className="h-4 w-4 mr-1" />
                    )}
                    Retry
                  </Button>
                </AlertDescription>
              </Alert>
            )}

          {/* Cluster Members Section */}
          {service.service.topology === 'cluster' &&
            service.service.members &&
            service.service.members.length > 0 && (
              <Card>
                <CardHeader>
                  <CardTitle className="flex items-center gap-2">
                    <span>Cluster Members</span>
                    <Badge variant="outline">
                      {service.service.members.length}
                    </Badge>
                  </CardTitle>
                  <CardDescription>
                    pg_auto_failover cluster nodes
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="space-y-2">
                    {service.service.members.map((member) => (
                      <div
                        key={member.id}
                        className="flex items-center justify-between p-3 rounded-md border border-border"
                      >
                        <div className="flex items-center gap-3">
                          {member.status === 'creating' ? (
                            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground flex-shrink-0" />
                          ) : (
                            <Server className="h-4 w-4 text-muted-foreground flex-shrink-0" />
                          )}
                          <div className="flex flex-col">
                            <div className="flex items-center gap-2">
                              <span className="font-mono text-sm">
                                {member.container_name}
                              </span>
                              <Badge
                                variant={
                                  member.role === 'primary'
                                    ? 'default'
                                    : 'secondary'
                                }
                                className="capitalize text-xs"
                              >
                                {member.role}
                              </Badge>
                            </div>
                            <div className="flex items-center gap-2 text-xs text-muted-foreground">
                              {member.hostname && (
                                <span>{member.hostname}</span>
                              )}
                              {member.port && <span>:{member.port}</span>}
                              {member.node_id && (
                                <span className="ml-1">
                                  (node {member.node_id})
                                </span>
                              )}
                            </div>
                          </div>
                        </div>
                        <Badge
                          variant={
                            member.status === 'running'
                              ? 'default'
                              : member.status === 'failed'
                                ? 'destructive'
                                : member.status === 'creating'
                                  ? 'outline'
                                  : 'secondary'
                          }
                          className="capitalize"
                        >
                          {member.status === 'creating' && (
                            <Loader2 className="h-3 w-3 animate-spin mr-1" />
                          )}
                          {member.status}
                        </Badge>
                      </div>
                    ))}
                  </div>
                </CardContent>
              </Card>
            )}

          {/* Service Configuration Section */}
          <Card>
            <CardHeader>
              <CardTitle>Configuration</CardTitle>
              <CardDescription>Current service parameters</CardDescription>
            </CardHeader>
            <CardContent>
              {service.current_parameters &&
              Object.keys(service.current_parameters).length > 0 ? (
                <div className="space-y-4">
                  {Object.entries(service.current_parameters).map(
                    ([key, value]) => {
                      const isSensitive = shouldMaskValue(key)
                      const isVisible = visibleParameters.has(key)
                      const displayValue =
                        isSensitive && !isVisible ? maskValue(value) : value

                      return (
                        <div key={key} className="space-y-1.5">
                          <div className="text-sm font-medium capitalize">
                            {key
                              .replace(/_/g, ' ')
                              .replace(/\b\w/g, (char) => char.toUpperCase())}
                          </div>
                          <div className="flex items-center gap-2 rounded-md border border-border bg-muted/50 p-3">
                            <span className="flex-1 break-all text-foreground font-mono text-sm">
                              {displayValue || (
                                <span className="text-muted-foreground">-</span>
                              )}
                            </span>
                            {isSensitive && (
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => {
                                  setVisibleParameters((prev) => {
                                    const next = new Set(prev)
                                    if (next.has(key)) {
                                      next.delete(key)
                                    } else {
                                      next.add(key)
                                    }
                                    return next
                                  })
                                }}
                                className="flex-shrink-0"
                                title={isVisible ? 'Hide value' : 'Show value'}
                              >
                                {isVisible ? (
                                  <EyeOff className="h-4 w-4" />
                                ) : (
                                  <Eye className="h-4 w-4" />
                                )}
                              </Button>
                            )}
                          </div>
                        </div>
                      )
                    }
                  )}
                </div>
              ) : (
                <div className="text-sm text-muted-foreground">
                  No parameters configured
                </div>
              )}
            </CardContent>
          </Card>

          {/* Environment Variables Section */}
          <Card>
            <CardHeader>
              <CardTitle>Environment Variables</CardTitle>
              <CardDescription>
                Preview of environment variables available to projects using
                this service
              </CardDescription>
            </CardHeader>
            <CardContent>
              {envVarsLoading ? (
                <div className="flex items-center justify-center py-8">
                  <Loader2 className="h-4 w-4 animate-spin mr-2" />
                  <span className="text-sm text-muted-foreground">
                    Loading environment variables...
                  </span>
                </div>
              ) : envVarsError ? (
                <div className="text-center py-8">
                  <AlertCircle className="h-8 w-8 mx-auto text-muted-foreground mb-2" />
                  <p className="text-sm text-muted-foreground">
                    Failed to load environment variables
                  </p>
                </div>
              ) : envVars ? (
                <>
                  <EnvVariablesDisplay
                    variables={envVars}
                    showCopy={true}
                    showMaskToggle={true}
                    defaultMasked={true}
                    maxHeight="20rem"
                  />
                  <p className="text-xs text-muted-foreground text-center mt-3">
                    These variables are automatically available to projects that
                    use this service
                  </p>
                </>
              ) : null}
            </CardContent>
          </Card>

          {isPostgres && pgUpgrades && pgUpgrades.length > 0 ? (
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center gap-2">
                  <ArrowUpCircle className="h-5 w-5" />
                  Major Version Upgrades
                </CardTitle>
                <CardDescription>
                  History of PostgreSQL major-version upgrades for this service.
                  Click a row to see phase progress and logs.
                </CardDescription>
              </CardHeader>
              <CardContent>
                <div className="space-y-2">
                  {pgUpgrades.map((u) => {
                    const totalPhases = PG_UPGRADE_PHASES.length - 1 // exclude "completed"
                    const pct =
                      u.status === 'completed'
                        ? 100
                        : Math.round(
                            (phaseIndex(u.phase) / totalPhases) * 100,
                          )
                    const statusVariant =
                      u.status === 'completed'
                        ? 'default'
                        : u.status === 'failed'
                        ? 'destructive'
                        : u.status === 'cancelled' ||
                          u.status === 'rolled_back'
                        ? 'secondary'
                        : 'outline'
                    const isActive = !isTerminal(u.status)
                    return (
                      <Link
                        key={u.id}
                        to={`/settings/storage/${id}/upgrades/${u.id}`}
                        className="block rounded-lg border p-3 hover:bg-muted/50 transition-colors"
                      >
                        <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                          <div className="flex flex-wrap items-center gap-2 min-w-0">
                            <span className="font-medium text-sm">
                              #{u.id}
                            </span>
                            <span className="text-sm text-muted-foreground truncate">
                              {u.from_version} → {u.to_version}
                            </span>
                            {isActive ? (
                              <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />
                            ) : null}
                          </div>
                          <div className="flex items-center gap-2">
                            <Badge variant={statusVariant} className="text-xs">
                              {u.status}
                            </Badge>
                            <span className="text-xs text-muted-foreground whitespace-nowrap">
                              <TimeAgo date={u.created_at} />
                            </span>
                          </div>
                        </div>
                        {isActive ? (
                          <div className="mt-2">
                            <div className="flex justify-between text-xs text-muted-foreground mb-1">
                              <span className="truncate">
                                Phase: {u.phase}
                              </span>
                              <span className="whitespace-nowrap ml-2">
                                {pct}%
                              </span>
                            </div>
                            <div className="h-1.5 bg-muted rounded overflow-hidden">
                              <div
                                className="h-full bg-primary transition-all"
                                style={{ width: `${pct}%` }}
                              />
                            </div>
                          </div>
                        ) : u.error_message ? (
                          <p className="mt-2 text-xs text-destructive line-clamp-2">
                            {u.error_message}
                          </p>
                        ) : null}
                      </Link>
                    )
                  })}
                </div>
              </CardContent>
            </Card>
          ) : null}
        </div>
      </div>

      <Dialog open={isStopDialogOpen} onOpenChange={setIsStopDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Stop Service</DialogTitle>
            <DialogDescription>
              Are you sure you want to stop{' '}
              <span className="font-medium text-foreground">
                {service.service.name}
              </span>
              ? All connected projects will lose access to this service until it
              is started again.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setIsStopDialogOpen(false)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={handleStop}
              disabled={stopService.isPending}
            >
              {stopService.isPending && (
                <Loader2 className="h-4 w-4 mr-2 animate-spin" />
              )}
              Stop Service
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={isDeleteDialogOpen} onOpenChange={setIsDeleteDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete Service</DialogTitle>
            <DialogDescription>
              Are you sure you want to delete this service? This action cannot
              be undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setIsDeleteDialogOpen(false)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={handleDelete}
              disabled={deleteService.isPending}
            >
              {deleteService.isPending && (
                <Loader2 className="h-4 w-4 mr-2 animate-spin" />
              )}
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <UpgradeServiceDialog
        open={isUpgradeDialogOpen}
        onOpenChange={setIsUpgradeDialogOpen}
        serviceId={parseInt(id!)}
        serviceName={service.service.name}
        currentImage={service.current_parameters?.docker_image || undefined}
        serviceType={service.service.service_type}
      />

      <MajorUpgradeDialog
        open={isMajorUpgradeDialogOpen}
        onOpenChange={setIsMajorUpgradeDialogOpen}
        serviceId={parseInt(id!)}
        serviceName={service.service.name}
        currentImage={service.current_parameters?.docker_image || ''}
      />

      <EditServiceDialog
        open={isEditDialogOpen}
        onOpenChange={setIsEditDialogOpen}
        service={service.service}
        currentParameters={service.current_parameters}
        onSuccess={() => {
          refetch()
          queryClient.invalidateQueries({
            queryKey: getServiceOptions({
              path: { id: parseInt(id!) },
            }).queryKey,
          })
        }}
      />

      <TriggerBackupDialog
        open={isBackupDialogOpen}
        onOpenChange={setIsBackupDialogOpen}
        serviceId={parseInt(id!)}
        serviceName={service.service.name}
      />
    </div>
  )
}
