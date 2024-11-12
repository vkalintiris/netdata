#!/usr/bin/env python3

import click

from .build_distribution import build_distribution


@click.group()
def cli():
    pass


cli.add_command(build_distribution)


def main():
    cli()
