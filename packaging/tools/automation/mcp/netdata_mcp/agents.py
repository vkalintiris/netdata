"""Agent registry (transport-free: no MCP imports).

Maps an LLM-supplied ``agent-id`` to its spec ``{worktree, profile, ...}``. The
agent-id is the user-facing handle; the build behind it is the worktree's single
``build/`` (the profile sets its build type). Runtime fields (port, run job) are
attached by the run layer once it launches.

In-memory only (does not survive a server restart).
"""

from __future__ import annotations

from dataclasses import dataclass

from . import buildcfg, runtime


@dataclass
class AgentSpec:
    agent_id: str
    worktree: str
    profile: str


class AgentRegistry:
    def __init__(self) -> None:
        self._agents: dict[str, AgentSpec] = {}

    def declare(self, agent_id: str, worktree: str, profile: str) -> AgentSpec:
        """Register (or idempotently update) an agent. Validates id and profile."""
        runtime.sanitize_agent_id(agent_id)
        buildcfg.validate_profile(profile)
        existing = self._agents.get(agent_id)
        if existing is not None:
            existing.worktree = worktree
            existing.profile = profile
            return existing
        spec = AgentSpec(agent_id=agent_id, worktree=worktree, profile=profile)
        self._agents[agent_id] = spec
        return spec

    def get(self, agent_id: str) -> AgentSpec | None:
        return self._agents.get(agent_id)
