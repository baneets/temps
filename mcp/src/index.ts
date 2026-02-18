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
import { listTools, callTool, toolCount } from './tools/index.js';

/**
 * Temps MCP Server
 *
 * Provides full Temps platform management through MCP tools and prompts.
 * Requires TEMPS_API_URL and TEMPS_API_KEY environment variables.
 *
 * Usage:
 *   npx @temps-sdk/mcp
 *   bunx @temps-sdk/mcp
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

// Start
async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);

  console.error(`Temps MCP Server running on stdio (${toolCount} tools available)`);
  console.error(
    `API URL: ${process.env.TEMPS_API_URL || '(not configured)'}`
  );
  console.error(
    `API Key: ${process.env.TEMPS_API_KEY ? '***configured***' : '(not configured)'}`
  );
}

main().catch((error) => {
  console.error('Fatal error:', error);
  process.exit(1);
});
