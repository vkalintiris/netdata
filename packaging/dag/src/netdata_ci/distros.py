"""The supported distros and their complete, declarative definitions.

One entry per distro: its identity (the Distro enum, the CLI boundary
type), its source-build environment, and — where a native package product
exists — its packaging definition. Family-shared data (dependency lists,
repo setup, install-test commands) is factored into module constants so
each entry carries only what genuinely distinguishes it.

This table is the source of truth for the pipeline. It was seeded from
.github/data/distros.yml, packaging/installer/install-required-packages.sh,
and netdata/helper-images (package-builders and legacy Dockerfiles) —
reference material only, never executed or parsed at runtime. Which
distros CI actually runs is NOT expressed here: enumeration belongs to the
declaration layer on top (today ci.py's declared tiers, later the CI
workflow definitions).
"""

from __future__ import annotations

import enum
from collections.abc import Mapping
from dataclasses import dataclass

from dagger import enum_type

from .envs import STD_PATH, EnvSpec, PkgMgr


@enum_type
class Distro(enum.Enum):
    """A specific buildable distro.

    Values are stable identifiers used in cache keys; members are the
    CLI-facing names (dagger call ... --distro=DEBIAN_12).
    """

    ALPINE_EDGE = "alpine-edge"
    ALPINE_3_23 = "alpine-3.23"
    ALPINE_3_22 = "alpine-3.22"
    AMAZONLINUX_2 = "amazonlinux-2"
    AMAZONLINUX_2023 = "amazonlinux-2023"
    ARCHLINUX = "archlinux"
    CENTOS_7 = "centos-7"
    CENTOS_STREAM_9 = "centos-stream-9"
    CENTOS_STREAM_10 = "centos-stream-10"
    DEBIAN_11 = "debian-11"
    DEBIAN_12 = "debian-12"
    DEBIAN_13 = "debian-13"
    FEDORA_43 = "fedora-43"
    FEDORA_44 = "fedora-44"
    OPENSUSE_16_0 = "opensuse-16.0"
    OPENSUSE_TUMBLEWEED = "opensuse-tumbleweed"
    ORACLELINUX_8 = "oraclelinux-8"
    ORACLELINUX_9 = "oraclelinux-9"
    ORACLELINUX_10 = "oraclelinux-10"
    ROCKYLINUX_8 = "rockylinux-8"
    ROCKYLINUX_9 = "rockylinux-9"
    ROCKYLINUX_10 = "rockylinux-10"
    UBUNTU_22_04 = "ubuntu-22.04"
    UBUNTU_24_04 = "ubuntu-24.04"
    UBUNTU_25_10 = "ubuntu-25.10"
    UBUNTU_26_04 = "ubuntu-26.04"


class RpmProtobuf(enum.StrEnum):
    """How the RPM build links protobuf.

    RPM distros ship no static protobuf, and netdata's cmake prefers
    static libs when BUILD_SHARED_LIBS is unset — so each RPM entry states
    its resolution explicitly (mirroring what netdata.spec.in does).
    """

    SHARED = "shared"  # hint the shared lib: /usr/lib64/libprotobuf.so
    BUNDLED = "bundled"  # build the vendored protobuf (opensuse)


@dataclass(frozen=True)
class PkgFeatures:
    """Optional-library features of the packaging build.

    Declares what the packaging environment supports; the cpack configure
    step (packaging_configure_args) drives the build from these directly,
    for DEB and RPM alike.
    """

    mongodb: bool
    nfacct: bool
    # Applies on 64-bit targets only; the configure step narrows it.
    xenstat: bool
    freeipmi: bool


@dataclass(frozen=True)
class DebPackaging:
    env: EnvSpec
    features: PkgFeatures
    # Clean-image package install command for the round-trip test.
    test_install: str
    # Exact PREBUILT_DISTRO stamp written into .install-type.
    prebuilt_distro: str
    # Extra env deps that only exist/matter on 64-bit targets (libxen).
    deps_64bit: tuple[str, ...] = ()


@dataclass(frozen=True)
class RpmPackaging:
    env: EnvSpec
    features: PkgFeatures
    test_install: str
    prebuilt_distro: str
    protobuf: RpmProtobuf = RpmProtobuf.SHARED
    # The spec's centos_ver == 7 tier (EL <= 7, Amazon Linux 2): CUPS and
    # the systemd-units plugin are too old to build, C++17 and modern
    # libbpf do not compile against the toolchain and kernel headers.
    legacy: bool = False


@dataclass(frozen=True)
class DistroSpec:
    """The complete definition of one distro."""

    build: EnvSpec
    # Whether the source-build feature profile enables the systemd
    # journal/units plugins (needs usable libsystemd headers; musl distros
    # and systemd-219-era distros stay off).
    systemd: bool = True
    packaging: DebPackaging | RpmPackaging | None = None


# --- source-build dependency lists (install-required-packages.sh parity) ----

