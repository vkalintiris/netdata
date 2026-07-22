"""Native DEB/RPM packaging of the Netdata agent.

The mechanics of producing native packages — one cpack-driven flow for
both formats — and the clean-image install test. All per-distro knowledge
(environments, features, install commands) comes from the definitions in
distros.py; the format-specific package composition lives in the build
system (packaging/cmake/Modules/Packaging.cmake).
"""

from __future__ import annotations

from dataclasses import replace

import dagger
from dagger import dag

from .distros import SPECS, DebPackaging, Distro, RpmPackaging, RpmProtobuf
from .envs import bootstrap, compiler_launcher_args, has_ccache, with_build_caches

SRC_DIR = "/netdata"
BUILD_DIR = "/build"

# Docker platform -> (rpm arch, GOARCH).
_PLATFORM_ARCH: dict[str, tuple[str, str]] = {
    "linux/amd64": ("x86_64", "amd64"),
    "linux/386": ("i686", "386"),
    "linux/arm64": ("aarch64", "arm64"),
    "linux/arm64/v8": ("aarch64", "arm64"),
    "linux/arm/v7": ("armv7l", "arm"),
    "linux/arm/v6": ("armv6l", "arm"),
}

_64BIT_PLATFORMS = ("linux/amd64", "linux/arm64", "linux/arm64/v8")


def _packaging(distro: Distro) -> DebPackaging | RpmPackaging:
    pkg = SPECS[distro].packaging
    if pkg is None:
        raise ValueError(f"{distro.value} has no native package product")
    return pkg


def pkg_env(distro: Distro, platform: str) -> dagger.Container:
    """Container with everything needed to build native packages."""
    pkg = _packaging(distro)
    env = pkg.env
    if isinstance(pkg, DebPackaging) and pkg.deps_64bit and platform in _64BIT_PLATFORMS:
        env = replace(env, deps=(*env.deps, *pkg.deps_64bit))
    return bootstrap(env, platform)


def _flag(name: str, on: bool) -> str:
    return f"-DENABLE_{name}={'On' if on else 'Off'}"


def packaging_configure_args(distro: Distro, platform: str, build_type: str = "Debug") -> list[str]:
    pkg = _packaging(distro)
    amd64 = platform == "linux/amd64"
    bits64 = platform in _64BIT_PLATFORMS
    feats = pkg.features
    rpm = pkg if isinstance(pkg, RpmPackaging) else None
    legacy = rpm is not None and rpm.legacy

    args = [
        "cmake",
        "-S",
        SRC_DIR,
        "-B",
        BUILD_DIR,
        "-G",
        "Ninja",
        *compiler_launcher_args(has_ccache(pkg.env)),
        f"-DCMAKE_BUILD_TYPE={build_type}",
        "-DCMAKE_INSTALL_PREFIX=/",
        "-DBUILD_FOR_PACKAGING=On",
        # Drives the format-specific payload staging (top-level
        # CMakeLists.txt) and the per-distro CPack RPM configuration
        # (Packaging.cmake).
        f"-DNETDATA_PACKAGING_FORMAT={'rpm' if rpm else 'deb'}",
        _flag("DASHBOARD", True),
        _flag("DBENGINE", True),
        _flag("ML", True),
        _flag("PLUGIN_APPS", True),
        _flag("PLUGIN_CGROUP_NETWORK", True),
        _flag("PLUGIN_DEBUGFS", True),
        _flag("PLUGIN_GO", True),
        _flag("PLUGIN_SCRIPTS", True),
        _flag("PLUGIN_PYTHON", True),
        _flag("PLUGIN_CHARTS", True),
        _flag("PLUGIN_LOCAL_LISTENERS", True),
        _flag("PLUGIN_NETFLOW", True),
        _flag("PLUGIN_OTEL", True),
        _flag("PLUGIN_PERF", True),
        _flag("PLUGIN_SLABINFO", True),
        _flag("PLUGIN_SYSTEMD_JOURNAL", True),
        _flag("EXPORTER_PROMETHEUS_REMOTE_WRITE", True),
        _flag("BUNDLED_JSONC", False),
        _flag("BUNDLED_YAML", False),
        _flag("LIBBACKTRACE", True),
        _flag("SENTRY", False),
        # Legacy tier: CUPS and systemd are too old (the spec's
        # centos_ver == 7 conditionals, mirrored by build-package.sh).
        _flag("PLUGIN_CUPS", not legacy),
        _flag("PLUGIN_SYSTEMD_UNITS", not legacy),
        # Distro/arch-dependent features, resolved explicitly.
        _flag("PLUGIN_FREEIPMI", feats.freeipmi),
        _flag("PLUGIN_NFACCT", feats.nfacct),
        # xenstat is only built on 64-bit targets (build-package.sh matrix).
        _flag("PLUGIN_XENSTAT", feats.xenstat and bits64),
        _flag("EXPORTER_MONGODB", feats.mongodb),
        _flag("PLUGIN_EBPF", amd64),
        _flag("PLUGIN_IBM", amd64 and not legacy),
    ]

    if legacy:
        # C++17 and modern libbpf do not compile against the legacy
        # toolchain and kernel headers. FORCE_LEGACY_LIBBPF is a
        # dependent option; it is ignored where eBPF is off.
        args += ["-DUSE_CXX_11=On", "-DFORCE_LEGACY_LIBBPF=On"]

    # Each RPM entry declares its protobuf resolution (RPM distros ship no
    # static protobuf); the debian-family -dev packages ship libprotobuf.a,
    # so system protobuf needs no hint there.
    if rpm is not None and rpm.protobuf is RpmProtobuf.BUNDLED:
        args.append(_flag("BUNDLED_PROTOBUF", True))
    else:
        args.append(_flag("BUNDLED_PROTOBUF", False))
        if rpm is not None:
            args.append("-DProtobuf_LIBRARY=/usr/lib64/libprotobuf.so")

    return args


