import type { Command } from 'commander'
import { requireAuth } from '../../config/store.js'
import { setupClient, client, getErrorMessage } from '../../lib/api-client.js'
import {
  listDomains2 as listEmailDomains,
  createDomain2 as createEmailDomain,
  getDomain as getEmailDomain,
  deleteDomain2 as deleteEmailDomain,
  getDomainByName as getEmailDomainByName,
  getDomainDnsRecords as getEmailDomainDnsRecords,
  setupDns as setupEmailDns,
  verifyDomain as verifyEmailDomain,
} from '../../api/sdk.gen.js'
import type { EmailDomainResponse, DnsRecordResponse } from '../../api/types.gen.js'
import { withSpinner } from '../../ui/spinner.js'
import { printTable, statusBadge, type TableColumn } from '../../ui/table.js'
import { promptText, promptConfirm } from '../../ui/prompts.js'
import {
  newline, header, icons, json, colors, success, info, warning,
  keyValue, formatDate,
} from '../../ui/output.js'

interface CreateOptions {
  domain?: string
  providerId?: string
  yes?: boolean
}

interface ShowOptions {
  id: string
  json?: boolean
}

interface RemoveOptions {
  id: string
  force?: boolean
  yes?: boolean
}

interface ByNameOptions {
  domain: string
  json?: boolean
}

interface DnsRecordsOptions {
  id: string
  json?: boolean
}

interface SetupDnsOptions {
  id: string
  dnsProviderId?: string
}

interface VerifyOptions {
  id: string
}

export function registerEmailDomainsCommands(program: Command): void {
  const emailDomains = program
    .command('email-domains')
    .alias('edom')
    .description('Manage email domains for transactional email')

  emailDomains
    .command('list')
    .alias('ls')
    .description('List all email domains')
    .option('--json', 'Output in JSON format')
    .action(listDomainsAction)

  emailDomains
    .command('create')
    .alias('add')
    .description('Create a new email domain')
    .option('-d, --domain <domain>', 'Domain name (e.g., mail.example.com)')
    .option('--provider-id <id>', 'Email provider ID')
    .option('-y, --yes', 'Skip confirmation prompts (for automation)')
    .action(createDomainAction)

  emailDomains
    .command('show')
    .description('Show email domain details')
    .requiredOption('--id <id>', 'Email domain ID')
    .option('--json', 'Output in JSON format')
    .action(showDomainAction)

  emailDomains
    .command('remove')
    .alias('rm')
    .description('Remove an email domain')
    .requiredOption('--id <id>', 'Email domain ID')
    .option('-f, --force', 'Skip confirmation')
    .option('-y, --yes', 'Skip confirmation prompts (alias for --force)')
    .action(removeDomainAction)

  emailDomains
    .command('by-name')
    .description('Look up an email domain by domain name')
    .requiredOption('-d, --domain <domain>', 'Domain name')
    .option('--json', 'Output in JSON format')
    .action(byNameAction)

  emailDomains
    .command('dns-records')
    .description('Get DNS records for an email domain')
    .requiredOption('--id <id>', 'Email domain ID')
    .option('--json', 'Output in JSON format')
    .action(dnsRecordsAction)

  emailDomains
    .command('setup-dns')
    .description('Setup DNS records using a configured DNS provider')
    .requiredOption('--id <id>', 'Email domain ID')
    .option('--dns-provider-id <id>', 'DNS provider ID to use')
    .action(setupDnsAction)

  emailDomains
    .command('verify')
    .description('Verify an email domain DNS configuration')
    .requiredOption('--id <id>', 'Email domain ID')
    .action(verifyDomainAction)
}

async function listDomainsAction(options: { json?: boolean }): Promise<void> {
  await requireAuth()
  await setupClient()

  const domains = await withSpinner('Fetching email domains...', async () => {
    const { data, error } = await listEmailDomains({ client })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data ?? []
  })

  if (options.json) {
    json(domains)
    return
  }

  newline()
  header(`${icons.globe} Email Domains (${domains.length})`)

  if (domains.length === 0) {
    info('No email domains configured')
    info('Run: temps email-domains create --domain mail.example.com --provider-id <id> -y')
    newline()
    return
  }

  const columns: TableColumn<EmailDomainResponse>[] = [
    { header: 'ID', key: 'id', width: 6 },
    { header: 'Domain', key: 'domain', color: (v) => colors.bold(v) },
    { header: 'Status', key: 'status', color: (v) => statusBadge(v === 'verified' ? 'active' : v) },
    { header: 'Provider ID', key: 'provider_id' },
    { header: 'Last Verified', accessor: (d) => d.last_verified_at ? formatDate(d.last_verified_at) : '-', color: (v) => colors.muted(v) },
    { header: 'Created', accessor: (d) => formatDate(d.created_at), color: (v) => colors.muted(v) },
  ]

  printTable(domains, columns, { style: 'minimal' })
  newline()
}

