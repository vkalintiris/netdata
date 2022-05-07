#!/usr/bin/env python

import os
import sys
import json
import shutil
import subprocess
import time
import uuid

from string import Template

NETDATA_CONFIG_TEMPLATE = """\
[global]
    hostname = $hostname
    config directory = $config_directory
    log directory = $log_directory
    cache directory = $cache_directory
    lib directory = $lib_directory
    errors flood protection period = 9999
    errors to trigger flood protection = 999999


[web]
    default port = $default_port
"""

STREAM_CONFIG_TEMPLATE = """\
[stream]
    enabled = yes
    destination = tcp:localhost
    api key = 00000000-0000-0000-0000-000000000000
    timeout seconds = 60
    default port = 19999
    send charts matching = *
    buffer size bytes = 1048576
    reconnect delay seconds = 5
    initial clock resync iterations = 60
"""

class Agent:
    def __init__(self, uid):
        self.uid = uid

        self.hostname = f"nd{self.uid}"
        self.netdata_config_file = f"{self.hostname}.conf"

        cwd = os.getcwd()
        self.config_directory = os.path.join(cwd, f"{self.hostname}/etc/netdata")
        self.log_directory = os.path.join(cwd, f"{self.hostname}/var/log/netdata")
        self.cache_directory = os.path.join(cwd, f"{self.hostname}/var/cache/netdata")
        self.lib_directory = os.path.join(cwd, f"{self.hostname}/var/lib/netdata")
        self.default_port = 20000 + uid

        self.stream_config_file = os.path.join(self.config_directory, f"stream.conf")
        self.registry_id_file = os.path.join(self.lib_directory, 'registry', 'netdata.public.unique.id')

        self.p = None

    def setup(self):
        try:
            shutil.rmtree(self.hostname)
        except:
            pass

        t = Template(NETDATA_CONFIG_TEMPLATE).substitute({
            "hostname": self.hostname,
            "config_directory": self.config_directory,
            "log_directory": self.log_directory,
            "cache_directory": self.cache_directory,
            "lib_directory": self.lib_directory,
            "default_port": self.default_port,
        })
        with open(self.netdata_config_file, "w") as fp:
            fp.write(t)

        os.makedirs(self.config_directory, exist_ok=True)
        os.makedirs(self.log_directory, exist_ok=True)
        os.makedirs(self.cache_directory, exist_ok=True)
        os.makedirs(self.lib_directory, exist_ok=True)
        os.makedirs(os.path.dirname(self.registry_id_file), exist_ok=True)

        with open(self.stream_config_file, "w") as fp:
            fp.write(STREAM_CONFIG_TEMPLATE)

        with open(self.registry_id_file, "w") as fp:
            fp.write(str(uuid.uuid3(uuid.NAMESPACE_DNS, self.hostname)))

    def start(self):
        self.p = subprocess.Popen([
            '/home/vk/opt/gaps/netdata/usr/sbin/netdata',
            '-D',
            '-c', self.netdata_config_file
        ])

    def kill(self):
        self.p.kill()

agents = []
for i in range(1):
    agent = Agent(i)

    agent.setup()

    print(f"Starting agent {i}")
    agent.start()

    agents.append(agent)
    time.sleep(1)

time.sleep(99999)

for agent in agents:
    agent.kill()
