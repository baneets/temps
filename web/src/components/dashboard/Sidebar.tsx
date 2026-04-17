import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  useSidebar,
} from '@/components/ui/sidebar'
import {
  Activity,
  BadgeCheck,
  Box,
  ChevronsUpDown,
  Folder,
  LogOut,
  MoreHorizontal,
  ScrollText,
  Settings,
} from 'lucide-react'

import { ProjectResponse } from '@/api/client'
import { useAuth } from '@/contexts/AuthContext'
import { usePluginsContext } from '@/contexts/PluginsContext'
import { useProjects } from '@/contexts/ProjectsContext'
import { resolvePluginIcon } from '@/lib/pluginIcons'
import { cn } from '@/lib/utils'
import { ChevronRight, type LucideIcon } from 'lucide-react'
import { useEffect, useMemo } from 'react'
import { Link, useLocation } from 'react-router-dom'
import { Avatar, AvatarFallback, AvatarImage } from '../ui/avatar'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '../ui/collapsible'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '../ui/dropdown-menu'

const navMain = [
  {
    title: 'Projects',
    url: '/projects',
    icon: Folder,
    isActive: true,
  },
  {
    title: 'Sandboxes',
    url: '/sandboxes',
    icon: Box,
  },
  {
    title: 'Monitoring',
    url: '/monitoring',
    icon: Activity,
  },
]

const navObserve = [
  {
    title: 'Proxy Logs',
    url: '/proxy-logs',
    icon: Activity,
  },
  {
    title: 'Audit Logs',
    url: '/audit-logs',
    icon: ScrollText,
  },
]

function NavProjects({ projects }: { projects: ProjectResponse[] }) {
  const { isMinimal, isMobile } = useSidebar()

  return (
    <SidebarGroup
      className={
        isMinimal && !isMobile ? '' : 'group-data-[collapsible=icon]:hidden'
      }
    >
      <SidebarGroupLabel className={isMinimal && !isMobile ? 'hidden' : ''}>
        Projects
      </SidebarGroupLabel>
      <SidebarMenu>
        {projects.map((item) => (
          <SidebarMenuItem key={item.id}>
            <SidebarMenuButton
              asChild
              tooltip={isMinimal && !isMobile ? item.name : undefined}
              className={cn(
                'justify-center',
                (!isMinimal || isMobile) && 'justify-start'
              )}
            >
              <Link to={`/projects/${item.slug}`}>
                <Avatar className="size-6">
                  <AvatarImage src={`/api/projects/${item.id}/favicon`} />
                  <AvatarFallback>{item.name.charAt(0)}</AvatarFallback>
                </Avatar>
                {(!isMinimal || isMobile) && <span>{item.name}</span>}
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        ))}
        <SidebarMenuItem>
          <SidebarMenuButton
            asChild
            tooltip={isMinimal && !isMobile ? 'More Projects' : undefined}
            className={cn(
              'justify-center',
              (!isMinimal || isMobile) && 'justify-start'
            )}
          >
            <Link to="/projects">
              <MoreHorizontal />
              {(!isMinimal || isMobile) && <span>More</span>}
            </Link>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    </SidebarGroup>
  )
}

function NavMain({
  items,
}: {
  items: {
    title: string
    url: string
    icon: LucideIcon
    isActive?: boolean
    items?: { title: string; url: string }[]
  }[]
}) {
  const location = useLocation()
  const { isMinimal, isMobile } = useSidebar()

  return (
    <SidebarGroup>
      <SidebarGroupLabel className={isMinimal && !isMobile ? 'hidden' : ''}>
        Platform
      </SidebarGroupLabel>
      <SidebarMenu>
        {items.map((item) => {
          const isActive = location.pathname.startsWith(item.url)
          return (
            <Collapsible key={item.title} asChild defaultOpen={item.isActive}>
              <SidebarMenuItem>
                <SidebarMenuButton
                  asChild
                  tooltip={isMinimal && !isMobile ? item.title : undefined}
                  className={cn(
                    'justify-center',
                    (!isMinimal || isMobile) && 'justify-start',
                    isActive &&
                      'bg-sidebar-accent text-sidebar-accent-foreground'
                  )}
                >
                  <Link to={item.url}>
                    <item.icon />
                    {(!isMinimal || isMobile) && <span>{item.title}</span>}
                  </Link>
                </SidebarMenuButton>
                {(!isMinimal || isMobile) && item.items?.length ? (
                  <>
                    <CollapsibleTrigger asChild>
                      <SidebarMenuAction className="data-[state=open]:rotate-90">
                        <ChevronRight />
                        <span className="sr-only">Toggle</span>
                      </SidebarMenuAction>
                    </CollapsibleTrigger>
                    <CollapsibleContent>
                      <SidebarMenuSub>
                        {item.items?.map((subItem) => (
                          <SidebarMenuSubItem key={subItem.title}>
                            <SidebarMenuSubButton asChild>
                              <Link to={subItem.url}>
                                <span>{subItem.title}</span>
                              </Link>
                            </SidebarMenuSubButton>
                          </SidebarMenuSubItem>
                        ))}
                      </SidebarMenuSub>
                    </CollapsibleContent>
                  </>
                ) : null}
              </SidebarMenuItem>
            </Collapsible>
          )
        })}
      </SidebarMenu>
    </SidebarGroup>
  )
}

