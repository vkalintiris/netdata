"""Native build/packaging matrix for the Netdata agent.

This is the single typed source of truth for the distributions,
architectures, and package targets the pipeline covers. It was seeded from
.github/data/distros.yml (2026-07-16) and evolves independently of it; the
YAML file remains CI-only until the workflows consume this module instead.

The *_matrix() emitters reproduce the exact JSON the corresponding
.github/scripts/gen-matrix-*.py scripts print, so the seed can be validated
by diffing outputs. Entry order, key names, and string values (including
trailing newlines on shell snippets) are significant.
"""

from __future__ import annotations

import enum
import json
from dataclasses import dataclass


class PkgType(enum.StrEnum):
    DEB = "deb"
    RPM = "rpm"


class Tier(enum.StrEnum):
    CORE = "Core"
    INTERMEDIATE = "Intermediate"
    COMMUNITY = "Community"


# Docker platform strings per packaging architecture.
PLATFORM_MAP: dict[str, str] = {
    "aarch64": "linux/arm64/v8",
    "amd64": "linux/amd64",
    "arm64": "linux/arm64/v8",
    "armhf": "linux/arm/v7",
    "armhfp": "linux/arm/v7",
    "armv6l": "linux/arm/v6",
    "armv7l": "linux/arm/v7",
    "i386": "linux/386",
    "x86_64": "linux/amd64",
}

# Sort order for per-architecture jobs.
ARCH_ORDER: list[str] = [
    "amd64",
    "x86_64",
    "i386",
    "armhf",
    "armhfp",
    "armv6l",
    "armv7l",
    "arm64",
    "aarch64",
]


@dataclass(frozen=True)
class ArchData:
    runner: str
    qemu: bool = False


_X86_RUNNER = "ubuntu-24.04"
_ARM_RUNNER = "ubuntu-24.04-arm"

ARCH_DATA: dict[str, ArchData] = {
    "amd64": ArchData(_X86_RUNNER),
    "x86_64": ArchData(_X86_RUNNER),
    "i386": ArchData(_X86_RUNNER),
    "armhf": ArchData(_ARM_RUNNER),
    "armhfp": ArchData(_ARM_RUNNER),
    "armv6l": ArchData(_ARM_RUNNER),
    "armv7l": ArchData(_ARM_RUNNER),
    "arm64": ArchData(_ARM_RUNNER),
    "aarch64": ArchData(_ARM_RUNNER),
}

STATIC_ARCHES: list[str] = ["x86_64", "armv6l", "armv7l", "aarch64"]
DOCKER_ARCHES: list[str] = ["amd64", "armv7l", "arm64"]


@dataclass(frozen=True)
class Packages:
    """Native package target for a distro."""

    type: PkgType
    repo_distro: str
    arches: tuple[str, ...]
    builder_rev: str = "v1"
    alt_links: tuple[str, ...] = ()


@dataclass(frozen=True)
class Distro:
    name: str
    version: str
    tier: Tier
    # Container image; empty means "<name>:<version>".
    image: str = ""
    env_prep: str = ""
    jsonc_removal: str = ""
    packages: Packages | None = None
    ebpf_core: bool = False
    skip_local_build: bool = False
    # Architectures whose packages bundle the Sentry SDK.
    sentry_arches: frozenset[str] = frozenset()
    # True/False, or a named special-case checker (e.g. "amazon-linux").
    eol_check: bool | str = True
    eol_lts: bool = False
    # Legacy: packages still published for existing installs; no CI builds.
    legacy: bool = False

    @property
    def base_image(self) -> str:
        return self.image or f"{self.name}:{self.version}"

    @property
    def artifact_key(self) -> str:
        return self.name + self.version.replace(".", "")


_RPM_2 = ("x86_64", "aarch64")
_DEB_3 = ("amd64", "armhf", "arm64")
_DEB_4 = ("i386", "amd64", "armhf", "arm64")

_SENTRY_AMD64 = frozenset({"amd64"})

_APT_PREP = "apt-get update\n"
_DNF_JSONC = "dnf remove -y json-c-devel\n"

