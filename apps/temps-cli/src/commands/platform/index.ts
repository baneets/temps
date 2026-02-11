import type { Command } from 'commander'
import { requireAuth } from '../../config/store.js'
import { setupClient, client, getErrorMessage } from '../../lib/api-client.js'
import {
  getPlatformInfo,
  getAccessInfo,
  getPrivateIp,
  getPublicIp,
} from '../../api/sdk.gen.js'
import { withSpinner } from '../../ui/spinner.js'
import { newline, header, icons, json, colors, info, keyValue, success } from '../../ui/output.js'

export function registerPlatformCommands(program: Command): void {
  const platform = program
    .command('platform')
    .alias('plat')
    .description('View platform and server information')

  platform
    .command('info')
    .description('Get platform information')
    .option('--json', 'Output in JSON format')
    .action(platformInfoAction)

  platform
    .command('access')
    .description('Get access and networking information')
    .option('--json', 'Output in JSON format')
    .action(accessInfoAction)

  platform
    .command('private-ip')
    .description('Get the server private IP address')
    .action(privateIpAction)

  platform
    .command('public-ip')
    .description('Get the server public IP address')
    .action(publicIpAction)
}

async function platformInfoAction(options: { json?: boolean }): Promise<void> {
  await requireAuth()
  await setupClient()

  const platformInfo = await withSpinner('Fetching platform info...', async () => {
    const { data, error } = await getPlatformInfo({ client })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  if (!platformInfo) {
    info('Platform information not available')
    return
  }

  if (options.json) {
    json(platformInfo)
    return
  }

  newline()
  header(`${icons.globe} Platform Information`)
  keyValue('OS Type', platformInfo.os_type)
  keyValue('Architecture', platformInfo.architecture)
  keyValue('Platforms', platformInfo.platforms.join(', ') || colors.muted('none'))
  newline()
}

async function accessInfoAction(options: { json?: boolean }): Promise<void> {
  await requireAuth()
  await setupClient()

  const accessData = await withSpinner('Fetching access info...', async () => {
    const { data, error } = await getAccessInfo({ client })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  if (!accessData) {
    info('Access information not available')
    return
  }

  if (options.json) {
    json(accessData)
    return
  }

  newline()
  header(`${icons.globe} Access Information`)
  keyValue('Access Mode', accessData.access_mode)
  keyValue('Can Create Domains', accessData.can_create_domains ? colors.success('Yes') : colors.muted('No'))
  if (accessData.domain_creation_error) {
    keyValue('Domain Error', colors.warning(accessData.domain_creation_error))
  }
  keyValue('Public IP', accessData.public_ip || colors.muted('not available'))
  keyValue('Private IP', accessData.private_ip || colors.muted('not available'))
  newline()
}

async function privateIpAction(): Promise<void> {
  await requireAuth()
  await setupClient()

  const result = await withSpinner('Fetching private IP...', async () => {
    const { data, error } = await getPrivateIp({ client })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  if (result && typeof result === 'object' && 'ip' in result) {
    success(String((result as { ip: string }).ip))
  } else if (typeof result === 'string') {
    success(result)
  } else {
    json(result)
  }
}

async function publicIpAction(): Promise<void> {
  await requireAuth()
  await setupClient()

  const result = await withSpinner('Fetching public IP...', async () => {
    const { data, error } = await getPublicIp({ client })
    if (error) {
      throw new Error(getErrorMessage(error))
    }
    return data
  })

  if (result && typeof result === 'object' && 'ip' in result) {
    success(String((result as { ip: string }).ip))
  } else if (typeof result === 'string') {
    success(result)
  } else {
    json(result)
  }
}