_ALPINE_DEPS = (
    "alpine-sdk",
    "ccache",
    "bison",
    "cmake",
    "coreutils",
    "curl",
    "curl-dev",
    "elfutils-dev",
    "flex",
    "g++",
    "gcc",
    "git",
    "gzip",
    "json-c-dev",
    "libatomic",
    "libmnl-dev",
    "libuv-dev",
    "lz4-dev",
    "make",
    "openssl-dev",
    "patch",
    "pcre2-dev",
    "pkgconf",
    "python3",
    "tar",
    "util-linux-dev",
    "yaml-dev",
    "zlib-dev",
    "zstd-dev",
)

_ARCH_DEPS = (
    "bison",
    "ccache",
    "cmake",
    "curl",
    "flex",
    "gcc",
    "git",
    "gzip",
    "json-c",
    "libelf",
    "libmnl",
    "libuv",
    "libyaml",
    "lz4",
    "make",
    "openssl",
    "pcre2",
    "pkgconfig",
    "python3",
    "tar",
    "util-linux",
    "zlib",
)

_DEB_DEPS = (
    "bison",
    "ccache",
    # Not in the CI-resolved list: there apt runs without
    # --no-install-recommends and certs arrive via Recommends. We install
    # lean, so TLS trust must be explicit (debian images ship none).
    "ca-certificates",
    "cmake",
    "curl",
    "flex",
    "g++",
    "gcc",
    "git",
    "gzip",
    "libatomic1",
    "libcurl4-openssl-dev",
    "libelf-dev",
    "libjson-c-dev",
    "liblz4-dev",
    "libmnl-dev",
    "libpcre2-dev",
    "libssl-dev",
    "libsystemd-dev",
    "libuv1-dev",
    "libyaml-dev",
    "libzstd-dev",
    "make",
    "patch",
    "pkg-config",
    "python3",
    "tar",
    "uuid-dev",
    "zlib1g-dev",
)

_FEDORA_DEPS = (
    "bison",
    "ccache",
    "cmake",
    "curl",
    "elfutils-libelf-devel",
    "findutils",
    "flex",
    "gcc",
    "gcc-c++",
    "git",
    "gzip",
    "json-c-devel",
    "libatomic",
    "libcurl-devel",
    "libmnl-devel",
    "libuuid-devel",
    "libuv-devel",
    "libyaml-devel",
    "libzstd-devel",
    "lz4-devel",
    "make",
    "openssl-devel",
    "patch",
    "pcre2-devel",
    "pkgconfig",
    "python3",
    "systemd-devel",
    "tar",
    "zlib-devel",
)

# EL rebuilds (CentOS Stream, Rocky) additionally need kernel headers;
# ccache is EPEL-only there (enabled in the packaging envs, not here).
_EL_DEPS = (*(p for p in _FEDORA_DEPS if p != "ccache"), "kernel-headers")

# Oracle Linux repos lack findutils/pcre2-devel in the resolved set.
_OL_DEPS = tuple(p for p in _FEDORA_DEPS if p not in ("findutils", "pcre2-devel", "ccache"))

_SUSE_DEPS = (
    "bison",
    "ccache",
    "cmake",
    "curl",
    "flex",
    "gcc",
    "gcc-c++",
    "git",
    "gzip",
    "libatomic1",
    "libcurl-devel",
    "libelf-devel",
    "libjson-c-devel",
    "liblz4-devel",
    "libmnl-devel",
    "libopenssl-devel",
    "libuuid-devel",
    "libuv-devel",
    "libyaml-devel",
    "libzstd-devel",
    "make",
    "pcre2-devel",
    "pkg-config",
    "python3",
    "systemd-devel",
    "tar",
    "zlib-devel",
)

# --- packaging dependency lists (helper-images package-builders parity) -----

_DEB_PKG_DEPS = (
    "bison",
    "ccache",
    "build-essential",
    "ca-certificates",
    "clang",
    "cmake",
    "curl",
    "dpkg-dev",
    "file",
    "flex",
    "g++",
    "gcc",
    "git-core",
    "libatomic1",
    "libcups2-dev",
    "libcurl4-openssl-dev",
    "libdistro-info-perl",
    "libelf-dev",
    "libipmimonitoring-dev",
    "libjson-c-dev",
    "libjudy-dev",
    "liblz4-dev",
    "libmnl-dev",
    "libmongoc-dev",
    "libnetfilter-acct-dev",
    "libpcre2-dev",
    "libprotobuf-dev",
    "libprotoc-dev",
    "libsnappy-dev",
    "libssl-dev",
    "libsystemd-dev",
    "libunwind-dev",
    "libuv1-dev",
    "libyaml-dev",
    "libzstd-dev",
    "make",
    "ninja-build",
    "patch",
    "pkg-config",
    "protobuf-compiler",
    "systemd",
    "unixodbc-dev",
    "uuid-dev",
    "wget",
    "zlib1g-dev",
)

