import click
import asyncio
import sys

import dagger
import docker

import pathlib

from ndag.core.platform import SUPPORTED_PLATFORMS
from ndag.core import distro_container
from ndag.cmake import install_agent
from ndag import cmake


def run_async(func):
    def wrapper(*args, **kwargs):
        return asyncio.run(func(*args, **kwargs))

    return wrapper


async def dump_cmake_logs(ctr: dagger.Container, build_dir: str) -> None:
    """Dump CMake output logs when configuration fails."""
    try:
        # Try to read CMakeOutput.log
        output_log = await ctr.file(f"{build_dir}/CMakeFiles/CMakeOutput.log").contents()
        print("\n=== CMakeOutput.log ===")
        print(output_log.decode('utf-8'))
    except:
        print("Failed to read CMakeOutput.log")

    try:
        # Try to read CMakeError.log
        error_log = await ctr.file(f"{build_dir}/CMakeFiles/CMakeError.log").contents()
        print("\n=== CMakeError.log ===")
        print(error_log.decode('utf-8'))
    except:
        print("Failed to read CMakeError.log")

@run_async
async def _rfs_impl(platform: str, repo_root: pathlib.Path, cfg: cmake.Config):
    config = dagger.Config(log_output=sys.stderr)

    async with dagger.Connection(config) as client:
        ctr = install_agent(client, platform, repo_root, cfg)

        ctr = (
            client.container()
            .from_("debian:bookworm")
            .with_env_variable("DEBIAN_FRONTEND", "noninteractive")
            .with_exec(["apt-get", "update"])
            .with_exec(["apt-get", "install", "-y", "--no-install-recommends",
                       "ca-certificates", "curl", "systemd", "procps", "lsb-release"])
            .with_exec(["apt-get", "install", "-y", "--no-install-recommends", "libmongoc-1.0-0", "libuv1"])
            .with_exec(["apt-get", "clean"])
            .with_exec(["rm", "-rf", "/var/lib/apt/lists/*"])
            .with_directory("/opt/netdata", ctr.directory(str(cfg.install_prefix)))
        )

        tar_path = "/home/vk/repos/tmp/rfs/netdata-rfs.tar"
        await ctr.export(tar_path)

        docker_client = docker.from_env()

        with open(tar_path, 'rb') as f:
            image = docker_client.images.load(f)[0]

        image.tag("netdata-rfs:latest")
        print(f"Exported to local Docker.")

        image_ref = "10.10.14.1:5000/netdata-rfs:latest"
        image.tag(image_ref)

        for line in docker_client.images.push(image_ref, stream=True, decode=True):
            if 'status' in line:
                print(f"Push status: {line['status']}")
            elif 'error' in line:
                raise Exception(f"Push error: {line['error']}")

        print(f"Published '{image_ref}' to registry.")


