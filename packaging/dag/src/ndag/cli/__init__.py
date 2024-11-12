#!/usr/bin/env python3

import click

from .build_distribution import build_distribution
from ndag.config import cmake


@click.group()
def cli():
    pass


cli.add_command(build_distribution)


def main():
    cfg = cmake.CMakeConfig()
    print('cmake ' + ' '.join(cfg.args()))
    # cli()
