import { ContainerDetailResponse } from '@/api/client'
import { CopyButton } from '@/components/ui/copy-button'

interface ContainerConfigurationProps {
  container: ContainerDetailResponse
}

export function ContainerConfiguration({
  container,
}: ContainerConfigurationProps) {
  const envVars = normalizeEnvVars(container.environment_variables)
  const hasPorts = !!(container.container_port || container.host_port)

  return (
    <div className="flex flex-col gap-10">
      <Section
        title="Basic information"
        description="Identity and runtime metadata for this container."
      >
        <FieldGrid>
          <Field label="Container ID" mono copyable value={container.container_id} />
          <Field label="Image" mono copyable value={container.image_name} />
          <Field label="Status" value={container.status} />
          <Field
            label="Uptime"
            value={formatUptimeFromTimestamp(container.created_at)}
          />
          {container.service_name && (
            <Field label="Service" mono value={container.service_name} />
          )}
          {container.restart_count != null && container.restart_count > 0 && (
            <Field
              label="Restarts"
              value={String(container.restart_count)}
              tone="warn"
            />
          )}
        </FieldGrid>
      </Section>

      {hasPorts && (
        <Section
          title="Ports"
          description="Port mappings between the host and the container."
        >
          <FieldGrid>
            {container.container_port ? (
              <Field
                label="Container port"
                mono
                value={String(container.container_port)}
              />
            ) : null}
            {container.host_port ? (
              <Field
                label="Host port"
                mono
                value={String(container.host_port)}
              />
            ) : null}
          </FieldGrid>
        </Section>
      )}

      {envVars.length > 0 && (
        <Section
          title="Environment variables"
          description={`${envVars.length} variable${envVars.length === 1 ? '' : 's'} injected at runtime. Sensitive values are masked.`}
        >
          <div className="divide-y divide-neutral-950/5 overflow-hidden rounded-md border border-neutral-950/10 dark:divide-white/5 dark:border-white/10">
            {envVars.map(({ key, value }, i) => (
              <div
                key={`${key}-${i}`}
                className="grid grid-cols-1 gap-1 px-3 py-2.5 sm:grid-cols-[minmax(10rem,16rem)_1fr] sm:gap-4 sm:items-start"
              >
                <div className="font-mono text-[0.8125rem] font-medium text-neutral-900 break-all dark:text-white">
                  {key}
                </div>
                <div className="group flex items-start gap-2 min-w-0">
                  <div className="font-mono text-[0.8125rem] text-neutral-600 break-all dark:text-neutral-400 min-w-0 flex-1">
                    {value || (
                      <span className="italic text-neutral-400 dark:text-neutral-500">
                        empty
                      </span>
                    )}
                  </div>
                  {value && (
                    <CopyButton
                      text={value}
                      className="shrink-0 opacity-0 transition group-hover:opacity-100 focus:opacity-100"
                    />
                  )}
                </div>
              </div>
            ))}
          </div>
        </Section>
      )}
    </div>
  )
}

function Section({
  title,
  description,
  children,
}: {
  title: string
  description?: string
  children: React.ReactNode
}) {
  return (
    <section className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,18rem)_1fr]">
      <header>
        <h3 className="text-base font-semibold text-neutral-900 dark:text-white">
          {title}
        </h3>
        {description && (
          <p className="mt-1 text-sm text-neutral-600 dark:text-neutral-400">
            {description}
          </p>
        )}
      </header>
      <div className="min-w-0">{children}</div>
    </section>
  )
}

function FieldGrid({ children }: { children: React.ReactNode }) {
  return (
    <dl className="grid grid-cols-1 gap-x-6 gap-y-5 sm:grid-cols-2">
      {children}
    </dl>
  )
}

function Field({
  label,
  value,
  mono,
  copyable,
  tone,
}: {
  label: string
  value: string
  mono?: boolean
  copyable?: boolean
  tone?: 'warn'
}) {
  return (
    <div className="flex flex-col gap-1 min-w-0">
      <dt className="text-xs font-medium uppercase tracking-wide text-neutral-500 dark:text-neutral-400">
        {label}
      </dt>
      <dd
        className={`flex items-center gap-2 min-w-0 ${
          tone === 'warn'
            ? 'text-amber-700 dark:text-amber-400'
            : 'text-neutral-900 dark:text-white'
        }`}
      >
        <span
          className={`min-w-0 flex-1 truncate ${mono ? 'font-mono text-[0.8125rem]' : 'text-sm'}`}
          title={value}
        >
          {value}
        </span>
        {copyable && <CopyButton value={value} minimal className="shrink-0" />}
      </dd>
    </div>
  )
}

function normalizeEnvVars(
  vars: ContainerDetailResponse['environment_variables']
): Array<{ key: string; value: string }> {
  if (!vars) return []
  const out: Array<{ key: string; value: string }> = []
  if (Array.isArray(vars)) {
    for (const ev of vars as unknown[]) {
      if (typeof ev === 'string') {
        const [k, ...rest] = (ev as string).split('=')
        if (k) out.push({ key: k, value: rest.join('=') })
        continue
      }
      if (ev && typeof ev === 'object') {
        const obj = ev as Record<string, unknown>
        if ('name' in obj && 'value' in obj) {
          out.push({ key: String(obj.name), value: String(obj.value ?? '') })
        } else if ('key' in obj && 'value' in obj) {
          out.push({ key: String(obj.key), value: String(obj.value ?? '') })
        } else {
          const entries = Object.entries(obj)
          if (entries.length >= 2) {
            out.push({
              key: String(entries[0][1]),
              value: String(entries[1][1] ?? ''),
            })
          } else if (entries.length === 1) {
            out.push({
              key: String(entries[0][0]),
              value: String(entries[0][1] ?? ''),
            })
          }
        }
      }
    }
  } else if (typeof vars === 'object') {
    for (const [k, v] of Object.entries(vars as Record<string, unknown>)) {
      out.push({ key: k, value: String(v ?? '') })
    }
  }
  return out
}

function formatUptimeFromTimestamp(createdAt?: string): string {
  if (!createdAt) return 'N/A'
  const elapsedMs = Date.now() - new Date(createdAt).getTime()
  const elapsedSeconds = Math.floor(elapsedMs / 1000)
  return formatUptime(elapsedSeconds)
}

function formatUptime(seconds: number): string {
  if (seconds < 60) return `${Math.floor(seconds)}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86400)}d`
}
