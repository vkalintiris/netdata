"""CI orchestration: run tiers of jobs concurrently and report.

Tiers:
- smoke: one source build, go/c tests, and the streaming test — the
  pre-push sanity set.
- build: every distro in the build matrix (native platform).
- packages: every native-platform packaging entry, each install-tested.
- static: the x86_64 self-extracting installer.
- image: the container image.
- full: all of the above.

Non-native architectures are excluded here: they emulate via QEMU and
belong on the shared engine, not a workstation.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable

import dagger

from . import build as build_mod
from . import docker as docker_mod
from . import pkgs, stream, tests
from . import static as static_mod
from .matrix import Distro, active_distros

_NATIVE = "linux/amd64"
_NATIVE_PKG_ARCHES = ("amd64", "x86_64")

# Concurrent heavy jobs. Each job already parallelizes across all CPUs,
# and ASAN test runs are memory-hungry: one slot is right for a
# workstation; raise it on the shared engine.
_DEFAULT_SLOTS = 1


async def _run(name: str, coro: Awaitable[object]) -> str:
    try:
        await coro
    except Exception as exc:
        detail = str(exc).strip().splitlines()
        return f"FAIL {name}: {detail[-1] if detail else exc!r}"
    return f"ok   {name}"


def _gated(
    sem: asyncio.Semaphore, name: str, fn: Callable[[], Awaitable[object]]
) -> Awaitable[str]:
    async def runner() -> str:
        async with sem:
            return await _run(name, fn())

    return runner()


async def run_ci(source: dagger.Directory, tier: str = "smoke", slots: int = 0) -> str:
    sem = asyncio.Semaphore(slots if slots > 0 else _DEFAULT_SLOTS)
    jobs: list[Awaitable[str]] = []

    def build_job(d: Distro) -> Callable[[], Awaitable[object]]:
        def fn() -> Awaitable[object]:
            return build_mod.source_build(d, _NATIVE, source).sync()

        return fn

    def pkg_job(d: Distro) -> Callable[[], Awaitable[object]]:
        async def fn() -> None:
            artifacts = await pkgs.package(d, _NATIVE, source)
            await pkgs.test_package(d, _NATIVE, artifacts).sync()

        return fn

    if tier in ("smoke", "full"):
        d = next(x for x in active_distros() if x.name == "debian" and x.version == "12")
        jobs += [
            _gated(sem, "build debian:12", build_job(d)),
            _gated(sem, "go-test", lambda: tests.go_test(source).sync()),
            _gated(sem, "c-test", lambda: tests.c_test(source).sync()),
            _gated(sem, "stream-test", lambda: stream.stream_test(source).sync()),
        ]

    if tier in ("build", "full"):
        for d in active_distros():
            if d.skip_local_build or (d.name, d.version) == ("debian", "12"):
                continue
            jobs.append(_gated(sem, f"build {d.name}:{d.version}", build_job(d)))

    if tier in ("packages", "full"):
        for d in active_distros():
            if d.packages is None:
                continue
            if not any(a in _NATIVE_PKG_ARCHES for a in d.packages.arches):
                continue
            jobs.append(_gated(sem, f"package {d.name}:{d.version}", pkg_job(d)))

    if tier in ("static", "full"):
        jobs.append(
            _gated(
                sem,
                "static x86_64",
                lambda: static_mod.static_build(source, "x86_64"),
            )
        )

    if tier in ("image", "full"):
        jobs.append(
            _gated(sem, "docker-image", lambda: docker_mod.docker_image(source, _NATIVE).sync())
        )

    if not jobs:
        raise ValueError(f"unknown tier {tier!r} (smoke|build|packages|static|image|full)")

    results = await asyncio.gather(*jobs)
    report = "\n".join(sorted(results))
    failed = sum(1 for r in results if r.startswith("FAIL"))
    summary = f"\n{len(results) - failed}/{len(results)} jobs passed (tier={tier})"
    if failed:
        raise RuntimeError(report + summary)
    return report + summary
