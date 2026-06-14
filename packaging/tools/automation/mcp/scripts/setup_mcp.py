#!/usr/bin/env python3
"""One-off setup: wire the netdata-build MCP server into a local agent client.

Configures GLOBAL (user-scope) config for THIS checkout:
  - opencode:    safe, idempotent merge into ~/.config/opencode/opencode.json
  - Claude Code: `claude mcp add --scope user` (the CLI owns ~/.claude.json)

It mutates the USER's config, never the repo. Because global config carries an
absolute path to this checkout, re-run it after switching your primary worktree.

Launched agents are auto-claimed to Netdata Cloud, so this wires the claim
credentials into the client's per-server env. The token is REQUIRED: pass
--claim-token or set NETDATA_CLAIM_TOKEN (rooms/url optional). The token is
written into the user-global client config (and briefly on the `claude` argv) —
that is the intended cost of pinning it per-server.

stdlib-only on purpose: it must run on a fresh clone before `uv sync`.

Usage:
    python3 setup_mcp.py [--tool opencode|claude|all] [--source-dir <repo-root>]
        [--claim-token T] [--claim-rooms R] [--claim-url U]
Or via the build:
    ninja setup-mcp
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

SERVER_NAME = "netdata-build"
MCP_SUBDIR = "packaging/tools/automation/mcp"
OPENCODE_CONFIG = Path.home() / ".config" / "opencode" / "opencode.json"


def _say(msg: str) -> None:
    print(msg, flush=True)


def _action(cmd_desc: str) -> None:
    # Transparency: show the mutation about to happen, like a shell `run()` echo.
    print(f"  $ {cmd_desc}", flush=True)


def _redact(text: str) -> str:
    """Mask NETDATA_CLAIM_*=<value> so a forwarded CLI error can't leak the token."""
    return re.sub(r"(NETDATA_CLAIM_[A-Z]+=)\S+", r"\1***", text)


def tool_dir(source_dir: Path) -> Path:
    """Absolute path to the MCP tool within a checkout."""
    return (source_dir / MCP_SUBDIR).resolve()


def _git_root() -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True
    )
    if out.returncode != 0:
        raise SystemExit("not in a git repo; pass --source-dir <repo-root>")
    return Path(out.stdout.strip())


# ── claim credentials ─────────────────────────────────────────────────────────
def resolve_claim_creds(args: argparse.Namespace, environ: dict) -> dict[str, str]:
    """Resolve NETDATA_CLAIM_* for injection: CLI flag wins, else the env var.

    Claiming is mandatory, so the token MUST resolve via one of them — fail clearly
    otherwise. Rooms/url are optional and omitted when unset. Blank values count as
    unset (matching the server's own claim_env handling).
    """
    def pick(cli_val: str | None, env_key: str) -> str:
        return ((cli_val if cli_val is not None else environ.get(env_key)) or "").strip()

    claim_token = pick(args.claim_token, "NETDATA_CLAIM_TOKEN")
    if not claim_token:
        raise SystemExit(
            "claim token required: pass --claim-token or set NETDATA_CLAIM_TOKEN. "
            "Agents launched by this server are auto-claimed to Netdata Cloud."
        )
    creds = {"NETDATA_CLAIM_TOKEN": claim_token}
    rooms = pick(args.claim_rooms, "NETDATA_CLAIM_ROOMS")
    if rooms:
        creds["NETDATA_CLAIM_ROOMS"] = rooms
    url = pick(args.claim_url, "NETDATA_CLAIM_URL")
    if url:
        creds["NETDATA_CLAIM_URL"] = url
    return creds


# ── opencode ────────────────────────────────────────────────────────────────
def opencode_entry(td: Path, claim: dict[str, str]) -> dict:
    """The opencode `mcp.<name>` entry: a local stdio server rooted at this checkout.

    Claim creds go in `environment` (opencode's per-server env map) so the launched
    netdata claims to Cloud; they are not on the command line.
    """
    return {
        "type": "local",
        "command": ["uv", "run", "netdata-build-mcp", "--transport", "stdio"],
        "cwd": str(td),
        "environment": dict(claim),
        "enabled": True,
    }


def merge_opencode_config(config: dict, td: Path, claim: dict[str, str]) -> dict:
    """Return `config` with only `mcp.netdata-build` set; every other key untouched."""
    out = dict(config)
    mcp = dict(out.get("mcp") or {})
    mcp[SERVER_NAME] = opencode_entry(td, claim)
    out["mcp"] = mcp
    return out


