---
name: add-error-tracking
description: |
  Add Temps error tracking to applications using the Sentry-compatible SDK. Temps exposes a Sentry-compatible DSN that works with the official Sentry SDK for each language/framework — no code changes beyond initialization are required. Use when the user wants to: (1) Add error tracking to any app (React, Next.js, Vue, Svelte, Angular, Node.js, Python, Go, Rust, Ruby, Java, PHP, .NET, React Native, Flutter), (2) Wire up uncaught exception and unhandled promise rejection capture, (3) Configure session replay for errors, (4) Upload source maps for readable stack traces, (5) Report releases and environments, (6) Capture custom errors/messages. Triggers: "add error tracking", "add sentry", "track exceptions", "report errors", "temps error tracking", "wire up error monitoring".
---

# Add Error Tracking

Integrate Temps error tracking (Sentry-compatible) into an application. The Temps DSN is a drop-in replacement for a Sentry DSN — use the official Sentry SDK for the user's platform and point it at the Temps DSN via an environment variable.

## Prefer Sentry's official per-framework skills when available

Temps is Sentry wire-compatible, so every skill Sentry publishes for their SDKs works against a Temps DSN. If the user's CLI already has one of these installed, route them to it and only substitute the DSN:

| User's platform | Sentry skill | Source |
|---|---|---|
| Next.js | `/sentry-nextjs-sdk` | `getsentry/sentry-for-ai` |
| React (Vite, Remix, etc.) | `/sentry-react-sdk` | `getsentry/sentry-for-ai` |
| Vanilla browser JS | `/sentry-browser-sdk` | `getsentry/sentry-for-ai` |
| Node.js | `/sentry-node-sdk` | `getsentry/sentry-for-ai` |
| React Native | `/sentry-react-native-sdk` | `getsentry/sentry-for-ai` |
| Generic (language-agnostic) | `/sentry-sdk-setup` | `getsentry/sentry-for-ai` |

For platforms Sentry has no dedicated skill for (Vue, Svelte, Angular, Python, Go, Rust, Ruby, Java, PHP, .NET, Flutter, etc.), follow the setup in this file directly.

**Always set the DSN from an environment variable — never hardcode it.** The user will point the env var at their Temps DSN instead of a Sentry DSN.

## Detect the platform

Infer the platform from the codebase:

- `package.json` with `"next"` → **Next.js** → `@sentry/nextjs`
- `package.json` with `"react"` and Vite/Remix/CRA → **React** → `@sentry/react`
- `package.json` with `"vue"` or `"nuxt"` → **Vue** → `@sentry/vue`
- `package.json` with `"svelte"` or `"@sveltejs/kit"` → **Svelte** → `@sentry/sveltekit`
- `package.json` with `"@angular/core"` → **Angular** → `@sentry/angular`
- `package.json` with `"express"`, `"fastify"`, `"@nestjs/core"` → **Node.js** → `@sentry/node`
- `package.json` with `"react-native"` or `"expo"` → **React Native** → `@sentry/react-native`
- `requirements.txt`/`pyproject.toml` with Flask, Django, FastAPI → **Python** → `sentry-sdk`
- `go.mod` → **Go** → `github.com/getsentry/sentry-go`
- `Cargo.toml` → **Rust** → `sentry`
- `Gemfile` with `rails` → **Ruby** → `sentry-ruby` + `sentry-rails`
- `pom.xml`/`build.gradle` with Spring → **Java** → `sentry-spring-boot-starter-jakarta`
- `composer.json` with `laravel/framework` or `symfony/*` → **PHP** → `sentry/sentry`
- `.csproj` with `Microsoft.AspNetCore.*` → **.NET** → `Sentry.AspNetCore`
- `pubspec.yaml` with `flutter` → **Flutter** → `sentry_flutter`

## Get the DSN

The user's Temps project exposes a DSN at **Error Tracking → DSN & Setup**. It looks like:

```
https://<public_key>@<temps-host>/<project_id>
```

If the user has not provided a DSN, tell them to:
1. Open their project in the Temps dashboard
2. Go to **Error Tracking → DSN & Setup**
3. Copy the DSN for the target environment

Always store the DSN in an environment variable. The exact variable name depends on the platform (browser bundlers often require a prefix to expose vars to the client):

| Platform | Env var name |
|---|---|
| Next.js | `NEXT_PUBLIC_SENTRY_DSN` |
| Vite / React / Vue | `VITE_SENTRY_DSN` |
| SvelteKit | `PUBLIC_SENTRY_DSN` |
| Angular | `SENTRY_DSN` (injected via `environment.ts`) |
| Everything else (Node, Python, Go, Rust, Ruby, Java, PHP, .NET, Flutter) | `SENTRY_DSN` |

```bash
# .env
SENTRY_DSN=https://<public_key>@<temps-host>/<project_id>
```

## Platform setup

Every snippet below reads the DSN from an env var — do not hardcode it.

### Next.js

```bash
npx @sentry/wizard@latest -i nextjs
```

Or manually:

```bash
npm install @sentry/nextjs
```

