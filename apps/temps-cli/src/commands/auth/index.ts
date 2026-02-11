import type { Command } from 'commander'
import { login } from './login.js'
import { logout } from './logout.js'
import { whoami } from './whoami.js'

export function registerAuthCommands(program: Command): void {
  program
    .command('login')
    .description('Authenticate with the current Temps instance')
    .option('-k, --api-key <key>', 'API key (will prompt if not provided)')
    .option('--email [email]', 'Login with email and password')
    .option('--magic [email]', 'Login via magic link (email-based)')
    .action(login)

  program
    .command('logout')
    .description('Log out and clear credentials')
    .action(logout)

  program
    .command('whoami')
    .description('Display current authenticated user')
    .option('--json', 'Output as JSON')
    .action(whoami)
}
