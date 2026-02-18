import { getClient } from '../api/index.js';
import {
  ok,
  json,
  handleToolCall,
  requireParam,
} from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

export const tools: ToolDefinition[] = [
  {
    name: 'get_notification_preferences',
    description:
      'Get the current notification preferences for the authenticated user',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const prefs = await client.get<Record<string, unknown>>(
          '/notification-preferences'
        );
        return json('Notification Preferences', prefs);
      }),
  },

  {
    name: 'update_notification_preference',
    description:
      'Update a single notification preference by key. Fetches current preferences first, then updates the specified key.',
    inputSchema: {
      type: 'object',
      properties: {
        key: {
          type: 'string',
          description:
            'The preference key to update (e.g. deployment_success, deployment_failure)',
        },
        value: {
          description:
            'The new value for the preference (boolean, string, or object depending on the key)',
        },
      },
      required: ['key', 'value'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const key = requireParam<string>(args, 'key');
        const value = requireParam<unknown>(args, 'value');

        // Fetch current preferences to merge
        const current = await client.get<Record<string, unknown>>(
          '/notification-preferences'
        );

        const preferences = { ...current, [key]: value };

        const result = await client.put<Record<string, unknown>>(
          '/notification-preferences',
          { preferences }
        );

        return ok(
          `Notification preference '${key}' updated successfully.\n\n\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\``
        );
      }),
  },

  {
    name: 'reset_notification_preferences',
    description:
      'Reset all notification preferences to their default values',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        await client.delete('/notification-preferences');
        return ok('Notification preferences reset to defaults.');
      }),
  },
];
