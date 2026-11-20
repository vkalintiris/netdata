"""Agent frameworks: the model calls game.check_guess as a tool and the framework runs the loop.

One setup and one game function per framework; the config picks the instrumentation. No application span is added,
so a trace holds only what the framework and its instrumentation emit.
"""

import asyncio
import os
import uuid

import game

# Frameworks stop an agent after a number of steps; binary search needs at most 7 guesses, models wander more.
STEPS = 25


def _openai_async():
    from openai import AsyncOpenAI
    return AsyncOpenAI(base_url=game.BASE_URL, api_key=game.api_key(), max_retries=2)


# --- Pydantic AI (native: Agent.instrument_all) ---

def setup_pydantic_ai(config):
    shutdown = game.setup_sdk()
    from pydantic_ai import Agent
    Agent.instrument_all()
    return shutdown


def play_pydantic_ai(model):
    from pydantic_ai import Agent
    from pydantic_ai.models.openai import OpenAIChatModel
    from pydantic_ai.providers.openai import OpenAIProvider
    m = OpenAIChatModel(model, provider=OpenAIProvider(openai_client=_openai_async()))
    agent = Agent(m, system_prompt=game.AGENT_SYSTEM, tools=[game.check_guess])
    agent.run_sync(game.AGENT_PROMPT)


# --- OpenAI Agents SDK (OpenInference) ---

def setup_openai_agents(config):
    from agents import set_trace_processors
    # Drop the default exporter to OpenAI's trace ingest.
    set_trace_processors([])
    shutdown = game.setup_sdk()
    from openinference.instrumentation.openai_agents import OpenAIAgentsInstrumentor
    OpenAIAgentsInstrumentor().instrument()
    return shutdown


def play_openai_agents(model):
    from agents import Agent, OpenAIChatCompletionsModel, Runner, function_tool
    agent = Agent(name="guesser", instructions=game.AGENT_SYSTEM, tools=[function_tool(game.check_guess)],
                  model=OpenAIChatCompletionsModel(model=model, openai_client=_openai_async()))
    asyncio.run(Runner.run(agent, game.AGENT_PROMPT, max_turns=STEPS))


# --- LangGraph through LangChain's create_agent (OpenInference) ---

def setup_langgraph(config):
    shutdown = game.setup_sdk()
    from openinference.instrumentation.langchain import LangChainInstrumentor
    LangChainInstrumentor().instrument()
    return shutdown


def play_langgraph(model):
    from langchain.agents import create_agent
    from langchain_openai import ChatOpenAI
    llm = ChatOpenAI(model=model, base_url=game.BASE_URL, api_key=game.api_key(), max_retries=2)
    agent = create_agent(llm, [game.check_guess], system_prompt=game.AGENT_SYSTEM)
    # Each guess takes two graph steps (model, tool).
    agent.invoke({"messages": [{"role": "user", "content": game.AGENT_PROMPT}]}, {"recursion_limit": 2 * STEPS})


# --- LlamaIndex FunctionAgent (OpenInference) ---

def setup_llama_index(config):
    shutdown = game.setup_sdk()
    from openinference.instrumentation.llama_index import LlamaIndexInstrumentor
    LlamaIndexInstrumentor().instrument()
    return shutdown


def play_llama_index(model):
    from llama_index.core.agent.workflow import FunctionAgent
    from llama_index.llms.openai_like import OpenAILike
    llm = OpenAILike(model=model, api_base=game.BASE_URL, api_key=game.api_key(), is_chat_model=True,
                     is_function_calling_model=True, max_retries=2)
    agent = FunctionAgent(tools=[game.check_guess], llm=llm, system_prompt=game.AGENT_SYSTEM)

    async def go():
        await agent.run(game.AGENT_PROMPT, max_iterations=STEPS)
    asyncio.run(go())


# --- CrewAI (OpenInference) ---

