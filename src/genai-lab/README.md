# GenAI lab

A small UI for experimenting with the OTel plugin's GenAI data before the view moves into cloud-frontend.
Not shipped: nothing in the Agent build refers to this directory, and it stays out of netdata PRs.

Stack: Vite, React, TypeScript, Tailwind, shadcn/ui on Radix, TanStack Query, React Router, uPlot.

## Run

```bash
pnpm install
pnpm dev            # http://127.0.0.1:8094/
```

- The dev server forwards `/api` to the Agent at `GENAI_LAB_AGENT` (default `http://127.0.0.1:29995`).
- A claimed Agent answers data queries only with a bearer: put one in `.bearer` (gitignored). It is read per
  request, so a new one needs no restart.

## Pages

- Every page opens with Service, Model and Provider panels: every value with its call count, scrollable, with a find
  box; clicking values selects several, and each panel ignores its own selection so it can be widened. The window
  and filters live in the URL.
- Metrics (`/`): tiles, trends, latency and breakdowns from the `otel.derived_metrics.genai.*` charts; the panels count
  from the same charts. Drag a chart to pan, Shift + drag to zoom the whole page, double-click to return to the last
  relative window.
- Calls: model-call spans from the `otel-traces` Function; the panels count stored spans, plus an Outcome panel
  (this page only). A duration heatmap (a cell lists its calls) and a table; a row opens the call with its
  conversation, a summary and the trace.
- Runs (`/runs`): the same model-call spans, one row per trace: calls and failures, models, providers, wall time
  (first call's start to last call's end), time in calls, tokens summed. A row opens its calls; a call opens its
  detail. Grouped in the browser from the calls loaded (200 per request), so the oldest runs listed can miss calls.
