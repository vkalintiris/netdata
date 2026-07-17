"""Native build of the official Netdata agent container image.

Reimplements packaging/docker/Dockerfile natively, with both stages'
environments defined here (seeded from helper-images builder/Dockerfile.v3
and base/Dockerfile.v3, 2026-07-16, reference-only): a Debian trixie
builder producing the agent under an empty prefix, the /app staging tree
with the documented permission model, and a Debian trixie runtime image
with the netdata user and log redirection. run.sh/health.sh are product
content consumed from the source tree.

Known delta vs docker build: the HEALTHCHECK directive is Docker-specific
(not OCI) and cannot be expressed through the Dagger API; compose files
relying on it must define their own healthcheck.
"""

from __future__ import annotations

import dagger

from .envs import EnvSpec, PkgMgr, base_env, bootstrap, with_build_caches

DEBIAN_IMAGE = "debian:trixie"

NETDATA_UID = 201
NETDATA_GID = 201

_BUILDER_DEPS = (
    "autoconf",
    "ccache",
    "autoconf-archive",
    "automake",
    "bison",
    "build-essential",
    "ca-certificates",
    "clang",
    "cmake",
    "curl",
    "dpkg-dev",
    "flex",
    "git",
    "jq",
    "libcurl4-openssl-dev",
    "libfreeipmi-dev",
    "libgcrypt-dev",
    "libipmimonitoring-dev",
    "libjson-c-dev",
    "liblz4-dev",
    "libmariadb-dev",
    "libmnl-dev",
    "libmongoc-dev",
    "libpcre2-dev",
    "libprotobuf-dev",
    "libsnappy-dev",
    "libssl-dev",
    "libsystemd-dev",
    "libtool",
    "libunwind-dev",
    "libuv1-dev",
    "libyaml-dev",
    "libzstd-dev",
    "ninja-build",
    "openssl",
    "patch",
    "pkgconf",
    "protobuf-compiler",
    "python3",
    "python3-dev",
    "unixodbc-dev",
    "uuid-dev",
    "zlib1g-dev",
)

_RUNTIME_DEPS = (
    "ca-certificates",
    "curl",
    "fping",
    "freeipmi",
    "iproute2",
    "jq",
    "libcurl4t64",
    "libgcrypt20",
    "libipmimonitoring6",
    "libjson-c5",
    "liblz4-1",
    "libmariadb3",
    "libmnl0",
    "libmongoc-1.0-0t64",
    "libprotobuf32t64",
    "libsnappy1v5",
    "libssl3t64",
    "libsystemd0",
    "libunwind8",
    "libuuid1",
    "libuv1t64",
    "libvirt-clients",
    "libyaml-0-2",
    "libzstd1",
    "lm-sensors",
    "msmtp",
    "msmtp-mta",
    "ncurses-base",
    "netcat-openbsd",
    "nvme-cli",
    "openssl",
    "procps",
    "python3",
    "smartmontools",
    "unixodbc",
    "vim-tiny",
    "zlib1g",
)

# Plugins that get setuid-root in the image (Dockerfile list).
_SETUID_PLUGINS = (
    "cgroup-network",
    "local-listeners",
    "apps.plugin",
    "debugfs.plugin",
    "freeipmi.plugin",
    "go.d.plugin",
    "perf.plugin",
    "ndsudo",
    "slabinfo.plugin",
    "network-viewer.plugin",
    "otel-plugin",
    "systemd-journal.plugin",
)


_BUILDER_SPEC = EnvSpec(DEBIAN_IMAGE, PkgMgr.APT, _BUILDER_DEPS, prep="apt-get update\n")
_RUNTIME_SPEC = EnvSpec(DEBIAN_IMAGE, PkgMgr.APT, _RUNTIME_DEPS, prep="apt-get update\n")


def docker_builder_env(platform: str) -> dagger.Container:
    return bootstrap(_BUILDER_SPEC, platform)


def _flag(name: str, on: bool) -> str:
    return f"-DENABLE_{name}={'On' if on else 'Off'}"


def docker_configure_args(build_type: str = "Debug") -> list[str]:
    """Effective config of the official image build (installer-derived).

    Mirrors the Dockerfile's installer flags: system protobuf, no ebpf,
    otel+netflow on, journal with the internal file reader, empty install
    prefix. Library-detection features resolved against the builder env.
    """
    return [
        "cmake",
        "-S",
        ".",
        "-B",
        "build",
        "-DCMAKE_INSTALL_PREFIX=",
        "-DCMAKE_C_COMPILER_LAUNCHER=ccache",
        "-DCMAKE_CXX_COMPILER_LAUNCHER=ccache",
        f"-DCMAKE_BUILD_TYPE={build_type}",
        _flag("PLUGIN_GO", True),
        _flag("PLUGIN_PYTHON", True),
        _flag("PLUGIN_CHARTS", True),
        _flag("BUNDLED_PROTOBUF", False),
        _flag("BUNDLED_JSONC", False),
        _flag("DBENGINE", True),
        _flag("ML", True),
        _flag("PLUGIN_APPS", True),
        _flag("PLUGIN_DEBUGFS", True),
        _flag("PLUGIN_PERF", True),
        _flag("PLUGIN_SLABINFO", True),
        _flag("PLUGIN_CGROUP_NETWORK", True),
        _flag("PLUGIN_LOCAL_LISTENERS", True),
        _flag("PLUGIN_NETWORK_VIEWER", True),
        _flag("PLUGIN_EBPF", False),
        _flag("PLUGIN_SYSTEMD_JOURNAL", True),
        _flag("NETDATA_JOURNAL_FILE_READER", True),
        _flag("PLUGIN_SYSTEMD_UNITS", True),
        _flag("PLUGIN_OTEL", True),
        _flag("PLUGIN_NETFLOW", True),
        _flag("PLUGIN_FREEIPMI", True),
        _flag("PLUGIN_CUPS", False),
        _flag("PLUGIN_NFACCT", False),
        _flag("PLUGIN_XENSTAT", False),
        _flag("PLUGIN_IBM", False),
        _flag("PLUGIN_SCRIPTS", False),
        _flag("EXPORTER_MONGODB", True),
        _flag("EXPORTER_PROMETHEUS_REMOTE_WRITE", True),
        _flag("SENTRY", False),
    ]


