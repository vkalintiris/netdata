"""Real GenAI traffic for the lab: an LLM guesses a number between 1 and 100, one version per library or framework.

  guess.py list                     the versions
  guess.py <version> [--games N]    play with one version (forever by default), service name guess-<version>
  guess.py rotate [<version> ...]   play one game per version in turn (all by default), each in its own venv, forever

Every game picks one of game.MODELS. Pauses cap the spend while nobody watches; `rotate` plays one game at a time.
Env: LITELLM_API_KEY (required), GUESS_VENVS (the root of the venvs below), GUESS_GAME_PAUSE, GUESS_TURN_PAUSE,
GUESS_MODELS, GUESS_BASE_URL, plus OTEL_EXPORTER_OTLP_ENDPOINT.
"""

import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path[:0] = [HERE, os.path.join(HERE, "players")]

# version: (venv under GUESS_VENVS, player module, config)
VERSIONS = {
    "otel-genai": ("libs/venvs/otel-genai", "chat", "otel-genai"),
    "otel-v2": ("libs/venvs/otel-v2", "chat", "otel-v2"),
    "openinference": ("libs/venvs/openinference", "chat", "openinference"),
    "openllmetry": ("libs/venvs/openllmetry", "chat", "openllmetry"),
    "openlit": ("libs/venvs/openlit", "chat", "openlit"),
    "logfire": ("libs/venvs/logfire", "chat", "logfire"),
    "pydantic-ai": ("frameworks/venvs/pydantic-ai", "frameworks", "pydantic_ai"),
    "openai-agents": ("frameworks/venvs/openai-agents", "frameworks", "openai_agents"),
    "langgraph": ("frameworks/venvs/langchain", "frameworks", "langgraph"),
    "llama-index": ("frameworks/venvs/llama-index", "frameworks", "llama_index"),
    "crewai": ("frameworks/venvs/crewai", "frameworks", "crewai"),
    "strands": ("frameworks/venvs/strands", "frameworks", "strands"),
    "google-adk": ("frameworks/venvs/google-adk", "frameworks", "google_adk"),
    "agno": ("frameworks/venvs/agno", "frameworks", "agno"),
    "dspy": ("frameworks/venvs/dspy", "frameworks", "dspy"),
}

GAME_PAUSE = float(os.environ.get("GUESS_GAME_PAUSE", "20"))


def play(version, games):
    _, module, config = VERSIONS[version]
    os.environ["OTEL_SERVICE_NAME"] = "guess-" + version
    # Message content on the spans, for the instrumentations that read these.
    os.environ.setdefault("OTEL_SEMCONV_STABILITY_OPT_IN", "gen_ai_latest_experimental")
    os.environ.setdefault("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "SPAN_ONLY")
    import importlib

    import game
    player = importlib.import_module(module)
    if module == "chat":
        setup, run = player.setup, player.play
    else:
        setup, run = getattr(player, "setup_" + config), getattr(player, "play_" + config)
    shutdown = setup(config)
    n = 0
    try:
        while games is None or n < games:
            n += 1
            model = game.pick_model()
            game.new_game()
            try:
                run(model)
                print(f"{version} {model}: {game.guesses()} guesses", flush=True)
            except Exception as exc:
                # The failure is on the spans already; keep playing.
                print(f"{version} {model}: {type(exc).__name__}: {str(exc)[:200]}", flush=True)
            if games is None or n < games:
                time.sleep(GAME_PAUSE)
    finally:
        shutdown()


def python_of(version):
    return os.path.join(os.environ["GUESS_VENVS"], VERSIONS[version][0], "bin", "python")


def rotate(versions):
    while True:
        for version in versions:
            python = python_of(version)
            try:
                subprocess.run([python, "-I", os.path.abspath(__file__), version, "--games", "1"], timeout=600)
            except subprocess.TimeoutExpired:
                print(f"{version}: killed after 600 s", flush=True)
            time.sleep(GAME_PAUSE)


def main(argv):
    if not argv or argv[0] == "list":
        print("\n".join(VERSIONS))
    elif argv[0] == "rotate":
        rotate(argv[1:] or list(VERSIONS))
    elif argv[0] in VERSIONS:
        games = int(argv[2]) if len(argv) > 2 and argv[1] == "--games" else None
        play(argv[0], games)
    else:
        raise SystemExit(f"unknown version {argv[0]!r}; see 'guess.py list'")


if __name__ == "__main__":
    main(sys.argv[1:])
