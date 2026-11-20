"""Instrumentation libraries on the plain OpenAI client: the application runs the loop and answers each guess.

Each game is wrapped in one application span, so its calls share a trace.
"""

import os
import re
import time

import game

TURN_PAUSE = float(os.environ.get("GUESS_TURN_PAUSE", "3"))


def setup(config):
    service = os.environ["OTEL_SERVICE_NAME"]
    endpoint = os.environ["OTEL_EXPORTER_OTLP_ENDPOINT"]
    if config == "otel-genai":
        shutdown = game.setup_sdk()
        from opentelemetry.instrumentation.genai.openai import OpenAIInstrumentor
        OpenAIInstrumentor().instrument()
        return shutdown
    if config == "otel-v2":
        shutdown = game.setup_sdk()
        from opentelemetry.instrumentation.openai_v2 import OpenAIInstrumentor
        OpenAIInstrumentor().instrument()
        return shutdown
    if config == "openinference":
        shutdown = game.setup_sdk()
        from openinference.instrumentation.openai import OpenAIInstrumentor
        OpenAIInstrumentor().instrument()
        return shutdown
    # The rest install their own tracer provider; flushing it is enough at exit.
    if config == "openllmetry":
        from traceloop.sdk import Traceloop
        from traceloop.sdk.instruments import Instruments
        Traceloop.init(app_name=service, api_endpoint=endpoint, telemetry_enabled=False,
                       instruments={Instruments.OPENAI})
    elif config == "openlit":
        import openlit
        openlit.init(otlp_endpoint=endpoint, application_name=service, service_name=service, environment="lab")
    elif config == "logfire":
        import logfire
        logfire.configure(send_to_logfire=False, service_name=service, console=False)
        logfire.instrument_openai()
    else:
        raise SystemExit(f"unknown chat config {config!r}")
    from opentelemetry import trace
    return lambda: trace.get_tracer_provider().force_flush()


def play(model):
    import openai
    from opentelemetry import trace
    from opentelemetry.trace import Status, StatusCode

    client = openai.OpenAI(base_url=game.BASE_URL, api_key=game.api_key(), max_retries=2)
    tracer = trace.get_tracer("genai-lab.guess")
    messages = [{"role": "system", "content": game.CHAT_SYSTEM}, {"role": "user", "content": "Your first guess?"}]
    with tracer.start_as_current_span("guess-the-number") as span:
        span.set_attribute("game.model", model)
        for _ in range(game.MAX_TURNS):
            reply = client.chat.completions.create(model=model, messages=messages, max_tokens=1500)
            text = reply.choices[0].message.content or ""
            messages.append({"role": "assistant", "content": text})
            found = re.search(r"\d+", text)
            answer = game.check_guess(int(found.group())) if found else "Please answer with a single number."
            if answer == "correct":
                span.set_attribute("game.turns", game.guesses())
                return
            messages.append({"role": "user", "content": answer})
            time.sleep(TURN_PAUSE)
        span.set_status(Status(StatusCode.ERROR, "no guess within the turn limit"))
