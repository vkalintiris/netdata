"""Build environments for the Netdata agent pipeline.

Everything a distro needs to compile the agent from source lives here as
typed data: package lists per family, repository enablement, and pinned
toolchains (Go, Rust). Seeded from packaging/installer/
install-required-packages.sh (the `netdata` bundle, as CI source builds
resolve it) and packaging/check-for-go-toolchain.sh (2026-07-16); those
scripts are reference material only and are never executed by this module.
"""

from __future__ import annotations

import enum
from dataclasses import dataclass

import dagger
from dagger import dag

from .matrix import Distro

# Pinned Go toolchain (netdata requires >= 1.26.0).
GO_VERSION = "1.26.5"

# sha256 per Go release architecture.
_GO_SHA256: dict[str, str] = {
    "386": "88c162b204e6eefcc32499453b492e80209f4a4c78c33092636901c540fb0d05",
    "amd64": "5c2c3b16caefa1d968a94c1daca04a7ca301a496d9b086e17ad77bb81393f053",
    "arm64": "fe4789e92b1f33358680864bbe8704289e7bb5fc207d80623c308935bd696d49",
    "armv6l": "6dae9edab81c13bccf962dec15f1fd2ec26c14a6821b4d2c92dab4130c289d7a",
    "ppc64le": "c5d60e2b303bb612f20cd82786594b64874e73b35134025e27d3390bf284ae43",
    "riscv64": "d4a24dd4484d3f86b99c2d300af0dea5d184557e6d61eb7aba19ff61662750e3",
    "s390x": "09ce3c504c0323968b75a717244dca4f25cd4cf0443e5ff6bc0bfa74add89fa7",
}

# Pinned rustup installer.
RUSTUP_TAG = "1.28.1"
_RUSTUP_INIT_URL = (
    f"https://raw.githubusercontent.com/rust-lang/rustup/refs/tags/{RUSTUP_TAG}/rustup-init.sh"
)
_RUSTUP_INIT_SHA256 = "b25b33de9e5678e976905db7f21b42a58fb124dd098b35a962f963734b790a9b"

# Docker platform -> Go release architecture.
_GO_ARCH: dict[str, str] = {
    "linux/amd64": "amd64",
    "linux/386": "386",
    "linux/arm64": "arm64",
    "linux/arm64/v8": "arm64",
    "linux/arm/v6": "armv6l",
    "linux/arm/v7": "armv6l",
    "linux/ppc64le": "ppc64le",
    "linux/riscv64": "riscv64",
    "linux/s390x": "s390x",
}


class PkgMgr(enum.StrEnum):
    APK = "apk"
    APT = "apt"
    DNF = "dnf"
    PACMAN = "pacman"
    ZYPPER = "zypper"


@dataclass(frozen=True)
class EnvSpec:
    mgr: PkgMgr
    deps: tuple[str, ...]
    # Files written before setup runs: (path, contents).
    files: tuple[tuple[str, str], ...] = ()
    # Repo-enablement commands run before installing deps.
    setup: tuple[tuple[str, ...], ...] = ()
    # Extra install flags, e.g. --allowerasing where curl-minimal conflicts
    # with the real curl (EL9+ and Amazon Linux base images).
    install_flags: tuple[str, ...] = ()


