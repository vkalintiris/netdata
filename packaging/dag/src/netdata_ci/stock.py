"""Synthetic topology IP-intel stock payload (CI pull-request parity).

Natively reproduces the synthetic branch of
.github/scripts/prepare-topology-ip-intel-stock.sh: three vendored CSV
rows fed to the repo's topology-ip-intel-downloader produce the four-file
payload that CMake installs into the plugin-netflow component under
usr/share/netdata/topology-ip-intel. The data sources are local fixtures
(no upstream fetch); the Go toolchain still resolves modules from the
proxy on a cold go-mod-cache. The payload is arch-independent data, so it
builds once on one platform and the cached result serves every package
and static target.

Release-grade payloads (real DB-IP lite databases) deliberately stay OUT
of this module: the downloader resolves the current month's artifact URL
from the DB-IP site at run time, so a network fetch inside a
content-addressed engine would cache one monthly snapshot forever while
presenting it as fresh. CI stages release payloads externally for
nightlies/releases; callers that have one pass it as a Directory override
on package/static instead.
"""

from __future__ import annotations

import dagger
from dagger import dag

from .envs import STD_PATH, install_go

# Where consumers mount the payload. Staged before configure: CMake
# validates the payload files at configure time (FATAL_ERROR on a
# missing file).
STOCK_MOUNT = "/topology-ip-intel-stock"

# Synthetic fixtures, vendored from prepare-topology-ip-intel-stock.sh
# (synthetic mode) — keep byte-identical so dag validates the same payload
# CI's pull-request builds ship.
_ASN_CSV = """\
1.1.1.0,1.1.1.255,13335,"Cloudflare, Inc."
8.8.8.0,8.8.8.255,15169,Google LLC
203.0.113.0,203.0.113.255,64500,Example Transit
"""

_GEO_CSV = """\
1.1.1.0,1.1.1.255,,AU,Queensland,South Brisbane,-27.4748,153.0170
8.8.8.0,8.8.8.255,,US,California,Mountain View,37.4220,-122.0850
203.0.113.0,203.0.113.255,,US,Virginia,Ashburn,39.0438,-77.4874
"""

_CONFIG = """\
sources:
  - name: pr-synthetic-asn
    family: asn
    provider: dbip
    artifact: asn-lite
    format: csv
    path: /tmp/stock/asn.csv
  - name: pr-synthetic-geo
    family: geo
    provider: dbip
    artifact: city-lite
    format: csv
    path: /tmp/stock/geo.csv
"""


# The payload is pure data (mmdb/json/README), so it is always built on
# one fixed platform: one build, one cache entry, no QEMU for arm
# targets — and no dependency on the base image publishing every
# consumer's architecture (debian ships no arm/v6 variant).
_BUILD_PLATFORM = "linux/amd64"


def topology_stock(source: dagger.Directory) -> dagger.Directory:
    """Build the synthetic topology IP-intel stock payload.

    Runs the repo's downloader tool against the vendored CSV fixtures in a
    minimal Go container. Only src/go is mounted, so agent-source changes
    outside the Go tree never invalidate the cached payload.
    """
    ctr = (
        dag.container(platform=dagger.Platform(_BUILD_PLATFORM))
        .from_("debian:bookworm-slim")
        .with_env_variable("PATH", STD_PATH)
        .with_env_variable("DEBIAN_FRONTEND", "noninteractive")
        # ca-certificates: the Go module proxy is reached over TLS.
        .with_exec(
            [
                "sh",
                "-c",
                "apt-get update && apt-get install -y --no-install-recommends ca-certificates",
            ]
        )
    )
    ctr = (
        install_go(ctr, _BUILD_PLATFORM)
        .with_mounted_cache("/go-build-cache", dag.cache_volume("go-build-cache"))
        .with_env_variable("GOCACHE", "/go-build-cache")
        .with_mounted_cache("/go-mod-cache", dag.cache_volume("go-mod-cache"))
        .with_env_variable("GOMODCACHE", "/go-mod-cache")
        # Pure-Go build; the slim image carries no C toolchain.
        .with_env_variable("CGO_ENABLED", "0")
        .with_directory("/netdata/src/go", source.directory("src/go"))
        .with_new_file("/tmp/stock/asn.csv", _ASN_CSV)
        .with_new_file("/tmp/stock/geo.csv", _GEO_CSV)
        .with_new_file("/tmp/stock/topology-ip-intel.yaml", _CONFIG)
        .with_workdir("/netdata/src/go")
        .with_exec(
            [
                "go",
                "run",
                "./tools/topology-ip-intel-downloader",
                "--config",
                "/tmp/stock/topology-ip-intel.yaml",
                "--output-dir",
                "/stock",
            ]
        )
        .with_exec(
            ["cp", "tools/topology-ip-intel-downloader/stock/README.md", "/stock/README.md"]
        )
    )
    return ctr.directory("/stock")


def stock_check_script(install_prefix: str = "") -> str:
    """Shell assertion that the four payload files landed in an install.

    Booting the agent never reads the payload, so the install tests assert
    the component composition explicitly. `install_prefix` is "" for
    native packages (prefix /) and the static NP prefix for installers.
    """
    d = f"{install_prefix}/usr/share/netdata/topology-ip-intel"
    return (
        "set -e; "
        "for f in README.md topology-ip-asn.mmdb topology-ip-geo.mmdb topology-ip-intel.json; "
        f'do [ -s "{d}/$f" ] '
        '|| { echo "missing stock payload file: $f"; exit 1; }; done; '
        'echo "stock-payload-ok"'
    )
