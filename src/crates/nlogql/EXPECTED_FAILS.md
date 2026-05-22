# Documented divergences from Loki

Cases where nlogql's parser accepts input that Loki rejects, or
rejects input that Loki accepts. Each entry lists one form, the
direction of divergence, and a one-line reason. The compliance
suite in `src/parser.rs` references this file when it sees
unexpected behaviour.

## Currently lenient (we accept, Loki rejects)

### `sum(N, expr)` and other non-`topk`/`bottomk`/`approx_topk` vector ops with a numeric first argument

Example: `sum(3, count_over_time({foo="bar"}[5h]))`

- **Loki**: rejected — `sum` is a 1-arg vector op.
- **nlogql**: accepted — `vector_aggregation_expr_inner` parses the
  2-arg form for any `VectorOp`, deferring the op-specific
  arity check to a later semantic pass.
- **Why we're lenient**: parse-time arity enforcement would require
  per-op branching that duplicates information already captured
  in the AST (`parameter: Option<f64>`). The downstream evaluator
  (Phase D+) is a natural place for this check.
- **When to tighten**: alongside the IR-lowering stage that turns
  `VectorAggregationExpr` into evaluator plans. If a `parameter`
  is `Some` and `op` is not in `{TopK, BottomK, ApproxTopK}`,
  surface a semantic error there.

### `topk(expr)` without the count argument

Example: `topk(rate({foo="bar"}[5m]))`

- **Loki**: rejected — `topk` requires `(N, expr)`.
- **nlogql**: accepted (`parameter` is `None`).
- **Resolution**: same as above — semantic check at lowering time.

## Currently strict (we reject, Loki accepts)

*(none today — list grows as the compliance corpus uncovers gaps.)*

## Out of scope for parser

- **`variants(...) of (...)`** — Loki experimental syntax; not
  implemented per the implementation plan.
- **PromQL-style subqueries** (`[10m:1m]`) — Loki has no such
  syntax; see the dropped SOW-14 in the implementation plan.
