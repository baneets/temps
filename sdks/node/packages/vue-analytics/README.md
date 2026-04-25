# @temps-sdk/vue-analytics

Temps analytics for Vue 3.

## Installation

```bash
npm install @temps-sdk/vue-analytics
```

## Quick Start

```ts
// main.ts
import { createApp } from "vue";
import { TempsAnalyticsPlugin } from "@temps-sdk/vue-analytics";
import App from "./App.vue";

const app = createApp(App);
app.use(TempsAnalyticsPlugin, {
  basePath: "/api/_temps",
  enableSessionRecording: true,
});
app.mount("#app");
```

## Composables

```vue
<script setup>
import {
  useTempsAnalytics,
  useTrackEvent,
  usePageLeave,
  useEngagementTracking,
  useScrollVisibility,
  useSessionRecording,
} from "@temps-sdk/vue-analytics";

const analytics = useTempsAnalytics();
const track = useTrackEvent();

const handleClick = () => track("cta_click", { variant: "blue" });

const { engagementData } = useEngagementTracking({ heartbeatInterval: 15000 });
const { triggerPageLeave } = usePageLeave();
const pricingRef = useScrollVisibility({ eventName: "pricing_viewed" });
const { isEnabled, toggle } = useSessionRecording();
</script>

<template>
  <button @click="handleClick">Track me</button>
  <section ref="pricingRef">Pricing</section>
</template>
```

## Declarative events

```html
<button temps-event-name="cta_click" temps-data-section="hero">Get started</button>
```

Any element with `temps-event-name` fires an event on click.

## API parity

Same surface as `@temps-sdk/react-analytics`, mapped to Vue 3 composables:

| React | Vue |
|---|---|
| `<TempsAnalyticsProvider>` | `app.use(TempsAnalyticsPlugin, options)` |
| `useTempsAnalytics()` | `useTempsAnalytics()` |
| `useTrackEvent()` | `useTrackEvent()` |
| `useTrackPageview()` | `useTrackPageview()` |
| `usePageLeave()` | `usePageLeave()` |
| `useEngagementTracking()` | `useEngagementTracking()` |
| `useScrollVisibility()` | `useScrollVisibility()` |
| `useSessionRecordingControl()` | `useSessionRecording()` |
