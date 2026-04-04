import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Bot, Cpu, HardDrive, Loader2, Save } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useSettings, useUpdateSettings } from '@/hooks/useSettings'
import { usePageTitle } from '@/hooks/usePageTitle'

export function AgentSandboxSettings() {
  usePageTitle('Agent Sandbox')

  const { data: settings, isLoading } = useSettings()
  const updateSettings = useUpdateSettings()

  const [enabled, setEnabled] = useState(false)
  const [cpuLimit, setCpuLimit] = useState(2)
  const [memoryLimitMb, setMemoryLimitMb] = useState(2048)
  const [isDirty, setIsDirty] = useState(false)

  useEffect(() => {
    if (settings?.agent_sandbox) {
      setEnabled(settings.agent_sandbox.enabled)
      setCpuLimit(settings.agent_sandbox.cpu_limit)
      setMemoryLimitMb(settings.agent_sandbox.memory_limit_mb)
    }
  }, [settings])

  const handleSave = async () => {
    try {
      await updateSettings.mutateAsync({
        agent_sandbox: {
          enabled,
          cpu_limit: cpuLimit,
          memory_limit_mb: memoryLimitMb,
        },
      })
      setIsDirty(false)
      toast.success('Agent sandbox settings saved')
    } catch {
      toast.error('Failed to save settings')
    }
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Bot className="h-5 w-5" />
            Agent Sandbox
          </CardTitle>
          <CardDescription>
            When enabled, agents run inside isolated Docker containers instead of
            directly on the host. Individual agents can override this setting.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="flex items-center justify-between">
            <div className="space-y-0.5">
              <Label htmlFor="sandbox-enabled">
                Enable sandbox by default
              </Label>
              <p className="text-sm text-muted-foreground">
                All agents will run in Docker containers unless individually
                disabled
              </p>
            </div>
            <Switch
              id="sandbox-enabled"
              checked={enabled}
              onCheckedChange={(checked) => {
                setEnabled(checked)
                setIsDirty(true)
              }}
            />
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-6">
            <div className="space-y-2">
              <Label
                htmlFor="cpu-limit"
                className="flex items-center gap-1.5"
              >
                <Cpu className="h-3.5 w-3.5 text-muted-foreground" />
                CPU limit (cores)
              </Label>
              <Input
                id="cpu-limit"
                type="number"
                min={0.5}
                max={16}
                step={0.5}
                value={cpuLimit}
                onChange={(e) => {
                  setCpuLimit(parseFloat(e.target.value) || 2)
                  setIsDirty(true)
                }}
              />
              <p className="text-xs text-muted-foreground">
                Maximum CPU cores per sandbox container (0.5–16)
              </p>
            </div>

            <div className="space-y-2">
              <Label
                htmlFor="memory-limit"
                className="flex items-center gap-1.5"
              >
                <HardDrive className="h-3.5 w-3.5 text-muted-foreground" />
                Memory limit (MB)
              </Label>
              <Input
                id="memory-limit"
                type="number"
                min={256}
                max={16384}
                step={256}
                value={memoryLimitMb}
                onChange={(e) => {
                  setMemoryLimitMb(parseInt(e.target.value) || 2048)
                  setIsDirty(true)
                }}
              />
              <p className="text-xs text-muted-foreground">
                Maximum memory per sandbox container (256–16384 MB)
              </p>
            </div>
          </div>
        </CardContent>
      </Card>

      <div className="flex justify-end">
        <Button
          onClick={handleSave}
          disabled={!isDirty || updateSettings.isPending}
        >
          {updateSettings.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin mr-2" />
          ) : (
            <Save className="h-4 w-4 mr-2" />
          )}
          Save Changes
        </Button>
      </div>
    </div>
  )
}