async function createDomainAction(options: CreateOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  let domain: string
  let providerId: number

  const isAutomation = options.yes && options.domain && options.providerId

  if (isAutomation) {
    domain = options.domain!
    providerId = parseInt(options.providerId!, 10)
    if (isNaN(providerId)) {
      warning('Invalid provider ID')
      return
    }
  } else {
    domain = options.domain || await promptText({
      message: 'Domain name (e.g., mail.example.com)',
      required: true,
    })

    const providerIdStr = options.providerId || await promptText({
      message: 'Email provider ID',
      required: true,
    })
    providerId = parseInt(providerIdStr, 10)
    if (isNaN(providerId)) {
      warning('Invalid provider ID')
      return
    }
  }

  newline()
  info(`Creating email domain ${colors.bold(domain)}`)

  const result = await withSpinner('Creating email domain...', async () => {
    const { data, error } = await createEmailDomain({
      client,
      body: {
        domain,
        provider_id: providerId,
      },
    })
    if (error) throw new Error(getErrorMessage(error))
    return data
  })

  newline()
  success(`Email domain ${domain} created`)

  if (result?.dns_records && result.dns_records.length > 0) {
    newline()
    info('DNS records to configure:')
    for (const record of result.dns_records) {
      keyValue(`${record.record_type}`, `${record.name} -> ${record.value}`)
    }
    newline()
    info(`Run "temps email-domains setup-dns --id ${result.domain.id}" to auto-configure DNS`)
    info(`Run "temps email-domains verify --id ${result.domain.id}" after DNS is configured`)
  }
}

async function showDomainAction(options: ShowOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const id = parseInt(options.id, 10)
  if (isNaN(id)) {
    warning('Invalid domain ID')
    return
  }

  const result = await withSpinner('Fetching email domain...', async () => {
    const { data, error } = await getEmailDomain({
      client,
      path: { id },
    })
    if (error || !data) {
      throw new Error(getErrorMessage(error) ?? `Email domain ${options.id} not found`)
    }
    return data
  })

  if (options.json) {
    json(result)
    return
  }

  const domain = result.domain
  newline()
  header(`${icons.globe} ${domain.domain}`)
  keyValue('ID', domain.id)
  keyValue('Status', statusBadge(domain.status === 'verified' ? 'active' : domain.status))
  keyValue('Provider ID', domain.provider_id)
  keyValue('Last Verified', domain.last_verified_at ? formatDate(domain.last_verified_at) : '-')
  if (domain.verification_error) {
    keyValue('Verification Error', colors.error(domain.verification_error))
  }
  keyValue('Created', formatDate(domain.created_at))
  keyValue('Updated', formatDate(domain.updated_at))

  if (result.dns_records && result.dns_records.length > 0) {
    newline()
    header('DNS Records')
    const dnsColumns: TableColumn<DnsRecordResponse>[] = [
      { header: 'Type', key: 'record_type' },
      { header: 'Name', key: 'name', color: (v) => colors.bold(v) },
      { header: 'Value', key: 'value' },
      { header: 'Status', key: 'status', color: (v) => statusBadge(v === 'verified' ? 'active' : v) },
      { header: 'Priority', accessor: (r) => r.priority !== null && r.priority !== undefined ? String(r.priority) : '-' },
    ]
    printTable(result.dns_records, dnsColumns, { style: 'minimal' })
  }
  newline()
}

