#!/usr/bin/env python3

import click

from .rfs import rfs


@click.group()
def cli():
    pass


cli.add_command(rfs)


def main():
    cli()
