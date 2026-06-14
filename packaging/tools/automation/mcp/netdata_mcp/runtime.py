"""Runtime environment for launched agents (transport-free: no MCP imports).

Owns where a run instance's isolated, writable state lives, agent-id validation,
free-port selection, per-agent ``netdata.conf`` generation, the launch command
line, and the HTTP readiness probe.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import socket
import urllib.request
from collections.abc import Mapping
from pathlib import Path

from . import profiles

# Cloud claim credentials, read from the environment and injected into a launched
# agent's process env (never its command line). Only a non-empty token enables
# claiming; rooms/url are optional (url defaults to app.netdata.cloud agent-side).
_CLAIM_TOKEN = "NETDATA_CLAIM_TOKEN"
_CLAIM_OPTIONAL = ("NETDATA_CLAIM_ROOMS", "NETDATA_CLAIM_URL")

# agent-id becomes a filesystem path component (run/<agent-id>), so it is
# strictly validated: 1-64 chars, starts alphanumeric, then [A-Za-z0-9_-].
# This rejects "..", "/", empty, and leading "-"/"_".
_AGENT_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$")


def sanitize_agent_id(agent_id: str) -> str:
    """Return ``agent_id`` if path-safe, else raise ValueError."""
    if not _AGENT_ID_RE.match(agent_id):
        raise ValueError(
            f"Invalid agent id {agent_id!r}: use 1-64 chars of [A-Za-z0-9_-], "
            "starting with a letter or digit."
        )
    return agent_id


def run_dir(agent_id: str) -> Path:
    """Per-agent isolated runtime dir; validated id keeps it a single component."""
    return Path.home() / "opt" / "netdata-mcp" / "run" / sanitize_agent_id(agent_id)


def free_port() -> int:
    """An OS-assigned free loopback TCP port.

    The kernel never hands out a port already in use, so this cannot collide
    with a running instance. There is a small TOCTOU window before netdata
    binds it; a bind failure surfaces as a `failed` run. Recovery is not
    automatic — the caller (LLM/human) starts the agent again, which picks a
    fresh port.
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]
    finally:
        sock.close()


def install_bin(worktree: str) -> Path:
    """Path to the installed netdata binary for a worktree (one install per worktree)."""
    return Path(profiles.install_prefix(worktree)) / "usr" / "sbin" / "netdata"


def _default_conf(agent_id: str, rd: Path) -> dict[str, dict[str, str]]:
    """Minimal ephemeral test-agent config: isolated dirs, ram db, loopback.

    ``hostname`` is the agent's Cloud display name — unique and stable per
    agent_id so distinct agents are distinct cloud nodes and a restart reuses the
    same one. ``is ephemeral node`` lets Cloud auto-clean the node once it goes
    offline, so stopped dev agents don't accumulate.
    """
    return {
        "global": {
            "hostname": f"mcp-{agent_id}",
            "is ephemeral node": "yes",
        },
        "db": {"mode": "ram"},
        "directories": {
            "cache": str(rd / "cache"),
            "lib": str(rd / "lib"),
            "log": str(rd / "log"),
        },
        "web": {"bind to": "127.0.0.1"},
    }


def _ini_safe(s: str) -> str:
    # Strip newlines (the section/key injection vector) and null bytes; defensive
    # ahead of exposing overrides to callers. ';', '#', '=' are intentionally NOT
    # stripped — they are valid in netdata.conf values.
    return str(s).replace("\x00", "").replace("\r", " ").replace("\n", " ")


def _render_ini(conf: dict[str, dict[str, str]]) -> str:
    lines: list[str] = []
    for section, kv in conf.items():
        lines.append(f"[{_ini_safe(section)}]")
        for key, value in kv.items():
            lines.append(f"    {_ini_safe(key)} = {_ini_safe(value)}")
        lines.append("")
    return "\n".join(lines)


def generate_runtime(agent_id: str, overrides: dict[str, dict[str, str]] | None = None) -> tuple[Path, Path]:
    """Create the isolated run dir + write netdata.conf; return (run_dir, conf_path).

    ``overrides`` is the per-agent extension point ({section: {key: value}}),
    deep-merged over the defaults — the hook for future runtime overrides
    (db mode, plugin toggles, log target, ...). Not exposed via a tool yet.
    """
    rd = run_dir(agent_id)
    for sub in ("etc", "cache", "lib", "log"):
        (rd / sub).mkdir(parents=True, exist_ok=True)
    conf = _default_conf(agent_id, rd)
    for section, kv in (overrides or {}).items():
        conf.setdefault(section, {}).update(kv)
    conf_path = rd / "etc" / "netdata.conf"
    conf_path.write_text(_render_ini(conf), encoding="utf-8")
    return rd, conf_path


def launch_command(netdata_bin: Path, port: int, conf_path: Path) -> list[str]:
    return [str(netdata_bin), "-D", "-p", str(port), "-c", str(conf_path)]


def claim_env(environ: Mapping[str, str] | None = None) -> dict[str, str]:
    """Cloud claim credentials to inject into a launched agent's env, or ``{}``.

    Read from ``environ`` (default ``os.environ``). Returns an empty dict — meaning
    "launch unclaimed" — unless a non-empty ``NETDATA_CLAIM_TOKEN`` is present.
    Rooms/url are included only when set. The caller passes the result as the
    launch ``env`` so credentials stay off the command line.
    """
    src = os.environ if environ is None else environ
    token = (src.get(_CLAIM_TOKEN) or "").strip()
    if not token:
        return {}
    out = {_CLAIM_TOKEN: token}
    for key in _CLAIM_OPTIONAL:
        value = (src.get(key) or "").strip()
        if value:
            out[key] = value
    return out


# A loopback-only opener: never route the agent probe through HTTP(S)_PROXY (which
# would both break the probe and leak the agent's /api/v1/info off-host).
_LOCAL_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _get_info(port: int, timeout: float) -> dict | None:
    """Fetch /api/v1/info and return the parsed JSON object, or None on any failure."""
    try:
        with _LOCAL_OPENER.open(f"http://127.0.0.1:{port}/api/v1/info", timeout=timeout) as resp:
            if resp.status != 200:
                return None
            data = json.loads(resp.read())
            return data if isinstance(data, dict) else None
    except Exception:
        return None


def _probe_once(port: int, timeout: float) -> bool:
    return _get_info(port, timeout) is not None


async def probe_ready(port: int, timeout: float = 2.0) -> bool:
    """True once the agent answers /api/v1/info with valid JSON on ``port``."""
    return await asyncio.to_thread(_probe_once, port, timeout)


def _as_bool(value: object) -> bool | None:
    # the /api/v1/info fields are booleans; coerce anything else to None rather than
    # let a stray type flow into the bool|None model fields.
    return value if isinstance(value, bool) else None


def _cloud_status_once(port: int, timeout: float) -> tuple[bool | None, bool | None]:
    d = _get_info(port, timeout)
    if d is None:
        return (None, None)
    return (_as_bool(d.get("agent-claimed")), _as_bool(d.get("aclk-available")))


async def cloud_status(port: int, timeout: float = 2.0) -> tuple[bool | None, bool | None]:
    """``(claimed, cloud_connected)`` from the agent's /api/v1/info, or ``(None, None)``.

    ``claimed`` is whether the agent has a claimed_id (set at startup);
    ``cloud_connected`` is whether ACLK is online (the node is live in the Cloud
    UI). Best-effort and never raises — this is *reported, never waited on* (D6):
    a fresh poll typically shows ``claimed=True`` before ``cloud_connected`` flips
    true a few seconds later.
    """
    return await asyncio.to_thread(_cloud_status_once, port, timeout)
