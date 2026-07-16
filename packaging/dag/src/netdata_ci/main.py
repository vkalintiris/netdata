"""Netdata CI: build, package, and test the Netdata agent with Dagger.

Native pipeline module: all build knowledge (distro matrix, environments,
build profiles, packaging, static builds) lives here in typed Python, with
no runtime dependence on the legacy shell scripts or YAML data files.
"""

import enum

from dagger import enum_type, function, object_type

from .matrix import build_matrix, docker_matrix, packaging_matrix, static_matrix


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
