"""Environment mechanics for the Netdata agent pipeline.

The machinery that turns an EnvSpec — a complete, declarative description
of an environment — into a container: the bootstrap sequence, package
manager command rendering, pinned toolchains (Go, Rust), and the shared
build caches. The per-distro EnvSpec data lives in distros.py; this module
knows how to execute a spec, never which distros exist.
"""

from __future__ import annotations

import enum
from dataclasses import dataclass

import dagger
from dagger import dag

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


# The default PATH set on every environment. Some images (opensuse) define
# no PATH in their OCI config; docker injects a default at runtime but
# dagger does not, so it must be explicit.
STD_PATH = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"


class PkgMgr(enum.StrEnum):
    APK = "apk"
    APT = "apt"
    DNF = "dnf"
    PACMAN = "pacman"
    YUM = "yum"
    ZYPPER = "zypper"


class RustSource(enum.StrEnum):
    """Where a build environment's Rust toolchain comes from.

    Every build environment carries Rust; this records provenance, never
    presence. The static (Alpine/musl) family uses the distro packages:
    32-bit ARM musl has no rustup host toolchain (Tier 2 without host
    tools), and distro rust is also what CI's static builder uses.
    """

    RUSTUP = "rustup"
    DISTRO = "distro"


@dataclass(frozen=True)
class EnvSpec:
    """Everything needed to bootstrap an environment container."""

    base_image: str
    mgr: PkgMgr
    deps: tuple[str, ...]
    # Files written before prep runs: (path, contents).
    files: tuple[tuple[str, str], ...] = ()
    # Shell run before repo setup (e.g. "apt-get update").
    prep: str = ""
    # Repo-enablement commands run before installing deps.
    setup: tuple[tuple[str, ...], ...] = ()
    # Extra install flags, e.g. --allowerasing where curl-minimal conflicts
    # with the real curl (EL9+ and Amazon Linux base images).
    install_flags: tuple[str, ...] = ()
    # Environment variables applied after the default PATH — a spec may
    # override PATH itself (centos7: devtoolset-11 + pinned cmake).
    env: tuple[tuple[str, str], ...] = ()
    # For RustSource.DISTRO, deps must list the distro rust packages.
    rust: RustSource = RustSource.RUSTUP


def _install_cmd(spec: EnvSpec) -> list[str]:
    match spec.mgr:
        case PkgMgr.APK:
            return ["apk", "add", "--no-cache", *spec.install_flags, *spec.deps]
        case PkgMgr.APT:
            return [
                "apt-get",
                "install",
                "-y",
                "--no-install-recommends",
                *spec.install_flags,
                *spec.deps,
            ]
        case PkgMgr.DNF:
            return ["dnf", "install", "-y", *spec.install_flags, *spec.deps]
        case PkgMgr.PACMAN:
            return ["pacman", "--noconfirm", "--needed", "-S", *spec.install_flags, *spec.deps]
        case PkgMgr.YUM:
            return ["yum", "install", "-y", *spec.install_flags, *spec.deps]
        case PkgMgr.ZYPPER:
            return ["zypper", "install", "-y", *spec.install_flags, *spec.deps]


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


def base_env(spec: EnvSpec, platform: str) -> dagger.Container:
    """Bootstrap an environment container, without toolchains.

    The single bootstrap sequence every environment goes through: base
    image, PATH, files, prep, repo setup, dependency install. Used directly
    only for non-build environments (the docker runtime stage); build
    environments go through bootstrap().
    """
    ctr = dag.container(platform=dagger.Platform(platform)).from_(spec.base_image)

    ctr = ctr.with_env_variable("PATH", STD_PATH)

    if spec.mgr is PkgMgr.APT:
        ctr = ctr.with_env_variable("DEBIAN_FRONTEND", "noninteractive")

    for name, value in spec.env:
        ctr = ctr.with_env_variable(name, value)

    for path, contents in spec.files:
        ctr = ctr.with_new_file(path, contents)

    if spec.prep:
        ctr = ctr.with_exec(["sh", "-c", spec.prep])

    for cmd in spec.setup:
        ctr = ctr.with_exec(list(cmd))

    return ctr.with_exec(_install_cmd(spec))


def bootstrap(spec: EnvSpec, platform: str) -> dagger.Container:
    """Bootstrap a build environment: base_env plus the Go and Rust toolchains.

    Every build environment carries both toolchains, unconditionally.
    spec.rust records where Rust comes from: the pinned rustup install, or
    the distro packages already listed in spec.deps.
    """
    ctr = install_go(base_env(spec, platform), platform)
    if spec.rust is RustSource.RUSTUP:
        ctr = install_rust(ctr, platform)
    return ctr


def has_ccache(spec: EnvSpec) -> bool:
    """Whether the environment carries ccache — the spec is the fact."""
    return "ccache" in spec.deps


def compiler_launcher_args(enabled: bool) -> list[str]:
    if not enabled:
        return []
    return [
        "-DCMAKE_C_COMPILER_LAUNCHER=ccache",
        "-DCMAKE_CXX_COMPILER_LAUNCHER=ccache",
    ]


def with_build_caches(ctr: dagger.Container, key: str) -> dagger.Container:
    """Mount shared toolchain caches (ccache, Go, cargo).

    Cache-volume contents are excluded from Dagger's cache keys, so these
    accelerate re-executions without affecting layer caching. `key` scopes
    compiler-output caches per environment; Go and cargo-registry caches
    are safely global.
    """
    return (
        ctr.with_mounted_cache("/ccache", dag.cache_volume(f"ccache-{key}"))
        .with_env_variable("CCACHE_DIR", "/ccache")
        # Compiler mtimes are unstable across images; hash the compiler.
        .with_env_variable("CCACHE_COMPILERCHECK", "content")
        .with_env_variable("CCACHE_MAXSIZE", "10G")
        .with_mounted_cache("/go-build-cache", dag.cache_volume("go-build-cache"))
        .with_env_variable("GOCACHE", "/go-build-cache")
        .with_mounted_cache("/go-mod-cache", dag.cache_volume("go-mod-cache"))
        .with_env_variable("GOMODCACHE", "/go-mod-cache")
        .with_mounted_cache("/cargo-registry", dag.cache_volume("cargo-registry"))
        .with_env_variable("CARGO_HOME", "/cargo-registry")
        .with_mounted_cache("/cargo-target", dag.cache_volume(f"cargo-target-{key}"))
        .with_env_variable("CARGO_TARGET_DIR", "/cargo-target")
    )
