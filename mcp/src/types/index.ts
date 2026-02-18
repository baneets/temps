/**
 * Shared types for Temps MCP Server
 */

import {
  TextContent,
  ImageContent,
  EmbeddedResource,
} from '@modelcontextprotocol/sdk/types.js';

export interface PromptDefinition {
  name: string;
  description: string;
  arguments?: Array<{
    name: string;
    description: string;
    required?: boolean;
  }>;
  handler: (args: Record<string, unknown>) => Promise<{
    messages: Array<{
      role: 'user' | 'assistant';
      content: TextContent | ImageContent | EmbeddedResource;
    }>;
  }>;
}

/**
 * MCP Tool definition used by all tool modules
 */
export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: {
    type: 'object';
    properties: Record<string, unknown>;
    required?: string[];
  };
  handler: (args: Record<string, unknown>) => Promise<ToolResult>;
}

export interface ToolResult {
  content: Array<{ type: 'text'; text: string }>;
  isError?: boolean;
}
