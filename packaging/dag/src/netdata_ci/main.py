"""Netdata CI: build, package, and test the Netdata agent with Dagger.

Native pipeline module: all build knowledge (distro matrix, environments,
build profiles, packaging, static builds) lives here in typed Python, with
no runtime dependence on the legacy shell scripts or YAML data files.
"""

import enum
from typing import Annotated

import dagger
from dagger import DefaultPath, Ignore, enum_type, function, object_type

from . import build as build_mod
from . import envs
from .matrix import build_matrix, docker_matrix, get_distro, packaging_matrix, static_matrix

# Netdata source tree; defaults to the repository this module lives in.
NetdataSource = Annotated[
    dagger.Directory,
    DefaultPath("/"),
    Ignore(
        [
            ".git",
            "build",
            "fluent-bit/build",
            "packaging/dag/sdk",
            "packaging/dag/.venv",
        ]
    ),
]


@enum_type
class MatrixKind(enum.Enum):
    """Which CI-equivalent job matrix to emit."""

    BUILD = "build"
    PACKAGING = "packaging"
    STATIC = "static"
    DOCKER = "docker"


@object_type
class NetdataCi:
    """Build, package, and test the Netdata agent."""

    @function
    def matrix(
        self,
        kind: MatrixKind,
        short: bool = False,
        native_only: bool = False,
    ) -> str:
        """Emit a job matrix as JSON.

        Output matches the corresponding .github/scripts/gen-matrix-*.py
        script so the two can be diffed while both exist. `short` trims the
        packaging matrix to always-run architectures; `native_only` drops
        emulated architectures from the static/docker matrices.
        """
        match kind:
            case MatrixKind.BUILD:
                return build_matrix()
            case MatrixKind.PACKAGING:
                return packaging_matrix(short)
            case MatrixKind.STATIC:
                return static_matrix(native_only)
            case MatrixKind.DOCKER:
                return docker_matrix(native_only)

    @function
    def build_env(
        self,
        distro: str,
        version: str,
        platform: str = "linux/amd64",
    ) -> dagger.Container:
        """Container with everything needed to build the agent from source.

        Chain with `terminal` to explore it interactively, e.g.:
        dagger call build-env --distro=debian --version=12 terminal
        """
        return envs.build_env(get_distro(distro, version), platform)

    @function
    def build(
        self,
        source: NetdataSource,
        distro: str,
        version: str,
        platform: str = "linux/amd64",
        jobs: int = 0,
    ) -> dagger.Container:
        """Compile and install the agent from source for one distro.

        Uses the explicit source-build feature profile (parity with the CI
        source-build jobs). The result has the agent installed under
        /opt/netdata; chain `terminal` to inspect it. `jobs` caps build
        parallelism (0 = one job per CPU).
        """
        return build_mod.source_build(get_distro(distro, version), platform, source, jobs)
