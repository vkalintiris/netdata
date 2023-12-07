#!/usr/bin/env python3

import asyncio
import click
import os
from pathlib import Path
import sys

import anyio

import dagger
from typing import List


SUPPORTED_PLATFORMS = [
    "linux/x86_64",
    "linux/arm64",
    "linux/i386",
    "linux/arm/v7",
    "linux/arm/v6",
    "linux/ppc64le",
    "linux/s390x",
    "linux/riscv64",
]


SUPPORTED_IMAGES = [
    "amazonlinux2",
    "amazonlinux2023",

    "centos7",
    "centos-stream8",
    "centos-stream9",

    "debian10",
    "debian11",
    "debian12",

    "fedora37",
    "fedora38",
    "fedora39",

    "opensuse15.4",
    "opensuse15.5",
    "opensusetumbleweed",

    "oraclelinux8",
    "oraclelinux9",

    "rockylinux8",
    "rockylinux9",

    "ubuntu20.04",
    "ubuntu22.04",
    "ubuntu23.10",
]


def netdata_installer(enable_ml=True, enable_ebpf=True):
    cmd = [
        "./netdata-installer.sh",
        "--disable-telemetry",
    ]

    if not enable_ebpf:
        cmd.append("--disable-ebpf")

    if not enable_ml:
        cmd.append("--disable-ml")

    cmd.extend([
        "--dont-wait",
        "--dont-start-it",
        "--install-prefix",
        "/opt"
    ])

    return cmd


def build_image_for_platform(client, platform, image):
    repo_path = str(Path(__file__).parent.parent.parent)
    cmake_build_release_path = os.path.join(repo_path, "cmake-build-release")
    tag = platform.replace('/', '_') + '_' + image

    externaldeps_cache = client.cache_volume(f"{tag}-externaldeps")
    fluent_bit_cache = client.cache_volume(f"{tag}-fluent_bit_build")

    source = (
        client.container(platform=dagger.Platform(platform))
        .from_("netdata/package-builders:" + image)
        .with_directory("/netdata", client.host().directory(repo_path), exclude=[f"{cmake_build_release_path}/*"])
        .with_mounted_cache("/netdata/externaldeps", externaldeps_cache)
        .with_mounted_cache("/netdata/fluent-bit/build", fluent_bit_cache)
        .with_env_variable('CFLAGS', '-Wall -Wextra -g -O0')
    )

    enable_ml = False if image.endswith("centos7") else True
    build_task = source.with_workdir("/netdata").with_exec(netdata_installer(enable_ml=enable_ml))

    shell_cmd = "/opt/netdata/usr/sbin/netdata -W buildinfo | tee /opt/netdata/buildinfo.log"
    buildinfo_task = build_task.with_exec(["sh", "-c", shell_cmd])

    build_dir = buildinfo_task.directory('/opt/netdata')
    artifact_dir = os.path.join(Path.home(), f'ci/{tag}-netdata')
    output_task = build_dir.export(artifact_dir)

    return output_task


def build_images(client, platforms: List[str], images: List[str]):
    tasks = []

    for platform in platforms:
        for image in images:
            print(f"Building {platform=}, {image}")
            task = build_image_for_platform(client, platform, image)
            tasks.append(task)

    return tasks


def validate_platforms(ctx, param, value):
    valid_platforms = set(SUPPORTED_PLATFORMS)
    input_platforms = set(value)
    if not input_platforms.issubset(valid_platforms):
        raise click.BadParameter(f"Unsupported platforms: {input_platforms - valid_platforms}")
    return value


def validate_images(ctx, param, value):
    valid_images = set(SUPPORTED_IMAGES)
    input_images = set(value)
    if not input_images.issubset(valid_images):
        raise click.BadParameter(f"Unsupported OCI images: {input_images - valid_images}")
    return value


def help_command():
    msg = """Build the agent with dagger.

    The script supports building the following images:

    {}

    for the following platforms:

    {}
"""


    return msg.format(', '.join(sorted(SUPPORTED_IMAGES)), ', '.join(sorted(SUPPORTED_PLATFORMS)))


def run_async(func):
    """
    Decorator to create an asynchronous runner for the main function.
    """
    def wrapper(*args, **kwargs):
        return asyncio.run(func(*args, **kwargs))
    return wrapper


@click.command(help=help_command())
@click.option(
    "--platforms",
    "-p",
    multiple=True,
    default=["linux/x86_64"],
    show_default=True,
    callback=validate_platforms,
    type=str,
    help='Space separated list of platforms to build for.',
)
@click.option(
    "--images",
    "-i",
    multiple=True,
    default=["debian12"],
    show_default=True,
    callback=validate_images,
    type=str,
    help="Space separated list of images to build.",
)
@click.option(
    "--concurrent",
    "-c",
    is_flag=True,
    default=False,
    show_default=True,
    help="Build the specified images concurrently."
)
@run_async
async def main(platforms, images, concurrent):
    platforms = list(platforms) if platforms else SUPPORTED_PLATFORMS
    images = list(images) if images else SUPPORTED_IMAGES

    config = dagger.Config(log_output=sys.stdout)
    async with dagger.Connection(config) as client:
        tasks = build_images(client, platforms, images)

        if concurrent:
            await asyncio.gather(*tasks)
        else:
            for task in tasks:
                await task


if __name__ == '__main__':
    main()