```ts
// sentry.client.config.ts
import * as Sentry from '@sentry/nextjs';

Sentry.init({
  dsn: process.env.NEXT_PUBLIC_SENTRY_DSN,
  tracesSampleRate: 1.0,
  replaysSessionSampleRate: 0.1,
  replaysOnErrorSampleRate: 1.0,
  integrations: [Sentry.replayIntegration()],
});
```

Mirror the config in `sentry.server.config.ts` and `sentry.edge.config.ts` (same `dsn`, no replay).

### React (Vite, Remix, CRA)

```bash
npm install @sentry/react
```

```tsx
// src/sentry.ts — import this first in main.tsx / root.tsx
import * as Sentry from '@sentry/react';

Sentry.init({
  dsn: import.meta.env.VITE_SENTRY_DSN,
  environment: import.meta.env.MODE,
  integrations: [Sentry.replayIntegration()],
  tracesSampleRate: 1.0,
  replaysSessionSampleRate: 0.1,
  replaysOnErrorSampleRate: 1.0,
});
```

Wrap the app root with `<Sentry.ErrorBoundary>` for React render errors.

### Vue (Vue 3 / Nuxt)

```bash
npm install @sentry/vue
```

```ts
// src/main.ts
import { createApp } from 'vue';
import * as Sentry from '@sentry/vue';
import App from './App.vue';

const app = createApp(App);

Sentry.init({
  app,
  dsn: import.meta.env.VITE_SENTRY_DSN,
  tracesSampleRate: 1.0,
});

app.mount('#app');
```

### Svelte / SvelteKit

```bash
npx @sentry/wizard@latest -i sveltekit
```

```ts
// src/hooks.client.ts
import * as Sentry from '@sentry/sveltekit';
import { PUBLIC_SENTRY_DSN } from '$env/static/public';

Sentry.init({
  dsn: PUBLIC_SENTRY_DSN,
  tracesSampleRate: 1.0,
});

export const handleError = Sentry.handleErrorWithSentry();
```

Mirror in `src/hooks.server.ts` using `$env/dynamic/private` for the server DSN.

### Angular

```bash
npm install @sentry/angular
```

```ts
// src/main.ts
import * as Sentry from '@sentry/angular';
import { environment } from './environments/environment';

Sentry.init({
  dsn: environment.sentryDsn,
  tracesSampleRate: 1.0,
});
```

Populate `environment.sentryDsn` from `process.env.SENTRY_DSN` at build time.

### Vanilla JavaScript (browser)

```bash
npm install @sentry/browser
```

```ts
import * as Sentry from '@sentry/browser';

Sentry.init({
  dsn: import.meta.env.VITE_SENTRY_DSN,
  tracesSampleRate: 1.0,
  integrations: [Sentry.browserTracingIntegration(), Sentry.replayIntegration()],
});
```

### Node.js

```bash
npm install @sentry/node
```

```ts
// Must be the first import in your entrypoint.
import * as Sentry from '@sentry/node';

Sentry.init({
  dsn: process.env.SENTRY_DSN,
  environment: process.env.NODE_ENV,
  tracesSampleRate: 1.0,
});
```

For Express:

```ts
import express from 'express';
import * as Sentry from '@sentry/node';

const app = express();
Sentry.setupExpressErrorHandler(app);
```

### React Native

```bash
npx @sentry/wizard@latest -s -i reactNative
```

```ts
import * as Sentry from '@sentry/react-native';

Sentry.init({
  dsn: process.env.SENTRY_DSN,
  tracesSampleRate: 1.0,
  replaysSessionSampleRate: 0.1,
  replaysOnErrorSampleRate: 1.0,
});

export default Sentry.wrap(App);
```

### Python

```bash
pip install sentry-sdk
```

```python
import os
import sentry_sdk

sentry_sdk.init(
    dsn=os.environ["SENTRY_DSN"],
    environment=os.environ.get("ENV", "development"),
    traces_sample_rate=1.0,
    profiles_sample_rate=1.0,
)
```

Framework integrations:

```python
# Flask
from sentry_sdk.integrations.flask import FlaskIntegration
sentry_sdk.init(dsn=os.environ["SENTRY_DSN"], integrations=[FlaskIntegration()])

# Django
from sentry_sdk.integrations.django import DjangoIntegration
sentry_sdk.init(dsn=os.environ["SENTRY_DSN"], integrations=[DjangoIntegration()])

# FastAPI
from sentry_sdk.integrations.starlette import StarletteIntegration
from sentry_sdk.integrations.fastapi import FastApiIntegration
sentry_sdk.init(
    dsn=os.environ["SENTRY_DSN"],
    integrations=[StarletteIntegration(), FastApiIntegration()],
)
```

### Go

```bash
go get github.com/getsentry/sentry-go
```

```go
package main

import (
    "log"
    "os"
    "time"

    "github.com/getsentry/sentry-go"
)

func main() {
    err := sentry.Init(sentry.ClientOptions{
        Dsn:              os.Getenv("SENTRY_DSN"),
        TracesSampleRate: 1.0,
        Environment:      os.Getenv("ENV"),
    })
    if err != nil {
        log.Fatalf("sentry.Init: %s", err)
    }
    defer sentry.Flush(2 * time.Second)
}
```

