"""Native static (makeself) build of the Netdata agent.

Reimplements packaging/makeself natively: the Alpine builder environment,
each bundled dependency as its own cached container build, the static
agent build via CMake, the pre-archive verification, and the makeself
archive creation. Seeded from packaging/makeself/jobs/*.sh,
bundled-packages.version, and helper-images static-builder/Dockerfile.v1
(2026-07-16) — reference-only; none of those scripts run here. The
vendored makeself.sh tool and the product install scripts it embeds
(post-installer.sh, install-or-update.sh) are consumed from the source
tree: they are shipped artifact content, not build orchestration.

Each bundled dependency builds in an isolated container and exports only
its install prefix, so agent-source changes never invalidate dependency
builds (replacing the artifacts/cache mechanism).
"""

from __future__ import annotations

from dataclasses import dataclass

import dagger
from dagger import dag

from .envs import EnvSpec, PkgMgr, RustSource, bootstrap, with_build_caches

ALPINE_IMAGE = "alpine:3.23"
NP = "/opt/netdata"  # NETDATA_INSTALL_PATH

# Bundled dependency pins (from bundled-packages.version).
OPENSSL_TAG = "openssl-3.6.0"
CURL_TAG = "curl-8_20_0"
BASH_VERSION = "5.3"
BASH_SHA256 = "0d5cd86965f869a26cf64f4b71be7b96f90a3ba8b3d74e27e8e9d9d5550f31ba"
IOPING_VERSION = "1.3"
IOPING_SHA256 = "7aa48e70aaa766bc112dea57ebbe56700626871052380709df3a26f46766e8c8"
NFACCT_VERSION = "1.0.3"
NFACCT_SHA256 = "4250ceef3efe2034f4ac05906c3ee427db31b9b0a2df41b2744f4bf79a959a1a"
LIBUCONTEXT_TAG = "libucontext-1.3.3"
LIBUNWIND_TAG = "v1.8.3"

# Builder image package set (helper-images static-builder v1).
_STATIC_DEPS = (
    "alpine-sdk",
    "autoconf",
    "automake",
    "bash",
    "binutils",
    "bison",
    "brotli-dev",
    "brotli-static",
    "cargo",
    "ccache",
    "clang",
    "cmake",
    "coreutils",
    "curl",
    "curl-static",
    "elfutils-dev",
    "flex",
    "gcc",
    "git",
    "gnutls-dev",
    "gzip",
    "jq",
    "libelf-static",
    "libidn2-dev",
    "libidn2-static",
    "libmnl-dev",
    "libmnl-static",
    "libtool",
    "libuv-dev",
    "libuv-static",
    "libpsl-dev",
    "libpsl-static",
    "libunistring-dev",
    "libunistring-static",
    "lz4-dev",
    "lz4-static",
    "make",
    "mongo-c-driver-dev",
    "mongo-c-driver-static",
    "musl-fts-dev",
    "ncurses-dev",
    "ncurses-static",
    "netcat-openbsd",
    "openssh",
    "patch",
    "pcre2-dev",
    "pcre2-static",
    "pkgconfig",
    "rust",
    "samurai",
    "snappy-dev",
    "snappy-static",
    "unixodbc-dev",
    "unixodbc-static",
    "util-linux-dev",
    "util-linux-static",
    "wget",
    "xz",
    "xz-static",
    "yaml-dev",
    "yaml-static",
    "zlib-dev",
    "zlib-static",
    "zstd-dev",
    "zstd-static",
)

_SNAPPY_PC = """\
prefix=/usr
exec_prefix=${prefix}
libdir=${exec_prefix}/lib
includedir=${prefix}/include

Name: snappy
Description: A library for compressing and decompressing snappy data
Version: 1.1.10
Libs: -L${libdir} -lsnappy
Cflags: -I${includedir}
"""


@dataclass(frozen=True)
class StaticArch:
    """Per-architecture build tuning (from build-static.sh)."""

    arch: str
    platform: str
    tuning_flags: str
    go_env: tuple[tuple[str, str], ...]
    arm32: bool = False


