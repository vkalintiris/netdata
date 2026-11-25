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

- The window and the filters (Service, Model, Provider; on Traces also Kind and Outcome) live in the URL and
  carry from tab to tab. Metrics shows them as panels of values with their call counts; Traces as a row of
  buttons that open the same panels.
- Metrics (`/`): tiles, trends, latency and breakdowns from the `otel.derived_metrics.genai.*` charts; the panels count
  from the same charts. Drag a chart to pan, Shift + drag to zoom the whole page, double-click to return to the last
  relative window.
- Breakdown (`/breakdown`): model calls added up per value of one attribute the user picks (any span or resource
  attribute the Function indexes by value): calls, failed calls, tokens in and out, time in calls, as a stacked
  trend and a table. From the `otel-traces` Function's histogram with `sums`. A row opens its calls on Traces.
- Traces (`/traces`): the list to find one trace or one call, from the `otel-traces` Function. Rows are traces
  (the model calls and tool calls the plugin recognised, added up per trace in the browser from the spans loaded,
  200 per request, so the oldest traces listed can miss spans) or calls (one row per model call or tool call; a
  row opens its detail). A cell of the Metrics duration heatmap opens the calls of that time and duration here.
- A trace (`/traces/<id>`): read as one chat. Model calls and tool calls in order; each call shows the messages
  that are new since the call it continues, then its answer, with its tokens and duration. A second tab holds the
  waterfall of every span; a step or a span opens its detail.
