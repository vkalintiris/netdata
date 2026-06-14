import asyncio

from netdata_mcp import agentfn
from netdata_mcp.tools.otel_logs import build_payload


def _payload(**kw):
    base = dict(
        info=False, after=None, before=None, query=None, selections=None,
        facets=None, histogram=None, direction=None, last=None, anchor=None, tenant=None,
    )
    base.update(kw)
    return build_payload(**base)


def test_build_payload_omits_unset_fields():
    assert _payload() == {}  # nothing set → function applies its own defaults


def test_build_payload_info_flag():
    assert _payload(info=True) == {"info": True}


def test_build_payload_includes_only_set_fields():
    p = _payload(
        after=1000, before=2000, query="error",
        selections={"level": ["error", "warn"]}, facets=["level", "host"],
        histogram="level", direction="forward", last=50, tenant="t1",
    )
    assert p == {
        "after": 1000, "before": 2000, "query": "error",
        "histogram": "level", "direction": "forward", "last": 50, "tenant": "t1",
        "selections": {"level": ["error", "warn"]}, "facets": ["level", "host"],
    }


def test_build_payload_skips_empty_selections_and_facets():
    # empty containers are falsy → omitted (an empty selections must not mean
    # "match nothing"; absence lets the function default apply)
    assert _payload(selections={}, facets=[]) == {}


def test_function_url():
    url = agentfn.function_url("http://127.0.0.1:19999", "otel-logs", 60)
    assert url == "http://127.0.0.1:19999/api/v3/function?function=otel-logs&timeout=60"
    # trailing slash on base is handled
    assert agentfn.function_url("http://127.0.0.1:19999/", "otel-logs", 30).startswith(
        "http://127.0.0.1:19999/api/v3/function?"
    )


class _FakeResp:
    status = 200

    def read(self):
        return b'{"ok": true}'

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def _capture_request(monkeypatch):
    """Patch the agentfn opener to capture the urllib.Request it sends."""
    captured = {}

    def fake_open(req, timeout=None):
        captured["headers"] = dict(req.header_items())
        return _FakeResp()

    monkeypatch.setattr(agentfn._LOCAL_OPENER, "open", fake_open)
    return captured


def test_call_function_sends_bearer_when_given(monkeypatch):
    cap = _capture_request(monkeypatch)
    status, data, err = asyncio.run(
        agentfn.call_function("http://127.0.0.1:1", "otel-logs", {"info": True}, bearer="BEARER-xyz")
    )
    assert status == 200 and err is None
    # urllib title-cases header keys
    assert cap["headers"].get("Authorization") == "Bearer BEARER-xyz"


def test_call_function_anonymous_has_no_authorization(monkeypatch):
    cap = _capture_request(monkeypatch)
    asyncio.run(agentfn.call_function("http://127.0.0.1:1", "otel-logs", {"info": True}))
    assert "Authorization" not in cap["headers"]