_FEDORA_PKG_DEPS = (
    "bash",
    "ccache",
    "bison",
    "clang",
    "cmake",
    "cups-devel",
    "curl",
    "diffutils",
    "elfutils-libelf-devel",
    "findutils",
    "flex",
    "freeipmi-devel",
    "gcc",
    "gcc-c++",
    "git-core",
    "gzip",
    "json-c-devel",
    "Judy-devel",
    "libatomic",
    "libcurl-devel",
    "libmnl-devel",
    "libnetfilter_acct-devel",
    "libunwind-devel",
    "libuuid-devel",
    "libuv-devel",
    "libyaml-devel",
    "libzstd-devel",
    "lz4-devel",
    "make",
    "ninja-build",
    "openssl-devel",
    "openssl-perl",
    "patch",
    "pcre2-devel",
    "pkgconfig",
    "pkgconfig(libmongoc-1.0)",
    "pkgconfig(odbc)",
    "procps",
    "protobuf-c-devel",
    "protobuf-compiler",
    "protobuf-devel",
    # CPack's RPM generator shells out to rpmbuild.
    "rpm-build",
    "snappy-devel",
    "systemd-devel",
    "systemd-rpm-macros",
    "tar",
    "xen-devel",
    "zlib-devel",
)

# EL rebuilds (Rocky, CentOS Stream, Oracle): no xen, no netfilter_acct,
# no Judy in the distro repos.
_EL_PKG_DEPS = (
    *tuple(
        p
        for p in _FEDORA_PKG_DEPS
        if p not in ("xen-devel", "libnetfilter_acct-devel", "Judy-devel")
    ),
    "lm_sensors",
    "nc",
    "python3",
    "python3-pyyaml",
    "wget",
)

# Amazon Linux 2023 additionally lacks freeipmi and mongoc.
_AL2023_PKG_DEPS = (
    *tuple(
        p
        for p in _EL_PKG_DEPS
        if p not in ("freeipmi-devel", "pkgconfig(libmongoc-1.0)", "lm_sensors", "nc", "ccache")
    ),
    "bison-devel",
    "flex-devel",
)

_SUSE_PKG_DEPS = (
    "autoconf",
    "ccache",
    "automake",
    "bison",
    "clang",
    "cmake",
    "cups",
    "cups-devel",
    "curl",
    "diffutils",
    "flex",
    "freeipmi-devel",
    "gcc",
    "gcc-c++",
    "git-core",
    "json-glib-devel",
    "judy-devel",
    "libatomic1",
    "libcurl-devel",
    "libelf-devel",
    "libjson-c-devel",
    "liblz4-devel",
    "libmnl-devel",
    "libopenssl-devel",
    "libpcre2-8-0",
    "libtool",
    "libunwind-devel",
    "libuuid-devel",
    "libuv-devel",
    "libyaml-devel",
    "libzstd-devel",
    "make",
    "ninja",
    "patch",
    "pkg-config",
    "protobuf-c",
    "protobuf-devel",
    # CPack's RPM generator shells out to rpmbuild; on openSUSE
    # rpmdevtools would not pull it in (helper-images hit this).
    "rpm-build",
    "snappy-devel",
    "systemd-devel",
    "systemd-rpm-macros",
    "tar",
    "unixODBC-devel",
    "wget",
)

# Tumbleweed carries netfilter_acct and xen on top of the Leap set.
_TUMBLEWEED_EXTRA = ("libnetfilter_acct1", "libnetfilter_acct-devel", "xen-devel")

# centos7 / amazonlinux2 packaging set (helper-images Dockerfile.centos7.v1
# and Dockerfile.amazonlinux2.v1 — the lists are identical modulo the SCL
# toolchain, so the shared part lives here).
_LEGACY_RPM_PKG_DEPS = (
    "autoconf",
    "autoconf-archive",
    "autogen",
    "automake",
    "bison",
    "bison-devel",
    "clang",
    "cmake",
    "cups-devel",
    "curl",
    "diffutils",
    "elfutils-libelf-devel",
    "findutils",
    "flex",
    "flex-devel",
    "freeipmi-devel",
    "gcc",
    "gcc-c++",
    "git-core",
    "json-c-devel",
    "libyaml-devel",
    "libatomic",
    "libcurl-devel",
    "libmnl-devel",
    "libnetfilter_acct-devel",
    "libtool",
    "libunwind-devel",
    "libuuid-devel",
    "libuv-devel",
    "libzstd-devel",
    "lm_sensors",
    "lz4-devel",
    "make",
    "ninja-build",
    "openssl-devel",
    "openssl-perl",
    "patch",
    "pcre2-devel",
    "pkgconfig",
    "pkgconfig(libmongoc-1.0)",
    "pkgconfig(odbc)",
    "procps",
    "protobuf-c-devel",
    "protobuf-compiler",
    "protobuf-devel",
    # CPack's RPM generator shells out to rpmbuild. No systemd-rpm-macros
    # here: on EL 7 the %systemd_* scriptlet macros come from the base
    # systemd package, and EPEL's compat package would add a sysusers
    # file-attribute generator that changes the built packages' Provides.
    "rpm-build",
    "snappy-devel",
    "systemd-devel",
    "wget",
    "zlib-devel",
)

_C7_PKG_DEPS = (*_LEGACY_RPM_PKG_DEPS, "bash", "devtoolset-11")

