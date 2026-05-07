// Globe icon + popover for the workspace session header. Surfaces the
// sandbox's preview ports without making the user expand the chat-side
// SessionPreviewCard. Lets them hop to a known port chip or punch in an
// arbitrary port via the same `preview_url_template` that the card uses.

import { useMemo, useState } from 'react'
import { Eye, EyeOff, ExternalLink, Globe, KeyRound } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { CopyButton } from '@/components/ui/copy-button'
import { Input } from '@/components/ui/input'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import type { WorkspaceSessionResponse as WorkspaceSession } from '@/api/client/types.gen'

interface SessionPreviewPopoverProps {
  session: WorkspaceSession
}

export function SessionPreviewPopover({ session }: SessionPreviewPopoverProps) {
  const [customPort, setCustomPort] = useState('')
  // Preview password is hidden by default so it isn't on screen the moment
  // the popover opens; one click on Show reveals it. Sessions whose
  // password predates reversible storage return null here — fall back to
  // the 4-char hint and prompt the user to regenerate.
  const [passwordVisible, setPasswordVisible] = useState(false)
  const previewPassword = session.preview_password ?? null

  const customPortValid = useMemo(() => {
    if (!/^\d+$/.test(customPort)) return false
    const n = Number(customPort)
    return n >= 1 && n <= 65535
  }, [customPort])

  const customUrl =
    customPortValid && session.preview_url_template
      ? session.preview_url_template.replace('{port}', customPort)
      : null

  const openCustom = () => {
    if (!customUrl) return
    window.open(customUrl, '_blank', 'noopener,noreferrer')
  }

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button size="icon" variant="ghost" title="Sandbox preview ports">
          <Globe className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 p-3 space-y-3">
        <div className="flex items-center gap-2 text-xs font-medium">
          <Globe className="h-3.5 w-3.5 text-muted-foreground" />
          Preview ports
        </div>

        {session.preview_urls.length > 0 ? (
          <div className="flex flex-wrap gap-1.5">
            {session.preview_urls.map((p) => (
              <a
                key={p.port}
                href={p.url}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs hover:bg-accent"
                title={p.url}
              >
                <ExternalLink className="h-3 w-3" />
                {p.port}
              </a>
            ))}
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">
            No detected ports yet — start a server in the sandbox or open one
            below.
          </p>
        )}

        <div className="space-y-1.5">
          <div className="text-[11px] font-medium text-muted-foreground">
            Open custom port
          </div>
          <div className="flex gap-1.5">
            <Input
              value={customPort}
              onChange={(e) => setCustomPort(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && customPortValid) openCustom()
              }}
              placeholder="3000"
              inputMode="numeric"
              className="h-8 text-xs"
            />
            <Button
              size="sm"
              variant="outline"
              disabled={!customPortValid}
              onClick={openCustom}
              className="h-8"
            >
              <ExternalLink className="h-3.5 w-3.5" />
              Open
            </Button>
          </div>
          {customUrl && (
            <div className="flex items-center gap-1 text-[11px] text-muted-foreground">
              <span className="truncate font-mono" title={customUrl}>
                {customUrl}
              </span>
              <CopyButton
                value={customUrl}
                minimal
                className="rounded p-1 hover:bg-accent"
              />
            </div>
          )}
        </div>

        {previewPassword ? (
          <div className="space-y-1 rounded-md border bg-muted/30 p-2">
            <div className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
              <KeyRound className="h-3 w-3" />
              Preview password
            </div>
            <div className="flex items-center gap-1.5">
              <code className="flex-1 truncate rounded bg-muted px-2 py-1 text-xs font-mono">
                {passwordVisible
                  ? previewPassword
                  : '•'.repeat(previewPassword.length)}
              </code>
              <Button
                size="icon"
                variant="ghost"
                className="h-7 w-7"
                onClick={() => setPasswordVisible((v) => !v)}
                title={passwordVisible ? 'Hide password' : 'Show password'}
                aria-label={passwordVisible ? 'Hide password' : 'Show password'}
              >
                {passwordVisible ? (
                  <EyeOff className="h-3.5 w-3.5" />
                ) : (
                  <Eye className="h-3.5 w-3.5" />
                )}
              </Button>
              <CopyButton
                value={previewPassword}
                minimal
                className="h-7 w-7 rounded-md hover:bg-accent"
              />
            </div>
          </div>
        ) : (
          session.preview_password_hint && (
            <div className="flex items-center gap-1.5 rounded-md bg-muted/50 px-2 py-1.5 text-[11px] text-muted-foreground">
              <KeyRound className="h-3 w-3 shrink-0" />
              <span>
                Preview password ends in{' '}
                <span className="font-mono">
                  {session.preview_password_hint}
                </span>
                . Regenerate from the chat panel to view it.
              </span>
            </div>
          )
        )}
      </PopoverContent>
    </Popover>
  )
}
