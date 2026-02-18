/**
 * Temps API Client
 *
 * Generic REST client that uses TEMPS_API_URL and TEMPS_API_KEY
 * to communicate with the Temps platform API.
 */

export interface TempsConfig {
  apiUrl: string;
  apiKey: string;
}

function normalizeApiUrl(url: string): string {
  let normalized = url.replace(/\/+$/, '');
  if (!normalized.endsWith('/api')) {
    normalized += '/api';
  }
  return normalized;
}

export function getConfig(): TempsConfig {
  const apiUrl = process.env.TEMPS_API_URL;
  const apiKey = process.env.TEMPS_API_KEY;

  if (!apiUrl) {
    throw new Error(
      'TEMPS_API_URL environment variable is required. Set it to your Temps instance URL (e.g. https://temps.example.com)'
    );
  }

  if (!apiKey) {
    throw new Error(
      'TEMPS_API_KEY environment variable is required. Create an API key in the Temps dashboard under Settings > API Keys'
    );
  }

  return { apiUrl: normalizeApiUrl(apiUrl), apiKey };
}

export class TempsClient {
  private config: TempsConfig;

  constructor(config?: TempsConfig) {
    this.config = config || getConfig();
  }

  async get<T = unknown>(path: string, query?: Record<string, unknown>): Promise<T> {
    const url = this.buildUrl(path, query);
    return this.request<T>(url, { method: 'GET' });
  }

  async post<T = unknown>(path: string, body?: unknown): Promise<T> {
    return this.request<T>(this.buildUrl(path), {
      method: 'POST',
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  }

  async put<T = unknown>(path: string, body?: unknown): Promise<T> {
    return this.request<T>(this.buildUrl(path), {
      method: 'PUT',
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  }

  async patch<T = unknown>(path: string, body?: unknown): Promise<T> {
    return this.request<T>(this.buildUrl(path), {
      method: 'PATCH',
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  }

  async delete<T = unknown>(path: string): Promise<T> {
    return this.request<T>(this.buildUrl(path), { method: 'DELETE' });
  }

  /**
   * Connect to a WebSocket endpoint, collect messages until the server
   * closes the connection, and return them as a string array.
   * Use `follow: false` in query params for a finite snapshot.
   */
  async ws(
    path: string,
    query?: Record<string, unknown>,
    options?: { timeoutMs?: number }
  ): Promise<string[]> {
    const httpUrl = this.buildUrl(path, query);
    const wsUrl = httpUrl
      .replace(/^https:/, 'wss:')
      .replace(/^http:/, 'ws:');
    const timeoutMs = options?.timeoutMs ?? 15_000;

    return new Promise<string[]>((resolve, reject) => {
      const messages: string[] = [];
      let timer: ReturnType<typeof setTimeout> | undefined;

      const ws = new WebSocket(wsUrl, {
        headers: {
          Authorization: `Bearer ${this.config.apiKey}`,
        },
      } as any);

      const cleanup = () => {
        if (timer) clearTimeout(timer);
        try { ws.close(); } catch { /* already closed */ }
      };

      timer = setTimeout(() => {
        cleanup();
        resolve(messages);
      }, timeoutMs);

      ws.onmessage = (event: MessageEvent) => {
        const data = typeof event.data === 'string'
          ? event.data
          : String(event.data);

        // Parse JSON messages — the server may send errors or plain log lines
        try {
          const parsed = JSON.parse(data);
          if (parsed.error) {
            cleanup();
            reject(new Error(parsed.error + (parsed.detail ? `: ${parsed.detail}` : '')));
            return;
          }
          if (parsed.message !== undefined) {
            messages.push(String(parsed.message).replace(/\r?\n$/, ''));
            return;
          }
        } catch { /* plain text */ }

        messages.push(data.replace(/\r?\n$/, ''));
      };

      ws.onerror = () => {
        cleanup();
        reject(new Error(`WebSocket connection failed for ${path}`));
      };

      ws.onclose = () => {
        if (timer) clearTimeout(timer);
        resolve(messages);
      };
    });
  }

  private buildUrl(path: string, query?: Record<string, unknown>): string {
    const url = new URL(`${this.config.apiUrl}${path}`);
    if (query) {
      for (const [key, value] of Object.entries(query)) {
        if (value !== undefined && value !== null && value !== '') {
          url.searchParams.set(key, String(value));
        }
      }
    }
    return url.toString();
  }

  private async request<T>(url: string, options: RequestInit): Promise<T> {
    const response = await fetch(url, {
      ...options,
      headers: {
        Authorization: `Bearer ${this.config.apiKey}`,
        'Content-Type': 'application/json',
        Accept: 'application/json',
        ...options.headers,
      },
    });

    // Always read body as text first to avoid "body already read" errors
    const text = await response.text();

    if (!response.ok) {
      let detail = text;
      try {
        const body = JSON.parse(text) as Record<string, unknown>;
        detail =
          (body.detail as string) || (body.message as string) || (body.error as string) || text;
      } catch {
        // text is already the raw body
      }
      throw new Error(
        `API Error ${response.status} ${response.statusText}: ${detail}`
      );
    }

    // Handle 204 No Content or empty body
    if (response.status === 204 || !text) {
      return undefined as T;
    }

    return JSON.parse(text) as T;
  }
}

// Singleton
let client: TempsClient | null = null;

export function getClient(): TempsClient {
  if (!client) {
    client = new TempsClient();
  }
  return client;
}
