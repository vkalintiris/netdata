# Documented divergences from Loki

Cases where nlogql's parser accepts input that Loki rejects, or
rejects input that Loki accepts. Each entry lists one form, the
direction of divergence, and a one-line reason. The compliance
suite in `src/parser.rs` references this file when it sees
unexpected behaviour.

## Parser-lenient but lower-strict

Both of these were originally listed as parser-only leniencies that
deferred validation to a downstream pass. **As of `nlogql-eval`
SOW-D3 (commit landing this update) both are rejected at lower
time**, so any consumer that goes through `nlogql_eval::lower()`
gets a clean semantic error. The parser itself stays permissive —
which is the right shape, because the parser deals in syntax and
the lowering layer deals in semantics.

### `sum(N, expr)` and other non-`topk`/`bottomk`/`approx_topk` vector ops with a numeric first argument

Example: `sum(3, count_over_time({foo="bar"}[5h]))`

- **Loki**: rejected at parse.
- **nlogql parser**: accepted (parses as `parameter = Some(3.0)`).
- **`nlogql_eval::lower`**: rejected with
  `LowerError::UnexpectedParameter { op: "sum" }`.

### `topk(expr)` without the count argument

Example: `topk(rate({foo="bar"}[5m]))`

- **Loki**: rejected at parse.
- **nlogql parser**: accepted (parses as `parameter = None`).
- **`nlogql_eval::lower`**: rejected with
  `LowerError::MissingParameter { op: "topk" }`.

## Currently strict (we reject, Loki accepts)

*(none today — list grows as the compliance corpus uncovers gaps.)*

## Out of scope for parser

- **`variants(...) of (...)`** — Loki experimental syntax; not
  implemented per the implementation plan.
- **PromQL-style subqueries** (`[10m:1m]`) — Loki has no such
  syntax; see the dropped SOW-14 in the implementation plan.