STATIC_ARCHS: dict[str, StaticArch] = {
    "x86_64": StaticArch(
        "x86_64", "linux/amd64", "-march=x86-64", (("GOARCH", "amd64"), ("GOAMD64", "v1"))
    ),
    "armv6l": StaticArch(
        "armv6l",
        "linux/arm/v6",
        "-march=armv6zk -mtune=arm1176jzf-s",
        (("GOARCH", "arm"), ("GOARM", "6")),
        arm32=True,
    ),
    "armv7l": StaticArch(
        "armv7l",
        "linux/arm/v7",
        "-march=armv7-a",
        (("GOARCH", "arm"), ("GOARM", "7")),
        arm32=True,
    ),
    "aarch64": StaticArch(
        "aarch64",
        "linux/arm64/v8",
        "-march=armv8-a",
        (("GOARCH", "arm64"), ("GOARM64", "v8.0")),
    ),
}


# Rust is DISTRO-provenance: _STATIC_DEPS lists cargo/rust (Alpine builds
# them for all four static arches; rustup has no 32-bit ARM musl host).
_STATIC_SPEC = EnvSpec(
    ALPINE_IMAGE,
    PkgMgr.APK,
    _STATIC_DEPS,
    files=(("/usr/lib/pkgconfig/snappy.pc", _SNAPPY_PC),),
    rust=RustSource.DISTRO,
)


def static_env(a: StaticArch) -> dagger.Container:
    """Alpine builder with static libraries and toolchains."""
    return bootstrap(_STATIC_SPEC, a.platform)


def _untar(ctr: dagger.Container, url: str, sha256: str, path: str) -> dagger.Container:
    return (
        ctr.with_file("/tmp/src.tar", dag.http(url))
        .with_exec(["sh", "-c", f"echo '{sha256}  /tmp/src.tar' | sha256sum -c -"])
        .with_exec(
            ["sh", "-c", f"mkdir -p {path} && tar -xaf /tmp/src.tar -C {path} --strip-components=1"]
        )
    )


def build_libucontext(a: StaticArch) -> dagger.Directory:
    tree = dag.git("https://github.com/kaniini/libucontext").tag(LIBUCONTEXT_TAG).tree()
    ctr = (
        static_env(a)
        .with_directory("/build/libucontext", tree)
        .with_workdir("/build/libucontext")
        .with_env_variable("CFLAGS", f"{a.tuning_flags} -pipe")
        .with_exec(["sh", "-c", "make ARCH=arm EXPORT_UNPREFIXED=yes -j$(nproc)"])
        .with_exec(
            [
                "sh",
                "-c",
                "make ARCH=arm EXPORT_UNPREFIXED=yes DESTDIR=/libucontext-static"
                " -j$(nproc) install",
            ]
        )
    )
    return ctr.directory("/libucontext-static")


def build_libunwind(a: StaticArch, ucontext: dagger.Directory) -> dagger.Directory:
    tree = dag.git("https://github.com/libunwind/libunwind").tag(LIBUNWIND_TAG).tree()
    ctr = (
        static_env(a)
        .with_directory("/libucontext-static", ucontext)
        .with_directory("/build/libunwind", tree)
        .with_workdir("/build/libunwind")
        .with_env_variable(
            "CFLAGS", f"{a.tuning_flags} -I/libucontext-static/usr/include -fno-lto -pipe"
        )
        .with_env_variable("LDFLAGS", "-static -L/libucontext-static/usr/lib/ -lucontext")
        .with_env_variable("PKG_CONFIG", "pkg-config --static")
        .with_exec(["autoreconf", "-ivf"])
        .with_exec(
            [
                "sh",
                "-c",
                "./configure --prefix=/libunwind-static --build=$(gcc -dumpmachine)"
                " --disable-cxx-exceptions --disable-documentation --disable-tests"
                " --disable-shared --enable-static --disable-dependency-tracking",
            ]
        )
        .with_exec(["sh", "-c", "make -j$(nproc) && make -j$(nproc) install"])
    )
    return ctr.directory("/libunwind-static")


