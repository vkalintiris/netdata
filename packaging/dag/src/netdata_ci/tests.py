"""Go and C test jobs.

Go: replicates go-tests.yml per-module semantics natively — module
discovery by locating go.mod files, then gofmt/vet/build and race-enabled
tests per module. C: replicates tests/run-unit-tests.sh natively — an
address-sanitized Debug build whose netdata binary runs `-W unittest`.
"""

from __future__ import annotations

import dagger

from . import build as build_mod
from . import envs
from .distros import SPECS, Distro

# Test workloads run on the reference distro.
_DISTRO = Distro.DEBIAN_12

# CI scope and semantics (go-tests.yml + get-go-version.py): modules under
# src/go plus the standalone cgroup-name module; CGO-off builds compile the
# main packages only (excluding examples and the CGO-requiring ibmdplugin);
# vet runs CGO-off without tests; tests run with the race detector (CGO on).
# Note: ebpfgo.plugin and the stdio-golang bridge are NOT in CI's matrix and
# ebpfgo.plugin's CGO-off build is currently broken upstream — parity means
# we skip them too (tracked in the SOW).
_GO_MODULE_TEST = """
set -e
status=0
mods="$(find ./src/go -name go.mod -not -path '*/vendor/*')"
[ -f ./src/collectors/cgroups.plugin/cgroup-name/go.mod ] &&
  mods="$mods ./src/collectors/cgroups.plugin/cgroup-name/go.mod"
for mod in $mods; do
  dir="$(dirname "$mod")"
  echo "=== module: $dir"
  (
    cd "$dir"
    for main in $(find . -name main.go -not -path '*examples*' -not -path '*ibmdplugin*'); do
      CGO_ENABLED=0 go build -o /tmp/go-test-build "$(dirname "$main")/"
    done
    unformatted="$(gofmt -l . | grep -v vendor || true)"
    if [ -n "$unformatted" ]; then
      echo "gofmt failures:"; echo "$unformatted"; exit 1
    fi
    CGO_ENABLED=0 go vet -tests=false ./...
    CGO_ENABLED=1 go test -race ./...
  ) || status=1
done
exit $status
"""

# setarch -R disables ASLR for the test process: ASan's shadow mapping
# randomly collides with high-entropy ASLR (hosts with mmap_rnd_bits=32),
# crashing at startup before any test runs. CI runner VMs patch this at
# the kernel level; containers inherit the host's setting, so we must not
# depend on it.
_C_UNITTEST = """
set -e
ASAN_OPTIONS=detect_leaks=0 setarch "$(uname -m)" -R \
  /opt/netdata/usr/sbin/netdata -W unittest
"""


def go_test(source: dagger.Directory) -> dagger.Container:
    """Run gofmt/vet/build/test -race across every Go module.

    Runs as a non-root user, as CI does: several tests assert on
    permission-denied behavior that root cannot observe. unixodbc-dev
    matches CI's explicit CGO dependency for the ibm.d packages.
    """
    ctr = (
        envs.bootstrap(SPECS[_DISTRO].build, "linux/amd64")
        .with_exec(["apt-get", "install", "-y", "--no-install-recommends", "unixodbc-dev"])
        .with_exec(["useradd", "-m", "-s", "/bin/sh", "runner"])
        # Minimal containers ship no machine-id; the journal host-identity
        # code (used by snmp_traps tests) requires one. Fixed value keeps
        # the layer cache stable.
        .with_new_file("/etc/machine-id", "0123456789abcdef0123456789abcdef\n")
        .with_directory("/netdata", source)
        .with_workdir("/netdata")
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("HOME", "/home/runner")
        # Non-root user needs owned cache mounts (root-owned volumes are
        # unwritable for it).
        .with_mounted_cache(
            "/go-build-cache", dagger.dag.cache_volume("go-build-cache"), owner="runner"
        )
        .with_env_variable("GOCACHE", "/go-build-cache")
        .with_mounted_cache(
            "/go-mod-cache", dagger.dag.cache_volume("go-mod-cache"), owner="runner"
        )
        .with_env_variable("GOMODCACHE", "/go-mod-cache")
        .with_env_variable("GOPATH", "/home/runner/go")
        .with_user("runner")
        .with_exec(["sh", "-c", _GO_MODULE_TEST])
    )
    return ctr


def c_test(source: dagger.Directory, jobs: int = 0) -> dagger.Container:
    """Address-sanitized Debug build + the in-binary C unit tests."""
    spec = SPECS[_DISTRO]
    parallel = str(jobs) if jobs > 0 else "$(nproc)"
    args = build_mod.configure_args(spec, "linux/amd64")
    args.append("-DENABLE_ADDRESS_SANITIZER=On")
    ctr = (
        envs.with_build_caches(envs.bootstrap(spec.build, "linux/amd64"), "c-test")
        .with_directory("/netdata", source)
        .with_workdir("/netdata")
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_exec(args)
        .with_exec(["sh", "-c", f"cmake --build {build_mod.BUILD_DIR} --parallel {parallel}"])
        .with_exec(["cmake", "--install", build_mod.BUILD_DIR])
        # Privileged: the default seccomp profile blocks personality(2),
        # which setarch -R needs. CI does the same via seccomp=unconfined.
        .with_exec(["sh", "-c", _C_UNITTEST], insecure_root_capabilities=True)
    )
    return ctr
