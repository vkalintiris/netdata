"""Native DEB/RPM packaging of the Netdata agent (cpack).

Packaging environments and the cpack build profile live here as typed
data. Seeded from netdata/helper-images package-builders v2 Dockerfiles
and packaging/build-package.sh (2026-07-16), both reference-only.

The feature profile pins everything build-package.sh pins, plus the
library-detection features (cups, mongodb, nfacct, xenstat, freeipmi)
resolved explicitly per distro from what each packaging environment
actually provides — build-package.sh enables some of these blindly, which
only works where the builder image has the headers. Options neither pinned
here nor distro-dependent follow CMake defaults, as they do today.
"""

from __future__ import annotations

from dataclasses import dataclass

import dagger
from dagger import dag

from .envs import (
    EnvSpec,
    PkgMgr,
    compiler_launcher_args,
    env_spec,
    has_ccache,
    install_go,
    install_rust,
    with_build_caches,
)
from .matrix import Distro, PkgType

# --- packaging environments -------------------------------------------------

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
    # rpm-build/rpm-devel: required by the interim spec-driven RPM path
    # (SOW D11=C); drop again when CPack RPM support replaces it.
    "rpm-build",
    "rpm-devel",
    "rpmdevtools",
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

# Amazon Linux additionally lacks freeipmi and mongoc.
_AMAZON_PKG_DEPS = (
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
    "rpm-build",
    "rpm-devel",
    "rpmdevtools",
    "snappy-devel",
    "systemd-devel",
    "systemd-rpm-macros",
    "tar",
    "unixODBC-devel",
    "wget",
)

# Tumbleweed carries netfilter_acct and xen on top of the Leap set.
_TUMBLEWEED_EXTRA = ("libnetfilter_acct1", "libnetfilter_acct-devel", "xen-devel")

# Oracle's EPEL rebuild (helper-images ships the same definition).
_OL_DEVELOPER_EPEL_REPO = """\
[ol{major}_developer_EPEL]
name=Oracle Linux $releasever EPEL Packages for Development ($basearch)
baseurl=https://yum$ociregion.$ocidomain/repo/OracleLinux/OL{major}/developer/EPEL/$basearch/
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-oracle
gpgcheck=1
enabled=1
"""


def pkg_env_spec(d: Distro, platform: str) -> EnvSpec:
    """Packaging environment spec: build deps + package tooling."""
    base = env_spec(d)
    deps: tuple[str, ...]
    match d.name:
        case "debian" | "ubuntu":
            deps = _DEB_PKG_DEPS
            # xenstat is only built on 64-bit targets (see pkg_features);
            # the Dockerfiles install libxen-dev arch-conditionally too.
            if platform in ("linux/amd64", "linux/arm64", "linux/arm64/v8"):
                deps = (*deps, "libxen-dev")
        case "fedora":
            deps = _FEDORA_PKG_DEPS
        case "rockylinux" | "centos-stream" | "oraclelinux":
            deps = _EL_PKG_DEPS
        case "amazonlinux":
            deps = _AMAZON_PKG_DEPS
        case "opensuse":
            deps = _SUSE_PKG_DEPS
            if d.version == "tumbleweed":
                deps = (*deps, *_TUMBLEWEED_EXTRA)
        case _:
            raise ValueError(f"no packaging environment for distro {d.name}")
    # Packaging needs EPEL on the EL family (libunwind-devel, libmongoc):
    # the source-build envs deliberately do not enable it, so extend here.
    files = base.files
    setup = base.setup
    match d.name:
        case "rockylinux" | "centos-stream":
            setup = (*setup, ("dnf", "install", "-y", "epel-release"))
        case "oraclelinux":
            ver = "8" if d.version == "8" else "9" if d.version == "9" else "10"
            files = (
                *files,
                (
                    f"/etc/yum.repos.d/ol{ver}-epel.repo",
                    _OL_DEVELOPER_EPEL_REPO.format(major=ver),
                ),
            )
    return EnvSpec(base.mgr, deps, files=files, setup=setup, install_flags=base.install_flags)


# --- feature availability per packaging environment --------------------------