@click.command()
@click.option(
    "--repo-path",
    type=click.Path(exists=True, file_okay=False, dir_okay=True, path_type=pathlib.Path),
    required=True,
    help="Path to the Netdata repository",
)
@click.option(
    "--platform",
    "-p",
    type=click.Choice(sorted([str(p) for p in SUPPORTED_PLATFORMS])),
    default="linux/x86_64",
    help="Specify the platform.",
)
@click.option(
    "--source-dir",
    type=str,
    default="/src/netdata",
    help="Netdata source code mount point",
)
@click.option(
    "--build-dir",
    type=str,
    default="/src/netdata/build",
    help="Directory for build artifacts",
)
@click.option(
    "--install-prefix",
    type=str,
    default="/opt/netdata",
    help="Installation prefix directory",
)
@click.option(
    "--build-type",
    type=str,
    default="Debug",
    help="CMake build type",
)
@click.option(
    "--build-shared-libs/--no-build-shared-libs",
    default=None,
    help="Build shared libraries",
)
@click.option(
    "--static-build/--no-static-build",
    default=False,
    help="Enable static build",
)
@click.option(
    "--build-for-packaging/--no-build-for-packaging",
    default=False,
    help="Build for packaging",
)
@click.option(
    "--use-cxx11/--no-use-cxx11",
    default=False,
    help="Use C++11",
)
@click.option(
    "--use-mold/--no-use-mold",
    default=True,
    help="Use mold linker",
)
@click.option(
    "--enable-ml/--no-enable-ml",
    default=True,
    help="Enable machine learning",
)
@click.option(
    "--enable-dbengine/--no-enable-dbengine",
    default=True,
    help="Enable database engine",
)
@click.option(
    "--enable-dashboard/--no-enable-dashboard",
    default=True,
    help="Enable dashboard",
)
@click.option(
    "--enable-plugin-go/--no-enable-plugin-go",
    default=True,
    help="Enable Go plugin",
)
@click.option(
    "--enable-plugin-python/--no-enable-plugin-python",
    default=True,
    help="Enable Python plugin",
)
@click.option(
    "--enable-plugin-apps/--no-enable-plugin-apps",
    default=True,
    help="Enable applications plugin",
)
@click.option(
    "--enable-plugin-charts/--no-enable-plugin-charts",
    default=True,
    help="Enable charts plugin",
)
@click.option(
    "--enable-plugin-cups/--no-enable-plugin-cups",
    default=True,
    help="Enable CUPS plugin",
)
@click.option(
    "--enable-plugin-freeipmi/--no-enable-plugin-freeipmi",
    default=True,
    help="Enable FreeIPMI plugin",
)
@click.option(
    "--enable-plugin-nfacct/--no-enable-plugin-nfacct",
    default=True,
    help="Enable NFACCT plugin",
)
@click.option(
    "--enable-plugin-xenstat/--no-enable-plugin-xenstat",
    default=True,
    help="Enable Xenstat plugin",
)
@click.option(
    "--enable-plugin-cgroup-network/--no-enable-plugin-cgroup-network",
    default=True,
    help="Enable cgroup network plugin",
)
@click.option(
    "--enable-plugin-debugfs/--no-enable-plugin-debugfs",
    default=False,
    help="Enable debugfs plugin",
)
@click.option(
    "--enable-plugin-ebpf/--no-enable-plugin-ebpf",
    default=True,
    help="Enable eBPF plugin",
)
@click.option(
    "--enable-legacy-ebpf-programs/--no-enable-legacy-ebpf-programs",
    default=True,
    help="Enable legacy eBPF programs",
)
@click.option(
    "--enable-plugin-local-listeners/--no-enable-plugin-local-listeners",
    default=True,
    help="Enable local listeners plugin",
)
@click.option(
    "--enable-plugin-network-viewer/--no-enable-plugin-network-viewer",
    default=True,
    help="Enable network viewer plugin",
)
@click.option(
    "--enable-plugin-perf/--no-enable-plugin-perf",
    default=True,
    help="Enable perf plugin",
)
@click.option(
    "--enable-plugin-slabinfo/--no-enable-plugin-slabinfo",
    default=True,
    help="Enable slabinfo plugin",
)
@click.option(
    "--enable-plugin-systemd-journal/--no-enable-plugin-systemd-journal",
    default=True,
    help="Enable systemd journal plugin",
)
@click.option(
    "--enable-exporter-prometheus-remote-write/--no-enable-exporter-prometheus-remote-write",
    default=True,
    help="Enable Prometheus remote write exporter",
)
@click.option(
    "--enable-exporter-mongodb/--no-enable-exporter-mongodb",
    default=True,
    help="Enable MongoDB exporter",
)
@click.option(
    "--enable-bundled-jsonc/--no-enable-bundled-jsonc",
    default=False,
    help="Use bundled JSON-C library",
)
@click.option(
    "--enable-bundled-yaml/--no-enable-bundled-yaml",
    default=False,
    help="Use bundled YAML library",
)
@click.option(
    "--enable-bundled-protobuf/--no-enable-bundled-protobuf",
    default=False,
    help="Use bundled Protobuf library",
)
@click.option(
    "--enable-webrtc/--no-enable-webrtc",
    default=False,
    help="Enable WebRTC support",
)
@click.option(
    "--enable-h2o/--no-enable-h2o",
    default=False,
    help="Enable H2O web server",
)
@click.option(
    "--enable-sentry/--no-enable-sentry",
    default=False,
    help="Enable Sentry error reporting",
)
@click.option(
    "--force-legacy-libbpf/--no-force-legacy-libbpf",
    default=False,
    help="Force use of legacy libbpf",
)
@click.option(
    "--netdata-user",
    type=str,
    default="netdata",
    help="Netdata user",
)
def rfs(**kwargs):
    platform = kwargs.pop("platform")
    repo_path = kwargs.pop("repo_path")
    cfg = cmake.Config(**kwargs)
    _rfs_impl(platform, repo_path, cfg)