DISTROS: tuple[Distro, ...] = (
    Distro(
        name="alpine",
        version="edge",
        tier=Tier.COMMUNITY,
        env_prep="apk add -U bash\n",
        jsonc_removal="apk del json-c-dev\n",
        ebpf_core=True,
        eol_check=False,
    ),
    Distro(
        name="alpine",
        version="3.23",
        tier=Tier.CORE,
        env_prep="apk add -U bash\n",
        jsonc_removal="apk del json-c-dev\n",
        ebpf_core=True,
    ),
    Distro(
        name="alpine",
        version="3.22",
        tier=Tier.CORE,
        env_prep="apk add -U bash\n",
        jsonc_removal="apk del json-c-dev\n",
        ebpf_core=True,
    ),
    Distro(
        name="archlinux",
        version="latest",
        tier=Tier.INTERMEDIATE,
        env_prep="pacman --noconfirm -Syu && pacman --noconfirm -Sy grep libffi\n",
        ebpf_core=True,
        eol_check=False,
    ),
    Distro(
        name="amazonlinux",
        version="2",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "amazonlinux/2", _RPM_2),
        skip_local_build=True,
        eol_check="amazon-linux",
    ),
    Distro(
        name="amazonlinux",
        version="2023",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "amazonlinux/2023", _RPM_2),
        eol_check="amazon-linux",
    ),
    Distro(
        name="centos",
        version="7",
        tier=Tier.CORE,
        image="netdata/legacy:centos7",
        packages=Packages(
            PkgType.RPM,
            "el/7",
            ("x86_64",),
            alt_links=("el/7Server", "el/7Client"),
        ),
        skip_local_build=True,
        eol_check=False,
    ),
    Distro(
        name="centos-stream",
        version="10",
        tier=Tier.COMMUNITY,
        image="quay.io/centos/centos:stream10",
        jsonc_removal=_DNF_JSONC,
        packages=Packages(PkgType.RPM, "el/c10s", _RPM_2),
        ebpf_core=True,
    ),
    Distro(
        name="centos-stream",
        version="9",
        tier=Tier.COMMUNITY,
        image="quay.io/centos/centos:stream9",
        jsonc_removal=_DNF_JSONC,
        packages=Packages(PkgType.RPM, "el/c9s", _RPM_2),
        ebpf_core=True,
    ),
    Distro(
        name="debian",
        version="13",
        tier=Tier.CORE,
        image="debian:trixie",
        env_prep=_APT_PREP,
        jsonc_removal="apt-get purge -y libjson-c-dev\n",
        packages=Packages(PkgType.DEB, "debian/trixie", _DEB_3, builder_rev="v2"),
        ebpf_core=True,
        sentry_arches=_SENTRY_AMD64,
        eol_lts=True,
    ),
    Distro(
        name="debian",
        version="12",
        tier=Tier.CORE,
        image="debian:bookworm",
        env_prep=_APT_PREP,
        jsonc_removal="apt-get purge -y libjson-c-dev\n",
        packages=Packages(PkgType.DEB, "debian/bookworm", _DEB_4, builder_rev="v2"),
        ebpf_core=True,
        sentry_arches=_SENTRY_AMD64,
        eol_lts=True,
    ),
    Distro(
        name="debian",
        version="11",
        tier=Tier.CORE,
        image="debian:bullseye",
        env_prep=_APT_PREP,
        jsonc_removal="apt-get purge -y libjson-c-dev\n",
        packages=Packages(PkgType.DEB, "debian/bullseye", _DEB_4, builder_rev="v2"),
        sentry_arches=_SENTRY_AMD64,
        eol_lts=True,
    ),
    Distro(
        name="fedora",
        version="44",
        tier=Tier.CORE,
        jsonc_removal=_DNF_JSONC,
        packages=Packages(PkgType.RPM, "fedora/44", _RPM_2),
        ebpf_core=True,
    ),
    Distro(
        name="fedora",
        version="43",
        tier=Tier.CORE,
        jsonc_removal=_DNF_JSONC,
        packages=Packages(PkgType.RPM, "fedora/43", _RPM_2),
        ebpf_core=True,
    ),
    Distro(
        name="opensuse",
        version="tumbleweed",
        tier=Tier.CORE,
        image="opensuse/tumbleweed",
        jsonc_removal="zypper rm -y libjson-c-devel\n",
        packages=Packages(PkgType.RPM, "opensuse/tumbleweed", _RPM_2),
        ebpf_core=True,
        eol_check=False,
    ),
    Distro(
        name="opensuse",
        version="16.0",
        tier=Tier.CORE,
        image="opensuse/leap:16.0",
        jsonc_removal="zypper rm -y libjson-c-devel\n",
        packages=Packages(PkgType.RPM, "opensuse/16.0", _RPM_2),
        ebpf_core=True,
    ),
    Distro(
        name="oraclelinux",
        version="10",
        tier=Tier.CORE,
        jsonc_removal=_DNF_JSONC,
        packages=Packages(PkgType.RPM, "ol/10", _RPM_2),
        ebpf_core=True,
        skip_local_build=True,
    ),
    Distro(
        name="oraclelinux",
        version="9",
        tier=Tier.CORE,
        jsonc_removal=_DNF_JSONC,
        packages=Packages(PkgType.RPM, "ol/9", _RPM_2),
        ebpf_core=True,
    ),
    Distro(
        name="oraclelinux",
        version="8",
        tier=Tier.CORE,
        jsonc_removal=_DNF_JSONC,
        packages=Packages(PkgType.RPM, "ol/8", _RPM_2),
        ebpf_core=True,
    ),
    Distro(
        name="rockylinux",
        version="10",
        tier=Tier.CORE,
        image="quay.io/rockylinux/rockylinux:10",
        jsonc_removal=_DNF_JSONC,
        packages=Packages(
            PkgType.RPM,
            "el/10",
            _RPM_2,
            alt_links=(
                "el/10Server",
                "el/10Client",
                "el/10RedHatVirtualizationHost",
            ),
        ),
        ebpf_core=True,
    ),
    Distro(
        name="rockylinux",
        version="9",
        tier=Tier.CORE,
        image="rockylinux:9",
        jsonc_removal=_DNF_JSONC,
        packages=Packages(
            PkgType.RPM,
            "el/9",
            _RPM_2,
            alt_links=(
                "el/9Server",
                "el/9Client",
                "el/9RedHatVirtualizationHost",
            ),
        ),
        ebpf_core=True,
    ),
    Distro(
        name="rockylinux",
        version="8",
        tier=Tier.CORE,
        image="rockylinux:8",
        jsonc_removal=_DNF_JSONC,
        packages=Packages(
            PkgType.RPM,
            "el/8",
            _RPM_2,
            alt_links=(
                "el/8Server",
                "el/8Client",
                "el/8RedHatVirtualizationHost",
            ),
        ),
        ebpf_core=True,
    ),
    Distro(
        name="ubuntu",
        version="26.04",
        tier=Tier.CORE,
        env_prep="rm -f /etc/apt/apt.conf.d/docker && apt-get update\n",
        jsonc_removal="apt-get remove -y libjson-c-dev\n",
        packages=Packages(PkgType.DEB, "ubuntu/resolute", _DEB_3, builder_rev="v2"),
        ebpf_core=True,
        sentry_arches=_SENTRY_AMD64,
    ),
    Distro(
        name="ubuntu",
        version="25.10",
        tier=Tier.CORE,
        env_prep="rm -f /etc/apt/apt.conf.d/docker && apt-get update\n",
        jsonc_removal="apt-get remove -y libjson-c-dev\n",
        packages=Packages(PkgType.DEB, "ubuntu/questing", _DEB_3, builder_rev="v2"),
        ebpf_core=True,
        sentry_arches=_SENTRY_AMD64,
    ),
    Distro(
        name="ubuntu",
        version="24.04",
        tier=Tier.CORE,
        env_prep="rm -f /etc/apt/apt.conf.d/docker && apt-get update\n",
        jsonc_removal="apt-get remove -y libjson-c-dev\n",
        packages=Packages(PkgType.DEB, "ubuntu/noble", _DEB_3, builder_rev="v2"),
        ebpf_core=True,
        sentry_arches=_SENTRY_AMD64,
    ),
    Distro(
        name="ubuntu",
        version="22.04",
        tier=Tier.CORE,
        env_prep="rm -f /etc/apt/apt.conf.d/docker && apt-get update\n",
        jsonc_removal="apt-get remove -y libjson-c-dev\n",
        packages=Packages(PkgType.DEB, "ubuntu/jammy", _DEB_3, builder_rev="v2"),
        ebpf_core=True,
        sentry_arches=_SENTRY_AMD64,
    ),
    # Legacy: packages still published for existing installs; no CI builds.
    Distro(
        name="debian",
        version="10",
        tier=Tier.CORE,
        image="debian:buster",
        packages=Packages(PkgType.DEB, "debian/buster", _DEB_3, builder_rev="v2"),
        legacy=True,
    ),
    Distro(
        name="fedora",
        version="37",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "fedora/37", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="fedora",
        version="38",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "fedora/38", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="fedora",
        version="39",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "fedora/39", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="fedora",
        version="40",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "fedora/40", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="fedora",
        version="41",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "fedora/41", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="fedora",
        version="42",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "fedora/42", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="opensuse",
        version="15.4",
        tier=Tier.CORE,
        packages=Packages(PkgType.RPM, "opensuse/15.4", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="opensuse",
        version="15.5",
        tier=Tier.CORE,
        image="opensuse/leap:15.5",
        packages=Packages(PkgType.RPM, "opensuse/15.5", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="opensuse",
        version="15.6",
        tier=Tier.CORE,
        image="opensuse/leap:15.6",
        packages=Packages(PkgType.RPM, "opensuse/15.6", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="centos-stream",
        version="8",
        tier=Tier.COMMUNITY,
        image="quay.io/centos/centos:stream8",
        packages=Packages(PkgType.RPM, "el/c8s", _RPM_2),
        legacy=True,
    ),
    Distro(
        name="ubuntu",
        version="23.10",
        tier=Tier.CORE,
        packages=Packages(PkgType.DEB, "ubuntu/mantic", _DEB_3, builder_rev="v2"),
        legacy=True,
    ),
    Distro(
        name="ubuntu",
        version="24.10",
        tier=Tier.CORE,
        packages=Packages(PkgType.DEB, "ubuntu/oracular", _DEB_3, builder_rev="v2"),
        legacy=True,
    ),
    Distro(
        name="ubuntu",
        version="25.04",
        tier=Tier.CORE,
        packages=Packages(PkgType.DEB, "ubuntu/plucky", _DEB_3, builder_rev="v2"),
        legacy=True,
    ),
    Distro(
        name="ubuntu",
        version="20.04",
        tier=Tier.CORE,
        packages=Packages(PkgType.DEB, "ubuntu/focal", _DEB_3, builder_rev="v2"),
        legacy=True,
    ),
)


def active_distros() -> list[Distro]:
    return [d for d in DISTROS if not d.legacy]


def get_distro(name: str, version: str) -> Distro:
    for d in DISTROS:
        if d.name == name and d.version == version:
            return d
    known = ", ".join(f"{d.name}:{d.version}" for d in DISTROS)
    raise ValueError(f"unknown distro {name}:{version} (known: {known})")


# Architectures that run even in a shortened packaging matrix.
_ALWAYS_RUN_ARCHES = ["amd64", "x86_64", "i386", "armhf", "aarch64", "arm64"]


def build_matrix() -> str:
    entries = []
    for d in active_distros():
        if d.skip_local_build:
            continue
        e: dict[str, str] = {
            "artifact_key": d.artifact_key,
            "version": d.version,
            "distro": d.base_image,
        }
        if d.env_prep:
            e["env_prep"] = d.env_prep
        if d.jsonc_removal:
            e["jsonc_removal"] = d.jsonc_removal
        entries.append(e)
    entries.sort(key=lambda k: k["distro"])
    return json.dumps({"include": entries}, sort_keys=True)


def packaging_matrix(short: bool = False) -> str:
    entries: list[tuple[int, str, str, dict[str, str | bool]]] = []
    for d in active_distros():
        if d.packages is None:
            continue
        for arch in d.packages.arches:
            if short and arch not in _ALWAYS_RUN_ARCHES:
                continue
            e: dict[str, str | bool] = {
                "distro": d.name,
                "version": d.version,
                "repo_distro": d.packages.repo_distro,
                "format": str(d.packages.type),
                "base_image": d.base_image,
                "builder_rev": d.packages.builder_rev,
                "platform": PLATFORM_MAP[arch],
                "bundle_sentry": arch in d.sentry_arches,
                "arch": arch,
                "runner": ARCH_DATA[arch].runner,
                "qemu": ARCH_DATA[arch].qemu,
            }
            entries.append((ARCH_ORDER.index(arch), d.name, d.version, e))
    entries.sort(key=lambda k: k[:3])
    return json.dumps({"include": [e for *_, e in entries]}, sort_keys=True)


def _arch_matrix(arches: list[str], native_only: bool, with_platform: bool) -> str:
    entries: list[tuple[str, dict[str, str | bool]]] = []
    for arch in arches:
        if native_only and ARCH_DATA[arch].qemu:
            continue
        e: dict[str, str | bool] = {"arch": arch}
        if with_platform:
            e["platform"] = PLATFORM_MAP[arch]
        e["runner"] = ARCH_DATA[arch].runner
        e["qemu"] = ARCH_DATA[arch].qemu
        entries.append((arch, e))
    entries.sort(key=lambda k: ARCH_ORDER.index(k[0]))
    return json.dumps({"include": [e for _, e in entries]}, sort_keys=True)


def static_matrix(native_only: bool = False) -> str:
    return _arch_matrix(STATIC_ARCHES, native_only, with_platform=False)


def docker_matrix(native_only: bool = False) -> str:
    return _arch_matrix(DOCKER_ARCHES, native_only, with_platform=True)
