import { PreviewGatewayCard } from '@/components/settings/PreviewGatewayCard'

// Thin wrapper — PreviewGatewayCard already owns its status fetching, image
// upgrade flow, and logs viewer. No need to fragment it further.
export function AgentSandboxPreviewPage() {
  return <PreviewGatewayCard />
}
