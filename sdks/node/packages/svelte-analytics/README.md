# @temps-sdk/svelte-analytics

Temps analytics for Svelte 4 and 5 — stores, actions, and helpers.

## Installation

```bash
npm install @temps-sdk/svelte-analytics
```

## Quick Start

```ts
// src/routes/+layout.ts (or src/main.ts)
import { initTempsAnalytics } from "@temps-sdk/svelte-analytics";

initTempsAnalytics({
  basePath: "/api/_temps",
  enableSessionRecording: false,
});
```

## Track events

```svelte
<script>
  import { getTempsAnalytics } from "@temps-sdk/svelte-analytics";
  const analytics = getTempsAnalytics();
</script>

<button on:click={() => analytics.trackEvent("cta_click", { variant: "blue" })}>
  Track me
</button>

<!-- or declaratively -->
<button temps-event-name="cta_click" temps-data-section="hero">Get started</button>
```

## Track visibility (Svelte action)

```svelte
<script>
  import { trackVisibility } from "@temps-sdk/svelte-analytics";
</script>

<section use:trackVisibility={{ eventName: "pricing_viewed", threshold: 0.75 }}>
  Pricing Plans
</section>
```

## Engagement store

```svelte
<script>
  import { engagementStore } from "@temps-sdk/svelte-analytics";
  const engagement = engagementStore({ heartbeatInterval: 15000 });
</script>

<p>Engaged: {$engagement.engagement_time_seconds}s</p>
```

## Session recording

```svelte
<script>
  import { sessionRecordingStore } from "@temps-sdk/svelte-analytics";
  const recording = sessionRecordingStore(false);
</script>

<button on:click={recording.toggle}>
  Recording: {$recording ? "ON" : "OFF"}
</button>
```

## API parity

| React | Svelte |
|---|---|
| `<TempsAnalyticsProvider>` | `initTempsAnalytics(options)` at app root |
| `useTempsAnalytics()` | `getTempsAnalytics()` / `analyticsStore` |
| `useTrackEvent()` | `getTempsAnalytics().trackEvent(...)` |
| `useScrollVisibility()` | `use:trackVisibility={{ ... }}` |
| `useEngagementTracking()` | `engagementStore({ ... })` |
| `useSessionRecordingControl()` | `sessionRecordingStore()` |