_ALPINE_DEPS = (
    "alpine-sdk",
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

# EL rebuilds (CentOS Stream, Rocky) additionally need kernel headers.
_EL_DEPS = (*_FEDORA_DEPS, "kernel-headers")

# Oracle Linux repos lack findutils/pcre2-devel in the resolved set.
_OL_DEPS = tuple(p for p in _FEDORA_DEPS if p not in ("findutils", "pcre2-devel"))

_SUSE_DEPS = (
    "bison",
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

_DNF_CONFIG_MANAGER = ("dnf", "install", "-y", "dnf-command(config-manager)")

_OL8_CODEREADY_REPO = """\
[ol8_codeready_builder]
name=Oracle Linux $releasever CodeReady Builder ($basearch)
baseurl=http://yum.oracle.com/repo/OracleLinux/OL8/codeready/builder/$basearch
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-oracle
gpgcheck=1
enabled=1
"""


def env_spec(d: Distro) -> EnvSpec:
    match d.name:
        case "alpine":
            return EnvSpec(PkgMgr.APK, _ALPINE_DEPS)
        case "archlinux":
            return EnvSpec(PkgMgr.PACMAN, _ARCH_DEPS)
        case "debian" | "ubuntu":
            return EnvSpec(PkgMgr.APT, _DEB_DEPS)
        case "fedora":
            return EnvSpec(PkgMgr.DNF, _FEDORA_DEPS)
        case "amazonlinux":
            return EnvSpec(PkgMgr.DNF, _FEDORA_DEPS, install_flags=("--allowerasing",))
        case "centos-stream":
            return EnvSpec(
                PkgMgr.DNF,
                _EL_DEPS,
                setup=(
                    _DNF_CONFIG_MANAGER,
                    ("dnf", "config-manager", "--set-enabled", "crb"),
                ),
                install_flags=("--allowerasing",),
            )
        case "rockylinux" if d.version == "8":
            return EnvSpec(
                PkgMgr.DNF,
                _EL_DEPS,
                setup=(
                    _DNF_CONFIG_MANAGER,
                    ("dnf", "config-manager", "--set-enabled", "powertools"),
                    ("dnf", "install", "-y", "libarchive"),
                ),
                install_flags=("--allowerasing",),
            )
        case "rockylinux":
            return EnvSpec(
                PkgMgr.DNF,
                _EL_DEPS,
                setup=(
                    _DNF_CONFIG_MANAGER,
                    ("dnf", "config-manager", "--set-enabled", "crb"),
                ),
                install_flags=("--allowerasing",),
            )
        case "oraclelinux" if d.version == "8":
            return EnvSpec(
                PkgMgr.DNF,
                _OL_DEPS,
                files=(("/etc/yum.repos.d/ol8_codeready.repo", _OL8_CODEREADY_REPO),),
            )
        case "oraclelinux":
            return EnvSpec(
                PkgMgr.DNF,
                _OL_DEPS,
                setup=(
                    ("dnf", "config-manager", "--set-enabled", "ol9_codeready_builder"),
                ),
            )
        case "opensuse":
            return EnvSpec(PkgMgr.ZYPPER, _SUSE_DEPS)
        case "centos":
            raise ValueError("centos 7 has no source-build environment (skip-local-build)")
        case _:
            raise ValueError(f"no environment spec for distro {d.name}")


def _install_cmd(spec: EnvSpec) -> list[str]:
    match spec.mgr:
        case PkgMgr.APK:
            return ["apk", "add", "--no-cache", *spec.deps]
        case PkgMgr.APT:
            return ["apt-get", "install", "-y", "--no-install-recommends", *spec.deps]
        case PkgMgr.DNF:
            return ["dnf", "install", "-y", *spec.install_flags, *spec.deps]
        case PkgMgr.PACMAN:
            return ["pacman", "--noconfirm", "--needed", "-S", *spec.deps]
        case PkgMgr.ZYPPER:
            return ["zypper", "install", "-y", *spec.deps]


def go_arch(platform: str) -> str:
    try:
        return _GO_ARCH[platform]
    except KeyError:
        raise ValueError(f"no Go toolchain mapping for platform {platform}") from None


def install_go(ctr: dagger.Container, platform: str) -> dagger.Container:
    arch = go_arch(platform)
    url = f"https://go.dev/dl/go{GO_VERSION}.linux-{arch}.tar.gz"
    sha256 = _GO_SHA256[arch]
    return (
        ctr.with_file("/tmp/go.tar.gz", dag.http(url))
        .with_exec(["sh", "-c", f"echo '{sha256}  /tmp/go.tar.gz' | sha256sum -c -"])
        .with_exec(
            [
                "sh",
                "-c",
                "rm -rf /usr/local/go && tar -C /usr/local -xzf /tmp/go.tar.gz"
                " && rm -f /tmp/go.tar.gz",
            ]
        )
        .with_env_variable("PATH", "/usr/local/go/bin:$PATH", expand=True)
    )


def install_rust(ctr: dagger.Container, platform: str) -> dagger.Container:
    # 32-bit x86 needs the i686 host toolchain spelled out.
    extra = []
    if platform == "linux/386":
        extra = ["--default-toolchain", "stable-i686-unknown-linux-gnu"]
    return (
        ctr.with_file("/tmp/rustup-init.sh", dag.http(_RUSTUP_INIT_URL))
        .with_exec(
            ["sh", "-c", f"echo '{_RUSTUP_INIT_SHA256}  /tmp/rustup-init.sh' | sha256sum -c -"]
        )
        .with_exec(["sh", "/tmp/rustup-init.sh", "-y", "-v", *extra])
        .with_exec(["rm", "-f", "/tmp/rustup-init.sh"])
        .with_env_variable("PATH", "/root/.cargo/bin:$PATH", expand=True)
    )


def build_env(d: Distro, platform: str) -> dagger.Container:
    """Container with everything needed to build the agent from source."""
    spec = env_spec(d)

    ctr = dag.container(platform=dagger.Platform(platform)).from_(d.base_image)

    # Some images (opensuse) define no PATH in their OCI config; docker
    # injects a default at runtime but dagger does not, so set it explicitly.
    ctr = ctr.with_env_variable(
        "PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    )

    if spec.mgr is PkgMgr.APT:
        ctr = ctr.with_env_variable("DEBIAN_FRONTEND", "noninteractive")

    for path, contents in spec.files:
        ctr = ctr.with_new_file(path, contents)

    if d.env_prep:
        ctr = ctr.with_exec(["sh", "-c", d.env_prep])

    for cmd in spec.setup:
        ctr = ctr.with_exec(list(cmd))

    ctr = ctr.with_exec(_install_cmd(spec))
    ctr = install_go(ctr, platform)
    ctr = install_rust(ctr, platform)

    return ctr
