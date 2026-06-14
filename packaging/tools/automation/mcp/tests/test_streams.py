import asyncio

from netdata_mcp import streams


# ── command builders (pure) ─────────────────────────────────────────────────────
def test_synth_cmd_basics():
    cmd = streams.synth_cmd(
        "127.0.0.1:4317", count=25, field_cardinality=4, spacing_nanos=1_000_000_000,
        start_time_nanos=None, seed=0, tenant_id=None, batch_size=100,
        flush_interval_ms=300, connect_timeout_secs=30,
    )
    assert cmd[:7] == ["cargo", "run", "--quiet", "-p", "otel-streams", "--bin", "synth"]
    assert "--otel-endpoint" in cmd and "http://127.0.0.1:4317" in cmd
    assert cmd[cmd.index("--count") + 1] == "25"
    # omitted optionals stay out
    assert "--start-time-nanos" not in cmd and "--tenant-id" not in cmd


def test_synth_cmd_includes_set_optionals():
    cmd = streams.synth_cmd(
        "h:1", count=1, field_cardinality=1, spacing_nanos=0, start_time_nanos=123,
        seed=5, tenant_id="t1", batch_size=1, flush_interval_ms=1, connect_timeout_secs=1,
    )
    assert cmd[cmd.index("--start-time-nanos") + 1] == "123"
    assert cmd[cmd.index("--tenant-id") + 1] == "t1"
    assert cmd[cmd.index("--seed") + 1] == "5"


def test_stream_cmd_certstream_url_flag():
    cmd = streams.stream_cmd(
        "certstream", "h:1", url="ws://x/", collections=None, start=None, rate=None,
        tenant_id=None, batch_size=100, flush_interval_ms=1000,
    )
    assert cmd[:7] == ["cargo", "run", "--quiet", "-p", "otel-streams", "--bin", "certstream"]
    assert "--certstream-url" in cmd and "ws://x/" in cmd
    assert "--jetstream-url" not in cmd


def test_stream_cmd_jetstream_url_and_collections():
    cmd = streams.stream_cmd(
        "jetstream", "h:1", url="wss://y/", collections="app.bsky.feed.post", start=None,
        rate=None, tenant_id="t", batch_size=50, flush_interval_ms=500,
    )
    assert "--jetstream-url" in cmd and "wss://y/" in cmd
    assert cmd[cmd.index("--collections") + 1] == "app.bsky.feed.post"
    assert cmd[cmd.index("--tenant-id") + 1] == "t"


def test_stream_cmd_github_start_and_rate():
    cmd = streams.stream_cmd(
        "github", "h:1", url=None, collections=None, start="2024-06-01-12", rate=0,
        tenant_id=None, batch_size=100, flush_interval_ms=1000,
    )
    assert cmd[cmd.index("--start") + 1] == "2024-06-01-12"
    assert cmd[cmd.index("--rate") + 1] == "0"  # rate=0 (unlimited) must be emitted


# ── StreamRegistry lifecycle (real subprocess, harmless commands) ────────────────
def _crates(tmp_path):
    d = tmp_path / "src" / "crates"
    d.mkdir(parents=True)
    return str(tmp_path)


def test_registry_start_then_stop(tmp_path):
    async def run():
        reg = streams.StreamRegistry()
        wt = _crates(tmp_path)
        s = reg.start("ag", wt, "127.0.0.1:1", "certstream", ["sleep", "30"])
        assert s.state == "running" and reg.get(s.stream_id) is s
        await asyncio.sleep(0.2)  # let it actually spawn
        stopped = await reg.stop(s.stream_id, wait=3.0)
        assert stopped is not None and stopped.state == "stopped"
        assert stopped.returncode is not None and stopped.returncode < 0  # killed by signal

    asyncio.run(run())


def test_registry_marks_unexpected_exit_failed(tmp_path):
    async def run():
        reg = streams.StreamRegistry()
        wt = _crates(tmp_path)
        s = reg.start("ag", wt, "127.0.0.1:1", "github", ["sh", "-c", "exit 0"])
        await s._task  # daemon exits on its own → failure (we didn't stop it)
        assert s.state == "failed" and s.returncode == 0 and "exited" in (s.error or "")

    asyncio.run(run())


def test_registry_stop_unknown_returns_none():
    async def run():
        reg = streams.StreamRegistry()
        assert await reg.stop("nope") is None

    asyncio.run(run())
