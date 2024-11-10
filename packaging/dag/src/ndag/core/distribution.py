import dagger

def _install_go(ctr: dagger.Container, platform: dagger.Platform) -> dagger.Container:
    arch_map = {
        "linux/amd64": "amd64",
        "linux/386": "386",
        "linux/arm64": "arm64",
        "linux/arm/v7": "armv6l",
        "linux/arm/v6": "armv6l",
        "linux/ppc64le": "ppc64le",
        "linux/s390x": "s390x",
        "linux/riscv64": "riscv64"
    }

    arch = arch_map.get(str(platform), "amd64")

    go_version = "1.23.5"
    go_tarball = f"go{go_version}.linux-{arch}.tar.gz"
    download_url = f"https://go.dev/dl/{go_tarball}"

    go_root = "/usr/local/go"
    go_path = "/root/go"
    bin_paths = [
        "/root/.cargo/bin",
        f"{go_root}/bin",
        f"{go_path}/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ]

    ctr = (
        ctr.with_workdir("/")
        .with_exec(["wget", "-q", download_url])
        .with_exec(["tar", "-C", "/usr/local", "-xzf", go_tarball])
        .with_exec(["rm", go_tarball])
        .with_env_variable("GOROOT", go_root)
        .with_env_variable("GOPATH", go_path)
        .with_env_variable("PATH", ":".join(bin_paths))
        .with_exec(["go", "version"])
    )

    return ctr


def _install_cargo(ctr: dagger.Container) -> dagger.Container:
    bin_paths = [
        "/root/.cargo/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ]

    ctr = (
        ctr.with_workdir("/")
        .with_exec(["sh", "-c", "curl https://sh.rustup.rs -sSf | sh -s -- -y"])
        .with_env_variable("PATH", ":".join(bin_paths))
        .with_exec(["cargo", "new", "--bin", "hello"])
        .with_workdir("/hello")
        .with_exec(["cargo", "run", "-v", "-v"])
        .with_exec(["rm", "-rf", "/hello"])
    )

    return ctr


_DEBIAN_COMMON_PACKAGES = [
    "autoconf",
    "autoconf-archive",
    "autogen",
    "automake",
    "bison",
    "build-essential",
    "ca-certificates",
    "ccache",
    "cmake",
    "curl",
    "dh-autoreconf",
    "dh-make",
    "dpkg-dev",
    "flex",
    "g++",
    "gcc",
    "git-buildpackage",
    "git-core",
    "golang",
    "libatomic1",
    "libcurl4-openssl-dev",
    "libcups2-dev",
    "libdistro-info-perl",
    "libelf-dev",
    "libipmimonitoring-dev",
    "libjson-c-dev",
    "libyaml-dev",
    "libjudy-dev",
    "liblz4-dev",
    "libmnl-dev",
    "libmongoc-dev",
    "libnetfilter-acct-dev",
    "libpcre2-dev",
    "libprotobuf-dev",
    "libprotoc-dev",
    "libsnappy-dev",
    "libsystemd-dev",
    "libssl-dev",
    "libtool",
    "libuv1-dev",
    "libzstd-dev",
    "make",
    "ninja-build",
    "pkg-config",
    "protobuf-compiler",
    "systemd",
    "uuid-dev",
    "wget",
    "zlib1g-dev",
]

def debian_12(
    client: dagger.Client, platform: dagger.Platform
) -> dagger.Container:
    ctr = client.container(platform=platform).from_("debian:bookworm")

    pkgs = [pkg for pkg in _DEBIAN_COMMON_PACKAGES]

    if platform != dagger.Platform("linux/i386"):
        pkgs.append("libxen-dev")

    ctr = (
        ctr.with_env_variable("DEBIAN_FRONTEND", "noninteractive")
        .with_exec(["apt-get", "update"])
        .with_exec(["apt-get", "upgrade", "-y"])
        .with_exec(["apt-get", "install", "-y", "--no-install-recommends"] + pkgs)
        .with_exec(["apt-get", "clean"])
        .with_exec(["c_rehash"])
        .with_exec(["rm", "-rf", "/var/lib/apt/lists/*"])
    )

    ctr = _install_cargo(ctr)
    ctr = _install_go(ctr, platform)
    return ctr