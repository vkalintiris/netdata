"""Parent/child streaming smoke test.

Builds the agent once, then runs a parent and a child as services: the
child streams to the parent over the API key, and a client container
asserts the parent sees both hosts and that bearer-token endpoints stay
protected (HTTP 451). Modernized from the 2024 prototype's test command;
the stream.conf content is native data, not templates on disk.
"""

from __future__ import annotations

import uuid

import dagger

from . import build as build_mod
from .matrix import get_distro

_DISTRO = ("debian", "12")
_PARENT_PORT = 22000

_PARENT_CONF = """\
[{api_key}]
    enabled = yes
    allow from = *
    default history = 3600
    health enabled by default = auto
    default postpone alarms on connect seconds = 60
    multiple connections = allow
"""

_CHILD_CONF = """\
[stream]
    enabled = yes
    destination = parent:{port}
    api key = {api_key}
    timeout seconds = 60
    default port = 19999
    send charts matching = *
    buffer size bytes = 1048576
    reconnect delay seconds = 5
    initial clock resync iterations = 60
"""

_CLIENT_CHECK = f"""
set -e
get_hosts() {{
  sed -n 's/.*"hosts-available":[[:space:]]*\\([0-9]*\\).*/\\1/p' /tmp/info.json
}}
for i in $(seq 1 90); do
  if curl -fsS "http://parent:{_PARENT_PORT}/api/v1/info" > /tmp/info.json 2>/dev/null; then
    [ "$(get_hosts)" = "2" ] && break
  fi
  sleep 2
done
hosts="$(get_hosts)"
if [ "$hosts" != "2" ]; then
  echo "FAIL: parent sees $hosts hosts, expected 2"; exit 1
fi
for ep in bearer_protection bearer_get_token; do
  code="$(curl -s -o /dev/null -w '%{{http_code}}' "http://parent:{_PARENT_PORT}/api/v2/$ep")"
  if [ "$code" != "451" ]; then
    echo "FAIL: /api/v2/$ep returned $code, expected 451"; exit 1
  fi
done
echo "stream-test-ok hosts=2 bearer-protected"
"""


def stream_test(source: dagger.Directory, jobs: int = 0) -> dagger.Container:
    """Parent/child streaming + bearer-protection assertions."""
    d = get_distro(*_DISTRO)
    api_key = str(uuid.uuid5(uuid.NAMESPACE_DNS, "netdata-ci-stream-test"))

    agent = build_mod.source_build(d, "linux/amd64", source, jobs)
    netdata = "/opt/netdata/usr/sbin/netdata"

    parent = (
        agent.with_new_file(
            "/opt/netdata/etc/netdata/stream.conf", _PARENT_CONF.format(api_key=api_key)
        )
        .with_exposed_port(_PARENT_PORT)
        .as_service(
            args=[netdata, "-D", "-i", "0.0.0.0", "-p", str(_PARENT_PORT)],
            use_entrypoint=False,
        )
    )

    child = (
        agent.with_new_file(
            "/opt/netdata/etc/netdata/stream.conf",
            _CHILD_CONF.format(port=_PARENT_PORT, api_key=api_key),
        )
        .with_service_binding("parent", parent)
        .with_exposed_port(19999)
        .as_service(
            args=[netdata, "-D", "-i", "0.0.0.0", "-p", "19999"],
            use_entrypoint=False,
        )
    )

    client = (
        agent.with_service_binding("parent", parent)
        .with_service_binding("child", child)
        .with_exec(["sh", "-c", _CLIENT_CHECK])
    )
    return client