function NavObserve({
  items,
}: {
  items: { title: string; url: string; icon: LucideIcon }[]
}) {
  const location = useLocation()
  const { isMinimal, isMobile } = useSidebar()

  return (
    <SidebarGroup
      className={
        isMinimal && !isMobile ? '' : 'group-data-[collapsible=icon]:hidden'
      }
    >
      <SidebarGroupLabel className={isMinimal && !isMobile ? 'hidden' : ''}>
        Observe
      </SidebarGroupLabel>
      <SidebarMenu>
        {items.map((item) => {
          const isActive = location.pathname.startsWith(item.url)
          return (
            <SidebarMenuItem key={item.title}>
              <SidebarMenuButton
                asChild
                tooltip={isMinimal && !isMobile ? item.title : undefined}
                className={cn(
                  'justify-center',
                  (!isMinimal || isMobile) && 'justify-start',
                  isActive && 'bg-sidebar-accent text-sidebar-accent-foreground'
                )}
              >
                <Link to={item.url}>
                  <item.icon />
                  {(!isMinimal || isMobile) && <span>{item.title}</span>}
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          )
        })}
      </SidebarMenu>
    </SidebarGroup>
  )
}

function NavPlugins({
  items,
}: {
  items: { title: string; url: string; icon: LucideIcon }[]
}) {
  const location = useLocation()
  const { isMinimal, isMobile } = useSidebar()

  if (items.length === 0) return null

  return (
    <SidebarGroup
      className={
        isMinimal && !isMobile ? '' : 'group-data-[collapsible=icon]:hidden'
      }
    >
      <SidebarGroupLabel className={isMinimal && !isMobile ? 'hidden' : ''}>
        Plugins
      </SidebarGroupLabel>
      <SidebarMenu>
        {items.map((item) => {
          const isActive = location.pathname.startsWith(item.url)
          return (
            <SidebarMenuItem key={item.title}>
              <SidebarMenuButton
                asChild
                tooltip={isMinimal && !isMobile ? item.title : undefined}
                className={cn(
                  'justify-center',
                  (!isMinimal || isMobile) && 'justify-start',
                  isActive && 'bg-sidebar-accent text-sidebar-accent-foreground'
                )}
              >
                <Link to={item.url}>
                  <item.icon />
                  {(!isMinimal || isMobile) && <span>{item.title}</span>}
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          )
        })}
      </SidebarMenu>
    </SidebarGroup>
  )
}

function NavSettingsLink() {
  const location = useLocation()
  const { isMinimal, isMobile } = useSidebar()
  const isActive = location.pathname.startsWith('/settings')

  return (
    <SidebarGroup
      className={
        isMinimal && !isMobile ? '' : 'group-data-[collapsible=icon]:hidden'
      }
    >
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton
            asChild
            tooltip={isMinimal && !isMobile ? 'Settings' : undefined}
            className={cn(
              'justify-center',
              (!isMinimal || isMobile) && 'justify-start',
              isActive && 'bg-sidebar-accent text-sidebar-accent-foreground'
            )}
          >
            <Link to="/settings">
              <Settings />
              {(!isMinimal || isMobile) && <span>Settings</span>}
            </Link>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    </SidebarGroup>
  )
}