# Source builds do not package; they carry no rpm tooling.
_RPM_TOOLING = ("rpm-build",)

_C7_BUILD_DEPS = tuple(p for p in _C7_PKG_DEPS if p not in _RPM_TOOLING)
_AL2_BUILD_DEPS = tuple(p for p in _LEGACY_RPM_PKG_DEPS if p not in _RPM_TOOLING)

# --- repo setup, prep, and vendored repo data --------------------------------

_APT_UPDATE = "apt-get update\n"
_UBUNTU_PREP = "rm -f /etc/apt/apt.conf.d/docker && apt-get update\n"
_ALPINE_PREP = "apk add -U bash\n"
_ARCH_PREP = "pacman --noconfirm -Syu && pacman --noconfirm -Sy grep libffi\n"

_DNF_CONFIG_MANAGER = ("dnf", "install", "-y", "dnf-command(config-manager)")

_CRB_SETUP = (
    _DNF_CONFIG_MANAGER,
    ("dnf", "config-manager", "--set-enabled", "crb"),
)

_ROCKY8_SETUP = (
    _DNF_CONFIG_MANAGER,
    ("dnf", "config-manager", "--set-enabled", "powertools"),
    ("dnf", "install", "-y", "libarchive"),
)

_EPEL_SETUP = (("dnf", "install", "-y", "epel-release"),)

_OL8_CODEREADY_REPO = """\
[ol8_codeready_builder]
name=Oracle Linux $releasever CodeReady Builder ($basearch)
baseurl=http://yum.oracle.com/repo/OracleLinux/OL8/codeready/builder/$basearch
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-oracle
gpgcheck=1
enabled=1
"""

# Oracle's EPEL rebuild (helper-images ships the same definition).
_OL_DEVELOPER_EPEL_REPO = """\
[ol{major}_developer_EPEL]
name=Oracle Linux $releasever EPEL Packages for Development ($basearch)
baseurl=https://yum$ociregion.$ocidomain/repo/OracleLinux/OL{major}/developer/EPEL/$basearch/
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-oracle
gpgcheck=1
enabled=1
"""


def _ol_epel_file(major: str) -> tuple[str, str]:
    return (
        f"/etc/yum.repos.d/ol{major}-epel.repo",
        _OL_DEVELOPER_EPEL_REPO.format(major=major),
    )


# CentOS 7 is EOL: the mirror network is gone and packages live on
# vault.centos.org. These replace/extend the stock repo definitions
# (vendored from netdata/helper-images legacy + package-builders, which we
# are retiring).
_C7_VAULT_REPO = """\
[base]
name=CentOS-$releasever - Base
baseurl=http://vault.centos.org/centos/$releasever/os/$basearch/
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-7

[updates]
name=CentOS-$releasever - Updates
baseurl=http://vault.centos.org/centos/$releasever/updates/$basearch/
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-7

[extras]
name=CentOS-$releasever - Extras
baseurl=http://vault.centos.org/centos/$releasever/extras/$basearch/
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-7
"""

# SCLo provides devtoolset-11 (gcc 11); system gcc 4.8 cannot build netdata.
_C7_SCLO_REPO = """\
[centos-sclo-sclo]
name=CentOS-7 - SCLo sclo
baseurl=http://vault.centos.org/centos/7.9.2009/sclo/$basearch/sclo/
gpgcheck=1
enabled=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-SIG-SCLo

[centos-sclo-rh]
name=CentOS-7 - SCLo rh
baseurl=http://vault.centos.org/centos/7.9.2009/sclo/$basearch/rh/
gpgcheck=1
enabled=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-SIG-SCLo
"""

_C7_SCLO_GPG_KEY = """\
-----BEGIN PGP PUBLIC KEY BLOCK-----
Version: GnuPG v2.0.22 (GNU/Linux)

mQENBFYM/AoBCADR9Q5cb+H5ndx+QkzNBQ88wcD+g112yvnHNlSiBMOnNEGHuKPJ
tujZ+eWXP3K6ucJckT91WxfQ2fxPr9jQ0xpZytcHcZdTfn3vKL9+OwR0npp+qmcz
rK8/EzVz/SWSgBQ5xT/HUvaeoVAbzBHSng0r2njnBAqABKAoTxgyRGKSCWduKD32
7PF2ZpqeDFFhd99Ykt6ar8SlV8ToqH6F7An0ILeejINVbHUxd6+wsbpcOwQ4mGAa
/CPXeqqLGj62ASBv36xQr34hlN/9zQMViaKkacl8zkuvwhuHf4b4VlGVCe6VILpQ
8ytKMV/lcg7YpMfRq4KVWBjCwkvk6zg6KxaHABEBAAG0aENlbnRPUyBTb2Z0d2Fy
ZUNvbGxlY3Rpb25zIFNJRyAoaHR0cHM6Ly93aWtpLmNlbnRvcy5vcmcvU3BlY2lh
bEludGVyZXN0R3JvdXAvU0NMbykgPHNlY3VyaXR5QGNlbnRvcy5vcmc+iQE5BBMB
AgAjBQJWDPwKAhsDBwsJCAcDAgEGFQgCCQoLBBYCAwECHgECF4AACgkQTrhOcfLu
nVXNewgAg7RVclomjTY4w80XiztUuUaFlCHyR76KazdaGfx/8XckWH2GdQtwii+3
Tg7+PT2H0Xyuj1aod+jVTPXTPVUr+rEHAjuNDY+xyAJrNljoOHiz111zs9pk7PLX
CPwKWQLnmrcKIi8v/51L79FFsUMvhClTBdLUQ51lkCwbcXQi+bOhPvZTVbRhjoB/
a9z0d8t65X16zEzE7fBhnVoj4xye/MPMbTH41Mv+FWVciBTuAepOLmgJ9oxODliv
rgZa28IEWkvHQ8m9GLJ0y9mI6olh0cGFybnd5y4Ss1cMttlRGR4qthLhN2gHZpO9
2y4WgkeVXCj1BK1fzVrDMLPbuNNCZQ==
=UtPD
-----END PGP PUBLIC KEY BLOCK-----
"""

