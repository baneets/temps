import { AgentSecrets } from '@/components/agents/ProjectSecrets'

// Thin wrapper — AgentSecrets already renders a row-per-secret table view,
// which is exactly the shape we want for this sub-page. No reason to dupe
// the component just to host it under its own route.
export function AgentSandboxSecretsPage() {
  return <AgentSecrets />
}
