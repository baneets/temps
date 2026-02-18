#!/usr/bin/env node

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  ListPromptsRequestSchema,
  GetPromptRequestSchema,
  ListToolsRequestSchema,
  CallToolRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';

import { listPrompts, getPrompt } from './handlers/prompts-handler.js';
import { listTools, callTool, toolCount, availableCategories } from './tools/index.js';

/**
 * Temps MCP Server
 *
 * Provides full Temps platform management through MCP tools and prompts.
 * Requires TEMPS_API_URL and TEMPS_API_KEY environment variables.
 *
 * Usage:
 *   npx @temps-sdk/mcp
 *   npx @temps-sdk/mcp --tools deployments,analytics,projects
 *   TEMPS_MCP_TOOLS=deployments,analytics npx @temps-sdk/mcp
 *
 * Available tool categories:
 *   all (default), or any combination of:
 *   projects, deployments, environments, domains, services, backups,
 *   monitors, containers, users, settings, api-keys, webhooks, audit,
 *   dns-providers, notifications, scans, custom-domains, errors,
 *   proxy-logs, dsn, ip-access, incidents, funnels, presets, platform,
 *   email-domains, email-providers, load-balancer, notification-prefs,
 *   analytics
 */

const server = new Server(
  {
    name: '@temps-sdk/mcp',
    version: '0.1.0',
  },
  {
    capabilities: {
      prompts: {},
      tools: {},
    },
  }
);

// Prompts
server.setRequestHandler(ListPromptsRequestSchema, async () => {
  return listPrompts();
});

server.setRequestHandler(GetPromptRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;
  return await getPrompt(name, args || {});
});

// Tools
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return listTools();
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;
  const result = await callTool(name, args || {});
  return {
    content: result.content,
    isError: result.isError,
  };
});

// Handle --help
if (process.argv.includes('--help') || process.argv.includes('-h')) {
  console.log(`Temps MCP Server — ${availableCategories.length} tool categories available\n`);
  console.log('Usage:');
  console.log('  npx @temps-sdk/mcp                                    # all tools');
  console.log('  npx @temps-sdk/mcp --tools deployments,analytics      # specific categories');
  console.log('  TEMPS_MCP_TOOLS=deployments npx @temps-sdk/mcp        # via env var\n');
  console.log('Available categories:');
  console.log(`  ${availableCategories.join(', ')}\n`);
  console.log('Environment variables:');
  console.log('  TEMPS_API_URL       Temps API URL (required)');
  console.log('  TEMPS_API_KEY       API key with appropriate permissions (required)');
  console.log('  TEMPS_MCP_TOOLS     Comma-separated tool categories (alternative to --tools)');
  process.exit(0);
}

// Start
async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);

  console.error(`Temps MCP Server running on stdio (${toolCount} tools available)`);
  console.error(
    `API URL: ${process.env.TEMPS_API_URL || '(not configured)'}`,
  );
  console.error(
    `API Key: ${process.env.TEMPS_API_KEY ? '***configured***' : '(not configured)'}`,
  );
}

main().catch((error) => {
  console.error('Fatal error:', error);
  process.exit(1);
});