def setup_opencode(td: Path, claim: dict[str, str], cfg_path: Path = OPENCODE_CONFIG) -> None:
    if cfg_path.exists():
        try:
            config = json.loads(cfg_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise RuntimeError(
                f"{cfg_path} is not valid JSON ({exc.msg} at line {exc.lineno}); "
                "fix it and re-run"
            ) from exc
    else:
        config = {}
    updated = merge_opencode_config(config, td, claim)
    if updated == config:
        _say(f"opencode: '{SERVER_NAME}' already current in {cfg_path}")
        return
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    _action(f"write {cfg_path} (mcp.{SERVER_NAME})")
    cfg_path.write_text(json.dumps(updated, indent=2) + "\n", encoding="utf-8")
    _say(f"opencode: configured '{SERVER_NAME}' -> {td}")


# ── Claude Code ─────────────────────────────────────────────────────────────
def claude_command(td: Path, claim: dict[str, str]) -> list[str]:
    """`claude mcp add` argv: user scope, stdio, rooted at this checkout via --directory.

    Claim creds are passed as `--env KEY=VALUE` (before the `--` server separator).
    NOTE: these values are on the `claude` argv, so they are briefly visible in `ps`
    while this command runs — the accepted cost of pinning creds per-server.
    """
    # `claude mcp add --env <env...>` is variadic, so it greedily eats the next
    # positional. Put the server name BEFORE a single --env, and terminate the
    # KEY=val list with `--` (which also separates the server command).
    cmd = ["claude", "mcp", "add", "--scope", "user", SERVER_NAME]
    if claim:
        cmd.append("--env")
        cmd += [f"{key}={value}" for key, value in claim.items()]
    cmd += ["--", "uv", "run", "--directory", str(td), "netdata-build-mcp", "--transport", "stdio"]
    return cmd


def setup_claude(td: Path, claim: dict[str, str]) -> None:
    if shutil.which("claude") is None:
        _say("claude: CLI not on PATH — skipping (install Claude Code to use it).")
        return
    # Idempotent: drop any existing user-scope entry first (ignore if absent),
    # since `claude mcp add` errors on a duplicate name.
    _action(f"claude mcp remove --scope user {SERVER_NAME}")
    subprocess.run(
        ["claude", "mcp", "remove", "--scope", "user", SERVER_NAME],
        capture_output=True, text=True,
    )
    cmd = claude_command(td, claim)
    # Don't echo the resolved --env values (they carry the token); show a redacted form.
    _action("claude mcp add --scope user [--env NETDATA_CLAIM_*=***] " + SERVER_NAME + " -- ...")
    res = subprocess.run(cmd, capture_output=True, text=True)
    if res.returncode != 0:
        # Scrub the token: claude's stderr may echo the --env values on failure.
        raise RuntimeError(_redact(res.stderr.strip()) or "claude mcp add failed")
    _say(f"claude: configured '{SERVER_NAME}' (user scope) -> {td}")


# ── entry ───────────────────────────────────────────────────────────────────
def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        prog="setup_mcp.py",
        description="Wire the netdata-build MCP server into opencode/Claude Code (global config).",
    )
    p.add_argument("--tool", choices=("opencode", "claude", "all"), default="all")
    p.add_argument("--source-dir", help="repo root (default: `git rev-parse --show-toplevel`)")
    p.add_argument("--claim-token", help="Netdata Cloud claim token (else NETDATA_CLAIM_TOKEN; required)")
    p.add_argument("--claim-rooms", help="Cloud room id(s) (else NETDATA_CLAIM_ROOMS; optional)")
    p.add_argument("--claim-url", help="Cloud base URL (else NETDATA_CLAIM_URL; optional)")
    return p.parse_args(argv)


def main(argv: list[str] | None = None, environ: dict | None = None) -> int:
    args = parse_args(argv)
    source_dir = Path(args.source_dir).resolve() if args.source_dir else _git_root()
    td = tool_dir(source_dir)
    if not td.is_dir():
        _say(f"error: MCP tool dir not found: {td}")
        return 2

    # Resolve claim creds up front: missing token fails fast, before wiring anything.
    claim = resolve_claim_creds(args, os.environ if environ is None else environ)

    tools = ["opencode", "claude"] if args.tool == "all" else [args.tool]
    errors: list[str] = []
    for t in tools:
        try:
            setup_opencode(td, claim) if t == "opencode" else setup_claude(td, claim)
        except Exception as exc:  # one tool's failure must not abort the others
            errors.append(f"{t}: {exc}")

    if errors:
        for e in errors:
            _say(f"FAILED {e}")
        return 1
    _say("Done. Restart your agent client to pick up the server.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
