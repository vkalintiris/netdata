"""CI orchestration: run tiers of jobs concurrently and report.

Tiers:
- smoke: one source build, go/c tests, and the streaming test — the
  pre-push sanity set.
- build: the declared source-build set (native platform).
- packages: the declared packaging set, each install-tested.
- static: the x86_64 self-extracting installer.
- image: the container image.
- full: all of the above.

The tier tuples below are DECLARATIONS — the module's capability layer
never enumerates distros; which targets run is decided here, deliberately,
until the real CI declaration layer (M2) replaces this file's role.
Non-native architectures are excluded: they emulate via QEMU and belong on
the shared engine, not a workstation.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable

import dagger

from . import build as build_mod
from . import docker as docker_mod
from . import pkgs, stream, tests
from . import static as static_mod
from .distros import Distro

_NATIVE = "linux/amd64"

# Concurrent heavy jobs. Each job already parallelizes across all CPUs,
# and ASAN test runs are memory-hungry: one slot is right for a
# workstation; raise it on the shared engine.
_DEFAULT_SLOTS = 1

_SMOKE_DISTRO = Distro.DEBIAN_12

# Source-build declarations. amazonlinux-2, centos-7, and oraclelinux-10
# have build environments but are not declared here, matching what CI
# builds today (their packaging jobs still compile from source below).
_BUILD_TIER: tuple[Distro, ...] = (
    Distro.ALPINE_EDGE,
    Distro.ALPINE_3_23,
    Distro.ALPINE_3_22,
    Distro.AMAZONLINUX_2023,
    Distro.ARCHLINUX,
    Distro.CENTOS_STREAM_9,
    Distro.CENTOS_STREAM_10,
    Distro.DEBIAN_11,
    Distro.DEBIAN_12,
    Distro.DEBIAN_13,
    Distro.FEDORA_43,
    Distro.FEDORA_44,
    Distro.OPENSUSE_16_0,
    Distro.OPENSUSE_TUMBLEWEED,
    Distro.ORACLELINUX_8,
    Distro.ORACLELINUX_9,
    Distro.ROCKYLINUX_8,
    Distro.ROCKYLINUX_9,
    Distro.ROCKYLINUX_10,
    Distro.UBUNTU_22_04,
    Distro.UBUNTU_24_04,
    Distro.UBUNTU_25_10,
    Distro.UBUNTU_26_04,
)

# Packaging declarations: every distro with a native package product.
_PACKAGES_TIER: tuple[Distro, ...] = (
    Distro.AMAZONLINUX_2,
    Distro.AMAZONLINUX_2023,
    Distro.CENTOS_7,
    Distro.CENTOS_STREAM_9,
    Distro.CENTOS_STREAM_10,
    Distro.DEBIAN_11,
    Distro.DEBIAN_12,
    Distro.DEBIAN_13,
    Distro.FEDORA_43,
    Distro.FEDORA_44,
    Distro.OPENSUSE_16_0,
    Distro.OPENSUSE_TUMBLEWEED,
    Distro.ORACLELINUX_8,
    Distro.ORACLELINUX_9,
    Distro.ORACLELINUX_10,
    Distro.ROCKYLINUX_8,
    Distro.ROCKYLINUX_9,
    Distro.ROCKYLINUX_10,
    Distro.UBUNTU_22_04,
    Distro.UBUNTU_24_04,
    Distro.UBUNTU_25_10,
    Distro.UBUNTU_26_04,
)


async def _run(name: str, coro: Awaitable[object]) -> str:
    try:
        await coro
    except Exception as exc:
        detail = str(exc).strip().splitlines()
        line = f"FAIL {name}: {detail[-1] if detail else exc!r}"
    else:
        line = f"ok   {name}"
    # Stream per-job results as they land; the final report repeats them.
    print(line, flush=True)
    return line


def _gated(
    sem: asyncio.Semaphore, name: str, fn: Callable[[], Awaitable[object]]
) -> Awaitable[str]:
    async def runner() -> str:
        async with sem:
            return await _run(name, fn())

    return runner()


async def run_ci(
    source: dagger.Directory,
    tier: str = "smoke",
    slots: int = 0,
    build_type: str = "Debug",
    jobs: int = 0,
) -> str:
    sem = asyncio.Semaphore(slots if slots > 0 else _DEFAULT_SLOTS)
    queued: list[Awaitable[str]] = []

    def build_job(d: Distro) -> Callable[[], Awaitable[object]]:
        def fn() -> Awaitable[object]:
            return build_mod.source_build(
                d, _NATIVE, source, jobs=jobs, build_type=build_type
            ).sync()

        return fn

    def pkg_job(d: Distro) -> Callable[[], Awaitable[object]]:
        async def fn() -> None:
            artifacts = pkgs.package(d, _NATIVE, source, jobs=jobs, build_type=build_type)
            await pkgs.test_package(d, _NATIVE, artifacts).sync()

        return fn

    if tier in ("smoke", "full"):
        queued += [
            _gated(sem, f"build {_SMOKE_DISTRO.value}", build_job(_SMOKE_DISTRO)),
            _gated(sem, "go-test", lambda: tests.go_test(source).sync()),
            _gated(sem, "c-test", lambda: tests.c_test(source, jobs=jobs).sync()),
            _gated(sem, "stream-test", lambda: stream.stream_test(source, jobs=jobs).sync()),
        ]

    if tier in ("build", "full"):
        for d in _BUILD_TIER:
            if d is _SMOKE_DISTRO:
                continue
            queued.append(_gated(sem, f"build {d.value}", build_job(d)))

    if tier in ("packages", "full"):
        for d in _PACKAGES_TIER:
            queued.append(_gated(sem, f"package {d.value}", pkg_job(d)))

    if tier in ("static", "full"):
        queued.append(
            _gated(
                sem,
                "static x86_64",
                lambda: static_mod.static_build(source, "x86_64", jobs=jobs, build_type=build_type),
            )
        )

    if tier in ("image", "full"):
        queued.append(
            _gated(
                sem,
                "docker-image",
                lambda: docker_mod.docker_image(
                    source, _NATIVE, jobs=jobs, build_type=build_type
                ).sync(),
            )
        )

    if not queued:
        raise ValueError(f"unknown tier {tier!r} (smoke|build|packages|static|image|full)")

    results = await asyncio.gather(*queued)
    report = "\n".join(sorted(results))
    failed = sum(1 for r in results if r.startswith("FAIL"))
    summary = f"\n{len(results) - failed}/{len(results)} jobs passed (tier={tier})"
    if failed:
        raise RuntimeError(report + summary)
    return report + summary
