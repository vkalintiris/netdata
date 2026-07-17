# netdata-ci — the Netdata agent pipeline as a Dagger module

This module builds, packages, and tests the Netdata agent in containers,
natively: the distro matrix, build environments, toolchain pins, CMake
profiles, packaging, and static-build recipes are all typed Python in
`src/netdata_ci/` — no shell-script or YAML orchestration. Results are
content-cached by the Dagger engine, so unchanged steps are free on
re-runs.

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

Run from anywhere in the repository (`-m packaging/dag` when outside it):

```sh
dagger functions                          # discover the surface

dagger call ci                            # smoke tier: one build + all tests
dagger call ci --tier=full                # everything native-arch

dagger call build --distro=debian --version=12    # one source build
dagger call build-env --distro=fedora --version=43 terminal   # debug shell

dagger call package --distro=debian --version=12 \
    export --path=./artifacts             # DEB/RPM artifacts
dagger call test-package --distro=debian --version=12 \
    --artifacts=./artifacts               # clean-image install + boot

dagger call static --arch=x86_64 export --path=./artifacts    # .gz.run
dagger call docker-image export --path=./netdata-img.tar      # OCI image

dagger call go-test                       # Go modules: fmt/vet/build/race
dagger call c-test                        # ASAN build + C unit tests
dagger call stream-test                   # parent/child streaming check

dagger call matrix --kind=PACKAGING       # CI-compatible job matrix JSON
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

| File        | Owns |
|-------------|------|
| `matrix.py` | distros, versions, arches, package targets (the matrix) |
| `envs.py`   | build environments: per-distro deps, repo setup, Go/Rust pins |
| `build.py`  | source-build CMake profile and build/install steps |
| `pkgs.py`   | packaging environments, DEB (cpack) and RPM (spec) builds, install tests |
| `static.py` | static builder env, bundled-dep builds, makeself archive |
| `docker.py` | both stages of the official container image |
| `tests.py`  | Go and C test jobs |
| `stream.py` | parent/child streaming integration test |
| `ci.py`     | tier orchestration |

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
