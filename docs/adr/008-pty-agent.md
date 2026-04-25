# ADR-008: In-Sandbox PTY Agent Replaces `docker exec` + `dtach`

**Status:** Proposed
**Date:** 2026-04-14
**Author:** David Viejo

## Context

Workspace sessions expose interactive terminals (claude/opencode/codex/bash) to the browser via a websocket. The current transport is:

```
browser xterm.js ─WS─► Axum handler ─docker exec─► sh -c "… dtach -A /run/temps-pty/…sock …" ─► shares PTY with CLI
```

Every browser reload creates a fresh `docker exec` that spawns a fresh `dtach -A` client. The master created on the first attach owns the PTY + child program and survives client detach — that part works. The problem is the **client process**: when the websocket closes, bollard tears down the hijacked HTTP/2 stream on our side, but the in-container `dtach` client is a session leader that ignores SIGHUP by design. We have observed 12–20 orphaned clients accumulating per sandbox after normal use. Each orphan still subscribes to SIGWINCH and shares the PTY fd, so every new attach contends with N stale clients. Symptom: terminal attach gets progressively slower (first 2–3 reloads fast, then 10–15 s each).

We've now patched this with a `trap 'kill $DPID' EXIT` wrapper around a backgrounded `dtach` — stops the bleeding but highlights the deeper issue: **we have no control-plane handle on in-container processes we spawn**. Every terminal feature we've layered on — stdin rate limiting, resize forwarding, heartbeats, OSC 52, boot-id socket wipe, orphan cleanup, tab enumeration via `tmux list-sessions` — is working around the fact that `docker exec` was designed for run-once commands, not long-lived bidirectional streams.

Features that are blocked or fragile today:

- **Tab persistence across container restart** — we currently infer the tab list by scanning tmux sessions, which breaks when containers are stopped/idle-reaped.
- **Accurate per-tab status** — we don't know if a background CLI has exited, only that its socket still exists.
- **Clean shutdown** — no way to signal "release this tab, kill its program" without an `exec sh -c 'pkill …'` hack.
- **Resource accounting** — no per-PTY byte counters or attach timestamps.
- **Multi-attach awareness** — second attach to the same tab silently shares the PTY; we can't implement "steal" or "read-only observer" modes.

Every one of those needs the same missing primitive: a long-lived, host-addressable control channel into the sandbox.

## Decision

Ship a small Rust binary — **`temps-pty-agent`** — baked into the sandbox image. It runs as PID-2 under `/sbin/docker-init` for the sandbox's entire lifetime and owns every PTY inside the container. The host reaches it through a single bidirectional stream per attach (via `docker exec socat UNIX-CONNECT:/run/temps-pty/agent.sock -`) and speaks a framed binary protocol.

The agent replaces `dtach`, the `tmux list-sessions` discovery hack, and the wrapper shell script that launches each attach.

### Responsibilities

