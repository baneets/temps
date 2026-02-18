/**
 * Shared helpers for tool response formatting
 */

import type { ToolResult } from '../types/index.js';

export function ok(text: string): ToolResult {
  return { content: [{ type: 'text', text }] };
}

export function err(message: string): ToolResult {
  return { content: [{ type: 'text', text: `Error: ${message}` }], isError: true };
}

export function json(label: string, data: unknown): ToolResult {
  return ok(`## ${label}\n\n\`\`\`json\n${JSON.stringify(data, null, 2)}\n\`\`\``);
}

export function formatDate(d: string | number | null | undefined): string {
  if (!d) return 'N/A';
  const date = typeof d === 'number' ? new Date(d * 1000) : new Date(d);
  return date.toISOString();
}

export function table(headers: string[], rows: string[][]): string {
  const sep = headers.map(() => '---');
  const lines = [
    `| ${headers.join(' | ')} |`,
    `| ${sep.join(' | ')} |`,
    ...rows.map((r) => `| ${r.join(' | ')} |`),
  ];
  return lines.join('\n');
}

/** Safely handle async tool calls with consistent error formatting */
export async function handleToolCall(
  fn: () => Promise<ToolResult>
): Promise<ToolResult> {
  try {
    return await fn();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return err(message);
  }
}

export function requireParam<T>(args: Record<string, unknown>, name: string): T {
  const val = args[name];
  if (val === undefined || val === null) {
    throw new Error(`Missing required parameter: ${name}`);
  }
  return val as T;
}

export function optionalParam<T>(args: Record<string, unknown>, name: string, defaultVal?: T): T | undefined {
  const val = args[name];
  if (val === undefined || val === null) return defaultVal;
  return val as T;
}
