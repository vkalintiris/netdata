#!/usr/bin/env python3

import click

from .build_distribution import build_distribution
from .install_netdata import install_netdata


@click.group()
def cli():
    pass


cli.add_command(build_distribution)
cli.add_command(install_netdata)


def main():
    cli()
