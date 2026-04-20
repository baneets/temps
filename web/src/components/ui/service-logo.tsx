import { ServiceTypeRoute } from '@/api/client'
import { cn } from '@/lib/utils'
import { useTheme } from 'next-themes'
import { useMemo } from 'react'

const serviceLogos: Partial<Record<ServiceTypeRoute, { src: string; alt: string }>> = {
  mongodb: {
    src: '/storage/mongodb.svg',
    alt: 'MongoDB logo',
  },
  postgres: {
    src: '/storage/postgresql.svg',
    alt: 'PostgreSQL logo',
  },
  redis: {
    src: '/storage/redis.svg',
    alt: 'Redis logo',
  },
  kv: {
    src: '/storage/redis.svg',
    alt: 'Redis logo',
  },
  s3: {
    src: '/storage/rustfs.svg',
    alt: 'S3 / RustFS logo',
  },
  blob: {
    src: '/storage/rustfs.svg',
    alt: 'RustFS logo',
  },
  rustfs: {
    src: '/storage/rustfs.svg',
    alt: 'RustFS logo',
  },
  minio: {
    src: '/storage/minio.svg',
    alt: 'MinIO logo',
  },
}

interface ServiceLogoProps extends React.HTMLAttributes<HTMLImageElement> {
  service: ServiceTypeRoute
  size?: number
  invertOnDark?: boolean
}

export function ServiceLogo({
  service,
  size = 40,
  invertOnDark = true,
  className,
  ...props
}: ServiceLogoProps) {
  const { theme } = useTheme()
  const isDark = useMemo(() => theme === 'dark', [theme])

  const logo = serviceLogos[service]

  if (!logo) return null

  return (
    <img
      src={logo.src}
      alt={logo.alt}
      width={size}
      height={size}
      className={cn(
        'rounded-md transition-all duration-100',
        isDark && invertOnDark && 'invert brightness-0 opacity-80',
        isDark && !invertOnDark && 'opacity-90',
        className
      )}
      {...props}
    />
  )
}
