import { credentials, config } from '../../config/store.js'
import { promptPassword, promptText, promptSelect, promptEmail } from '../../ui/prompts.js'
import { withSpinner } from '../../ui/spinner.js'
import { info, icons, colors, newline, box, warning } from '../../ui/output.js'
import { setupClient, client } from '../../lib/api-client.js'
import { getCurrentUser } from '../../api/sdk.gen.js'
import { AuthenticationError } from '../../utils/errors.js'

interface LoginOptions {
  apiKey?: string
  email?: string
  magic?: string
}

export async function login(options: LoginOptions): Promise<void> {
  newline()

  if (await credentials.isAuthenticated()) {
    const existingEmail = await credentials.get('email')
    info(`Already logged in as ${colors.bold(existingEmail ?? 'unknown')}`)
    info('Run "temps logout" first to switch accounts')
    return
  }

  if (options.magic) {
    await loginWithMagicLink(typeof options.magic === 'string' ? options.magic : undefined)
    return
  }

  if (options.email) {
    await loginWithEmail(typeof options.email === 'string' ? options.email : undefined)
    return
  }

  // If no specific method, check if --api-key or prompt for method
  if (options.apiKey) {
    await loginWithApiKey(options.apiKey)
    return
  }

  // Interactive: ask which method
  const method = await promptSelect({
    message: 'How would you like to log in?',
    choices: [
      { name: 'API Key', value: 'api-key', description: 'Paste an API key from the dashboard' },
      { name: 'Email & Password', value: 'email', description: 'Log in with email and password' },
      { name: 'Magic Link', value: 'magic', description: 'Receive a login link via email' },
    ],
  })

  switch (method) {
    case 'api-key':
      await loginWithApiKey()
      break
    case 'email':
      await loginWithEmail()
      break
    case 'magic':
      await loginWithMagicLink()
      break
  }
}

export async function loginWithApiKey(apiKey?: string): Promise<void> {
  const key = apiKey ?? (await promptPassword({
    message: 'API Key',
    validate: (value) => {
      if (!value || value.trim().length === 0) {
        return 'API key is required'
      }
      return true
    },
  }))

  // Temporarily set the API key to validate it
  await credentials.set('apiKey', key)
  await setupClient()

  try {
    const result = await withSpinner(
      'Validating API key...',
      async () => {
        const { data, error } = await getCurrentUser({ client })
        if (error) {
          throw new AuthenticationError('Invalid API key')
        }
        return data
      },
      { successText: 'API key validated' }
    )
    if (!result) {
      throw new AuthenticationError('Invalid API key')
    }

    await credentials.setAll({
      apiKey: key,
      userId: result.id,
      email: result.email ?? undefined,
    })

    displayWelcome(result.email)
  } catch (err) {
    await credentials.clear()
    throw new AuthenticationError('Invalid API key')
  }
}

