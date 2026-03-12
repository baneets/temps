import { EnvironmentResponse } from '@/api/client'
import {
  sleepEnvironmentMutation,
  wakeEnvironmentMutation,
} from '@/api/client/@tanstack/react-query.gen'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Box, Loader2, Moon, Play, Settings } from 'lucide-react'
import { useCallback } from 'react'
import { toast } from 'sonner'

interface EnvironmentSidebarProps {
  environment: EnvironmentResponse
  activeView: string
  onViewChange: (view: string) => void
  isStatic: boolean
}

export interface NavItem {
  title: string
  view: string
  icon: React.ComponentType<{ className?: string }>
  visible?: boolean
  shortcut?: string
}

export function EnvironmentSidebar({
  environment,
  activeView,
  onViewChange,
  isStatic,
}: EnvironmentSidebarProps) {
  const queryClient = useQueryClient()
  const isOnDemand = environment.deployment_config?.onDemand ?? false

  const wakeMutation = useMutation({
    ...wakeEnvironmentMutation(),
    onSuccess: () => {
      toast.success('Environment is waking up')
      queryClient.invalidateQueries({ queryKey: ['environment'] })
    },
    meta: { errorTitle: 'Failed to wake environment' },
  })

  const sleepMutation = useMutation({
    ...sleepEnvironmentMutation(),
    onSuccess: () => {
      toast.success('Environment is going to sleep')
      queryClient.invalidateQueries({ queryKey: ['environment'] })
    },
    meta: { errorTitle: 'Failed to sleep environment' },
  })

  const navItems: NavItem[] = [
    {
      title: 'Containers',
      view: 'containers',
      icon: Box,
      visible: !isStatic,
      shortcut: '⌘C',
    },
    {
      title: 'Settings',
      view: 'settings',
      icon: Settings,
      visible: true,
      shortcut: '⌘,',
    },
  ]

  const visibleItems = navItems.filter((item) => item.visible !== false)

  const handleNavClick = useCallback(
    (view: string) => {
      onViewChange(view)
    },
    [onViewChange]
  )

  return (
    <>
      {/* Mobile Navigation - Tabs */}
      <div className="lg:hidden border-b bg-background">
        <Tabs value={activeView} onValueChange={handleNavClick}>
          <TabsList className="w-full rounded-none bg-transparent border-b-0 justify-start h-auto p-0">
            {visibleItems.map((item) => {
              const Icon = item.icon
              return (
                <TabsTrigger
                  key={item.view}
                  value={item.view}
                  className="rounded-none border-b-2 border-b-transparent data-[state=active]:border-b-primary data-[state=active]:bg-transparent"
                >
                  <Icon className="h-4 w-4 mr-2" />
                  <span className="text-sm">{item.title}</span>
                </TabsTrigger>
              )
            })}
          </TabsList>
        </Tabs>
      </div>

      {/* Desktop Navigation - Sidebar */}
      <div className="hidden lg:flex w-64 border-r bg-muted/30 flex-col h-full">
        {/* Environment Info */}
        <div className="p-4 border-b">
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <h3 className="font-semibold text-sm truncate">
                {environment.name}
              </h3>
              {environment.sleeping && (
                <Badge variant="outline" className="text-[10px] gap-1 text-yellow-600 dark:text-yellow-400 border-yellow-500/30 bg-yellow-500/10">
                  <Moon className="h-3 w-3" />
                  Sleeping
                </Badge>
              )}
            </div>
            {environment.branch && (
              <p className="text-xs text-muted-foreground truncate">
                Branch: {environment.branch}
              </p>
            )}
            {isOnDemand && (
              <div className="pt-1">
                {environment.sleeping ? (
                  <Button
                    variant="outline"
                    size="sm"
                    className="w-full h-7 text-xs"
                    disabled={wakeMutation.isPending}
                    onClick={() =>
                      wakeMutation.mutate({
                        path: {
                          project_id: environment.project_id,
                          env_id: environment.id,
                        },
                      })
                    }
                  >
                    {wakeMutation.isPending ? (
                      <Loader2 className="h-3 w-3 mr-1.5 animate-spin" />
                    ) : (
                      <Play className="h-3 w-3 mr-1.5" />
                    )}
                    Wake Up
                  </Button>
                ) : (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="w-full h-7 text-xs text-muted-foreground"
                    disabled={sleepMutation.isPending}
                    onClick={() =>
                      sleepMutation.mutate({
                        path: {
                          project_id: environment.project_id,
                          env_id: environment.id,
                        },
                      })
                    }
                  >
                    {sleepMutation.isPending ? (
                      <Loader2 className="h-3 w-3 mr-1.5 animate-spin" />
                    ) : (
                      <Moon className="h-3 w-3 mr-1.5" />
                    )}
                    Sleep
                  </Button>
                )}
              </div>
            )}
          </div>
        </div>

        {/* Navigation */}
        <nav className="flex-1 p-2 overflow-y-auto space-y-1">
          <TooltipProvider>
            {visibleItems.map((item) => {
              const Icon = item.icon
              const isActive = activeView === item.view

              return (
                <Tooltip key={item.view}>
                  <TooltipTrigger asChild>
                    <Button
                      variant={isActive ? 'secondary' : 'ghost'}
                      size="sm"
                      className="w-full justify-start gap-2 h-9"
                      onClick={() => handleNavClick(item.view)}
                    >
                      <Icon className="h-4 w-4 flex-shrink-0" />
                      <span className="truncate">{item.title}</span>
                    </Button>
                  </TooltipTrigger>
                  {item.shortcut && (
                    <TooltipContent
                      side="right"
                      className="flex items-center gap-2"
                    >
                      <span>{item.title}</span>
                      <kbd className="hidden sm:inline-flex items-center gap-1 rounded border border-border bg-muted px-1.5 py-0.5 text-xs font-medium text-muted-foreground">
                        {item.shortcut}
                      </kbd>
                    </TooltipContent>
                  )}
                </Tooltip>
              )
            })}
          </TooltipProvider>
        </nav>

        {/* Environment Details Footer */}
        <div className="border-t p-3 text-xs text-muted-foreground space-y-1">
          <div>
            <span className="font-medium">ID:</span>{' '}
            <span className="font-mono text-xs">{environment.id}</span>
          </div>
          {environment.branch && (
            <div>
              <span className="font-medium">Branch:</span>{' '}
              <span className="font-mono text-xs">{environment.branch}</span>
            </div>
          )}
        </div>
      </div>
    </>
  )
}