@dataclass(frozen=True)
class PkgFeatures:
    mongodb: bool
    nfacct: bool
    xenstat: bool
    freeipmi: bool


def pkg_features(d: Distro, arch_amd64: bool, arch_arm64: bool) -> PkgFeatures:
    match d.name:
        case "debian" | "ubuntu":
            # Old Ubuntu LTS libmongoc is too old for the exporter.
            mongodb = not (d.name == "ubuntu" and d.version in ("20.04", "22.04", "24.04"))
            return PkgFeatures(
                mongodb=mongodb,
                nfacct=True,
                # build-package.sh DEB arch matrix.
                xenstat=arch_amd64 or arch_arm64,
                freeipmi=True,
            )
        case "fedora":
            return PkgFeatures(mongodb=True, nfacct=True, xenstat=True, freeipmi=True)
        case "rockylinux" | "centos-stream" | "oraclelinux":
            return PkgFeatures(mongodb=True, nfacct=False, xenstat=False, freeipmi=True)
        case "amazonlinux":
            return PkgFeatures(mongodb=False, nfacct=False, xenstat=False, freeipmi=False)
        case "opensuse":
            tw = d.version == "tumbleweed"
            return PkgFeatures(mongodb=False, nfacct=tw, xenstat=tw, freeipmi=True)
        case _:
            raise ValueError(f"no packaging features for distro {d.name}")


# --- cpack build ---------------------------------------------------------------

SRC_DIR = "/netdata"
BUILD_DIR = "/build"


def _flag(name: str, on: bool) -> str:
    return f"-DENABLE_{name}={'On' if on else 'Off'}"


def packaging_configure_args(d: Distro, platform: str, build_type: str = "Debug") -> list[str]:
    amd64 = platform in ("linux/amd64",)
    arm64 = platform in ("linux/arm64", "linux/arm64/v8")
    feats = pkg_features(d, amd64, arm64)

    args = [
        "cmake",
        "-S",
        SRC_DIR,
        "-B",
        BUILD_DIR,
        "-G",
        "Ninja",
        *compiler_launcher_args(has_ccache(d, packaging=True)),
        f"-DCMAKE_BUILD_TYPE={build_type}",
        "-DCMAKE_INSTALL_PREFIX=/",
        "-DBUILD_FOR_PACKAGING=On",
        _flag("DASHBOARD", True),
        _flag("DBENGINE", True),
        _flag("ML", True),
        _flag("PLUGIN_APPS", True),
        _flag("PLUGIN_CGROUP_NETWORK", True),
        _flag("PLUGIN_DEBUGFS", True),
        _flag("PLUGIN_GO", True),
        _flag("PLUGIN_SCRIPTS", True),
        _flag("PLUGIN_PYTHON", True),
        _flag("PLUGIN_CHARTS", True),
        _flag("PLUGIN_LOCAL_LISTENERS", True),
        _flag("PLUGIN_NETFLOW", True),
        _flag("PLUGIN_OTEL", True),
        _flag("PLUGIN_PERF", True),
        _flag("PLUGIN_SLABINFO", True),
        _flag("PLUGIN_SYSTEMD_JOURNAL", True),
        _flag("PLUGIN_SYSTEMD_UNITS", True),
        _flag("PLUGIN_CUPS", True),
        _flag("EXPORTER_PROMETHEUS_REMOTE_WRITE", True),
        _flag("BUNDLED_JSONC", False),
        _flag("BUNDLED_YAML", False),
        _flag("LIBBACKTRACE", True),
        _flag("SENTRY", False),
        # Distro/arch-dependent features, resolved explicitly.
        _flag("PLUGIN_FREEIPMI", feats.freeipmi),
        _flag("PLUGIN_NFACCT", feats.nfacct),
        _flag("PLUGIN_XENSTAT", feats.xenstat),
        _flag("EXPORTER_MONGODB", feats.mongodb),
        # build-package.sh DEB arch matrix; RPM effectively gets the same
        # via CMake defaults (ebpf on Linux, ibm off) narrowed to amd64.
        _flag("PLUGIN_EBPF", amd64),
        _flag("PLUGIN_IBM", amd64 and d.packages is not None and d.packages.type == PkgType.DEB),
    ]

    # Protobuf: netdata's cmake prefers static libs when BUILD_SHARED_LIBS
    # is unset, but RPM distros ship no static protobuf. Mirror the spec
    # file: point at the shared lib explicitly, and bundle on openSUSE
    # (whose system protobuf the spec also avoids). Debian-family -dev
    # packages ship libprotobuf.a, so no hint is needed there.
    if d.packages is not None and d.packages.type == PkgType.RPM:
        if d.name == "opensuse":
            args.append(_flag("BUNDLED_PROTOBUF", True))
        else:
            args.append(_flag("BUNDLED_PROTOBUF", False))
            args.append("-DProtobuf_LIBRARY=/usr/lib64/libprotobuf.so")
    else:
        args.append(_flag("BUNDLED_PROTOBUF", False))

    return args


