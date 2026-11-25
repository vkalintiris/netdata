"""Create the guessing game's teams on the lab's LiteLLM proxy, each with a budget and one key, and write the keys.

  setup_keys.py    reads LITELLM_MASTER_KEY from <worktree>/.local/gateways/litellm.env, writes
                   <worktree>/.local/gateways/litellm-keys.json ({team: key}); safe to run again.

The teams are guess.py's TEAMS, so the gateway's per-team numbers line up with the game's own `team` attribute.
"""

import json
import os
import subprocess
import urllib.request

PROXY = os.environ.get("LITELLM_PROXY", "http://127.0.0.1:14000")
# Monthly budgets in USD, uneven so the budget charts have something to compare.
BUDGETS = {"search": 5.0, "support": 2.0, "growth": 0.5}


def root():
    here = os.path.dirname(os.path.abspath(__file__))
    return subprocess.check_output(["git", "-C", here, "rev-parse", "--show-toplevel"], text=True).strip()


def master_key(env_file):
    with open(env_file) as f:
        for line in f:
            if line.startswith("LITELLM_MASTER_KEY="):
                return line.split("=", 1)[1].strip()
    raise SystemExit(f"LITELLM_MASTER_KEY missing in {env_file}")


def call(key, method, path, body=None):
    request = urllib.request.Request(
        PROXY + path,
        method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def main():
    local = os.path.join(root(), ".local", "gateways")
    key = master_key(os.path.join(local, "litellm.env"))
    out = os.path.join(local, "litellm-keys.json")
    keys = {}
    if os.path.exists(out):
        with open(out) as f:
            keys = json.load(f)
    teams = {team["team_alias"]: team["team_id"] for team in call(key, "GET", "/team/list")}
    for alias, budget in BUDGETS.items():
        if alias not in teams:
            team = call(key, "POST", "/team/new",
                        {"team_alias": alias, "max_budget": budget, "budget_duration": "30d"})
            teams[alias] = team["team_id"]
        if alias not in keys:
            generated = call(key, "POST", "/key/generate",
                             {"team_id": teams[alias], "key_alias": f"guess-{alias}"})
            keys[alias] = generated["key"]
    old = os.umask(0o077)
    try:
        with open(out, "w") as f:
            json.dump(keys, f)
    finally:
        os.umask(old)
    print(f"teams: {', '.join(sorted(teams))}; keys written to {out}")


if __name__ == "__main__":
    main()