export async function loginWithEmail(emailArg?: string): Promise<void> {
  const email = emailArg ?? await promptEmail('Email')

  const password = await promptPassword({
    message: 'Password',
    validate: (value) => {
      if (!value || value.trim().length === 0) {
        return 'Password is required'
      }
      return true
    },
  })

  // Use raw fetch to capture the session cookie
  const apiUrl = config.get('apiUrl')

  const authResponse = await withSpinner('Logging in...', async () => {
    const response = await fetch(`${apiUrl}/auth/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ email, password }),
    })

    if (!response.ok) {
      if (response.status === 401) {
        throw new AuthenticationError('Invalid email or password')
      }
      throw new AuthenticationError(`Login failed (status ${response.status})`)
    }

    const data = await response.json() as { success: boolean; mfa_required: boolean; user_id?: number | null; message: string }
    const setCookie = response.headers.get('set-cookie')

    return { data, setCookie }
  }, { successText: 'Authenticated' })

  // Handle MFA if required
  if (authResponse.data.mfa_required) {
    const mfaCode = await promptText({
      message: 'MFA Code',
      required: true,
      validate: (value) => /^\d{6}$/.test(value) || 'Enter a 6-digit code',
    })

    const sessionCookie = extractCookieValue(authResponse.setCookie, 'session_token') ??
                          extractCookieValue(authResponse.setCookie, 'id')

    await withSpinner('Verifying MFA...', async () => {
      const response = await fetch(`${apiUrl}/auth/verify-mfa`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(sessionCookie ? { 'Cookie': sessionCookie } : {}),
        },
        body: JSON.stringify({ code: mfaCode }),
      })

      if (!response.ok) {
        throw new AuthenticationError('Invalid MFA code')
      }

      // Update session cookie from MFA response
      const newSetCookie = response.headers.get('set-cookie')
      if (newSetCookie) {
        authResponse.setCookie = newSetCookie
      }
    }, { successText: 'MFA verified' })
  }

  // Extract session cookie and create an API token
  const sessionCookie = extractSessionCookie(authResponse.setCookie)

  if (!sessionCookie) {
    // Fallback: if we can't extract a session cookie, ask for API key
    warning('Could not extract session. Please create an API key from the dashboard.')
    info(`Dashboard: ${colors.primary(`${apiUrl}/dashboard/settings/api-keys`)}`)
    newline()
    info('Then run: temps login --api-key <your-key>')
    return
  }

  // Create a long-lived API token using the session
  const apiKey = await withSpinner('Creating API token...', async () => {
    const response = await fetch(`${apiUrl}/api/tokens`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Cookie': sessionCookie,
      },
      body: JSON.stringify({
        name: `temps-cli-${new Date().toISOString().split('T')[0]}`,
        permissions: ['*'],
      }),
    })

    if (!response.ok) {
      // If token creation fails, try to use the session cookie as auth
      throw new Error('Could not create API token from session')
    }

    const tokenData = await response.json() as { token?: string; api_key?: string }
    return tokenData.token ?? tokenData.api_key
  }, { successText: 'API token created' })

  if (!apiKey) {
    warning('Could not create API token. Please generate one from the dashboard.')
    info(`Dashboard: ${colors.primary(`${apiUrl}/dashboard/settings/api-keys`)}`)
    return
  }

  // Validate and store
  await credentials.set('apiKey', apiKey)
  await setupClient()

  const user = await withSpinner('Validating...', async () => {
    const { data, error } = await getCurrentUser({ client })
    if (error || !data) {
      throw new AuthenticationError('Token validation failed')
    }
    return data
  }, { successText: 'Validated' })

  await credentials.setAll({
    apiKey,
    userId: user.id,
    email: user.email ?? undefined,
  })

  displayWelcome(user.email)
}

export async function loginWithMagicLink(emailArg?: string): Promise<void> {
  const email = emailArg ?? await promptEmail('Email')
  const apiUrl = config.get('apiUrl')

  await withSpinner('Sending magic link...', async () => {
    const response = await fetch(`${apiUrl}/auth/magic-link/request`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ email }),
    })

    if (!response.ok && response.status !== 200) {
      if (response.status === 503) {
        throw new Error('Email service is not configured on this server')
      }
      throw new Error('Failed to send magic link')
    }
  }, { successText: 'Magic link sent' })

  newline()
  info('Check your email for a magic link.')
  info('After clicking the link, paste the token from the URL below.')
  newline()

  const tokenInput = await promptText({
    message: 'Magic link token (from URL or email)',
    required: true,
  })

  // Extract token from URL if user pastes full URL
  const token = extractTokenFromInput(tokenInput)

  // Verify the magic link token using raw fetch to capture session cookie
  const verifyResult = await withSpinner('Verifying token...', async () => {
    const response = await fetch(`${apiUrl}/auth/magic-link/verify?token=${encodeURIComponent(token)}`, {
      method: 'GET',
      headers: { 'Content-Type': 'application/json' },
    })

    if (!response.ok) {
      if (response.status === 400) {
        throw new AuthenticationError('Invalid or expired token')
      }
      throw new AuthenticationError('Verification failed')
    }

    const data = await response.json() as { success: boolean; message: string }
    const setCookie = response.headers.get('set-cookie')

    return { data, setCookie }
  }, { successText: 'Token verified' })

  // Create API token from session
  const sessionCookie = extractSessionCookie(verifyResult.setCookie)

  if (!sessionCookie) {
    warning('Could not extract session. Please create an API key from the dashboard.')
    info(`Dashboard: ${colors.primary(`${apiUrl}/dashboard/settings/api-keys`)}`)
    newline()
    info('Then run: temps login --api-key <your-key>')
    return
  }

  const apiKey = await withSpinner('Creating API token...', async () => {
    const response = await fetch(`${apiUrl}/api/tokens`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Cookie': sessionCookie,
      },
      body: JSON.stringify({
        name: `temps-cli-${new Date().toISOString().split('T')[0]}`,
        permissions: ['*'],
      }),
    })

    if (!response.ok) {
      throw new Error('Could not create API token from session')
    }

    const tokenData = await response.json() as { token?: string; api_key?: string }
    return tokenData.token ?? tokenData.api_key
  }, { successText: 'API token created' })

  if (!apiKey) {
    warning('Could not create API token. Please generate one from the dashboard.')
    return
  }

  await credentials.set('apiKey', apiKey)
  await setupClient()

  const user = await withSpinner('Validating...', async () => {
    const { data, error } = await getCurrentUser({ client })
    if (error || !data) throw new AuthenticationError('Token validation failed')
    return data
  }, { successText: 'Validated' })

  await credentials.setAll({
    apiKey,
    userId: user.id,
    email: user.email ?? undefined,
  })

  displayWelcome(user.email)
}

// ── Helpers ──

function extractCookieValue(setCookie: string | null, name: string): string | null {
  if (!setCookie) return null

  // set-cookie can have multiple cookies separated by commas (or multiple headers)
  const cookies = setCookie.split(/,(?=\s*\w+=)/)
  for (const cookie of cookies) {
    const match = cookie.match(new RegExp(`${name}=([^;]+)`))
    if (match) return `${name}=${match[1]}`
  }
  return null
}

function extractSessionCookie(setCookie: string | null): string | null {
  if (!setCookie) return null

  // Try common session cookie names
  const cookieNames = ['session_token', 'session', 'id', 'sid']
  for (const name of cookieNames) {
    const cookie = extractCookieValue(setCookie, name)
    if (cookie) return cookie
  }

  // Fallback: return the full set-cookie for single cookie
  const match = setCookie.match(/^([^=]+=[^;]+)/)
  if (match?.[1]) return match[1]

  return null
}

function extractTokenFromInput(input: string): string {
  const trimmed = input.trim()

  // If it looks like a URL, extract the token parameter
  try {
    const url = new URL(trimmed)
    const token = url.searchParams.get('token')
    if (token) return token
  } catch {
    // Not a URL, treat as raw token
  }

  return trimmed
}

export function displayWelcome(email?: string | null): void {
  newline()
  const lines = [
    email ? `Logged in as ${colors.bold(email)}` : null,
    `API: ${colors.muted(config.get('apiUrl'))}`,
    `Credentials stored in: ${colors.muted(credentials.path)}`,
  ].filter(Boolean).join('\n')
  box(lines, `${icons.sparkles} Welcome to Temps!`)
}
