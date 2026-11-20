"""Real GenAI traffic for the lab: an LLM guesses a number between 1 and 100, told "higher" or "lower" each turn.

Each game is one trace: a parent span around the chat calls the OpenAI instrumentation records. Every game picks one
of MODELS, so the breakdowns have more than one row. The pauses cap the spend while nobody watches.

Env: LITELLM_API_KEY (required), GUESS_BASE_URL, GUESS_MODELS (comma-separated), GUESS_TURN_PAUSE, GUESS_GAME_PAUSE,
plus the standard OTEL_* variables for the exporter, service name and content capture.
"""

import os
import random
import re
import time

import openai
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.instrumentation.openai_v2 import OpenAIInstrumentor
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.trace import Status, StatusCode

BASE_URL = os.environ.get("GUESS_BASE_URL", "https://llm.netdata.cloud/v1")
MODELS = os.environ.get("GUESS_MODELS", "deepseek-v4-flash,qwen3.8-flash").split(",")
TURN_PAUSE = float(os.environ.get("GUESS_TURN_PAUSE", "3"))
GAME_PAUSE = float(os.environ.get("GUESS_GAME_PAUSE", "20"))
MAX_TURNS = 20

SYSTEM = (
    "We play a game. I picked a whole number between 1 and 100. Guess it. After each guess I say 'higher' or "
    "'lower'. Answer with the number only."
)


def play(client, tracer, model):
    secret = random.randint(1, 100)
    messages = [{"role": "system", "content": SYSTEM}, {"role": "user", "content": "Your first guess?"}]
    with tracer.start_as_current_span("guess-the-number") as game:
        game.set_attribute("game.model", model)
        game.set_attribute("game.secret", secret)
        for turn in range(1, MAX_TURNS + 1):
            reply = client.chat.completions.create(model=model, messages=messages, max_tokens=1500)
            text = reply.choices[0].message.content or ""
            messages.append({"role": "assistant", "content": text})
            found = re.search(r"\d+", text)
            guess = int(found.group()) if found else None
            if guess == secret:
                game.set_attribute("game.turns", turn)
                return turn
            if guess is None:
                answer = "Please answer with a single number."
            else:
                answer = "higher" if guess < secret else "lower"
            messages.append({"role": "user", "content": answer})
            time.sleep(TURN_PAUSE)
        game.set_attribute("game.turns", MAX_TURNS)
        game.set_status(Status(StatusCode.ERROR, "no guess within the turn limit"))
        return None


def main():
    provider = TracerProvider(resource=Resource.create())
    provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
    trace.set_tracer_provider(provider)
    OpenAIInstrumentor().instrument()
    tracer = trace.get_tracer("genai-lab.guess")
    client = openai.OpenAI(base_url=BASE_URL, api_key=os.environ["LITELLM_API_KEY"], max_retries=2)
    while True:
        model = random.choice(MODELS)
        try:
            turns = play(client, tracer, model)
            print(f"{model}: {'found in ' + str(turns) if turns else 'gave up'}", flush=True)
        except openai.OpenAIError as exc:
            # The failed call is already on its span; keep the loop going.
            print(f"{model}: {type(exc).__name__}: {exc}", flush=True)
        time.sleep(GAME_PAUSE)


if __name__ == "__main__":
    main()
