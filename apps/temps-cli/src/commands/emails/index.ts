import type { Command } from 'commander'
import { requireAuth } from '../../config/store.js'
import { setupClient, client, getErrorMessage } from '../../lib/api-client.js'
import {
  listEmails,
  sendEmail,
  getEmail,
  getEmailStats,
  validateEmail,
} from '../../api/sdk.gen.js'
import { withSpinner } from '../../ui/spinner.js'
import { printTable, statusBadge, type TableColumn } from '../../ui/table.js'
import { promptText, promptConfirm } from '../../ui/prompts.js'
import { newline, header, icons, json, colors, success, info, warning, keyValue } from '../../ui/output.js'

interface ListOptions {
  json?: boolean
  page?: string
  pageSize?: string
  status?: string
  domainId?: string
  projectId?: string
  fromAddress?: string
}

interface SendOptions {
  to?: string
  subject?: string
  body?: string
  from?: string
  yes?: boolean
}

interface ShowOptions {
  id: string
  json?: boolean
}

interface StatsOptions {
  json?: boolean
}

interface ValidateOptions {
  email?: string
  json?: boolean
}

export function registerEmailsCommands(program: Command): void {
  const emails = program
    .command('emails')
    .alias('email')
    .description('Manage and send emails')

  emails
    .command('list')
    .alias('ls')
    .description('List sent emails')
    .option('--json', 'Output in JSON format')
    .option('--page <n>', 'Page number')
    .option('--page-size <n>', 'Items per page')
    .option('--status <status>', 'Filter by status (sent, delivered, failed)')
    .option('--domain-id <id>', 'Filter by domain ID')
    .option('--project-id <id>', 'Filter by project ID')
    .option('--from-address <email>', 'Filter by sender address')
    .action(listEmailsAction)

  emails
    .command('send')
    .description('Send an email')
    .option('--to <email>', 'Recipient email address')
    .option('--subject <subject>', 'Email subject')
    .option('--body <body>', 'Email body')
    .option('--from <email>', 'Sender email address')
    .option('-y, --yes', 'Skip confirmation prompts (for automation)')
    .action(sendEmailAction)

  emails
    .command('show')
    .description('Show email details')
    .requiredOption('--id <id>', 'Email ID')
    .option('--json', 'Output in JSON format')
    .action(showEmail)

  emails
    .command('stats')
    .description('Get email statistics')
    .option('--json', 'Output in JSON format')
    .action(emailStats)

  emails
    .command('validate')
    .description('Validate an email address')
    .option('--email <email>', 'Email address to validate')
    .option('--json', 'Output in JSON format')
    .action(validateEmailAction)
}

