import click
import asyncio
import sys
import dagger

from ndag.core.platform import SUPPORTED_PLATFORMS
from ndag.core.distribution import (Distribution, SUPPORTED_DISTRIBUTIONS)
from ndag.core import distro_container

def run_async(func):
    def wrapper(*args, **kwargs):
        return asyncio.run(func(*args, **kwargs))

    return wrapper


@run_async
async def simple_install(platform, distribution):
    config = dagger.Config(log_output=sys.stdout)

    async with dagger.Connection(config) as client:
        ctr = distro_container(client, platform, distribution)
        await ctr


@click.command()
@click.option(
    "--platform",
    "-p",
    type=click.Choice(sorted([str(p) for p in SUPPORTED_PLATFORMS])),
    default="linux/x86_64",
    help="Specify the platform.",
)
@click.option(
    "--distribution",
    "-d",
    type=click.Choice(sorted([str(p) for p in SUPPORTED_DISTRIBUTIONS])),
    default="debian12",
    help="Specify the distribution.",
)
def install_netdata(platform, distribution):
    simple_install(platform, distribution)