export default function AppSidebar() {
  const { projects } = useProjects()
  const { setIsMinimal, isMinimal, isMobile } = useSidebar()
  const { platformNavEntries } = usePluginsContext()
  const location = useLocation()

  // Convert plugin nav entries to sidebar item format
  const pluginItems = useMemo(
    () =>
      platformNavEntries.map((entry) => ({
        title: entry.label,
        url: entry.path,
        icon: resolvePluginIcon(entry.icon),
      })),
    [platformNavEntries]
  )

  // Auto-collapse sidebar when on project detail pages
  useEffect(() => {
    const isProjectDetailPage = location.pathname.match(
      /^\/projects\/[^/]+\/(project|deployments|analytics|storage|runtime|settings|speed|errors|logs|webhooks|monitors|monitoring|traces|ai-gateway|services|environments|security)/
    )

    if (isProjectDetailPage && !isMobile) {
      setIsMinimal(true)
    }
  }, [location.pathname, isMobile, setIsMinimal])

  return (
    <>
      <Sidebar>
        <SidebarHeader>
          <SidebarMenu>
            <SidebarMenuItem>
              <div
                className={cn(
                  'flex items-center gap-2',
                  isMinimal && !isMobile && 'justify-center'
                )}
              >
                <div
                  className={cn(
                    'flex aspect-square size-8 items-center justify-center rounded-lg',
                    isMinimal && !isMobile && 'w-6 h-6' // Make logo slightly smaller in minimal mode
                  )}
                >
                  <img src="/svg/temps-icon.svg" alt="logo" className="size-full" />
                </div>
                {(!isMinimal || isMobile) && (
                  <div
                    className={cn(
                      'grid flex-1 text-left text-sm leading-tight',
                      isMinimal && isMobile && 'text-xs'
                    )}
                  >
                    <span className="truncate font-semibold">Temps</span>
                    <span className="truncate text-xs">{import.meta.env.TEMPS_VERSION}</span>
                  </div>
                )}
              </div>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarHeader>
        <SidebarContent>
          <NavMain items={navMain} />
          <NavProjects projects={projects} />
          <NavObserve items={navObserve} />
          <NavPlugins items={pluginItems} />
          <NavSettingsLink />
          <SidebarGroup />
        </SidebarContent>
        <SidebarFooter>
          <NavUser />
        </SidebarFooter>
      </Sidebar>
    </>
  )
}

function NavUser() {
  const { user } = useAuth()
  const { isMobile, isMinimal } = useSidebar()
  const { logout } = useAuth()
  if (!user) return null

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <SidebarMenuButton
              size="lg"
              className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
            >
              <Avatar className="h-8 w-8 rounded-lg">
                <AvatarImage
                  src={user.avatar_url || ''}
                  alt={user.username || ''}
                />
                <AvatarFallback className="rounded-lg">
                  {user.username?.slice(0, 2).toUpperCase() || 'U'}
                </AvatarFallback>
              </Avatar>
              {(!isMinimal || isMobile) && (
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-semibold">
                    {user.username || 'User'}
                  </span>
                  <span className="truncate text-xs">{user.email}</span>
                </div>
              )}
              <ChevronsUpDown className="ml-auto size-4" />
            </SidebarMenuButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            className="w-(--radix-dropdown-menu-trigger-width) min-w-56 rounded-lg"
            side={isMobile ? 'bottom' : 'right'}
            align="end"
            sideOffset={4}
          >
            <DropdownMenuLabel className="p-0 font-normal">
              <div className="flex items-center gap-2 px-1 py-1.5 text-left text-sm">
                <Avatar className="h-8 w-8 rounded-lg">
                  <AvatarImage
                    src={user.avatar_url || ''}
                    alt={user.username || ''}
                  />
                  <AvatarFallback className="rounded-lg">
                    {user.username?.slice(0, 2).toUpperCase() || 'U'}
                  </AvatarFallback>
                </Avatar>
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-semibold">
                    {user.username || 'User'}
                  </span>
                  <span className="truncate text-xs">{user.email}</span>
                </div>
              </div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />

            <DropdownMenuGroup>
              <DropdownMenuItem>
                <Link to="/account" className="flex items-center">
                  <BadgeCheck className="mr-2 h-4 w-4" />
                  <span>Account</span>
                </Link>
              </DropdownMenuItem>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              onClick={async () => {
                await logout()
              }}
            >
              <LogOut />
              Log out
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  )
}