def pkg_env(d: Distro, platform: str) -> dagger.Container:
    """Container with everything needed to build native packages."""
    spec = pkg_env_spec(d, platform)

    ctr = dag.container(platform=dagger.Platform(platform)).from_(d.base_image)
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

    install: list[str]
    match spec.mgr:
        case PkgMgr.APT:
            install = ["apt-get", "install", "-y", "--no-install-recommends", *spec.deps]
        case PkgMgr.DNF:
            install = ["dnf", "install", "-y", *spec.install_flags, *spec.deps]
        case PkgMgr.ZYPPER:
            install = ["zypper", "install", "-y", "--allow-downgrade", *spec.deps]
        case _:
            raise ValueError(f"unsupported packaging package manager {spec.mgr}")
    ctr = ctr.with_exec(install)

    ctr = install_go(ctr, platform)
    ctr = install_rust(ctr, platform)
    return ctr


# Docker platform -> (rpm arch, GOARCH).
_PLATFORM_ARCH: dict[str, tuple[str, str]] = {
    "linux/amd64": ("x86_64", "amd64"),
    "linux/386": ("i686", "386"),
    "linux/arm64": ("aarch64", "arm64"),
    "linux/arm64/v8": ("aarch64", "arm64"),
    "linux/arm/v7": ("armv7l", "arm"),
    "linux/arm/v6": ("armv6l", "arm"),
}

# %setup replacement blocks: build from the local SOURCES tree instead of a
# tarball. Newer rpm (fedora >= 41, suse >= 16) expects the build tree under
# BUILD/<name>-<version>-build; older rpm builds directly in BUILD.
_SETUP_NEW = (
    "cd %{_topdir}\n rm -rf BUILD\n mkdir -p BUILD\n"
    " cp -rf %{_topdir}/SOURCES/netdata-%{version} BUILD/netdata-%{version}-build"
)
_SETUP_OLD = (
    "cd %{_topdir}\n rm -rf BUILD\n mkdir -p BUILD\n"
    " cp -rfT %{_topdir}/SOURCES/netdata-%{version} BUILD"
)


def _new_style_rpm(d: Distro) -> bool:
    if d.name == "fedora":
        return int(d.version) >= 41
    if d.name == "opensuse":
        # tumbleweed snapshot ids and Leap >= 16 both qualify.
        return int(d.version.split(".")[0]) >= 16 if d.version != "tumbleweed" else True
    return False


def prepare_spec(spec: str, d: Distro, pkg_version: str) -> str:
    """Adapt netdata.spec.in to build from local sources at pkg_version."""
    out: list[str] = []
    for line in spec.splitlines():
        if line.startswith("Source0"):
            out.append("")
        elif line.startswith("%setup"):
            out.append(_SETUP_NEW if _new_style_rpm(d) else _SETUP_OLD)
        else:
            out.append(line)
    text = "\n".join(out) + "\n"
    if not _new_style_rpm(d):
        text = text.replace("${RPM_BUILD_DIR}/%{name}-%{version}", "${RPM_BUILD_DIR}")
    return text.replace("@PACKAGE_VERSION@", pkg_version)


