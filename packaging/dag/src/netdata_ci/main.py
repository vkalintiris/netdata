"""Netdata CI: build, package, and test the Netdata agent with Dagger.

Native pipeline module: all build knowledge (distro definitions,
environments, build profiles, packaging, static builds) lives here in
typed Python, with no runtime dependence on the legacy shell scripts or
YAML data files. The module provides per-distro capabilities; which
targets a CI run covers is declared on top (ci.py tiers today).
"""

from typing import Annotated

import dagger
from dagger import DefaultPath, Ignore, function, object_type

from . import build as build_mod
from . import ci as ci_mod
from . import docker as docker_mod
from . import envs, pkgs, stream, tests
from . import static as static_mod
from .distros import SPECS, Distro

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


@object_type
class NetdataCi:
    """Build, package, and test the Netdata agent."""

    @function
    def build_env(
        self,
        distro: Distro,
        platform: str = "linux/amd64",
    ) -> dagger.Container:
        """Container with everything needed to build the agent from source.

        Chain with `terminal` to explore it interactively, e.g.:
        dagger call build-env --distro=DEBIAN_12 terminal
        """
        return envs.bootstrap(SPECS[distro].build, platform)

    @function
    def pkg_env(
        self,
        distro: Distro,
        platform: str = "linux/amd64",
    ) -> dagger.Container:
        """Container with everything needed to build native packages."""
        return pkgs.pkg_env(distro, platform)

    @function
    def static_env(self, arch: str = "x86_64") -> dagger.Container:
        """Alpine builder environment for the static (.gz.run) build."""
        if arch not in static_mod.STATIC_ARCHS:
            raise ValueError(
                f"unsupported static arch {arch} (know: {sorted(static_mod.STATIC_ARCHS)})"
            )
        return static_mod.static_env(static_mod.STATIC_ARCHS[arch])

    @function
    def docker_env(self, platform: str = "linux/amd64") -> dagger.Container:
        """Builder environment of the official container image."""
        return docker_mod.docker_builder_env(platform)

    @function
    def build(
        self,
        source: NetdataSource,
        distro: Distro,
        platform: str = "linux/amd64",
        jobs: int = 0,
        build_type: str = "Debug",
    ) -> dagger.Container:
        """Compile and install the agent from source for one distro.

        Uses the explicit source-build feature profile (parity with the CI
        source-build jobs). The result has the agent installed under
        /opt/netdata; chain `terminal` to inspect it. `jobs` caps build
        parallelism (0 = one job per CPU).
        """
        return build_mod.source_build(distro, platform, source, jobs, build_type)

    @function
    async def package(
        self,
        source: NetdataSource,
        distro: Distro,
        platform: str = "linux/amd64",
        jobs: int = 0,
        build_type: str = "Debug",
    ) -> dagger.Directory:
        """Build native DEB/RPM packages for one distro/arch.

        DEB via cpack; RPM via the spec (interim, per SOW D11=C). Returns
        the artifacts directory; chain `export --path=./artifacts` to copy
        the packages out.
        """
        return await pkgs.package(distro, platform, source, jobs, build_type)

    @function
    async def static(
        self,
        source: NetdataSource,
        arch: str = "x86_64",
        jobs: int = 0,
        build_type: str = "Debug",
    ) -> dagger.Directory:
        """Build the self-extracting static installer (.gz.run) for an arch.

        Returns the artifacts directory containing
        netdata-<arch>-<version>.gz.run and the -latest alias.
        """
        return await static_mod.static_build(source, arch, jobs, build_type)

    @function
    def docker_image(
        self,
        source: NetdataSource,
        platform: str = "linux/amd64",
        jobs: int = 0,
        build_type: str = "Debug",
    ) -> dagger.Container:
        """Build the official agent container image natively.

        Chain `export --path=img.tar` for a loadable tarball, or `publish`
        to push to a registry.
        """
        return docker_mod.docker_image(source, platform, jobs, build_type)

    @function
    async def ci(
        self,
        source: NetdataSource,
        tier: str = "smoke",
        slots: int = 0,
        build_type: str = "Debug",
        jobs: int = 0,
    ) -> str:
        """Run a tier of CI jobs concurrently and report per-job results.

        Tiers: smoke (default) | build | packages | static | image | full.
        `slots` caps concurrent heavy jobs (default 1); `jobs` caps each
        build's compile parallelism (default: one per CPU) so slots*jobs
        can be budgeted to the machine. Non-native architectures are
        excluded; run those on the shared engine.
        """
        return await ci_mod.run_ci(source, tier, slots, build_type, jobs)

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
        distro: Distro,
        artifacts: dagger.Directory,
        platform: str = "linux/amd64",
    ) -> str:
        """Install built packages in a clean base image and boot the agent."""
        return await pkgs.test_package(distro, platform, artifacts).stdout()