def build_openssl(a: StaticArch) -> dagger.Directory:
    tree = dag.git("https://github.com/openssl/openssl").tag(OPENSSL_TAG).tree()
    config_target = " linux-armv4" if a.arm32 else ""
    ctr = (
        static_env(a)
        .with_directory("/build/openssl", tree)
        .with_workdir("/build/openssl")
        .with_env_variable("CFLAGS", f"{a.tuning_flags} -fno-lto -pipe")
        .with_env_variable("LDFLAGS", "-static")
        .with_env_variable("PKG_CONFIG", "pkg-config --static")
        .with_exec(
            [
                "sed",
                "-i",
                "s/disable('static', 'pic', 'threads');/disable('static', 'pic');/",
                "Configure",
            ]
        )
        .with_exec(
            [
                "sh",
                "-c",
                "./config -static threads no-tests --prefix=/openssl-static"
                f" --openssldir={NP}/etc/ssl{config_target}",
            ]
        )
        .with_exec(["sh", "-c", "make -j$(nproc) && make -j$(nproc) install_sw"])
        .with_exec(["sh", "-c", "[ ! -d /openssl-static/lib ] || ln -s lib /openssl-static/lib64"])
    )
    return ctr.directory("/openssl-static")


def build_curl(a: StaticArch, openssl: dagger.Directory) -> dagger.Directory:
    tree = dag.git("https://github.com/curl/curl").tag(CURL_TAG).tree()
    ctr = (
        static_env(a)
        .with_directory("/openssl-static", openssl)
        .with_directory("/build/curl", tree)
        .with_workdir("/build/curl")
        .with_env_variable("CFLAGS", f"{a.tuning_flags} -I/openssl-static/include -pipe")
        .with_env_variable("LDFLAGS", "-static -L/openssl-static/lib64")
        .with_env_variable("PKG_CONFIG", "pkg-config --static")
        .with_env_variable("PKG_CONFIG_PATH", "/openssl-static/lib64/pkgconfig")
        .with_exec(["autoreconf", "-fi"])
        .with_exec(
            [
                "sh",
                "-c",
                "./configure --prefix=/curl-local --enable-optimize --disable-shared"
                " --enable-static --enable-http --disable-ldap --disable-ldaps"
                " --enable-proxy --disable-dict --disable-telnet --disable-tftp"
                " --disable-pop3 --disable-imap --disable-smb --disable-smtp"
                " --disable-gopher --enable-ipv6 --enable-cookies --with-ca-fallback"
                f" --with-openssl --with-ca-bundle={NP}/etc/ssl/certs/ca-certificates.crt"
                f" --with-ca-path={NP}/etc/ssl/certs --without-brotli"
                " --disable-dependency-tracking",
            ]
        )
        .with_exec(["sed", "-i", "-e", "s/LDFLAGS =/LDFLAGS = -all-static/", "src/Makefile"])
        .with_exec(["sh", "-c", "make -j$(nproc) && make install"])
    )
    return ctr.directory("/curl-local")


def build_bash(a: StaticArch) -> dagger.Directory:
    url = f"http://ftp.gnu.org/gnu/bash/bash-{BASH_VERSION}.tar.gz"
    ctr = _untar(static_env(a), url, BASH_SHA256, "/build/bash")
    ctr = (
        ctr.with_workdir("/build/bash")
        .with_env_variable("CFLAGS", f"{a.tuning_flags} -pipe")
        .with_exec(
            [
                "sh",
                "-c",
                f"./configure --prefix={NP} --without-bash-malloc --enable-static-link"
                " --enable-net-redirections --enable-array-variables --disable-progcomp"
                " --disable-profiling --disable-nls --disable-dependency-tracking",
            ]
        )
        .with_exec(
            ["sh", "-c", "printf 'all:\\nclean:\\ninstall:\\n' > examples/loadables/Makefile"]
        )
        .with_exec(["sh", "-c", "make -j$(nproc) && make install"])
    )
    return ctr.directory(NP)