def setup_crewai(config):
    # Nothing may go to CrewAI's servers. Not OTEL_SDK_DISABLED: it would also switch off our exporter.
    os.environ.update({"CREWAI_DISABLE_TELEMETRY": "true", "CREWAI_DISABLE_TRACKING": "true",
                       "CREWAI_TRACING_ENABLED": "false"})
    shutdown = game.setup_sdk()
    from openinference.instrumentation.crewai import CrewAIInstrumentor
    CrewAIInstrumentor().instrument()
    return shutdown


def play_crewai(model):
    from crewai import LLM, Agent, Crew, Task
    from crewai.tools import tool

    @tool("check_guess")
    def check(number: int) -> str:
        """Check a guess for the secret number between 1 and 100. Returns 'higher', 'lower' or 'correct'."""
        return game.check_guess(number)

    llm = LLM(model="openai/" + model, base_url=game.BASE_URL, api_key=game.api_key())
    agent = Agent(role="Guesser", goal="Find the secret number", backstory=game.AGENT_SYSTEM, tools=[check], llm=llm,
                  verbose=False, max_iter=STEPS)
    task = Task(description=game.AGENT_PROMPT, expected_output="The secret number", agent=agent)
    Crew(agents=[agent], tasks=[task], verbose=False).kickoff()


# --- Strands Agents (native) ---

def setup_strands(config):
    # Strands' tracer reads the global provider on first use, so the SDK comes first.
    return game.setup_sdk()


def play_strands(model):
    from strands import Agent, tool
    from strands.models.openai import OpenAIModel
    m = OpenAIModel(client_args={"base_url": game.BASE_URL, "api_key": game.api_key(), "max_retries": 2},
                    model_id=model)
    Agent(model=m, system_prompt=game.AGENT_SYSTEM, tools=[tool(game.check_guess)], callback_handler=None)(
        game.AGENT_PROMPT)


# --- Google ADK through LiteLLM (native) ---

def setup_google_adk(config):
    return game.setup_sdk()


def play_google_adk(model):
    from google.adk.agents import Agent
    from google.adk.models.lite_llm import LiteLlm
    from google.adk.runners import Runner
    from google.adk.sessions import InMemorySessionService
    from google.genai import types

    agent = Agent(name="guesser", instruction=game.AGENT_SYSTEM, tools=[game.check_guess],
                  model=LiteLlm(model="openai/" + model, api_base=game.BASE_URL, api_key=game.api_key()))
    svc = InMemorySessionService()
    runner = Runner(app_name="guess", agent=agent, session_service=svc)
    session = uuid.uuid4().hex

    async def go():
        await svc.create_session(app_name="guess", user_id="player", session_id=session)
        msg = types.Content(role="user", parts=[types.Part(text=game.AGENT_PROMPT)])
        async for _ in runner.run_async(user_id="player", session_id=session, new_message=msg):
            pass
    asyncio.run(go())


# --- Agno (OpenInference) ---

def setup_agno(config):
    shutdown = game.setup_sdk()
    from openinference.instrumentation.agno import AgnoInstrumentor
    AgnoInstrumentor().instrument()
    return shutdown


def play_agno(model):
    from agno.agent import Agent
    from agno.models.openai.like import OpenAILike
    m = OpenAILike(id=model, base_url=game.BASE_URL, api_key=game.api_key(), client_params={"max_retries": 2})
    Agent(model=m, tools=[game.check_guess], instructions=game.AGENT_SYSTEM, tool_call_limit=STEPS).run(
        game.AGENT_PROMPT)


# --- DSPy ReAct (OpenInference) ---

def setup_dspy(config):
    shutdown = game.setup_sdk()
    from openinference.instrumentation.dspy import DSPyInstrumentor
    DSPyInstrumentor().instrument()
    return shutdown


def play_dspy(model):
    import dspy
    # cache=False: every game makes real requests; model_type="chat": LiteLLM may not serve the Responses API.
    lm = dspy.LM("openai/" + model, api_base=game.BASE_URL, api_key=game.api_key(), cache=False, model_type="chat")
    with dspy.context(lm=lm):
        dspy.ReAct("question -> answer", tools=[dspy.Tool(game.check_guess)], max_iters=STEPS)(
            question=game.AGENT_SYSTEM + " " + game.AGENT_PROMPT)