_C7_FILES = (
    ("/etc/yum.repos.d/CentOS-Base.repo", _C7_VAULT_REPO),
    ("/etc/yum.repos.d/centos-7-scl.repo", _C7_SCLO_REPO),
    ("/etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-SIG-SCLo", _C7_SCLO_GPG_KEY),
)

# Pinned CMake release, installed to /opt with a /cmake symlink. RPM
# packaging needs CMake >= 4.1 (CPack emits the weak dependencies from 4.1
# on; the top-level CMakeLists.txt refuses older ones), and the centos7/AL2
# system cmake is far too old to build netdata at all. Same version, hashes
# (from Kitware's cmake-4.1.6-SHA-256.txt release asset), and layout as
# helper-images' scripts/install-cmake.sh. Runs as the env's post step: the
# download and unpack need curl/tar/gzip, which some base images only carry
# after the dependency install.
_CMAKE_PIN_STEP = """\
set -e
case "$(uname -m)" in
x86_64) sum=d5c2e72820e01f1c3a07092d0a29e209263a7d22f55b4ad7f414ee870ae6b8e0 ;;
aarch64) sum=8b3e3af8e4b4e95224a4490f4772adb7a512be34c8380fe76e7be003e0fd4394 ;;
*) echo "no pinned cmake for $(uname -m)"; exit 1 ;;
esac
tarball="cmake-4.1.6-linux-$(uname -m).tar.gz"
curl --fail -sSL --connect-timeout 20 --retry 3 --max-time 600 \
    --output "/tmp/${tarball}" \
    "https://github.com/Kitware/CMake/releases/download/v4.1.6/${tarball}"
echo "${sum}  /tmp/${tarball}" | sha256sum -c -
tar -xzf "/tmp/${tarball}" -C /opt
rm -f "/tmp/${tarball}"
test -x "/opt/cmake-4.1.6-linux-$(uname -m)/bin/cmake"
ln -sT "/opt/cmake-4.1.6-linux-$(uname -m)" /cmake
"""

# PATH for environments whose cmake is the pinned /cmake install.
_PINNED_CMAKE_PATH = (("PATH", f"/cmake/bin:{STD_PATH}"),)

_C7_ENV = (
    ("PATH", f"/opt/rh/devtoolset-11/root/usr/bin:/cmake/bin:{STD_PATH}"),
    (
        "LD_LIBRARY_PATH",
        "/opt/rh/devtoolset-11/root/usr/lib64:/opt/rh/devtoolset-11/root/usr/lib",
    ),
)

_C7_SETUP = (
    ("yum", "install", "-y", "epel-release"),
    ("yum", "update", "-y"),
)
_AL2_SETUP = (
    ("yum", "update", "-y"),
    # The pinned-cmake post step needs tar/gzip; the amazonlinux:2 base
    # image has neither and the dependency list does not pull them in.
    ("yum", "install", "-y", "tar", "gzip"),
)

# --- clean-image package install commands (round-trip test) ------------------

_DEB_TEST_INSTALL = (
    "apt-get update && apt-get install -y $(find /artifacts -type f -name 'netdata*.deb'"
    " ! -name '*dbgsym*' ! -name '*cups*' ! -name '*freeipmi*') && "
    "apt-get install -y --no-install-recommends curl jq"
)
_FEDORA_TEST_INSTALL = "dnf install -y /artifacts/netdata*.rpm && dnf install -y curl jq"
_EL_TEST_INSTALL = (
    "dnf install -y epel-release && dnf install -y /artifacts/netdata*.rpm && "
    "dnf install -y --allowerasing curl jq"
)
_AL2023_TEST_INSTALL = (
    "dnf install -y --allowerasing /artifacts/netdata*.rpm && dnf install -y --allowerasing curl jq"
)
_AL2_TEST_INSTALL = "yum install -y /artifacts/netdata*.rpm && yum install -y curl jq"
_C7_TEST_INSTALL = (
    "yum install -y epel-release && yum install -y /artifacts/netdata*.rpm && "
    "yum install -y curl jq"
)
_SUSE_TEST_INSTALL = (
    "zypper install -y --allow-downgrade --allow-unsigned-rpm /artifacts/netdata*.rpm"
    " && zypper install -y --allow-downgrade --no-recommends curl jq"
)

