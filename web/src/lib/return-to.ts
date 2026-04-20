const KEY = 'temps:returnTo'

const AUTH_PATHS = new Set(['/mfa-verify'])

function isAuthPath(path: string): boolean {
  const pathname = path.split('?')[0]?.split('#')[0] ?? path
  return AUTH_PATHS.has(pathname)
}

export function captureReturnTo(): void {
  if (typeof window === 'undefined') return
  const current = `${window.location.pathname}${window.location.search}${window.location.hash}`
  if (!current || current === '/' || isAuthPath(current)) return
  try {
    window.sessionStorage.setItem(KEY, current)
  } catch {
    /* storage disabled */
  }
}

export function consumeReturnTo(fallback = '/dashboard'): string {
  if (typeof window === 'undefined') return fallback
  try {
    const value = window.sessionStorage.getItem(KEY)
    window.sessionStorage.removeItem(KEY)
    if (value && !isAuthPath(value)) return value
  } catch {
    /* storage disabled */
  }
  return fallback
}
