"""Native CMake build of the Netdata agent.

The module drives CMake directly with an explicit, per-distro feature
profile. The profile below reproduces the effective configuration
netdata-installer.sh derives for the CI source builds (installer defaults,
Linux, one-time build) — extracted from packaging/installer/functions.sh
prepare_cmake_options() on 2026-07-16. The installer probes pkg-config at
configure time; we state the per-distro outcome explicitly instead, so the
configuration is deterministic and owned here.
"""

from __future__ import annotations

import dagger

from . import envs
from .matrix import Distro

# Features the installer enables unconditionally on Linux with defaults.
_COMMON_ON = (
    "PLUGIN_GO",
    "PLUGIN_PYTHON",
    "PLUGIN_CHARTS",
    "BUNDLED_PROTOBUF",
    "PLUGIN_DEBUGFS",
    "PLUGIN_PERF",
    "PLUGIN_SLABINFO",
    "PLUGIN_CGROUP_NETWORK",
    "PLUGIN_LOCAL_LISTENERS",
    "PLUGIN_NETWORK_VIEWER",
    "DBENGINE",
    "ML",
    "PLUGIN_APPS",
)

# Features the installer disables with defaults (unset env vars or missing
# optional libraries: cups, snappy, mongoc, ipmimonitoring, netfilter_acct,
# xenstat are not in the source-build dependency set).
_COMMON_OFF = (
    "NETDATA_JOURNAL_FILE_READER",
    "PLUGIN_CUPS",
    "BUNDLED_JSONC",
    "PLUGIN_NETFLOW",
    "PLUGIN_OTEL",
    "PLUGIN_IBM",
    "PLUGIN_SCRIPTS",
    "EXPORTER_PROMETHEUS_REMOTE_WRITE",
    "EXPORTER_MONGODB",
    "PLUGIN_FREEIPMI",
    "PLUGIN_NFACCT",
    "PLUGIN_XENSTAT",
)

# Distros whose build environment has no libsystemd (musl, or no systemd
# headers in the dependency set): journal/units plugins stay off there.
_NO_SYSTEMD = ("alpine", "archlinux")

INSTALL_PREFIX = "/opt/netdata"
BUILD_DIR = "build"
SRC_DIR = "/netdata"


def configure_args(d: Distro, platform: str, build_type: str = "Debug") -> list[str]:
    systemd = d.name not in _NO_SYSTEMD
    features: dict[str, bool] = {name: True for name in _COMMON_ON}
    features["PLUGIN_SYSTEMD_JOURNAL"] = systemd
    features["PLUGIN_SYSTEMD_UNITS"] = systemd
    # The installer force-enables eBPF on x86 hardware (netdata-installer.sh:256).
    features["PLUGIN_EBPF"] = platform in ("linux/amd64", "linux/386")
    features.update({name: False for name in _COMMON_OFF})

    args = ["cmake", "-S", ".", "-B", BUILD_DIR]
    args.append(f"-DCMAKE_BUILD_TYPE={build_type}")
    args.append(f"-DCMAKE_INSTALL_PREFIX={INSTALL_PREFIX}")
    for name, on in features.items():
        args.append(f"-DENABLE_{name}={'On' if on else 'Off'}")
    return args


def source_build(
    d: Distro,
    platform: str,
    source: dagger.Directory,
    jobs: int = 0,
    build_type: str = "Debug",
) -> dagger.Container:
    """Compile and install the agent from source in the distro's build env.

    `jobs` caps build parallelism; 0 means one job per CPU. Callers running
    several builds concurrently should budget jobs across them.
    """
    parallel = str(jobs) if jobs > 0 else "$(nproc)"
    ctr = (
        envs.build_env(d, platform)
        .with_directory(SRC_DIR, source)
        .with_workdir(SRC_DIR)
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_exec(configure_args(d, platform, build_type))
        .with_exec(["sh", "-c", f"cmake --build {BUILD_DIR} --parallel {parallel}"])
        .with_exec(["cmake", "--install", BUILD_DIR])
    )
    return ctr
