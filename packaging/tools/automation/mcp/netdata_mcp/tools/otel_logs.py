"""Otel-logs domain: a dedicated, typed tool to query a running agent's
``otel-logs`` function.

Unlike the generic ``netdata_agent_execute_function``, this POSTs an
``OtelLogsRequest`` straight to the agent's ``/api/v3/function?function=otel-logs``
endpoint, exposing every wire param and returning the parsed response so an LLM
can assert on it (matched counts, facets, histogram, rows) — and probe the
function's capabilities with ``info=true``.

Access requirement: ``otel-logs`` declares ``SIGNED_ID | SAME_SPACE |
SENSITIVE_DATA`` (otel-ledger/src/ledger/rpc/handler.rs), so a local, unclaimed,
anonymous agent rejects it with HTTP 412 ("authenticated via Netdata Cloud SSO")
on every transport — there is no localhost bypass. Querying requires a claimed
agent and an ``Authorization: Bearer`` token carrying that access (mint via the
``query-netdata-agents`` machinery). The push side (otel-streams ``synth``) and
``info`` probing do not need this; live query verification does.
"""

from __future__ import annotations

from typing import Annotated, Any, Literal

from mcp.server.fastmcp import Context, FastMCP
from pydantic import BaseModel, Field

from .. import agentfn
from ._common import get_runs

_FUNCTION = "otel-logs"

_AgentId = Annotated[str, Field(description="A ready agent (from netdata_agent_declare + netdata_run_start).")]
_Info = Annotated[bool, Field(description="If true, return the function's capability descriptor (accepted params, help) instead of querying.")]
_After = Annotated[int | None, Field(description="Window start, unix seconds (inclusive). Omit for the function default.")]
_Before = Annotated[int | None, Field(description="Window end, unix seconds (exclusive).")]
_Query = Annotated[str | None, Field(description="Full-text query over key=value pairs (unanchored 'contains').")]
_Selections = Annotated[dict[str, list[str]] | None, Field(description="Per-field value filter: {field: [allowed values]} (OR within a field, AND across fields).")]
_Facets = Annotated[list[str] | None, Field(description="Fields to compute facet (value→count) breakdowns for.")]
_Histogram = Annotated[str | None, Field(description="Field to build the time histogram over.")]
_Direction = Annotated[Literal["forward", "backward"] | None, Field(description="Result order: newest-first (backward) or oldest-first (forward).")]
_Last = Annotated[int | None, Field(description="Max log entries to return.")]
_Anchor = Annotated[str | None, Field(description="Pagination anchor: an opaque row cursor from a prior response (a bare timestamp form also exists for histogram-bar clicks).")]
_Tenant = Annotated[str | None, Field(description="Tenant scoping selector; omitted reads the 'default' tenant.")]
_Timeout = Annotated[int, Field(description="Seconds to wait for the function.", ge=1, le=600)]


class OtelLogsResult(BaseModel):
    """Result of an otel-logs function call. ``response`` is the parsed function
    output — an info descriptor (``info=true``) or a logs result (``items``,
    ``data``, ``columns``, ``facets``, ``available_histograms``, ``histogram``,
    ``pagination``) — so callers can assert on any field."""

    agent_id: str
    endpoint: str | None = None
    http_status: int | None = None
    request: dict[str, Any] | None = Field(default=None, description="The payload POSTed to the function.")
    response: dict[str, Any] | None = Field(default=None, description="Parsed function response.")
    error: str | None = None
    message: str = ""


def build_payload(
    *, info: bool, after: int | None, before: int | None, query: str | None,
    selections: dict[str, list[str]] | None, facets: list[str] | None, histogram: str | None,
    direction: str | None, last: int | None, anchor: str | None, tenant: str | None,
) -> dict[str, Any]:
    """Assemble the otel-logs request body, omitting unset fields so the
    function applies its own defaults (mirrors its `#[serde(default)]` shape)."""
    payload: dict[str, Any] = {}
    if info:
        payload["info"] = True
    for key, value in (
        ("after", after), ("before", before), ("query", query),
        ("histogram", histogram), ("direction", direction), ("last", last),
        ("anchor", anchor), ("tenant", tenant),
    ):
        if value is not None:
            payload[key] = value
    if selections:
        payload["selections"] = selections
    if facets:
        payload["facets"] = facets
    return payload


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="netdata_agent_otel_logs",
        description=(
            "Query a ready agent's otel-logs function with typed parameters, or probe it "
            "with info=true. POSTs directly to /api/v3/function?function=otel-logs and "
            "returns the parsed response (matched items, data rows, facets, histogram, "
            "pagination) for assertion. Pair with netdata_agent_otel_config's small "
            "rotation/retention thresholds + pushed test logs to verify storage edge cases."
        ),
    )
    async def netdata_agent_otel_logs(
        ctx: Context,
        agent_id: _AgentId,
        info: _Info = False,
        after: _After = None,
        before: _Before = None,
        query: _Query = None,
        selections: _Selections = None,
        facets: _Facets = None,
        histogram: _Histogram = None,
        direction: _Direction = None,
        last: _Last = None,
        anchor: _Anchor = None,
        tenant: _Tenant = None,
        timeout: _Timeout = 60,
    ) -> OtelLogsResult:
        run = get_runs(ctx).get(agent_id)
        if run is None or run.state != "ready" or not run.port:
            state = run.state if run is not None else "unknown"
            return OtelLogsResult(
                agent_id=agent_id,
                error=f"Agent {agent_id!r} is not ready (state={state}). "
                "Start it with netdata_run_start and poll netdata_run_status until 'ready'.",
            )
        payload = build_payload(
            info=info, after=after, before=before, query=query, selections=selections,
            facets=facets, histogram=histogram, direction=direction, last=last,
            anchor=anchor, tenant=tenant,
        )
        base = f"http://127.0.0.1:{run.port}"
        status, data, err = await agentfn.call_function(base, _FUNCTION, payload, timeout=timeout)
        endpoint = agentfn.function_url(base, _FUNCTION, timeout)
        if err is not None:
            return OtelLogsResult(agent_id=agent_id, endpoint=endpoint, http_status=status,
                                  request=payload, error=err)
        # A 412 here is the function's access gate, not a malformed request:
        # otel-logs needs SIGNED_ID, which a local unclaimed agent lacks.
        hint = (
            " (otel-logs requires a signed-in identity: claim the agent and pass a "
            "Cloud-minted bearer — a local unclaimed agent always gets 412 here)"
            if status == 412 else ""
        )
        return OtelLogsResult(
            agent_id=agent_id, endpoint=endpoint, http_status=status, request=payload,
            response=data if isinstance(data, dict) else {"value": data},
            message=f"ok{hint}",
        )