async function removeDomainAction(options: RemoveOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const id = parseInt(options.id, 10)
  if (isNaN(id)) {
    warning('Invalid domain ID')
    return
  }

  // Get domain details first
  const { data: domainData, error: getError } = await getEmailDomain({
    client,
    path: { id },
  })

  if (getError || !domainData) {
    warning(`Email domain ${options.id} not found`)
    return
  }

  const skipConfirmation = options.force || options.yes

  if (!skipConfirmation) {
    const confirmed = await promptConfirm({
      message: `Remove email domain "${domainData.domain.domain}"?`,
      default: false,
    })
    if (!confirmed) {
      info('Cancelled')
      return
    }
  }

  await withSpinner('Removing email domain...', async () => {
    const { error } = await deleteEmailDomain({
      client,
      path: { id },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
  })

  success('Email domain removed')
}

async function byNameAction(options: ByNameOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const result = await withSpinner(`Looking up domain ${options.domain}...`, async () => {
    const { data, error } = await getEmailDomainByName({
      client,
      path: { domain: options.domain },
    })
    if (error || !data) {
      throw new Error(getErrorMessage(error) ?? `Email domain ${options.domain} not found`)
    }
    return data
  })

  if (options.json) {
    json(result)
    return
  }

  const domain = result.domain
  newline()
  header(`${icons.globe} ${domain.domain}`)
  keyValue('ID', domain.id)
  keyValue('Status', statusBadge(domain.status === 'verified' ? 'active' : domain.status))
  keyValue('Provider ID', domain.provider_id)
  keyValue('Last Verified', domain.last_verified_at ? formatDate(domain.last_verified_at) : '-')
  if (domain.verification_error) {
    keyValue('Verification Error', colors.error(domain.verification_error))
  }
  keyValue('Created', formatDate(domain.created_at))
  keyValue('Updated', formatDate(domain.updated_at))

  if (result.dns_records && result.dns_records.length > 0) {
    newline()
    header('DNS Records')
    const dnsColumns: TableColumn<DnsRecordResponse>[] = [
      { header: 'Type', key: 'record_type' },
      { header: 'Name', key: 'name', color: (v) => colors.bold(v) },
      { header: 'Value', key: 'value' },
      { header: 'Status', key: 'status', color: (v) => statusBadge(v === 'verified' ? 'active' : v) },
      { header: 'Priority', accessor: (r) => r.priority !== null && r.priority !== undefined ? String(r.priority) : '-' },
    ]
    printTable(result.dns_records, dnsColumns, { style: 'minimal' })
  }
  newline()
}

async function dnsRecordsAction(options: DnsRecordsOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const id = parseInt(options.id, 10)
  if (isNaN(id)) {
    warning('Invalid domain ID')
    return
  }

  const records = await withSpinner('Fetching DNS records...', async () => {
    const { data, error } = await getEmailDomainDnsRecords({
      client,
      path: { id },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data ?? []
  })

  if (options.json) {
    json(records)
    return
  }

  newline()
  header(`${icons.info} DNS Records (${records.length})`)

  if (records.length === 0) {
    info('No DNS records found for this domain')
    newline()
    return
  }

  const columns: TableColumn<DnsRecordResponse>[] = [
    { header: 'Type', key: 'record_type' },
    { header: 'Name', key: 'name', color: (v) => colors.bold(v) },
    { header: 'Value', key: 'value' },
    { header: 'Status', key: 'status', color: (v) => statusBadge(v === 'verified' ? 'active' : v) },
    { header: 'Priority', accessor: (r) => r.priority !== null && r.priority !== undefined ? String(r.priority) : '-' },
  ]

  printTable(records, columns, { style: 'minimal' })
  newline()
}

async function setupDnsAction(options: SetupDnsOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const id = parseInt(options.id, 10)
  if (isNaN(id)) {
    warning('Invalid domain ID')
    return
  }

  let dnsProviderId: number
  if (options.dnsProviderId) {
    dnsProviderId = parseInt(options.dnsProviderId, 10)
    if (isNaN(dnsProviderId)) {
      warning('Invalid DNS provider ID')
      return
    }
  } else {
    const providerIdStr = await promptText({
      message: 'DNS provider ID to use for record creation',
      required: true,
    })
    dnsProviderId = parseInt(providerIdStr, 10)
    if (isNaN(dnsProviderId)) {
      warning('Invalid DNS provider ID')
      return
    }
  }

  const result = await withSpinner('Setting up DNS records...', async () => {
    const { data, error } = await setupEmailDns({
      client,
      path: { id },
      body: {
        dns_provider_id: dnsProviderId,
      },
    })
    if (error) throw new Error(getErrorMessage(error))
    return data
  })

  newline()
  if (result?.success) {
    success(result.message || 'DNS records setup completed')
  } else {
    warning(result?.message || 'DNS setup completed with issues')
  }

  if (result?.results && result.results.length > 0) {
    newline()
    for (const r of result.results) {
      const icon = r.success ? colors.success('*') : colors.error('*')
      const mode = r.automatic ? 'auto' : 'manual'
      console.log(`  ${icon} ${r.record_type} ${r.name} (${mode}): ${r.message}`)
    }
  }

  if (result?.records_created !== undefined) {
    newline()
    info(`Records created: ${result.records_created}`)
  }
  newline()
}

async function verifyDomainAction(options: VerifyOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const id = parseInt(options.id, 10)
  if (isNaN(id)) {
    warning('Invalid domain ID')
    return
  }

  const result = await withSpinner('Verifying email domain...', async () => {
    const { data, error } = await verifyEmailDomain({
      client,
      path: { id },
    })
    if (error) throw new Error(getErrorMessage(error))
    return data
  })

  newline()
  if (!result) {
    warning('No response received')
    return
  }

  const domain = result.domain
  if (domain.status === 'verified') {
    success(`Email domain ${domain.domain} verified successfully`)
  } else if (domain.status === 'pending') {
    warning(`Domain ${domain.domain} is still pending verification`)
    info('Please ensure all DNS records are properly configured')
  } else {
    warning(`Domain status: ${domain.status}`)
    if (domain.verification_error) {
      warning(`Error: ${domain.verification_error}`)
    }
  }

  if (result.dns_records && result.dns_records.length > 0) {
    newline()
    header('DNS Record Status')
    for (const record of result.dns_records) {
      const statusIcon = record.status === 'verified' ? colors.success('*') : record.status === 'failed' ? colors.error('*') : colors.warning('*')
      console.log(`  ${statusIcon} ${record.record_type} ${record.name}: ${record.status}`)
    }
  }
  newline()
}
