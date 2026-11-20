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

- Overview: tiles, trends, latency and breakdowns from the `otel.derived_metrics.genai.*` charts. Drag on a chart
  to zoom the whole page, Shift + drag to pan, double-click to return to the last hour. The window and filters live
  in the URL.
- Calls: model-call spans from the `otel-traces` Function, a chart by requested model and a table; a row opens
  the call with its conversation, a summary and the trace.
