import { listServicesOptions } from '@/api/client/@tanstack/react-query.gen'
import { ExternalServiceInfo } from '@/api/client/types.gen'
import {
  listServiceHealthStatuses,
  type HealthStatus,
} from '@/lib/service-health'
import { CreateServiceButton } from '@/components/storage/CreateServiceButton'
import { DeleteServiceButton } from '@/components/storage/DeleteServiceButton'
import { EditServiceDialog } from '@/components/storage/EditServiceDialog'
import { ImportServiceButton } from '@/components/storage/ImportServiceButton'
import { PlatformServices } from '@/components/storage/PlatformServices'
import EmptyStateStorage from '@/components/storage/EmptyStateStorage'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { ServiceLogo } from '@/components/ui/service-logo'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { useKeyboardShortcut } from '@/hooks/useKeyboardShortcut'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useQuery } from '@tanstack/react-query'
import {
  ChevronRight,
  Database,
  HardDrive,
  Pencil,
  RefreshCcw,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { TimeAgo } from '@/components/utils/TimeAgo'

export function Storage() {
  const { setBreadcrumbs } = useBreadcrumbs()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const [isEditDialogOpen, setIsEditDialogOpen] = useState(false)
  const [selectedService, setSelectedService] = useState<ExternalServiceInfo | null>(null)
  const [isCreateDropdownOpen, setIsCreateDropdownOpen] = useState(false)

  // Get active tab from URL or default to 'external'
  const activeTab = searchParams.get('tab') || 'external'

  const handleTabChange = (value: string) => {
    setSearchParams({ tab: value })
  }

  const {
    data: services,
    isLoading,
    error,
    refetch,
  } = useQuery({
    ...listServicesOptions(),
  })

  // Batch-fetch current health for every service shown so each row can render
  // a dot without firing N requests. Re-polls every 30s to match the backend
  // probe cadence.
  const serviceIds = (services ?? []).map((s) => s.id)
  const idsKey = serviceIds.join(',')
  const { data: healthMap } = useQuery({
    queryKey: ['service-health-batch', idsKey],
    queryFn: () => listServiceHealthStatuses(serviceIds),
    enabled: serviceIds.length > 0,
    refetchInterval: 30_000,
    staleTime: 25_000,
  })

  useEffect(() => {
    setBreadcrumbs([{ label: 'Storage', href: '/storage' }])
  }, [setBreadcrumbs])

  // Keyboard shortcut: N to open the create service dropdown
  useKeyboardShortcut({
    key: 'n',
    callback: () => setIsCreateDropdownOpen(true),
  })

  usePageTitle('Storage')

  // Render external services content based on loading/error/empty state
  const renderExternalServicesContent = () => {
    if (isLoading) {
      return (
        <div className="space-y-4">
          <div className="flex items-center justify-end">
            <div className="h-9 w-24 bg-muted rounded animate-pulse" />
          </div>
          <div className="grid gap-4">
            {[...Array(3)].map((_, i) => (
              <Card key={i}>
                <CardHeader>
                  <div className="flex items-center justify-between">
                    <div className="space-y-2">
                      <div className="h-5 w-40 bg-muted rounded animate-pulse" />
                      <div className="h-4 w-24 bg-muted rounded animate-pulse" />
                    </div>
                    <div className="h-8 w-20 bg-muted rounded animate-pulse" />
                  </div>
                </CardHeader>
                <CardContent>
                  <div className="h-4 w-full bg-muted rounded animate-pulse" />
                </CardContent>
              </Card>
            ))}
          </div>
        </div>
      )
    }

    if (error) {
      return (
        <div className="flex flex-col items-center justify-center py-12 text-center">
          <p className="text-sm text-muted-foreground mb-4">
            Failed to load services
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
      )
    }

    if (!services?.length) {
      return <EmptyStateStorage />
    }

    return (
      <>
        <div className="flex items-center justify-end mb-4">
          <div className="flex items-center gap-2">
            <ImportServiceButton onSuccess={() => refetch()} />
            <CreateServiceButton
              onSuccess={() => refetch()}
              open={isCreateDropdownOpen}
              onOpenChange={setIsCreateDropdownOpen}
            />
          </div>
        </div>

        <ServicesDividerList
          services={services}
          healthMap={healthMap}
          onOpen={(id) => navigate(`/storage/${id}`)}
          onEdit={(service) => {
            setSelectedService(service)
            setIsEditDialogOpen(true)
          }}
          onDeleteSuccess={() => refetch()}
        />
      </>
    )
  }

  return (
    <div className="flex-1 overflow-auto">
      <div className="sm:p-4 space-y-6 md:p-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold sm:text-2xl">Storage</h1>
        </div>

        <Tabs value={activeTab} onValueChange={handleTabChange} className="space-y-6">
          <TabsList>
            <TabsTrigger value="platform" className="gap-2">
              <Database className="h-4 w-4" />
              Platform Services
            </TabsTrigger>
            <TabsTrigger value="external" className="gap-2">
              <HardDrive className="h-4 w-4" />
              External Services
            </TabsTrigger>
          </TabsList>

          <TabsContent value="platform" className="space-y-6">
            <PlatformServices />
          </TabsContent>

          <TabsContent value="external" className="space-y-6">
            {renderExternalServicesContent()}
          </TabsContent>
        </Tabs>

        {selectedService && (
          <EditServiceDialog
            open={isEditDialogOpen}
            onOpenChange={(open) => {
              setIsEditDialogOpen(open)
              if (!open) {
                setSelectedService(null)
              }
            }}
            service={selectedService}
            onSuccess={() => {
              setIsEditDialogOpen(false)
              setSelectedService(null)
              refetch()
            }}
          />
        )}
      </div>
    </div>
  )
}

// ── Shared variant helpers ──

interface ServicesVariantProps {
  services: ExternalServiceInfo[]
  healthMap?: Map<number, { status?: HealthStatus | null }>
  onOpen: (id: number) => void
  onEdit: (service: ExternalServiceInfo) => void
  onDeleteSuccess: () => void
}

/**
 * Small solid dot reflecting the latest health probe
 * (green=operational, amber=degraded, red=down). We render nothing when the
 * service hasn't been probed yet so the row doesn't flash a placeholder.
 */
function HealthDot({
  status,
  className,
}: {
  status: HealthStatus | null | undefined
  className?: string
}) {
  if (!status) return null
  const tone =
    status === 'operational'
      ? 'bg-green-500'
      : status === 'degraded'
        ? 'bg-amber-500'
        : 'bg-red-500'
  const label =
    status === 'operational'
      ? 'Operational'
      : status === 'degraded'
        ? 'Degraded'
        : 'Down'
  return (
    <span
      className={cn('inline-block size-2 rounded-full', tone, className)}
      title={label}
      aria-label={label}
    />
  )
}

function ServiceStatusDot({ status }: { status: string }) {
  const tone =
    status === 'running' || status === 'active' || status === 'ready'
      ? 'bg-success'
      : status === 'error' || status === 'failed'
        ? 'bg-destructive'
        : status === 'pending' || status === 'initializing'
          ? 'bg-warning'
          : 'bg-muted-foreground/40'
  return (
    <span className="flex items-center gap-1.5">
      <span className={cn('size-1.5 rounded-full', tone)} />
      <span className="text-xs capitalize text-muted-foreground">
        {status || 'unknown'}
      </span>
    </span>
  )
}

function ServiceActions({
  service,
  onEdit,
  onDeleteSuccess,
}: {
  service: ExternalServiceInfo
  onEdit: (service: ExternalServiceInfo) => void
  onDeleteSuccess: () => void
}) {
  return (
    <div
      className="flex items-center gap-1"
      onClick={(e) => e.stopPropagation()}
    >
      <Button
        variant="ghost"
        size="icon"
        onClick={() => onEdit(service)}
        className="size-8"
      >
        <Pencil className="size-3.5" />
      </Button>
      <DeleteServiceButton
        serviceId={service.id}
        serviceName={service.name}
        onSuccess={onDeleteSuccess}
      />
    </div>
  )
}

// ── Variant: Divider list (Vercel-style) ──

function ServicesDividerList({
  services,
  healthMap,
  onOpen,
  onEdit,
  onDeleteSuccess,
}: ServicesVariantProps) {
  return (
    <div className="overflow-hidden rounded-lg border">
      <ul role="list" className="divide-y">
        {services.map((service) => {
          // Only show the health dot for services the backend is actually
          // probing (status === 'running'). Others haven't been checked.
          const health =
            service.status === 'running'
              ? healthMap?.get(service.id)?.status ?? null
              : null
          return (
            <li
              key={service.id}
              onClick={() => onOpen(service.id)}
              className="group flex cursor-pointer items-center gap-4 px-4 py-3 transition-colors hover:bg-muted/40"
            >
              {/*
                Wrap the logo so we can anchor the health dot in its
                bottom-right corner (Vercel deployment-status pattern).
              */}
              <div className="relative shrink-0">
                <div className="flex size-9 items-center justify-center rounded-md border bg-background">
                  <ServiceLogo service={service.service_type} />
                </div>
                {health ? (
                  <HealthDot
                    status={health}
                    className="absolute -bottom-0.5 -right-0.5 ring-2 ring-background"
                  />
                ) : null}
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <p className="truncate text-sm font-medium">{service.name}</p>
                  <span className="rounded border bg-muted/50 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
                    {service.service_type}
                  </span>
                  {service.topology === 'cluster' && (
                    <span className="rounded border px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted-foreground">
                      Cluster
                    </span>
                  )}
                </div>
                <div className="mt-1 flex items-center gap-3 text-xs text-muted-foreground">
                  <ServiceStatusDot status={service.status} />
                  {service.version && (
                    <span className="font-mono">v{service.version}</span>
                  )}
                  <span>
                    Created <TimeAgo date={service.created_at} />
                  </span>
                </div>
              </div>
              <ServiceActions
                service={service}
                onEdit={onEdit}
                onDeleteSuccess={onDeleteSuccess}
              />
              <ChevronRight className="size-4 shrink-0 text-muted-foreground/40 transition-transform group-hover:translate-x-0.5 group-hover:text-muted-foreground" />
            </li>
          )
        })}
      </ul>
    </div>
  )
}
