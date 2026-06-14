"""Call a netdata *function* over HTTP on a running agent (transport-free: no
MCP-server imports).

netdata serves functions at ``POST /api/v3/function?function=<name>``; the
request body is forwarded verbatim to the plugin as the function payload (see
``api_v1_function`` → ``rrd_function_run(..., w->payload)``). The call is
synchronous — one POST returns the function's JSON result. Localhost agents
allow anonymous access to most functions; access-gated ones (``SIGNED_ID``)
need an ``Authorization: Bearer`` token (see ``bearer.py``), passed via the
optional ``bearer`` argument. The token is sent only as a header — never logged
and never echoed back to the caller.
"""

from __future__ import annotations

import asyncio
import json
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

# Loopback-only opener: never route an agent call through HTTP(S)_PROXY.
_LOCAL_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def function_url(base_url: str, function: str, timeout: int) -> str:
    """The ``/api/v3/function`` URL for ``function`` on ``base_url``."""
    q = urllib.parse.urlencode({"function": function, "timeout": int(timeout)})
    return f"{base_url.rstrip('/')}/api/v3/function?{q}"


def _post(base_url: str, function: str, payload: dict | None, timeout: int, bearer: str | None):
    url = function_url(base_url, function, timeout)
    body = json.dumps(payload or {}).encode("utf-8")
    headers = {"Content-Type": "application/json"}
    if bearer:
        headers["Authorization"] = f"Bearer {bearer}"
    req = urllib.request.Request(url, data=body, method="POST", headers=headers)
    try:
        with _LOCAL_OPENER.open(req, timeout=timeout + 5) as resp:
            status, raw = resp.status, resp.read()
    except urllib.error.HTTPError as exc:  # error responses can still carry a JSON body
        status, raw = exc.code, exc.read()
    except Exception as exc:  # connection refused, timeout, etc.
        return None, None, f"request to {url} failed: {exc!r}"
    if not raw:
        return status, None, None
    try:
        return status, json.loads(raw), None
    except json.JSONDecodeError:
        preview = raw[:200].decode("utf-8", "replace")
        return status, None, f"non-JSON response (status {status}): {preview!r}"


async def call_function(
    base_url: str, function: str, payload: dict[str, Any] | None = None, timeout: int = 60,
    *, bearer: str | None = None,
) -> tuple[int | None, Any, str | None]:
    """POST ``payload`` to ``function`` on ``base_url``; return
    ``(http_status, parsed_json, error)``. When ``bearer`` is given it is sent
    as ``Authorization: Bearer`` (for access-gated functions). Never raises —
    failures come back as the error string so the calling tool returns cleanly."""
    return await asyncio.to_thread(_post, base_url, function, payload, timeout, bearer)