# --- feature sets ------------------------------------------------------------

_DEB_FEATURES = PkgFeatures(mongodb=True, nfacct=True, xenstat=True, freeipmi=True)
# Old Ubuntu LTS libmongoc is too old for the exporter.
_UBUNTU_LTS_FEATURES = PkgFeatures(mongodb=False, nfacct=True, xenstat=True, freeipmi=True)
_FEDORA_FEATURES = PkgFeatures(mongodb=True, nfacct=True, xenstat=True, freeipmi=True)
_EL_FEATURES = PkgFeatures(mongodb=True, nfacct=False, xenstat=False, freeipmi=True)
# The spec's _have_mongo_exporter is 0 on oraclelinux (unlike EL rebuilds).
_OL_FEATURES = PkgFeatures(mongodb=False, nfacct=False, xenstat=False, freeipmi=True)
# The spec's _have_nfacct is 0 for every centos_ver/amzn; _have_mongo_exporter
# is 0 on amzn but 1 on centos 7.
_C7_FEATURES = PkgFeatures(mongodb=True, nfacct=False, xenstat=False, freeipmi=True)
_AL2_FEATURES = PkgFeatures(mongodb=False, nfacct=False, xenstat=False, freeipmi=True)
_AL2023_FEATURES = PkgFeatures(mongodb=False, nfacct=False, xenstat=False, freeipmi=False)
# Leap 16 and Tumbleweed alike: the spec disables nfacct and xenstat for
# suse_version >= 1600, and mongodb on every suse.
_SUSE_FEATURES = PkgFeatures(mongodb=False, nfacct=False, xenstat=False, freeipmi=True)

# --- helpers for the repetitive families -------------------------------------


def _deb_family(
    image: str,
    prep: str,
    features: PkgFeatures,
    prebuilt_distro: str,
) -> DistroSpec:
    return DistroSpec(
        build=EnvSpec(image, PkgMgr.APT, _DEB_DEPS, prep=prep),
        packaging=DebPackaging(
            env=EnvSpec(image, PkgMgr.APT, _DEB_PKG_DEPS, prep=prep),
            features=features,
            test_install=_DEB_TEST_INSTALL,
            prebuilt_distro=prebuilt_distro,
            # xenstat is only built on 64-bit targets; the Dockerfiles
            # install libxen-dev arch-conditionally too.
            deps_64bit=("libxen-dev",),
        ),
    )


def _fedora(image: str, prebuilt_distro: str) -> DistroSpec:
    return DistroSpec(
        build=EnvSpec(image, PkgMgr.DNF, _FEDORA_DEPS),
        packaging=RpmPackaging(
            env=EnvSpec(
                image,
                PkgMgr.DNF,
                _FEDORA_PKG_DEPS,
                post=_CMAKE_PIN_STEP,
                env=_PINNED_CMAKE_PATH,
            ),
            features=_FEDORA_FEATURES,
            test_install=_FEDORA_TEST_INSTALL,
            prebuilt_distro=prebuilt_distro,
        ),
    )


def _el_family(
    image: str,
    setup: tuple[tuple[str, ...], ...],
    prebuilt_distro: str,
    pkg_deps: tuple[str, ...] = _EL_PKG_DEPS,
    features: PkgFeatures = _EL_FEATURES,
) -> DistroSpec:
    """CentOS Stream / Rocky: CRB-style repos + EPEL for packaging."""
    return DistroSpec(
        build=EnvSpec(image, PkgMgr.DNF, _EL_DEPS, setup=setup, install_flags=("--allowerasing",)),
        packaging=RpmPackaging(
            # Packaging needs EPEL (libunwind-devel, libmongoc); the
            # source-build envs deliberately do not enable it.
            env=EnvSpec(
                image,
                PkgMgr.DNF,
                pkg_deps,
                setup=(*setup, *_EPEL_SETUP),
                install_flags=("--allowerasing",),
                post=_CMAKE_PIN_STEP,
                env=_PINNED_CMAKE_PATH,
            ),
            features=features,
            test_install=_EL_TEST_INSTALL,
            prebuilt_distro=prebuilt_distro,
        ),
    )


def _oraclelinux(
    image: str,
    files: tuple[tuple[str, str], ...],
    setup: tuple[tuple[str, ...], ...],
    pkg_files: tuple[tuple[str, str], ...],
    pkg_setup: tuple[tuple[str, ...], ...],
    prebuilt_distro: str,
) -> DistroSpec:
    """Oracle Linux: per-release codeready + EPEL mechanisms differ, so
    the packaging env's files/setup are declared explicitly per entry."""
    return DistroSpec(
        build=EnvSpec(image, PkgMgr.DNF, _OL_DEPS, files=files, setup=setup),
        packaging=RpmPackaging(
            env=EnvSpec(
                image,
                PkgMgr.DNF,
                _EL_PKG_DEPS,
                files=pkg_files,
                setup=pkg_setup,
                post=_CMAKE_PIN_STEP,
                env=_PINNED_CMAKE_PATH,
            ),
            features=_OL_FEATURES,
            test_install=_FEDORA_TEST_INSTALL,
            prebuilt_distro=prebuilt_distro,
        ),
    )


