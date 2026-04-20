// Non-HTTP URL builders for workspace features. These wrap bare WebSocket /
// EventSource endpoints that the generated SDK can't represent — the SDK only
// covers JSON-over-HTTP. Everything else (list / create / update sessions,
// sandbox actions, terminal tab management) goes through the generated hooks
// in `@/api/client/@tanstack/react-query.gen`.

import { workspaceTerminalPasteImage } from '@/api/client/sdk.gen'

/**
 * Build the SSE stream URL for a workspace session.
 * Use with `new EventSource(url)`.
 */
export function sessionStreamUrl(
  projectId: number,
  sessionId: number,
  afterId = 0,
): string {
  return `/api/projects/${projectId}/workspace/sessions/${sessionId}/stream?after_id=${afterId}`
}

/**
 * Build the WebSocket URL for a workspace session terminal tab.
 *
 * Each (kind, tab) pair attaches to its own tmux session inside the sandbox,
 * so a workspace can have multiple independent terminals — one running
 * claude, others running raw shells. Reusing the same {kind,tab} re-attaches
 * to the same tmux session so refreshes don't lose state.
 *
 * Protocol:
 * - Binary frames → raw PTY bytes in both directions (xterm.js compatible)
 * - Text `{"type":"resize","cols":N,"rows":N}` → resize the remote PTY
 * - Text `{"type":"exit","code":N}` from server → session ended
 */
export function sessionTerminalUrl(
  projectId: number,
  sessionId: number,
  options: { kind?: 'claude' | 'shell'; tab?: string } = {},
): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  const params = new URLSearchParams()
  if (options.kind) params.set('kind', options.kind)
  if (options.tab) params.set('tab', options.tab)
  const qs = params.toString()
  return `${proto}//${window.location.host}/api/projects/${projectId}/workspace/sessions/${sessionId}/terminal${qs ? '?' + qs : ''}`
}

/**
 * Upload a pasted image into the session's sandbox container. Returns the
 * path inside the sandbox where the file was written. The frontend then types
 * that path into the PTY so Claude CLI picks it up as an image attachment.
 *
 * Wrapped here (rather than called from the generated SDK directly at each
 * site) because the terminal component hands us a `Uint8Array`, and the
 * server expects base64 + mime. Centralizing the encoding keeps callers
 * trivial.
 */
export async function pasteTerminalImage(
  projectId: number,
  sessionId: number,
  bytes: Uint8Array,
  mime: string,
): Promise<{ path: string }> {
  // Convert to base64 in chunks to avoid blowing the call stack on large images.
  let binary = ''
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
  }
  const data = btoa(binary)
  const { data: resp } = await workspaceTerminalPasteImage({
    path: { project_id: projectId, session_id: sessionId },
    body: { data, mime },
    throwOnError: true,
  })
  return { path: resp.path }
}

/** Shape of the PATCH body supported by `workspaceUpdateSession`. */
export interface UpdateSessionBody {
  idle_timeout_minutes?: number | null
  title?: string | null
  cpu_limit?: number | null
  memory_limit_mb?: number | null
  pids_limit?: number | null
}
