"""Native DEB/RPM packaging of the Netdata agent.

The mechanics of producing native packages: the cpack profile and DEB
build, the interim spec-driven RPM build (SOW D11=C, replaced when CPack
RPM support lands), and the clean-image install test. All per-distro
knowledge (environments, features, install commands, rpm layout) comes
from the definitions in distros.py.
"""

from __future__ import annotations

from dataclasses import replace

import dagger
from dagger import dag

from .distros import SPECS, DebPackaging, Distro, RpmLayout, RpmPackaging, RpmProtobuf
from .envs import bootstrap, compiler_launcher_args, has_ccache, with_build_caches

SRC_DIR = "/netdata"
BUILD_DIR = "/build"

# Docker platform -> (rpm arch, GOARCH).
_PLATFORM_ARCH: dict[str, tuple[str, str]] = {
    "linux/amd64": ("x86_64", "amd64"),
    "linux/386": ("i686", "386"),
    "linux/arm64": ("aarch64", "arm64"),
    "linux/arm64/v8": ("aarch64", "arm64"),
    "linux/arm/v7": ("armv7l", "arm"),
    "linux/arm/v6": ("armv6l", "arm"),
}

_64BIT_PLATFORMS = ("linux/amd64", "linux/arm64", "linux/arm64/v8")


def _packaging(distro: Distro) -> DebPackaging | RpmPackaging:
    pkg = SPECS[distro].packaging
    if pkg is None:
        raise ValueError(f"{distro.value} has no native package product")
    return pkg


def pkg_env(distro: Distro, platform: str) -> dagger.Container:
    """Container with everything needed to build native packages."""
    pkg = _packaging(distro)
    env = pkg.env
    if isinstance(pkg, DebPackaging) and pkg.deps_64bit and platform in _64BIT_PLATFORMS:
        env = replace(env, deps=(*env.deps, *pkg.deps_64bit))
    return bootstrap(env, platform)


def _flag(name: str, on: bool) -> str:
    return f"-DENABLE_{name}={'On' if on else 'Off'}"


def packaging_configure_args(distro: Distro, platform: str, build_type: str = "Debug") -> list[str]:
    pkg = _packaging(distro)
    amd64 = platform == "linux/amd64"
    bits64 = platform in _64BIT_PLATFORMS
    feats = pkg.features

    args = [
        "cmake",
        "-S",
        SRC_DIR,
        "-B",
        BUILD_DIR,
        "-G",
        "Ninja",
        *compiler_launcher_args(has_ccache(pkg.env)),
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
        # xenstat is only built on 64-bit targets (build-package.sh matrix).
        _flag("PLUGIN_XENSTAT", feats.xenstat and bits64),
        _flag("EXPORTER_MONGODB", feats.mongodb),
        _flag("PLUGIN_EBPF", amd64),
        _flag("PLUGIN_IBM", amd64 and isinstance(pkg, DebPackaging)),
    ]

    # Each RPM entry declares its protobuf resolution (RPM distros ship no
    # static protobuf); the debian-family -dev packages ship libprotobuf.a,
    # so system protobuf needs no hint there.
    if isinstance(pkg, RpmPackaging) and pkg.protobuf is RpmProtobuf.BUNDLED:
        args.append(_flag("BUNDLED_PROTOBUF", True))
    else:
        args.append(_flag("BUNDLED_PROTOBUF", False))
        if isinstance(pkg, RpmPackaging):
            args.append("-DProtobuf_LIBRARY=/usr/lib64/libprotobuf.so")

    return args


# %setup replacement blocks: build from the local SOURCES tree instead of a
# tarball. RpmLayout.MODERN rpm expects the build tree under
# BUILD/<name>-<version>-build; CLASSIC rpm builds directly in BUILD.
_SETUP_MODERN = (
    "cd %{_topdir}\n rm -rf BUILD\n mkdir -p BUILD\n"
    " cp -rf %{_topdir}/SOURCES/netdata-%{version} BUILD/netdata-%{version}-build"
)
_SETUP_CLASSIC = (
    "cd %{_topdir}\n rm -rf BUILD\n mkdir -p BUILD\n"
    " cp -rfT %{_topdir}/SOURCES/netdata-%{version} BUILD"
)


def prepare_spec(spec_text: str, layout: RpmLayout, pkg_version: str) -> str:
    """Adapt netdata.spec.in to build from local sources at pkg_version."""
    out: list[str] = []
    for line in spec_text.splitlines():
        if line.startswith("Source0"):
            out.append("")
        elif line.startswith("%setup"):
            out.append(_SETUP_MODERN if layout is RpmLayout.MODERN else _SETUP_CLASSIC)
        else:
            out.append(line)
    text = "\n".join(out) + "\n"
    if layout is RpmLayout.CLASSIC:
        text = text.replace("${RPM_BUILD_DIR}/%{name}-%{version}", "${RPM_BUILD_DIR}")
    return text.replace("@PACKAGE_VERSION@", pkg_version)


def _install_type_stamp(kind: str, arch: str, prebuilt_distro: str) -> str:
    return f"INSTALL_TYPE='{kind}'\nPREBUILT_ARCH='{arch}'\nPREBUILT_DISTRO='{prebuilt_distro}'\n"


