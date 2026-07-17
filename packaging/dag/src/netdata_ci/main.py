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
from . import ci as ci_mod
from . import docker as docker_mod
from . import envs, pkgs, stream, tests
from . import static as static_mod
from .matrix import build_matrix, docker_matrix, get_distro, packaging_matrix, static_matrix

# Netdata source tree; defaults to the repository this module lives in.
# packaging/dag is excluded entirely: nothing in the agent build consumes
# it, and including it would invalidate every cached build on any edit to
# this module's own code.
NetdataSource = Annotated[
    dagger.Directory,
    DefaultPath("/"),
    Ignore(
        [
            ".git",
            "build",
            "fluent-bit/build",
            "packaging/dag",
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
    def pkg_env(
        self,
        distro: str,
        version: str,
        platform: str = "linux/amd64",
    ) -> dagger.Container:
        """Container with everything needed to build native packages."""
        return pkgs.pkg_env(get_distro(distro, version), platform)

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

    @function
    async def package(
        self,
        source: NetdataSource,
        distro: str,
        version: str,
        platform: str = "linux/amd64",
        jobs: int = 0,
    ) -> dagger.Directory:
        """Build native DEB/RPM packages for one distro/arch.

        DEB via cpack; RPM via the spec (interim, per SOW D11=C). Returns
        the artifacts directory; chain `export --path=./artifacts` to copy
        the packages out.
        """
        return await pkgs.package(get_distro(distro, version), platform, source, jobs)

    @function
    async def static(
        self,
        source: NetdataSource,
        arch: str = "x86_64",
        jobs: int = 0,
    ) -> dagger.Directory:
        """Build the self-extracting static installer (.gz.run) for an arch.

        Returns the artifacts directory containing
        netdata-<arch>-<version>.gz.run and the -latest alias.
        """
        return await static_mod.static_build(source, arch, jobs)

    @function
    def docker_image(
        self,
        source: NetdataSource,
        platform: str = "linux/amd64",
        jobs: int = 0,
    ) -> dagger.Container:
        """Build the official agent container image natively.

        Chain `export --path=img.tar` for a loadable tarball, or `publish`
        to push to a registry.
        """
        return docker_mod.docker_image(source, platform, jobs)

    @function
    async def ci(self, source: NetdataSource, tier: str = "smoke", slots: int = 0) -> str:
        """Run a tier of CI jobs concurrently and report per-job results.

        Tiers: smoke (default) | build | packages | static | image | full.
        `slots` caps concurrent heavy jobs (default 1 - each job already
        uses every CPU). Non-native architectures are excluded; run those
        on the shared engine.
        """
        return await ci_mod.run_ci(source, tier, slots)

    @function
    async def go_test(self, source: NetdataSource) -> str:
        """gofmt/vet/build/test -race across every Go module."""
        return await tests.go_test(source).stdout()

    @function
    async def c_test(self, source: NetdataSource, jobs: int = 0) -> str:
        """Address-sanitized Debug build + the in-binary C unit tests."""
        return await tests.c_test(source, jobs).stdout()

    @function
    async def stream_test(self, source: NetdataSource, jobs: int = 0) -> str:
        """Parent/child streaming smoke test with bearer-protection checks."""
        return await stream.stream_test(source, jobs).stdout()

    @function
    async def test_package(
        self,
        distro: str,
        version: str,
        artifacts: dagger.Directory,
        platform: str = "linux/amd64",
    ) -> str:
        """Install built packages in a clean base image and boot the agent."""
        ctr = pkgs.test_package(get_distro(distro, version), platform, artifacts)
        return await ctr.stdout()
