"""Agent registry (transport-free: no MCP imports).

Maps an LLM-supplied ``agent-id`` to its spec ``{worktree, profile, ...}``. The
agent-id is the user-facing handle; the build behind it is the worktree's single
``build/`` (the profile sets its build type). Runtime fields (port, run job) are
attached by the run layer once it launches; a declared ``port`` pins the web port
the run layer uses instead of an auto-assigned one.

In-memory only (does not survive a server restart).
"""

from __future__ import annotations

from dataclasses import dataclass, field

from . import buildcfg, runtime
from .runtime import OtelConfig

DB_MODES = ("ram", "dbengine")


@dataclass
class AgentSpec:
    agent_id: str
    worktree: str
    profile: str
    # Web port pinned at declare time; None -> a fresh free port per launch.
    # A stable port keeps browser URLs (and a local UI's ?agent=) valid across
    # restarts.
    port: int | None = None
    # netdata's [db] mode: "ram" keeps about an hour of metrics in memory; "dbengine" keeps them on disk under the
    # run dir, so they survive restarts and cover longer windows.
    db_mode: str = "ram"
    # otel-plugin tuning applied at the next launch/restart (set via
    # netdata_agent_otel_config). Preserved across idempotent re-declare.
    otel: OtelConfig = field(default_factory=OtelConfig)


class AgentRegistry:
    def __init__(self) -> None:
        self._agents: dict[str, AgentSpec] = {}

    def declare(
        self, agent_id: str, worktree: str, profile: str, port: int | None = None, db_mode: str = "ram"
    ) -> AgentSpec:
        """Register (or idempotently update) an agent. Validates id, profile, port and db mode.

        Like worktree and profile, ``port`` and ``db_mode`` are replaced on every
        declare: omitting them returns the agent to auto-assigned ports and a ram db.
        """
        runtime.sanitize_agent_id(agent_id)
        runtime.check_runtime_socket_paths(agent_id)
        buildcfg.validate_profile(profile)
        if port is not None and not 1 <= port <= 65535:
            raise ValueError(f"port must be within 1-65535, got {port}")
        if db_mode not in DB_MODES:
            raise ValueError(f"db_mode must be one of {', '.join(DB_MODES)}, got {db_mode!r}")
        existing = self._agents.get(agent_id)
        if existing is not None:
            existing.worktree = worktree
            existing.profile = profile
            existing.port = port
            existing.db_mode = db_mode
            return existing
        spec = AgentSpec(agent_id=agent_id, worktree=worktree, profile=profile, port=port, db_mode=db_mode)
        self._agents[agent_id] = spec
        return spec

    def get(self, agent_id: str) -> AgentSpec | None:
        return self._agents.get(agent_id)

    def set_otel(self, agent_id: str, otel: OtelConfig) -> AgentSpec | None:
        """Replace the agent's otel config; applied at the next launch/restart.

        Returns the updated spec, or None if the agent isn't declared.
        """
        spec = self._agents.get(agent_id)
        if spec is None:
            return None
        spec.otel = otel
        return spec