def _package_deb(
    distro: Distro,
    pkg: DebPackaging,
    platform: str,
    source: dagger.Directory,
    parallel: str,
    build_type: str,
) -> dagger.Directory:
    rpm_arch, goarch = _PLATFORM_ARCH[platform]
    # Embed the distro id in the artifact name, as the repos require.
    collect = (
        'set -e; . /etc/os-release; distid="${ID}${VERSION_ID}"; mkdir -p /artifacts; '
        f"for p in {BUILD_DIR}/packages/*.deb {BUILD_DIR}/packages/*.ddeb; do "
        '[ -e "$p" ] || continue; '
        'ext="${p##*.}"; base="$(basename "$p" ".$ext")"; '
        'name="$(echo "$base" | cut -f 1 -d _)"; ver="$(echo "$base" | cut -f 2 -d _)"; '
        'arch="$(echo "$base" | cut -f 3 -d _)"; '
        'mv "$p" "/artifacts/${name}_${ver}+${distid}_${arch}.${ext}"; done'
    )
    key = f"pkg-{distro.value}-{platform.replace('/', '-')}"
    ctr = (
        with_build_caches(pkg_env(distro, platform), key)
        .with_directory(SRC_DIR, source)
        .with_workdir(SRC_DIR)
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("GOOS", "linux")
        .with_env_variable("GOARCH", goarch)
        .with_new_file(
            f"{SRC_DIR}/system/.install-type",
            _install_type_stamp("binpkg-deb", rpm_arch, pkg.prebuilt_distro),
        )
        .with_exec(packaging_configure_args(distro, platform, build_type))
        .with_exec(["sh", "-c", f"cmake --build {BUILD_DIR} --parallel {parallel}"])
        .with_workdir(BUILD_DIR)
        .with_exec(["cpack", "-V", "-G", "DEB"])
        .with_exec(["sh", "-c", collect])
    )
    return ctr.directory("/artifacts")


async def _package_rpm(
    distro: Distro,
    pkg: RpmPackaging,
    platform: str,
    source: dagger.Directory,
) -> dagger.Directory:
    """Spec-driven rpmbuild, matching the artifacts CI publishes today.

    Interim path per SOW decision D11=C: replaced by CPack RPM support in
    Packaging.cmake when that follow-up lands.
    """
    rpm_arch, goarch = _PLATFORM_ARCH[platform]
    topdir = pkg.topdir

    raw_version = await source.file("packaging/version").contents()
    pkg_version = raw_version.strip().lstrip("v").replace("-", ".")
    spec = prepare_spec(await source.file("netdata.spec.in").contents(), pkg.layout, pkg_version)

    src_path = f"{topdir}/SOURCES/netdata-{pkg_version}"
    rpmbuild_dirs = " ".join(
        f"{topdir}/{s}" for s in ("BUILD", "RPMS", "SOURCES", "SPECS", "SRPMS")
    )

    key = f"pkg-{distro.value}-{platform.replace('/', '-')}"
    ctr = (
        with_build_caches(pkg_env(distro, platform), key)
        .with_env_variable("DISABLE_TELEMETRY", "1")
        .with_env_variable("GOOS", "linux")
        .with_env_variable("GOARCH", goarch)
        .with_exec(["sh", "-c", f"mkdir -p {rpmbuild_dirs}"])
        .with_directory(src_path, source)
        .with_new_file(
            f"{src_path}/system/.install-type",
            _install_type_stamp("binpkg-rpm", rpm_arch, pkg.prebuilt_distro),
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
    distro: Distro,
    platform: str,
    source: dagger.Directory,
    jobs: int = 0,
    build_type: str = "Debug",
) -> dagger.Directory:
    """Build native DEB/RPM packages; returns the artifacts directory."""
    pkg = _packaging(distro)
    parallel = str(jobs) if jobs > 0 else "$(nproc)"

    match pkg:
        case DebPackaging():
            return _package_deb(distro, pkg, platform, source, parallel, build_type)
        case RpmPackaging():
            # The spec drives its own cmake invocation; build_type does not
            # apply until CPack RPM support replaces the spec path (SOW D11).
            return await _package_rpm(distro, pkg, platform, source)


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
    distro: Distro,
    platform: str,
    artifacts: dagger.Directory,
) -> dagger.Container:
    """Install built packages in a clean base image and boot the agent."""
    pkg = _packaging(distro)

    ctr = (
        dag.container(platform=dagger.Platform(platform))
        .from_(pkg.env.base_image)
        .with_env_variable("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .with_env_variable("DEBIAN_FRONTEND", "noninteractive")
        .with_env_variable("DISABLE_TELEMETRY", "1")
    )
    # The clean image may need the same repo files as the build env
    # (centos7: vault mirrors).
    for path, contents in pkg.env.files:
        ctr = ctr.with_new_file(path, contents)
    return (
        ctr.with_directory("/artifacts", artifacts)
        .with_exec(["sh", "-c", pkg.test_install])
        .with_exec(["sh", "-c", _RUNTIME_CHECK])
    )