def build_ioping(a: StaticArch) -> dagger.File:
    url = f"https://github.com/koct9i/ioping/archive/refs/tags/v{IOPING_VERSION}.tar.gz"
    ctr = _untar(static_env(a), url, IOPING_SHA256, "/build/ioping")
    ctr = (
        ctr.with_workdir("/build/ioping")
        .with_env_variable("CFLAGS", f"{a.tuning_flags} -static -pipe")
        .with_exec(["sh", "-c", "make -j$(nproc)"])
    )
    return ctr.file("/build/ioping/ioping")


def build_nfacct(a: StaticArch) -> dagger.Directory:
    url = (
        "https://www.netfilter.org/projects/libnetfilter_acct/files/"
        f"libnetfilter_acct-{NFACCT_VERSION}.tar.bz2"
    )
    ctr = _untar(static_env(a), url, NFACCT_SHA256, "/build/nfacct")
    ctr = (
        ctr.with_workdir("/build/nfacct")
        .with_env_variable("CFLAGS", f"{a.tuning_flags} -static -I/usr/include/libmnl -pipe")
        .with_env_variable("LDFLAGS", "-static -L/usr/lib -lmnl")
        .with_env_variable("PKG_CONFIG", "pkg-config --static")
        .with_env_variable("PKG_CONFIG_PATH", "/usr/lib/pkgconfig")
        .with_exec(
            [
                "./configure",
                "--prefix=/libnetfilter-acct-static",
                "--exec-prefix=/libnetfilter-acct-static",
            ]
        )
        .with_exec(["sh", "-c", "make -j$(nproc) && make install"])
    )
    return ctr.directory("/libnetfilter-acct-static")


def _flag(name: str, on: bool) -> str:
    return f"-DENABLE_{name}={'On' if on else 'Off'}"


def static_configure_args(a: StaticArch, build_type: str = "Debug") -> list[str]:
    """Effective CMake config of the static build (installer-derived)."""
    x86 = a.arch == "x86_64"
    full = not a.arm32 or a.arch == "armv7l"  # armv6l drops journal/otel/netflow
    journal = a.arch != "armv6l"
    return [
        "cmake",
        "-S",
        ".",
        "-B",
        "build",
        "-DCMAKE_C_COMPILER_LAUNCHER=ccache",
        "-DCMAKE_CXX_COMPILER_LAUNCHER=ccache",
        f"-DCMAKE_INSTALL_PREFIX={NP}",
        f"-DCMAKE_BUILD_TYPE={build_type}",
        "-DSTATIC_BUILD=On",
        "-DBUILD_SHARED_LIBS=Off",
        "-DENABLE_LIBBACKTRACE=On",
        # LTO only for optimized builds (parity mode); Debug skips it.
        f"-DCMAKE_INTERPROCEDURAL_OPTIMIZATION={'Off' if build_type == 'Debug' else 'On'}",
        f"-DUSE_LTO={'Off' if build_type == 'Debug' else 'On'}",
        _flag("PLUGIN_GO", True),
        _flag("PLUGIN_PYTHON", True),
        _flag("PLUGIN_CHARTS", True),
        _flag("BUNDLED_PROTOBUF", True),
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
        # x86-only: the installer force-enables eBPF on x86 hardware.
        _flag("PLUGIN_EBPF", x86),
        # musl: journal uses the internal (Rust) file reader; no systemd units.
        _flag("PLUGIN_SYSTEMD_JOURNAL", journal),
        _flag("NETDATA_JOURNAL_FILE_READER", journal),
        _flag("PLUGIN_SYSTEMD_UNITS", False),
        _flag("PLUGIN_OTEL", full),
        _flag("PLUGIN_NETFLOW", full),
        _flag("PLUGIN_NFACCT", True),
        _flag("PLUGIN_CUPS", False),
        _flag("PLUGIN_FREEIPMI", False),
        _flag("PLUGIN_XENSTAT", False),
        _flag("PLUGIN_IBM", False),
        _flag("PLUGIN_SCRIPTS", False),
        _flag("EXPORTER_MONGODB", False),
        _flag("EXPORTER_PROMETHEUS_REMOTE_WRITE", True),
        _flag("SENTRY", False),
    ]