def _install_type_stamp(kind: str, arch: str, d: Distro) -> str:
    distro_version = "" if d.version in ("tumbleweed", "latest", "edge") else d.version
    return (
        f"INSTALL_TYPE='{kind}'\n"
        f"PREBUILT_ARCH='{arch}'\n"
        f"PREBUILT_DISTRO='{d.name} {distro_version}'\n"
    )


def _package_deb(
    d: Distro,
    platform: str,
    source: dagger.Directory,
    parallel: str,
    build_type: str,
) -> dagger.Directory:
    rpm_arch, goarch = _PLATFORM_ARCH[platform]
    # Embed the distro id in the artifact name, as the repos require.
    collect = (
        'set -e; . /etc/os-release; distid="${ID}${VERSION_ID}"; mkdir -p /artifacts; '
        f"for pkg in {BUILD_DIR}/packages/*.deb {BUILD_DIR}/packages/*.ddeb; do "
        '[ -e "$pkg" ] || continue; '
        'ext="${pkg##*.}"; base="$(basename "$pkg" ".$ext")"; '
        'name="$(echo "$base" | cut -f 1 -d _)"; ver="$(echo "$base" | cut -f 2 -d _)"; '
        'arch="$(echo "$base" | cut -f 3 -d _)"; '
        'mv "$pkg" "/artifacts/${name}_${ver}+${distid}_${arch}.${ext}"; done'
    )
    key = f"pkg-{d.name}-{d.version}-{platform.replace('/', '-')}"
    ctr = (
        with_build_caches(pkg_env(d, platform), key)
        .with_directory(SRC_DIR, source)
        .with_workdir(SRC_DIR)
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("GOOS", "linux")
        .with_env_variable("GOARCH", goarch)
        .with_new_file(
            f"{SRC_DIR}/system/.install-type", _install_type_stamp("binpkg-deb", rpm_arch, d)
        )
        .with_exec(packaging_configure_args(d, platform, build_type))
        .with_exec(["sh", "-c", f"cmake --build {BUILD_DIR} --parallel {parallel}"])
        .with_workdir(BUILD_DIR)
        .with_exec(["cpack", "-V", "-G", "DEB"])
        .with_exec(["sh", "-c", collect])
    )
    return ctr.directory("/artifacts")


async def _package_rpm(
    d: Distro,
    platform: str,
    source: dagger.Directory,
) -> dagger.Directory:
    """Spec-driven rpmbuild, matching the artifacts CI publishes today.

    Interim path per SOW decision D11=C: replaced by CPack RPM support in
    Packaging.cmake when that follow-up lands.
    """
    rpm_arch, goarch = _PLATFORM_ARCH[platform]
    topdir = "/usr/src/packages" if d.name == "opensuse" else "/root/rpmbuild"

    raw_version = await source.file("packaging/version").contents()
    pkg_version = raw_version.strip().lstrip("v").replace("-", ".")
    spec = prepare_spec(await source.file("netdata.spec.in").contents(), d, pkg_version)

    src_path = f"{topdir}/SOURCES/netdata-{pkg_version}"
    rpmbuild_dirs = " ".join(
        f"{topdir}/{s}" for s in ("BUILD", "RPMS", "SOURCES", "SPECS", "SRPMS")
    )

    key = f"pkg-{d.name}-{d.version}-{platform.replace('/', '-')}"
    ctr = (
        with_build_caches(pkg_env(d, platform), key)
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("GOOS", "linux")
        .with_env_variable("GOARCH", goarch)
        .with_exec(["sh", "-c", f"mkdir -p {rpmbuild_dirs}"])
        .with_directory(src_path, source)
        .with_new_file(
            f"{src_path}/system/.install-type", _install_type_stamp("binpkg-rpm", rpm_arch, d)
        )
        .with_new_file(f"{topdir}/SPECS/netdata.spec", spec)
        .with_exec(
            [
                "rpmbuild",
                "--nobuild",
                "--define",
                "_upstream_go_toolchain 1",
                "--define",
                f"_topdir {topdir}/SOURCES",
                "--define",
                f"_sourcedir {topdir}/SOURCES",
                "--define",
                "source_date_epoch_from_changelog false",
                "--undefine",
                "_disable_source_fetch",
                f"{topdir}/SPECS/netdata.spec",
            ]
        )
        .with_exec(
            [
                "rpmbuild",
                "-bb",
                "--define",
                "_upstream_go_toolchain 1",
                "--rebuild",
                f"{topdir}/SPECS/netdata.spec",
            ]
        )
        .with_exec(
            [
                "sh",
                "-c",
                f"mkdir -p /artifacts && find {topdir}/RPMS -type f -name '*.rpm'"
                " -exec cp {} /artifacts/ \\;",
            ]
        )
    )
    return ctr.directory("/artifacts")


