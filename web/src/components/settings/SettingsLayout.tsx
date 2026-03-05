import { cn } from '@/lib/utils'
import {
  Bell,
  DatabaseBackup,
  Globe,
  HardDrive,
  Key,
  Monitor,
  Network,
  Puzzle,
  Server,
  Settings2,
  Shield,
  Users,
  type LucideIcon,
} from 'lucide-react'
import { Link, Outlet, useLocation } from 'react-router-dom'

interface SettingsNavItem {
  title: string
  url: string
  icon: LucideIcon
}

interface SettingsNavGroup {
  label: string
  items: SettingsNavItem[]
}

const settingsNavGroups: SettingsNavGroup[] = [
  {
    label: 'General',
    items: [
      { title: 'Platform', url: '/settings', icon: Settings2 },
      { title: 'Notifications', url: '/settings/notifications', icon: Bell },
    ],
  },
  {
    label: 'Access',
    items: [
      { title: 'Users', url: '/settings/users', icon: Users },
      { title: 'API Keys', url: '/settings/keys', icon: Key },
    ],
  },
  {
    label: 'Infrastructure',
    items: [
      {
        title: 'Load Balancer',
        url: '/settings/load-balancer',
        icon: Server,
      },
      {
        title: 'Docker Registry',
        url: '/settings/docker-registry',
        icon: Globe,
      },
      { title: 'Backups', url: '/settings/backups', icon: DatabaseBackup },
      { title: 'Worker Nodes', url: '/settings/nodes', icon: Network },
      { title: 'Plugins', url: '/settings/plugins', icon: Puzzle },
    ],
  },
  {
    label: 'Security',
    items: [
      {
        title: 'Security Headers',
        url: '/settings/security',
        icon: Shield,
      },
      {
        title: 'Rate Limiting',
        url: '/settings/rate-limiting',
        icon: Monitor,
      },
      {
        title: 'Disk Monitoring',
        url: '/settings/disk-monitoring',
        icon: HardDrive,
      },
    ],
  },
]

function isActive(pathname: string, url: string): boolean {
  if (url === '/settings') {
    return pathname === '/settings'
  }
  return pathname.startsWith(url)
}

export function SettingsLayout() {
  const location = useLocation()

  return (
    <div className="w-full px-4 sm:px-6 lg:px-8 py-6">
      <div className="max-w-7xl mx-auto">
        <div className="mb-6">
          <h2 className="text-2xl font-bold tracking-tight">Settings</h2>
          <p className="text-muted-foreground">
            Manage your platform configuration
          </p>
        </div>
        <div className="flex flex-col lg:flex-row gap-6">
          {/* Inner sidebar */}
          <nav className="w-full lg:w-56 shrink-0">
            <div className="flex flex-row lg:flex-col gap-1 overflow-x-auto lg:overflow-visible pb-2 lg:pb-0">
              {settingsNavGroups.map((group) => (
                <div key={group.label} className="mb-3 min-w-max lg:min-w-0">
                  <p className="px-3 mb-1 text-xs font-medium text-muted-foreground uppercase tracking-wider">
                    {group.label}
                  </p>
                  <div className="flex flex-row lg:flex-col gap-0.5">
                    {group.items.map((item) => {
                      const active = isActive(location.pathname, item.url)
                      return (
                        <Link
                          key={item.url}
                          to={item.url}
                          className={cn(
                            'flex items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors whitespace-nowrap',
                            active
                              ? 'bg-accent text-accent-foreground font-medium'
                              : 'text-muted-foreground hover:bg-accent/50 hover:text-accent-foreground'
                          )}
                        >
                          <item.icon className="h-4 w-4 shrink-0" />
                          {item.title}
                        </Link>
                      )
                    })}
                  </div>
                </div>
              ))}
            </div>
          </nav>

          {/* Content area */}
          <div className="flex-1 min-w-0">
            <Outlet />
          </div>
        </div>
      </div>
    </div>
  )
}
