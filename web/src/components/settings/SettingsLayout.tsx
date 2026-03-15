import { cn } from '@/lib/utils'
import {
  Bell,
  ChevronLeft,
  Cloud,
  Database,
  DatabaseBackup,
  GitBranch,
  Globe,
  HardDrive,
  Key,
  Mail,
  Monitor,
  Network,
  Puzzle,
  Server,
  Settings2,
  Shield,
  Sparkles,
  Users,
  type LucideIcon,
} from 'lucide-react'
import { useState } from 'react'
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
      { title: 'Domains', url: '/domains', icon: Globe },
      { title: 'Storage', url: '/storage', icon: Database },
      { title: 'Email', url: '/email', icon: Mail },
      { title: 'AI Gateway', url: '/ai-gateway', icon: Sparkles },
      {
        title: 'Git Providers',
        url: '/git-providers',
        icon: GitBranch,
      },
      {
        title: 'DNS Providers',
        url: '/dns-providers',
        icon: Cloud,
      },
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

function findActiveItem(pathname: string): SettingsNavItem | undefined {
  for (const group of settingsNavGroups) {
    for (const item of group.items) {
      if (isActive(pathname, item.url)) return item
    }
  }
  return undefined
}

function SettingsNav({ onClick }: { onClick?: () => void }) {
  const location = useLocation()

  return (
    <nav className="space-y-5">
      {settingsNavGroups.map((group) => (
        <div key={group.label}>
          <p className="px-3 mb-1.5 text-[11px] font-semibold text-muted-foreground/70 uppercase tracking-widest">
            {group.label}
          </p>
          <div className="flex flex-col gap-0.5">
            {group.items.map((item) => {
              const active = isActive(location.pathname, item.url)
              return (
                <Link
                  key={item.url}
                  to={item.url}
                  onClick={onClick}
                  className={cn(
                    'flex items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors',
                    active
                      ? 'bg-accent text-accent-foreground font-medium'
                      : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'
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
    </nav>
  )
}

export function SettingsLayout() {
  const location = useLocation()
  const [showNav, setShowNav] = useState(false)
  const activeItem = findActiveItem(location.pathname)

  return (
    <div className="w-full px-4 sm:px-6 lg:px-8 py-6">
      <div className="max-w-7xl mx-auto">
        <div className="mb-6">
          {/* Mobile: breadcrumb back button when viewing content */}
          <div className="flex items-center gap-2 lg:hidden">
            {activeItem && !showNav && (
              <>
                <button
                  onClick={() => setShowNav(true)}
                  className="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground transition-colors"
                >
                  <ChevronLeft className="h-4 w-4" />
                  <span>Settings</span>
                </button>
                <span className="text-sm text-muted-foreground">/</span>
              </>
            )}
            <h2 className="text-2xl font-bold tracking-tight lg:hidden">
              {showNav || !activeItem ? 'Settings' : activeItem.title}
            </h2>
          </div>
          {/* Desktop: always show Settings heading */}
          <h2 className="text-2xl font-bold tracking-tight hidden lg:block">
            Settings
          </h2>
          <p className="text-muted-foreground hidden lg:block">
            Manage your platform configuration
          </p>
          {(showNav || !activeItem) && (
            <p className="text-muted-foreground lg:hidden">
              Manage your platform configuration
            </p>
          )}
        </div>

        {/* Desktop: sidebar + content side by side */}
        <div className="hidden lg:flex gap-8">
          <div className="w-52 shrink-0">
            <SettingsNav />
          </div>
          <div className="flex-1 min-w-0">
            <Outlet />
          </div>
        </div>

        {/* Mobile: drill-in — show nav OR content */}
        <div className="lg:hidden">
          {showNav || !activeItem ? (
            <SettingsNav onClick={() => setShowNav(false)} />
          ) : (
            <div className="min-w-0">
              <Outlet />
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
