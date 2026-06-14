import pytest

from netdata_mcp.agents import AgentRegistry


def test_declare_registers_and_get_returns_it():
    reg = AgentRegistry()
    spec = reg.declare("parent", "/wt", "optimized")
    assert spec.agent_id == "parent"
    assert spec.worktree == "/wt"
    assert spec.profile == "optimized"
    assert reg.get("parent") is spec


def test_declare_is_idempotent_and_updates_spec():
    reg = AgentRegistry()
    first = reg.declare("a", "/wt", "debug")
    again = reg.declare("a", "/wt2", "optimized")
    assert again is first  # same object, addressed by id
    assert again.worktree == "/wt2"
    assert again.profile == "optimized"


def test_declare_rejects_unknown_profile():
    reg = AgentRegistry()
    with pytest.raises(ValueError):
        reg.declare("a", "/wt", "nonexistent")


def test_declare_rejects_unsafe_agent_id():
    reg = AgentRegistry()
    with pytest.raises(ValueError):
        reg.declare("../x", "/wt", "debug")


def test_get_unknown_returns_none():
    assert AgentRegistry().get("nope") is None