async def package(
    d: Distro,
    platform: str,
    source: dagger.Directory,
    jobs: int = 0,
    build_type: str = "Debug",
) -> dagger.Directory:
    """Build native DEB/RPM packages; returns the artifacts directory."""
    if d.packages is None:
        raise ValueError(f"{d.name}:{d.version} has no package target")

    parallel = str(jobs) if jobs > 0 else "$(nproc)"

    if d.packages.type is PkgType.DEB:
        return _package_deb(d, platform, source, parallel, build_type)
    # The spec drives its own cmake invocation; build_type does not apply
    # to RPMs until CPack RPM support replaces the spec path (SOW D11).
    return await _package_rpm(d, platform, source)


# --- package install test -----------------------------------------------------

_TEST_TOOL_INSTALL: dict[str, str] = {
    "debian": "apt-get update && apt-get install -y $(find /artifacts -type f -name 'netdata*.deb'"
    " ! -name '*dbgsym*' ! -name '*cups*' ! -name '*freeipmi*') && "
    "apt-get install -y --no-install-recommends curl jq",
    "fedora": "dnf install -y /artifacts/netdata*.rpm && dnf install -y curl jq",
    "oraclelinux": "dnf install -y /artifacts/netdata*.rpm && dnf install -y curl jq",
    "centos-stream": "dnf install -y epel-release && dnf install -y /artifacts/netdata*.rpm && "
    "dnf install -y --allowerasing curl jq",
    "rockylinux": "dnf install -y epel-release && dnf install -y /artifacts/netdata*.rpm && "
    "dnf install -y --allowerasing curl jq",
    "amazonlinux": "dnf install -y --allowerasing /artifacts/netdata*.rpm && "
    "dnf install -y --allowerasing curl jq",
    "opensuse": "zypper install -y --allow-downgrade --allow-unsigned-rpm /artifacts/netdata*.rpm"
    " && zypper install -y --allow-downgrade --no-recommends curl jq",
}

# Start the agent, wait for the API, and report version + basic health.
_RUNTIME_CHECK = r"""
set -e
/usr/sbin/netdata -W buildinfo > /tmp/buildinfo.txt
/usr/sbin/netdata -D > /tmp/netdata.log 2>&1 &
for i in $(seq 1 60); do
  if curl -fsS http://localhost:19999/api/v1/info > /tmp/info.json 2>/dev/null; then
    break
  fi
  sleep 1
done
[ -s /tmp/info.json ] || { echo "agent API never came up"; tail -50 /tmp/netdata.log; exit 1; }
ver="$(jq -r .version /tmp/info.json)"
echo "agent-up version=${ver}"
"""


def test_package(
    d: Distro,
    platform: str,
    artifacts: dagger.Directory,
) -> dagger.Container:
    """Install built packages in a clean base image and boot the agent."""
    install = _TEST_TOOL_INSTALL["debian"] if d.name == "ubuntu" else _TEST_TOOL_INSTALL[d.name]

    ctr = (
        dag.container(platform=dagger.Platform(platform))
        .from_(d.base_image)
        .with_env_variable("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .with_env_variable("DEBIAN_FRONTEND", "noninteractive")
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_directory("/artifacts", artifacts)
        .with_exec(["sh", "-c", install])
        .with_exec(["sh", "-c", _RUNTIME_CHECK])
    )
    return ctr