_NETDATA_WRAPPER = f"""\
#!{NP}/bin/bash
export NETDATA_BASH_LOADABLES="DISABLE"
export PATH="{NP}/bin:${{PATH}}"
exec "{NP}/bin/srv/netdata" "${{@}}"
"""

# INTERP present means dynamically linked: the whole point is a static agent.
_STATIC_CHECK = f"""
set -e
if readelf -l {NP}/bin/srv/netdata 2>/dev/null | grep -q INTERP; then
  echo "ERROR: netdata binary is not statically linked"; exit 1
fi
"""

_RUNTIME_CHECK = f"""
set -e
{NP}/bin/netdata -W buildinfo > /tmp/buildinfo.txt
{NP}/bin/netdata -D > /tmp/netdata.log 2>&1 &
nd_pid=$!
for i in $(seq 1 60); do
  if {NP}/bin/curl -fsS http://localhost:19999/api/v1/info > /tmp/info.json 2>/dev/null; then
    break
  fi
  sleep 1
done
[ -s /tmp/info.json ] || {{ echo "agent API never came up"; tail -50 /tmp/netdata.log; exit 1; }}
echo "static-agent-up version=$(jq -r .version /tmp/info.json)"
kill "$nd_pid" 2>/dev/null || true
wait "$nd_pid" 2>/dev/null || true
rm -rf {NP}/var/lib/netdata {NP}/var/cache/netdata /tmp/info.json /tmp/netdata.log
"""