def _opensuse(image: str, extra: tuple[str, ...], features: PkgFeatures, stamp: str) -> DistroSpec:
    return DistroSpec(
        build=EnvSpec(image, PkgMgr.ZYPPER, _SUSE_DEPS),
        packaging=RpmPackaging(
            env=EnvSpec(
                image,
                PkgMgr.ZYPPER,
                (*_SUSE_PKG_DEPS, *extra),
                install_flags=("--allow-downgrade",),
                post=_CMAKE_PIN_STEP,
                env=_PINNED_CMAKE_PATH,
            ),
            features=features,
            test_install=_SUSE_TEST_INSTALL,
            prebuilt_distro=stamp,
            protobuf=RpmProtobuf.BUNDLED,
        ),
    )


# --- the definitions ---------------------------------------------------------

SPECS: Mapping[Distro, DistroSpec] = {
    # musl: no systemd; journal/units plugins stay off in the source profile.
    Distro.ALPINE_EDGE: DistroSpec(
        build=EnvSpec("alpine:edge", PkgMgr.APK, _ALPINE_DEPS, prep=_ALPINE_PREP),
        systemd=False,
    ),
    Distro.ALPINE_3_23: DistroSpec(
        build=EnvSpec("alpine:3.23", PkgMgr.APK, _ALPINE_DEPS, prep=_ALPINE_PREP),
        systemd=False,
    ),
    Distro.ALPINE_3_22: DistroSpec(
        build=EnvSpec("alpine:3.22", PkgMgr.APK, _ALPINE_DEPS, prep=_ALPINE_PREP),
        systemd=False,
    ),
    # No libsystemd headers in the dependency set.
    Distro.ARCHLINUX: DistroSpec(
        build=EnvSpec("archlinux:latest", PkgMgr.PACMAN, _ARCH_DEPS, prep=_ARCH_PREP),
        systemd=False,
    ),
    # systemd 219: too old for the journal plugin's API use.
    Distro.AMAZONLINUX_2: DistroSpec(
        build=EnvSpec(
            "amazonlinux:2",
            PkgMgr.YUM,
            _AL2_BUILD_DEPS,
            setup=_AL2_SETUP,
            post=_CMAKE_PIN_STEP,
            env=_PINNED_CMAKE_PATH,
        ),
        systemd=False,
        packaging=RpmPackaging(
            env=EnvSpec(
                "amazonlinux:2",
                PkgMgr.YUM,
                _LEGACY_RPM_PKG_DEPS,
                setup=_AL2_SETUP,
                post=_CMAKE_PIN_STEP,
                env=_PINNED_CMAKE_PATH,
            ),
            features=_AL2_FEATURES,
            test_install=_AL2_TEST_INSTALL,
            prebuilt_distro="amazonlinux 2",
            # The legacy tier bundles protobuf (spec: centos_ver < 8, which
            # covers AL2 via its %rhel 7 remap); the system one is 2.5.
            protobuf=RpmProtobuf.BUNDLED,
            legacy=True,
        ),
    ),
    Distro.AMAZONLINUX_2023: DistroSpec(
        build=EnvSpec(
            "amazonlinux:2023",
            PkgMgr.DNF,
            tuple(p for p in _FEDORA_DEPS if p != "ccache"),
            install_flags=("--allowerasing",),
        ),
        packaging=RpmPackaging(
            env=EnvSpec(
                "amazonlinux:2023",
                PkgMgr.DNF,
                _AL2023_PKG_DEPS,
                install_flags=("--allowerasing",),
                post=_CMAKE_PIN_STEP,
                env=_PINNED_CMAKE_PATH,
            ),
            features=_AL2023_FEATURES,
            test_install=_AL2023_TEST_INSTALL,
            prebuilt_distro="amazonlinux 2023",
        ),
    ),
    # EOL distro on vault mirrors; gcc via SCLo devtoolset-11; systemd 219.
    Distro.CENTOS_7: DistroSpec(
        build=EnvSpec(
            "centos:7",
            PkgMgr.YUM,
            _C7_BUILD_DEPS,
            files=_C7_FILES,
            setup=_C7_SETUP,
            post=_CMAKE_PIN_STEP,
            env=_C7_ENV,
        ),
        systemd=False,
        packaging=RpmPackaging(
            env=EnvSpec(
                "centos:7",
                PkgMgr.YUM,
                _C7_PKG_DEPS,
                files=_C7_FILES,
                setup=_C7_SETUP,
                post=_CMAKE_PIN_STEP,
                env=_C7_ENV,
            ),
            features=_C7_FEATURES,
            test_install=_C7_TEST_INSTALL,
            prebuilt_distro="centos 7",
            # The legacy tier bundles protobuf (spec: centos_ver < 8); the
            # system one is 2.5.
            protobuf=RpmProtobuf.BUNDLED,
            legacy=True,
        ),
    ),
    Distro.CENTOS_STREAM_9: _el_family(
        "quay.io/centos/centos:stream9", _CRB_SETUP, "centos-stream 9"
    ),
    # EPEL 10's Stream view lacks mongo-c-driver-devel (checked
    # 2026-07-18; Rocky 10's EPEL has it) — no mongodb exporter here.
    # helper-images' cs10 builder installs it and so presumably fails
    # upstream today (tracked as a follow-up finding).
    Distro.CENTOS_STREAM_10: _el_family(
        "quay.io/centos/centos:stream10",
        _CRB_SETUP,
        "centos-stream 10",
        pkg_deps=tuple(p for p in _EL_PKG_DEPS if p != "pkgconfig(libmongoc-1.0)"),
        features=PkgFeatures(mongodb=False, nfacct=False, xenstat=False, freeipmi=True),
    ),
    Distro.DEBIAN_11: _deb_family("debian:bullseye", _APT_UPDATE, _DEB_FEATURES, "debian 11"),
    Distro.DEBIAN_12: _deb_family("debian:bookworm", _APT_UPDATE, _DEB_FEATURES, "debian 12"),
    Distro.DEBIAN_13: _deb_family("debian:trixie", _APT_UPDATE, _DEB_FEATURES, "debian 13"),
    Distro.FEDORA_43: _fedora("fedora:43", "fedora 43"),
    Distro.FEDORA_44: _fedora("fedora:44", "fedora 44"),
    Distro.OPENSUSE_16_0: _opensuse("opensuse/leap:16.0", (), _SUSE_FEATURES, "opensuse 16.0"),
    # PREBUILT_DISTRO carries no version for rolling releases (trailing
    # space is the shipped format).
    Distro.OPENSUSE_TUMBLEWEED: _opensuse(
        "opensuse/tumbleweed", _TUMBLEWEED_EXTRA, _SUSE_FEATURES, "opensuse "
    ),
    Distro.ORACLELINUX_8: _oraclelinux(
        "oraclelinux:8",
        files=(("/etc/yum.repos.d/ol8_codeready.repo", _OL8_CODEREADY_REPO),),
        setup=(),
        pkg_files=(
            ("/etc/yum.repos.d/ol8_codeready.repo", _OL8_CODEREADY_REPO),
            _ol_epel_file("8"),
        ),
        pkg_setup=(),
        prebuilt_distro="oraclelinux 8",
    ),
    Distro.ORACLELINUX_9: _oraclelinux(
        "oraclelinux:9",
        files=(),
        setup=(("dnf", "config-manager", "--set-enabled", "ol9_codeready_builder"),),
        pkg_files=(_ol_epel_file("9"),),
        pkg_setup=(("dnf", "config-manager", "--set-enabled", "ol9_codeready_builder"),),
        prebuilt_distro="oraclelinux 9",
    ),
    # OL10 ships EPEL as a package (oracle-epel-release-el10); no
    # hand-written developer-EPEL repo file exists for it.
    Distro.ORACLELINUX_10: _oraclelinux(
        "oraclelinux:10",
        files=(),
        setup=(("dnf", "config-manager", "--set-enabled", "ol10_codeready_builder"),),
        pkg_files=(),
        pkg_setup=(
            ("dnf", "config-manager", "--set-enabled", "ol10_codeready_builder"),
            ("dnf", "install", "-y", "oracle-epel-release-el10"),
        ),
        prebuilt_distro="oraclelinux 10",
    ),
    Distro.ROCKYLINUX_8: _el_family("rockylinux:8", _ROCKY8_SETUP, "rockylinux 8"),
    Distro.ROCKYLINUX_9: _el_family("rockylinux:9", _CRB_SETUP, "rockylinux 9"),
    Distro.ROCKYLINUX_10: _el_family(
        "quay.io/rockylinux/rockylinux:10", _CRB_SETUP, "rockylinux 10"
    ),
    Distro.UBUNTU_22_04: _deb_family(
        "ubuntu:22.04", _UBUNTU_PREP, _UBUNTU_LTS_FEATURES, "ubuntu 22.04"
    ),
    Distro.UBUNTU_24_04: _deb_family(
        "ubuntu:24.04", _UBUNTU_PREP, _UBUNTU_LTS_FEATURES, "ubuntu 24.04"
    ),
    Distro.UBUNTU_25_10: _deb_family("ubuntu:25.10", _UBUNTU_PREP, _DEB_FEATURES, "ubuntu 25.10"),
    Distro.UBUNTU_26_04: _deb_family("ubuntu:26.04", _UBUNTU_PREP, _DEB_FEATURES, "ubuntu 26.04"),
}

# Every Distro member must be defined — enforced at import so a new enum
# member without a definition fails immediately, everywhere.
_missing = [d.value for d in Distro if d not in SPECS]
assert not _missing, f"distros without definitions: {_missing}"
