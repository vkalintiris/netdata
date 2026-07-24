# netdata-ci — the Netdata agent pipeline as a Dagger module

This module builds, packages, and tests the Netdata agent in containers,
natively: distro definitions, build environments, toolchain pins, CMake
profiles, packaging, and static-build recipes are all typed Python in
`src/netdata_ci/` — no shell-script or YAML orchestration. Results are
content-cached by the Dagger engine, so unchanged steps are free on
re-runs.

The module is capability-oriented: every function operates on one
specific distro (a typed enum — invalid targets are rejected by the CLI).
Which targets a CI run covers is declared on top, in `ci.py`'s tier
tuples, never derived from a matrix.

## Setup

Install the pinned Dagger CLI (the module declares `engineVersion` in
`dagger.json`; keep the CLI matched to it):

```sh
curl -fsSL https://dl.dagger.io/dagger/install.sh |
    BIN_DIR=$HOME/.local/bin DAGGER_VERSION=0.21.7 sh
```

A container runtime (docker or podman) must be available; the engine
runs itself in a container on first use.

## Usage

The CLI finds the module by walking UP from your cwd, so from the
repository root (or anywhere outside `packaging/dag`) the module must be
named explicitly — `-m` goes before `call`:

```sh
dagger -m packaging/dag call build-env --distro=DEBIAN_12 terminal
```

Inside `packaging/dag/` auto-discovery works and `-m` is unneeded:

```sh
cd packaging/dag
dagger call build-env --distro=DEBIAN_12 terminal
```

The examples below assume one of the two forms:

```sh
dagger functions                          # discover the surface

dagger call ci                            # smoke tier: one build + all tests
dagger call ci --tier=full                # everything native-arch

dagger call build --distro=DEBIAN_12              # one source build
dagger call build-env --distro=FEDORA_43 terminal # debug shell
dagger call build-env --help                      # lists every distro

dagger call package --distro=DEBIAN_12 \
    export --path=./artifacts             # DEB/RPM artifacts
dagger call test-package --distro=DEBIAN_12 \
    --artifacts=./artifacts               # clean-image install + boot

dagger call static --arch=x86_64 export --path=./artifacts    # .gz.run
dagger call docker-image export --path=./netdata-img.tar      # OCI image

dagger call go-test                       # Go modules: fmt/vet/build/race
dagger call c-test                        # ASAN build + C unit tests
dagger call stream-test                   # parent/child streaming check
```

Every build accepts `--jobs=N` to cap compile parallelism (default: one
job per CPU). When running several builds concurrently, budget jobs
across them.

## Shared engine

To use a fast remote machine (over a trusted network — the wire is not
encrypted), run the engine there and point the CLI at it:

```sh
export _EXPERIMENTAL_DAGGER_RUNNER_HOST=tcp://buildhost:8080
```

All callers of a shared engine share its cache: a build your colleague
already ran returns instantly for you.

## Layout

| File         | Owns |
|--------------|------|
| `distros.py` | the Distro enum and each distro's complete definition (envs, packaging, features) |
| `envs.py`    | environment mechanics: bootstrap sequence, package managers, Go/Rust pins, caches |
| `build.py`   | source-build CMake profile and build/install steps |
| `pkgs.py`    | DEB/RPM (cpack) build mechanics, install tests |
| `stock.py`   | synthetic topology IP-intel stock payload (CI PR parity; release payloads stay external) |
| `static.py`  | static builder env, bundled-dep builds, makeself archive |
| `docker.py`  | both stages of the official container image |
| `tests.py`   | Go and C test jobs |
| `stream.py`  | parent/child streaming integration test |
| `ci.py`      | declared CI tiers (the only place that enumerates distros) |

Module development: `uv run --group dev mypy src/netdata_ci/` and
`uv run --group dev ruff check src/` must stay clean; `dagger develop`
regenerates the vendored SDK under `sdk/` (gitignored).

Non-native architectures build via QEMU (slow) — prefer the shared
engine for those and for full-matrix runs. Windows and macOS cannot be
built here (Linux containers only); they remain on GitHub CI.

## Progress in scripts

Interactive runs show per-job `ok/FAIL` lines live in the TUI. Piped or
scripted runs should add `--progress=plain` to see step-level progress;
the compact renderer only emits heartbeats until completion.