### Rust

```bash
cargo add sentry sentry-tracing
```

```rust
use std::env;

fn main() {
    let _guard = sentry::init((
        env::var("SENTRY_DSN").expect("SENTRY_DSN must be set"),
        sentry::ClientOptions {
            release: sentry::release_name!(),
            traces_sample_rate: 1.0,
            environment: env::var("ENV").ok().map(Into::into),
            ..Default::default()
        },
    ));

    // Your app entrypoint
}
```

### Ruby (Rails)

```bash
bundle add sentry-ruby sentry-rails
```

```ruby
# config/initializers/sentry.rb
require "sentry-ruby"
require "sentry-rails"

Sentry.init do |config|
  config.dsn = ENV["SENTRY_DSN"]
  config.environment = ENV.fetch("RAILS_ENV", "development")
  config.traces_sample_rate = 1.0
end
```

### Java (Spring Boot)

```xml
<!-- pom.xml -->
<dependency>
  <groupId>io.sentry</groupId>
  <artifactId>sentry-spring-boot-starter-jakarta</artifactId>
  <version>7.14.0</version>
</dependency>
```

```properties
# application.properties — Spring reads ${SENTRY_DSN} from the environment
sentry.dsn=${SENTRY_DSN}
sentry.environment=${ENV:development}
sentry.traces-sample-rate=1.0
```

### PHP

```bash
composer require sentry/sentry
```

```php
<?php
\Sentry\init([
    'dsn' => $_ENV['SENTRY_DSN'],
    'environment' => $_ENV['APP_ENV'] ?? 'development',
    'traces_sample_rate' => 1.0,
]);
```

For Laravel, use `sentry/sentry-laravel` and configure via `config/sentry.php` reading `env('SENTRY_DSN')`.

### .NET (ASP.NET Core)

```bash
dotnet add package Sentry.AspNetCore
```

```csharp
// Program.cs
builder.WebHost.UseSentry(options =>
{
    options.Dsn = Environment.GetEnvironmentVariable("SENTRY_DSN");
    options.Environment = builder.Environment.EnvironmentName;
    options.TracesSampleRate = 1.0;
});
```

### Flutter

```bash
flutter pub add sentry_flutter
```

```dart
import 'package:flutter/widgets.dart';
import 'package:sentry_flutter/sentry_flutter.dart';

Future<void> main() async {
  await SentryFlutter.init(
    (options) {
      options.dsn = const String.fromEnvironment('SENTRY_DSN');
      options.tracesSampleRate = 1.0;
    },
    appRunner: () => runApp(const MyApp()),
  );
}
```

Pass the DSN at build time: `flutter run --dart-define=SENTRY_DSN=$SENTRY_DSN`.

## Capture custom errors

### JavaScript / TypeScript

```ts
import * as Sentry from '@sentry/react'; // or /browser, /node, /nextjs, etc.

try {
  doRiskyThing();
} catch (err) {
  Sentry.captureException(err);
}

Sentry.captureMessage('Something notable happened', 'warning');
```

### Python

```python
try:
    do_risky_thing()
except Exception as exc:
    sentry_sdk.capture_exception(exc)

sentry_sdk.capture_message("Something notable happened", level="warning")
```

### Go

```go
sentry.CaptureException(err)
sentry.CaptureMessage("Something notable happened")
```

### Rust

```rust
sentry::capture_error(&err);
sentry::capture_message("Something notable happened", sentry::Level::Warning);
```

## Source maps (JS/TS only)

Upload source maps during CI so the Temps dashboard shows original source in stack traces.

```bash
npm install --save-dev @sentry/cli
```

```bash
sentry-cli sourcemaps inject ./dist
sentry-cli sourcemaps upload \
  --url-prefix '~/' \
  --release "$GIT_SHA" \
  ./dist
```

The Temps dashboard also accepts source map uploads via **Error Tracking → Source Maps** in the UI.

## Verification

After wiring up:

1. Throw a deliberate error from the app:
   - JS / TS: `throw new Error('Temps error tracking test');`
   - Python: `raise Exception('Temps error tracking test')`
   - Go: `sentry.CaptureException(errors.New("Temps test"))`
   - Rust: `panic!("Temps test")` (inside a handler caught by the Sentry integration)
2. Run the app and trigger the path that throws.
3. Open **Error Tracking → Error Groups** in the Temps dashboard — the error should appear within a few seconds.
4. Confirm the stack trace and environment are populated.

## Common issues

- **Nothing shows up**: Verify the DSN env var is loaded and the SDK is initialized *before* any code that might throw. For Node, `Sentry.init` must be the very first import.
- **Minified stack traces**: Upload source maps (see above).
- **Browser apps not reporting**: Make sure the env var uses the bundler's public prefix (`NEXT_PUBLIC_`, `VITE_`, `PUBLIC_`) so it reaches the client bundle.
- **Events missing in production**: Confirm the deployment environment sets the DSN env var — local `.env` files are not copied automatically.