async def static_build(
    source: dagger.Directory,
    arch: str = "x86_64",
    jobs: int = 0,
    build_type: str = "Debug",
) -> dagger.Directory:
    """Build the self-extracting static installer; returns artifacts dir."""
    if arch not in STATIC_ARCHS:
        raise ValueError(f"unsupported static arch {arch} (know: {sorted(STATIC_ARCHS)})")
    a = STATIC_ARCHS[arch]
    parallel = str(jobs) if jobs > 0 else "$(nproc)"

    version = (await source.file("packaging/version").contents()).strip()
    lsm = (await source.file("packaging/makeself/makeself.lsm").contents()).replace(
        "NETDATA_VERSION", version
    )

    openssl = build_openssl(a)
    curl = build_curl(a, openssl)
    bash_tree = build_bash(a)
    ioping = build_ioping(a)
    nfacct = build_nfacct(a)

    opt = "-O2 -funroll-loops" if build_type != "Debug" else "-O1 -ggdb"
    cflags = (
        f"{a.tuning_flags} {opt} -pipe -I/openssl-static/include"
        " -I/libnetfilter-acct-static/include/libnetfilter_acct -I/curl-local/include/curl"
        " -I/usr/include/libmnl"
    )
    ldflags = (
        "-Wl,--gc-sections -L/openssl-static/lib64 -L/libnetfilter-acct-static/lib"
        " -lnetfilter_acct -L/usr/lib -lmnl -L/usr/lib -lzstd -L/curl-local/lib"
    )
    pkg_config_path = (
        "/openssl-static/lib64/pkgconfig:/libnetfilter-acct-static/lib/pkgconfig"
        ":/usr/lib/pkgconfig:/curl-local/lib/pkgconfig"
    )

    ctr = (
        static_env(a)
        # Destination layout (job 00): bin is real, sbin/usr are views of it.
        .with_exec(
            [
                "sh",
                "-c",
                f"mkdir -p {NP}/bin {NP}/usr && cd {NP} && ln -s bin sbin"
                " && cd usr && ln -s ../bin bin && ln -s ../sbin sbin && ln -s . local",
            ]
        )
        .with_directory("/openssl-static", openssl)
        .with_directory("/curl-local", curl)
        .with_directory(NP, bash_tree)
        .with_directory("/libnetfilter-acct-static", nfacct)
        .with_file("/tmp/ioping", ioping)
        .with_exec(
            [
                "sh",
                "-c",
                f"mkdir -p {NP}/usr/libexec/netdata/plugins.d"
                f" && install -o root -g root -m 4750 /tmp/ioping"
                f" {NP}/usr/libexec/netdata/plugins.d/ioping"
                f" && strip {NP}/usr/libexec/netdata/plugins.d/ioping",
            ]
        )
        .with_exec(["sh", "-c", f"cp /curl-local/bin/curl {NP}/bin/curl && strip {NP}/bin/curl"])
    )

    if a.arm32:
        ucontext = build_libucontext(a)
        ctr = ctr.with_directory("/libucontext-static", ucontext).with_directory(
            "/libunwind-static", build_libunwind(a, ucontext)
        )

    for k, v in a.go_env:
        ctr = ctr.with_env_variable(k, v)

    ctr = (
        with_build_caches(ctr, f"static-{a.arch}")
        .with_directory("/netdata", source)
        .with_workdir("/netdata")
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("CFLAGS", cflags)
        .with_env_variable("LDFLAGS", ldflags)
        .with_env_variable("PKG_CONFIG", "pkg-config --static")
        .with_env_variable("PKG_CONFIG_PATH", pkg_config_path)
        .with_env_variable("RUSTFLAGS", "-C target-feature=+crt-static")
        .with_exec(static_configure_args(a, build_type))
        .with_exec(["sh", "-c", f"cmake --build build --parallel {parallel}"])
        .with_exec(["cmake", "--install", "build"])
        # Install-type stamp (job 71) and conf fixup (job 72).
        .with_new_file(
            f"{NP}/etc/netdata/.install-type",
            f"INSTALL_TYPE='manual-static'\nPREBUILT_ARCH='{a.arch}'\n",
        )
        .with_exec(["sh", "-c", f"rm -f {NP}/etc/netdata/netdata.conf"])
        # Archive-source prep (job 90): wrapper + system scripts + layout.
        .with_exec(
            [
                "sh",
                "-c",
                f"mkdir -p {NP}/system {NP}/bin/srv"
                f" && cp packaging/makeself/post-installer.sh"
                f" packaging/makeself/install-or-update.sh"
                f" packaging/installer/functions.sh {NP}/system/"
                f" && mv {NP}/usr/sbin/netdata {NP}/bin/srv/netdata 2>/dev/null"
                f" || mv {NP}/bin/netdata {NP}/bin/srv/netdata",
            ]
        )
        .with_new_file(f"{NP}/bin/netdata", _NETDATA_WRAPPER, permissions=0o755)
        .with_exec(["sh", "-c", _STATIC_CHECK])
        .with_exec(["sh", "-c", _RUNTIME_CHECK])
        .with_exec(
            [
                "sh",
                "-c",
                f"ln -sf {NP}/bin/netdata-claim.sh {NP}/bin/srv/netdata-claim.sh;"
                f" rm -f {NP}/sbin {NP}/usr/bin {NP}/usr/sbin {NP}/usr/local;"
                f" for d in var/lib/netdata var/cache/netdata var/log/netdata; do"
                f' mkdir -p "{NP}/$d" && touch "{NP}/$d/.keep"; done;'
                f" mkdir -p {NP}/share && cp -a /etc/ssl {NP}/share/ssl",
            ]
        )
        .with_new_file("/tmp/makeself.lsm", lsm)
        .with_exec(
            [
                "sh",
                "packaging/makeself/makeself.sh",
                "--gzip",
                "--complevel",
                "9",
                "--notemp",
                "--needroot",
                "--target",
                NP,
                "--header",
                "packaging/makeself/makeself-header.sh",
                "--lsm",
                "/tmp/makeself.lsm",
                "--license",
                "packaging/makeself/makeself-license.txt",
                "--help-header",
                "packaging/makeself/makeself-help-header.txt",
                NP,
                f"{NP}.gz.run",
                "Netdata, X-Ray Vision for your infrastructure",
                "./system/post-installer.sh",
            ]
        )
        .with_exec(
            [
                "sh",
                "-c",
                f"mkdir -p /artifacts && mv {NP}.gz.run"
                f" /artifacts/netdata-{a.arch}-{version}.gz.run"
                f" && cp /artifacts/netdata-{a.arch}-{version}.gz.run"
                f" /artifacts/netdata-{a.arch}-latest.gz.run",
            ]
        )
    )
    return ctr.directory("/artifacts")
