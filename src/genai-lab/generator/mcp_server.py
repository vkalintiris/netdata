"""The guessing game as MCP tools, for a game version whose agent reaches its tools through a gateway.

  python mcp_server.py [port]    streamable HTTP on 127.0.0.1:<port>/mcp (default 3200); needs the `mcp` package (2.x)

`new_game` picks a secret and returns its id; `check_guess` answers one guess for a game id. Secrets live here, not
in the player, because the player's agent reaches this server only through the gateway.
"""

import random
import sys
import uuid

from mcp.server.mcpserver import MCPServer

app = MCPServer(name="guessing-game", instructions="Tools of a number-guessing game between 1 and 100.")
# game id -> secret; a game the player abandons stays until the process restarts, a few bytes each.
_secrets: dict[str, int] = {}


@app.tool()
def new_game() -> str:
    """Start a game: pick a whole number between 1 and 100. Returns the game id that check_guess needs."""
    game_id = uuid.uuid4().hex[:12]
    _secrets[game_id] = random.randint(1, 100)
    return game_id


@app.tool()
def check_guess(game_id: str, number: int) -> str:
    """Check a guess for the secret number of game `game_id`. Returns 'higher', 'lower' or 'correct'."""
    secret = _secrets.get(game_id)
    if secret is None:
        return "unknown game"
    if int(number) == secret:
        del _secrets[game_id]
        return "correct"
    return "higher" if int(number) < secret else "lower"


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 3200
    app.run(transport="streamable-http", host="127.0.0.1", port=port, streamable_http_path="/mcp")
