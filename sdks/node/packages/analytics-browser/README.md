# @temps-sdk/analytics-browser

Vanilla JS analytics SDK for Temps. Use it programmatically or as a drop-in script tag.

## Installation

```bash
npm install @temps-sdk/analytics-browser
```

## Programmatic

```ts
import { init, trackEvent } from "@temps-sdk/analytics-browser";

init({
  basePath: "/api/_temps",
  domain: "example.com",
  enableSessionRecording: false,
});

// Later in your app:
trackEvent("signup", { plan: "pro" });
```

After `init()`, a global `window.temps` handle is exposed for convenience.

## Script tag (zero config)

```html
<script
  defer
  src="https://unpkg.com/@temps-sdk/analytics-browser/dist/auto.js"
  data-domain="example.com"
  data-base-path="/api/_temps"
></script>
```

All `data-*` attributes on the script tag map to `AnalyticsOptions` (kebab-case → camelCase).

### Attributes

| Attribute | Type | Notes |
|---|---|---|
| `data-domain` | string | Overrides detected hostname |
| `data-base-path` | string | Defaults to `/api/_temps` |
| `data-disabled` | `"true"` \| `"false"` | Kill switch |
| `data-ignore-localhost` | `"true"` \| `"false"` | Default: true |
| `data-auto-track-pageviews` | `"true"` \| `"false"` | Default: true |
| `data-auto-track-page-leave` | `"true"` \| `"false"` | Default: true |
| `data-auto-track-speed-analytics` | `"true"` \| `"false"` | Default: true |
| `data-auto-track-engagement` | `"true"` \| `"false"` | Default: true |
| `data-heartbeat-interval` | number (ms) | Default: 30000 |
| `data-session-recording` | `"true"` \| `"false"` | Default: false |
| `data-session-sample-rate` | 0.0 - 1.0 | Default: 1.0 |
| `data-excluded-paths` | csv | e.g. `/admin/*,/settings/*` |

## HTML attributes for declarative events

```html
<button temps-event-name="cta_click" temps-data-section="hero">Get started</button>
```

Any element with `temps-event-name` fires an event on click. All `temps-data-*`
attributes are sent as event properties.

## API

```ts
import { init, getAnalytics, trackEvent, trackPageview, destroy } from "@temps-sdk/analytics-browser";
import { createAnalytics } from "@temps-sdk/analytics-browser"; // re-exported from core
```

`init()` replaces any previous instance (useful for hot reload); `destroy()` tears it down.