async function listEmailsAction(options: ListOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const page = options.page ? parseInt(options.page, 10) : undefined
  const pageSize = options.pageSize ? parseInt(options.pageSize, 10) : undefined
  const domainId = options.domainId ? parseInt(options.domainId, 10) : undefined
  const projectId = options.projectId ? parseInt(options.projectId, 10) : undefined

  const response = await withSpinner('Fetching emails...', async () => {
    const { data, error } = await listEmails({
      client,
      query: {
        ...(page && { page }),
        ...(pageSize && { page_size: pageSize }),
        ...(options.status && { status: options.status }),
        ...(domainId && { domain_id: domainId }),
        ...(projectId && { project_id: projectId }),
        ...(options.fromAddress && { from_address: options.fromAddress }),
      },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  const emailsData = response?.data ?? []

  if (options.json) {
    json(response)
    return
  }

  newline()
  header(`${icons.info} Sent Emails (${response?.total ?? emailsData.length})`)

  if (emailsData.length === 0) {
    info('No emails sent yet')
    info('Run: temps emails send --to user@example.com --subject "Hello" --body "World" -y')
    newline()
    return
  }

  const columns: TableColumn<Record<string, unknown>>[] = [
    { header: 'ID', key: 'id', width: 6 },
    { header: 'To', key: 'to', color: (v) => colors.bold(v) },
    { header: 'Subject', key: 'subject', color: (v) => v.length > 40 ? v.slice(0, 40) + '...' : v },
    { header: 'Status', key: 'status', color: (v) => statusBadge(v === 'delivered' || v === 'sent' ? 'active' : v === 'failed' ? 'error' : 'pending') },
    { header: 'Sent', accessor: (e) => e.created_at ? new Date(e.created_at as string).toLocaleDateString() : '-' },
  ]

  printTable(emailsData as Record<string, unknown>[], columns, { style: 'minimal' })
  newline()
}

async function sendEmailAction(options: SendOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  let to: string
  let subject: string
  let body: string
  let from: string | undefined

  const isAutomation = options.yes && options.to && options.subject && options.body

  if (isAutomation) {
    to = options.to!
    subject = options.subject!
    body = options.body!
    from = options.from
  } else {
    to = options.to || await promptText({
      message: 'Recipient email address',
      required: true,
    })

    subject = options.subject || await promptText({
      message: 'Email subject',
      required: true,
    })

    body = options.body || await promptText({
      message: 'Email body',
      required: true,
    })

    from = options.from || await promptText({
      message: 'Sender email address (optional)',
      default: '',
    }) || undefined

    if (!options.yes) {
      newline()
      info(`To: ${to}`)
      info(`Subject: ${subject}`)
      if (from) info(`From: ${from}`)
      newline()

      const confirmed = await promptConfirm({
        message: 'Send this email?',
        default: true,
      })
      if (!confirmed) {
        info('Cancelled')
        return
      }
    }
  }

  await withSpinner('Sending email...', async () => {
    const { error } = await sendEmail({
      client,
      body: {
        to: [to],
        subject,
        text: body,
        from: from ?? to,
      },
    })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
  })

  success('Email sent successfully')
  info(`Recipient: ${to}`)
}

async function showEmail(options: ShowOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const id = options.id

  const email = await withSpinner('Fetching email...', async () => {
    const { data, error } = await getEmail({
      client,
      path: { id },
    })
    if (error || !data) {
      throw new Error(getErrorMessage(error) ?? `Email ${options.id} not found`)
    }
    return data
  })

  if (options.json) {
    json(email)
    return
  }

  const emailData = email as Record<string, unknown>

  newline()
  header(`${icons.info} Email #${emailData.id}`)
  keyValue('To', emailData.to as string)
  if (emailData.from) {
    keyValue('From', emailData.from as string)
  }
  keyValue('Subject', emailData.subject as string)
  keyValue('Status', statusBadge(
    (emailData.status as string) === 'delivered' || (emailData.status as string) === 'sent' ? 'active' :
    (emailData.status as string) === 'failed' ? 'error' : 'pending'
  ))
  if (emailData.created_at) {
    keyValue('Sent', new Date(emailData.created_at as string).toLocaleString())
  }
  if (emailData.delivered_at) {
    keyValue('Delivered', new Date(emailData.delivered_at as string).toLocaleString())
  }

  if (emailData.body) {
    newline()
    header('Body')
    console.log(`  ${emailData.body}`)
  }

  if (emailData.error_message) {
    newline()
    warning(`Error: ${emailData.error_message}`)
  }

  newline()
}

async function emailStats(options: StatsOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  const stats = await withSpinner('Fetching email statistics...', async () => {
    const { data, error } = await getEmailStats({ client })
    if (error || !data) {
      throw new Error(getErrorMessage(error) ?? 'Failed to fetch email statistics')
    }
    return data
  })

  if (options.json) {
    json(stats)
    return
  }

  const statsData = stats as Record<string, unknown>

  newline()
  header(`${icons.info} Email Statistics`)
  if (statsData.total_sent !== undefined) {
    keyValue('Total Sent', String(statsData.total_sent))
  }
  if (statsData.total_delivered !== undefined) {
    keyValue('Total Delivered', colors.success(String(statsData.total_delivered)))
  }
  if (statsData.total_failed !== undefined) {
    keyValue('Total Failed', colors.error(String(statsData.total_failed)))
  }
  if (statsData.total_pending !== undefined) {
    keyValue('Total Pending', colors.warning(String(statsData.total_pending)))
  }
  if (statsData.delivery_rate !== undefined) {
    const rate = statsData.delivery_rate as number
    const rateColor = rate >= 95 ? colors.success : rate >= 80 ? colors.warning : colors.error
    keyValue('Delivery Rate', rateColor(`${rate.toFixed(1)}%`))
  }
  newline()
}

async function validateEmailAction(options: ValidateOptions): Promise<void> {
  await requireAuth()
  await setupClient()

  let email: string

  if (options.email) {
    email = options.email
  } else {
    email = await promptText({
      message: 'Email address to validate',
      required: true,
    })
  }

  const result = await withSpinner(`Validating ${email}...`, async () => {
    const { data, error } = await validateEmail({
      client,
      body: { email },
    })
    if (error || !data) {
      throw new Error(getErrorMessage(error) ?? 'Failed to validate email')
    }
    return data
  })

  if (options.json) {
    json(result)
    return
  }

  const resultData = result as Record<string, unknown>

  newline()
  header(`${icons.info} Email Validation: ${email}`)
  keyValue('Email', email)
  if (resultData.is_valid !== undefined) {
    keyValue('Valid', resultData.is_valid ? colors.success('Yes') : colors.error('No'))
  }
  if (resultData.format_valid !== undefined) {
    keyValue('Format Valid', resultData.format_valid ? colors.success('Yes') : colors.error('No'))
  }
  if (resultData.mx_valid !== undefined) {
    keyValue('MX Records Valid', resultData.mx_valid ? colors.success('Yes') : colors.error('No'))
  }
  if (resultData.disposable !== undefined) {
    keyValue('Disposable', resultData.disposable ? colors.warning('Yes') : 'No')
  }
  if (resultData.reason) {
    keyValue('Reason', resultData.reason as string)
  }
  if (resultData.suggestion) {
    info(`Did you mean: ${colors.bold(resultData.suggestion as string)}?`)
  }
  newline()
}