- Own a map `tab_id → Tab { kind, label, pid, pty_master, child_stdin, created_at, last_attach_at, subscribers: Vec<AttachId> }`.
- On `OPEN`: spawn the requested command under a fresh PTY, store state, stream output to every subscriber.
- On subsequent `OPEN` for an existing tab: attach a new subscriber, replay the last N bytes of the scrollback buffer, then tee live output.
- On `DETACH` or socket close: drop the subscriber. If it was the last one, keep the PTY + child alive (persistent tab) — this preserves the dtach-master-survives-client behavior we rely on today.
- On `KILL`: SIGTERM the child, wait up to 2 s, SIGKILL, drop the tab. Notify all subscribers with an `EXIT` frame.
- On `LIST`: return all tabs with metadata (no scanning of anything — it's just the in-memory map).
- Periodically reap children that have exited on their own, emit `EXIT` to remaining subscribers, drop the tab.

### Wire Protocol

Length-prefixed binary frames on the Unix socket. Each frame: `u32 length (BE) | u8 type | payload`.

Client → Agent:

| Type | Payload | Meaning |
|------|---------|---------|
| `0x01 OPEN` | `json { tab_id, kind, cmd, cols, rows, replay_bytes }` | Start or attach to a tab. `replay_bytes` = how much scrollback to send on attach (0 = none, default 4 KiB). |
| `0x02 INPUT` | raw bytes | Forward to child stdin. |
| `0x03 RESIZE` | `u16 cols | u16 rows` | SIGWINCH the child. |
| `0x04 DETACH` | — | Disconnect subscriber; tab stays alive. |
| `0x05 KILL` | — | Terminate the tab's child + drop it. |
| `0x06 LIST` | — | Request tab enumeration. |
| `0x07 PING` | — | Keepalive. Agent replies PONG. |

Agent → Client:

| Type | Payload | Meaning |
|------|---------|---------|
| `0x81 OUTPUT` | raw bytes | PTY output. |
| `0x82 OPENED` | `json { tab_id, pid }` | OPEN succeeded. |
| `0x83 EXIT` | `json { tab_id, code, signal }` | Child exited. |
| `0x84 TABS` | `json [{ tab_id, kind, label, pid, created_at, subscriber_count }]` | Response to LIST. |
| `0x85 PONG` | — | Keepalive reply. |
| `0x8f ERROR` | `json { code, message }` | Request failed (unknown tab, spawn error, policy violation). |

Rationale for length-prefixing instead of newline-delimited: INPUT and OUTPUT frames carry arbitrary bytes including `\n` and `\0`. JSON for control messages only — they're rare and the readability + schema evolution is worth the handful of bytes. Output is never JSON-wrapped, it's passed through verbatim so the PTY's 8-bit binary (including ANSI/OSC 52) reaches the browser untouched.

### Attach Semantics

**Multi-subscriber model.** N clients may OPEN the same tab_id. All receive OUTPUT; INPUT from any is forwarded to the child (this matches `tmux attach -d` behavior for the default case). A later policy knob can switch to "steal" (kick prior subscribers) but multi-subscribe is the common case when a user has two browser tabs open on the same workspace.

**Replay buffer.** Agent keeps a ring buffer per tab (default 64 KiB). On attach, the last `replay_bytes` of it is sent as OUTPUT before live output starts. This replaces the "redraw on SIGWINCH" trick we use today — attach is now deterministic instead of relying on the TUI to repaint.

**No auto-close.** Even with zero subscribers, tabs stay alive. Tabs go away on explicit KILL, or when the child exits, or when the container stops.

### Host-side Bridge

The Axum handler replaces `docker.create_exec(…, "/bin/sh -c dtach …")` with:

```
docker.create_exec(…, ["socat", "-", "UNIX-CONNECT:/run/temps-pty/agent.sock"])
```

…and then speaks the agent protocol through the hijacked stream. On websocket close, the handler sends a `DETACH` frame then drops the stream. `socat` exits cleanly when its stdin closes, and the server-side `docker exec` terminates with it — nothing to orphan, because nothing long-lived runs inside the exec anymore. The only long-lived in-container process is the agent itself.

### Socket Location + Permissions

- Path: `/run/temps-pty/agent.sock`
- Owner: `temps:temps` (the sandbox's unprivileged user)
- Mode: `0600`
- Agent binds this on startup, aborts if the bind fails.

Only callers with `docker exec --user temps` can reach it — same trust boundary as today.

### Lifecycle: Starting the Agent

Sandbox Dockerfile:

```dockerfile
COPY --chown=temps:temps temps-pty-agent /usr/local/bin/temps-pty-agent
# docker-init is already PID 1 via `--init`. It reaps zombies and forwards
# signals. We need a supervisor that restarts the agent if it ever crashes.
# Rather than dragging in systemd or s6, we ship a tiny shell loop as an
# entrypoint auxiliary that's invoked by docker-init.
COPY sandbox-entrypoint.sh /usr/local/bin/sandbox-entrypoint.sh
ENTRYPOINT ["/sbin/docker-init", "--", "/usr/local/bin/sandbox-entrypoint.sh"]
```

`sandbox-entrypoint.sh`:

```sh
#!/bin/sh
set -e
mkdir -p /run/temps-pty
chown temps:temps /run/temps-pty
# Supervisor loop: respawn the agent up to 5 times in 60s, then give up
# (container health check will fail and the control plane restarts us).
(
  while :; do
    su temps -c '/usr/local/bin/temps-pty-agent' || true
    sleep 1
  done
) &
# Keep PID 1 alive forever — the sandbox's "main" process is the container
# itself, not any specific workload.
exec sleep infinity
```

Agent crash = clients see `EPIPE` on their socat stream; they retry OPEN after the agent respawns, and because tabs live only in the agent's memory, the tabs from before the crash are **gone**. This is acceptable because (a) agent crashes should be extremely rare — it's a tiny program with a narrow responsibility, and (b) the CLIs themselves (claude/opencode) keep their own on-disk session state and resume cleanly. If we later decide tabs must survive agent restarts we'd persist tab metadata to `/home/temps/.temps/tabs.json` — out of scope for V1.

### Migration

1. **Land the trap-kill stopgap** — already done. Stops orphan accumulation in the existing dtach path.
2. **Build + ship the agent** behind a probe: handler checks `docker exec test -S /run/temps-pty/agent.sock` at attach time. Present → new path. Absent → current dtach path (older sandbox images).
3. **Bump the sandbox image** to include the agent. Deploy. Existing containers continue on dtach; new containers get the agent.
4. **Force-reroll** idle sandboxes after N days so every container is on the agent.
5. **Delete the dtach path** from the Rust handler once telemetry shows zero dtach attaches in 7 days.

No breaking change for users — the terminal feels identical. Only internals change.

## Consequences

**Positive.**
- Every terminal-related feature we've wanted (tab persistence, per-tab status, clean shutdown, resource accounting, multi-subscribe policies) becomes a small change to the agent instead of a new shell hack.
- No more orphan process class — everything runs inside the agent's supervision or is explicitly killed.
- `docker exec` is used only as a byte pipe; its lifetime matches the websocket's lifetime exactly.
- Tab enumeration is O(1) on an in-memory map instead of parsing `tmux list-sessions` output.
- Resize, SIGWINCH, and scrollback replay become features of the agent, not quirks of dtach's `-r winch` flag.

**Negative.**
- One more binary to build, cross-compile (linux/amd64 + linux/arm64), and ship in the sandbox image. Image size + build time.
- A bug in the agent takes down *all* terminals in a sandbox at once. Mitigated by the supervisor loop and by the agent being simple (~500 LOC target).
- Upgrading the agent requires a new sandbox image — older containers miss out until they roll. Acceptable because workspaces are ephemeral by design.
- The `replay_bytes` ring buffer costs memory: 64 KiB × N tabs per sandbox. Negligible.

**Neutral.**
- `dtach` is no longer installed in the sandbox image after full migration, freeing a few hundred KiB.
- The `tmux list-sessions` tab-discovery hack goes away — `tmux` itself may still be used by users inside their shells, but not by us.
- We gain a natural home for in-sandbox supervisory features we've punted on (idle kill, resource limits per tab, audit logging of shell commands).

## Alternatives Considered

**(B) Proxy dtach's wire protocol from the host** — speak dtach's undocumented socket protocol directly from Rust, so no client process runs in the container. Rejected: protocol is not documented, implementation fragile, and we still wouldn't have tab enumeration or clean kill without reading dtach's internals.

**(C) Use `ttyd` or `gotty` inside the sandbox** — off-the-shelf HTTP→PTY bridge. Rejected: both expose HTTP, not a control protocol, so we'd still need to graft tab semantics on top. Also: extra attack surface, we'd have to punch auth through, and we already own the edge.

**(D) Just fix the leak, keep dtach** — the trap-kill patch we've already landed. Rejected as the long-term answer: it stops orphan accumulation but leaves every other open problem (tab persistence, kill semantics, enumeration) unsolved. Kept as a bridge while the agent ships.

**(E) tmux with a stable server** — use a single tmux server that outlives any attach, reached via `tmux -S /run/temps-pty/tmux.sock`. Rejected: tmux's control-mode protocol is richer than we need, parsing its output is its own adventure, and every feature we want is still indirect ("send `list-sessions`, parse text").

## Open Questions

1. **Cross-compilation target for the agent.** Sandbox image is debian-slim; build the agent with `musl` for a fully static binary, or dynamic-link against glibc like everything else? Static is simpler to ship.
2. **Do we need `replay_bytes` to be persistent across agent restarts?** For V1: no. Revisit if agent uptime telemetry shows restarts are user-visible.
3. **Auth.** Today the security boundary is "you need to be allowed to docker-exec into the sandbox." Agent inherits that. Do we want per-tab ACLs in the future? Leave room in the protocol (OPEN could carry a token) but V1 does not enforce.
4. **Does `socat` need to ship in the sandbox image?** Probably already there, but verify. Alternative is to use `docker exec` with a minimal in-Rust socket-copy helper built into the agent binary itself (`temps-pty-agent --client`), which also removes the `socat` dependency.
