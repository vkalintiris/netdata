"""The guessing game shared by every player: the secret, the tool that checks a guess, the prompts and the endpoint.

Players differ only in the library or framework that drives the model and records the telemetry.
"""

import os
import random

BASE_URL = os.environ.get("GUESS_BASE_URL", "https://llm.netdata.cloud/v1")
MODELS = os.environ.get("GUESS_MODELS", "deepseek-v4-flash,qwen3.8-flash").split(",")
MAX_TURNS = 20

# Agent players: the model calls the tool until it hears "correct"; the framework runs the loop.
AGENT_SYSTEM = "You play a guessing game. Use the check_guess tool, one guess per call, until it answers 'correct'."
AGENT_PROMPT = (
    "I picked a whole number between 1 and 100. Find it with check_guess; it answers 'higher', 'lower' or "
    "'correct'. When it says 'correct', reply with the number only."
)

# Chat players: no tools; the application answers each guess in the next user message.
CHAT_SYSTEM = (
    "We play a game. I picked a whole number between 1 and 100. Guess it. After each guess I say 'higher' or "
    "'lower'. Answer with the number only."
)

_secret = 0
_guesses = 0


def new_game():
    global _secret, _guesses
    _secret = random.randint(1, 100)
    _guesses = 0
    return _secret


def guesses():
    return _guesses


def check_guess(number: int) -> str:
    """Check a guess for the secret number between 1 and 100. Returns 'higher', 'lower' or 'correct'."""
    global _guesses
    _guesses += 1
    number = int(number)
    if number == _secret:
        return "correct"
    return "higher" if number < _secret else "lower"


def pick_model():
    return random.choice(MODELS)


def api_key():
    return os.environ["LITELLM_API_KEY"]


def setup_sdk():
    """Global tracer and logger providers exporting OTLP/HTTP to the OTEL_EXPORTER_OTLP_* endpoint.

    Returns a function that flushes and shuts them down.
    """
    from opentelemetry import trace
    from opentelemetry._logs import set_logger_provider
    from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
    from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
    from opentelemetry.sdk._logs import LoggerProvider
    from opentelemetry.sdk._logs.export import BatchLogRecordProcessor
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor

    res = Resource.create()
    tp = TracerProvider(resource=res)
    tp.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
    trace.set_tracer_provider(tp)
    # Some instrumentations send the message content as log events.
    lp = LoggerProvider(resource=res)
    lp.add_log_record_processor(BatchLogRecordProcessor(OTLPLogExporter()))
    set_logger_provider(lp)

    def shutdown():
        for p in (tp, lp):
            p.force_flush()
            p.shutdown()
    return shutdown