def _install_type_stamp(kind: str, arch: str, prebuilt_distro: str) -> str:
    return f"INSTALL_TYPE='{kind}'\nPREBUILT_ARCH='{arch}'\nPREBUILT_DISTRO='{prebuilt_distro}'\n"


# CI-speed override for rpmbuild's payload compression: distro defaults
# (zstd -19 on fedora) spend minutes single-threaded on the ~1.6 GB
# unstripped Debug payload. gzip -1 compresses it in seconds and every
# target rpm can read and write it (EL7/AL2's rpm 4.11 has no zstd, and
# the zstd threading flag needs rpm >= 4.16). Header metadata is
# unaffected; shipping builds keep their distro defaults.
_RPM_FAST_PAYLOAD = "%_binary_payload w1.gzdio\n"

# The repos require the distro id embedded in DEB artifact names; RPM names
# are already final (RPM-DEFAULT naming carries the %{dist} tag).
_COLLECT_DEB = (
    'set -e; . /etc/os-release; distid="${ID}${VERSION_ID}"; mkdir -p /artifacts; '
    f"for p in {BUILD_DIR}/packages/*.deb {BUILD_DIR}/packages/*.ddeb; do "
    '[ -e "$p" ] || continue; '
    'ext="${p##*.}"; base="$(basename "$p" ".$ext")"; '
    'name="$(echo "$base" | cut -f 1 -d _)"; ver="$(echo "$base" | cut -f 2 -d _)"; '
    'arch="$(echo "$base" | cut -f 3 -d _)"; '
    'mv "$p" "/artifacts/${name}_${ver}+${distid}_${arch}.${ext}"; done'
)
_COLLECT_RPM = f"set -e; mkdir -p /artifacts; cp {BUILD_DIR}/packages/*.rpm /artifacts/"


def package(
    distro: Distro,
    platform: str,
    source: dagger.Directory,
    jobs: int = 0,
    build_type: str = "Debug",
) -> dagger.Directory:
    """Build native DEB/RPM packages; returns the artifacts directory."""
    pkg = _packaging(distro)
    parallel = str(jobs) if jobs > 0 else "$(nproc)"

    match pkg:
        case DebPackaging():
            generator, kind, collect = "DEB", "binpkg-deb", _COLLECT_DEB
        case RpmPackaging():
            generator, kind, collect = "RPM", "binpkg-rpm", _COLLECT_RPM

    rpm_arch, goarch = _PLATFORM_ARCH[platform]
    key = f"pkg-{distro.value}-{platform.replace('/', '-')}"
    ctr = (
        with_build_caches(pkg_env(distro, platform), key)
        .with_directory(SRC_DIR, source)
        .with_workdir(SRC_DIR)
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("GOOS", "linux")
        .with_env_variable("GOARCH", goarch)
        .with_new_file(
            f"{SRC_DIR}/system/.install-type",
            _install_type_stamp(kind, rpm_arch, pkg.prebuilt_distro),
        )
        .with_exec(packaging_configure_args(distro, platform, build_type))
        .with_exec(["sh", "-c", f"cmake --build {BUILD_DIR} --parallel {parallel}"])
        .with_workdir(BUILD_DIR)
    )
    if isinstance(pkg, RpmPackaging):
        # Placed after the build step so editing the macro never
        # invalidates the cached build layers.
        ctr = ctr.with_new_file("/etc/rpm/macros", _RPM_FAST_PAYLOAD)
    ctr = ctr.with_exec(["cpack", "-V", "-G", generator]).with_exec(["sh", "-c", collect])
    return ctr.directory("/artifacts")


# Start the agent, wait for the API, and report version + basic health.
_RUNTIME_CHECK = r"""
set -e
/usr/sbin/netdata -W buildinfo > /tmp/buildinfo.txt
/usr/sbin/netdata -D > /tmp/netdata.log 2>&1 &
for i in $(seq 1 60); do
  if curl -fsS http://localhost:19999/api/v1/info > /tmp/info.json 2>/dev/null; then
    break
  fi
  sleep 1
done
[ -s /tmp/info.json ] || { echo "agent API never came up"; tail -50 /tmp/netdata.log; exit 1; }
ver="$(jq -r .version /tmp/info.json)"
echo "agent-up version=${ver}"
"""


def test_package(
    distro: Distro,
    platform: str,
    artifacts: dagger.Directory,
) -> dagger.Container:
    """Install built packages in a clean base image and boot the agent."""
    pkg = _packaging(distro)

    ctr = (
        dag.container(platform=dagger.Platform(platform))
        .from_(pkg.env.base_image)
        .with_env_variable("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .with_env_variable("DEBIAN_FRONTEND", "noninteractive")
        .with_env_variable("DISABLE_TELEMETRY", "1")
    )
    # The clean image may need the same repo files as the build env
    # (centos7: vault mirrors).
    for path, contents in pkg.env.files:
        ctr = ctr.with_new_file(path, contents)
    return (
        ctr.with_directory("/artifacts", artifacts)
        .with_exec(["sh", "-c", pkg.test_install])
        .with_exec(["sh", "-c", _RUNTIME_CHECK])
    )
