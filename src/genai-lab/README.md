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
- Spans (`/spans`): the spans the plugin recognised, from the `otel-traces` Function: model calls and tool calls
  (`netdata.genai.kind`), each model call once. The panels count stored spans, plus a Kind and an Outcome panel.
  A duration heatmap (a cell lists its spans) and a table; a row opens the span with its conversation, a summary
  and the trace.
- Traces (`/traces`): the same spans, one row per trace: model calls and tool calls with their failures, models,
  providers, wall time (first span's start to last span's end), time in model calls, tokens summed. A row opens
  its spans in order; a span opens its detail. Grouped in the browser from the spans loaded (200 per request), so
  the oldest traces listed can miss spans.