_APP_ASSEMBLY = """
set -e
mkdir -p /app/usr/sbin /app/usr/share /app/usr/libexec /app/usr/local \
         /app/usr/lib /app/var/cache /app/var/lib /app/etc
mv /usr/share/netdata /app/usr/share/
mv /usr/libexec/netdata /app/usr/libexec/
mv /usr/lib/netdata /app/usr/lib/
mv /var/cache/netdata /app/var/cache/
mv /var/lib/netdata /app/var/lib/
mv /etc/netdata /app/etc/
mv /usr/sbin/netdata /usr/sbin/netdatacli /usr/sbin/nd-run \
   /usr/sbin/systemd-cat-native /app/usr/sbin/
cp packaging/docker/run.sh packaging/docker/health.sh /app/usr/sbin/
mkdir -p /app/usr/local/etc
chmod -R o+rX /app
chmod +x /app/usr/sbin/run.sh
chmod 0755 /app/usr/libexec/netdata/plugins.d/*.plugin
for name in {setuid_plugins}; do
  [ ! -f "/app/usr/libexec/netdata/plugins.d/$name" ] \
    || chmod 4755 "/app/usr/libexec/netdata/plugins.d/$name"
done
find /app/var/lib/netdata /app/var/cache/netdata -type d -exec chmod 0770 {{}} \\;
find /app/var/lib/netdata /app/var/cache/netdata -type f -exec chmod 0660 {{}} \\;
chmod 0700 /app/var/lib/netdata/cloud.d
"""

_RUNTIME_SETUP = f"""
set -e
mkdir -p /opt/src /var/log/netdata
for f in access.log aclk.log debug.log collector.log health.log; do
  ln -sf /dev/stdout /var/log/netdata/$f
done
for f in error.log daemon.log; do
  ln -sf /dev/stderr /var/log/netdata/$f
done
chown -R {NETDATA_UID}:0 /var/log/netdata
chown -R {NETDATA_UID}:0 /usr/lib/netdata /var/cache/netdata /var/lib/netdata
addgroup --gid {NETDATA_GID} --system netdata
adduser --system --no-create-home --shell /usr/sbin/nologin \
        --uid {NETDATA_UID} --home /etc/netdata --ingroup netdata netdata
chown -R {NETDATA_UID}:{NETDATA_GID} /var/lib/netdata/cloud.d
cp -a /etc/netdata /etc/netdata.stock
"""


def docker_image(
    source: dagger.Directory, platform: str, jobs: int = 0, build_type: str = "Debug"
) -> dagger.Container:
    """Build the official agent container image natively."""
    parallel = str(jobs) if jobs > 0 else "$(nproc)"

    builder = (
        with_build_caches(docker_builder_env(platform), f"docker-{platform.replace('/', '-')}")
        .with_directory("/opt/netdata.git", source)
        .with_workdir("/opt/netdata.git")
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("LDFLAGS", "-Wl,--gc-sections")
    )
    # Optimized parity CFLAGS (gen-cflags) only. Debug builds rely on the
    # CMake Debug defaults (-O0 -g): at -Og, xxhash demands always_inline
    # of its SSE2 kernels but GCC's -Og inliner refuses, failing the build.
    if build_type != "Debug":
        builder = builder.with_env_variable("CFLAGS", "-O2 -funroll-loops -pipe")
    builder = (
        builder.with_exec(
            [
                "sh",
                "-c",
                "printf \"INSTALL_TYPE='oci'\\nPREBUILT_ARCH='%s'\\n\" \"$(uname -m)\""
                " > ./system/.install-type",
            ]
        )
        .with_exec(docker_configure_args(build_type))
        .with_exec(["sh", "-c", f"cmake --build build --parallel {parallel}"])
        .with_exec(["cmake", "--install", "build"])
        .with_exec(["sh", "-c", _APP_ASSEMBLY.format(setuid_plugins=" ".join(_SETUID_PLUGINS))])
    )
    app = builder.directory("/app")

    runtime = (
        # The shipped image base: shared bootstrap, no toolchains (base_env).
        base_env(_RUNTIME_SPEC, platform)
        .with_exec(["sh", "-c", "apt-get purge -y dpkg-dev || true"])
        .with_directory("/", app)
        .with_exec(["sh", "-c", _RUNTIME_SETUP])
        .with_env_variable("NETDATA_OFFICIAL_IMAGE", "false")
        .with_env_variable("DOCKER_GRP", "netdata")
        .with_env_variable("DOCKER_USR", "netdata")
        .with_env_variable("NETDATA_LISTENER_PORT", "19999")
        .with_exposed_port(19999)
        .with_entrypoint(["/usr/sbin/run.sh"])
    )
    return runtime
