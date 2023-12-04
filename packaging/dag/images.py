import dagger


def build_amazon_linux_2(client, platform):
    crt = client.container(platform=platform).from_("amazonlinux:2")

    crt = (
        crt.with_exec(["yum", "update", "-y"])
                .with_exec([
                    "yum", "install", "-y",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bison",
                    "bison-devel",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "flex-devel",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git-core",
                    "golang",
                    "json-c-devel",
                    "libyaml-devel",
                    "libatomic",
                    "libcurl-devel",
                    "libmnl-devel",
                    "libnetfilter_acct-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    if platform == "linux/amd64":
        machine = "x86_64"
    elif platform == "linux/arm64":
        machine = "aarch64"
    else:
        raise Exception("Amaxon Linux 2 supports only linux/amd64 and linux/arm64 platforms.")

    crt = (
        crt.with_file(f"cmake-{machine}.sha256", client.host().file(f"./ci/cmake-{machine}.sha256"))
            .with_exec([
                "curl", "--fail", "-sSL", "--connect-timeout", "20", "--retry", "3", "--output", f"cmake-{machine}.sh",
                f"https://github.com/Kitware/CMake/releases/download/v3.27.6/cmake-3.27.6-linux-{machine}.sh",
            ])
            .with_exec(["sha256sum", "-c", f"cmake-{machine}.sha256"])
            .with_exec(["chmod", "u+x", f"./cmake-{machine}.sh"])
            .with_exec([f"./cmake-{machine}.sh", "--skip-license", "--prefix=/usr/local"])
    )

    crt = (
        crt.with_exec(["yum", "clean", "all"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_centos_7(client, platform):
    crt = client.container(platform=platform).from_("centos:7")

    crt = (
        crt.with_exec(["yum", "install", "-y", "epel-release"])
                .with_exec(["yum", "update", "-y"])
                .with_exec([
                    "yum", "install", "-y",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bash",
                    "bison",
                    "bison-devel",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "flex-devel",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git-core",
                    "golang",
                    "json-c-devel",
                    "libyaml-devel",
                    "libatomic",
                    "libcurl-devel",
                    "libmnl-devel",
                    "libnetfilter_acct-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    if platform == "linux/amd64":
        machine = "x86_64"
    elif platform == "linux/arm64":
        machine = "aarch64"
    else:
        raise Exception("CentOS 7 supports only linux/amd64 and linux/arm64 platforms.")

    crt = (
        crt.with_file(f"cmake-{machine}.sha256", client.host().file(f"./ci/cmake-{machine}.sha256"))
            .with_exec([
                "curl", "--fail", "-sSL", "--connect-timeout", "20", "--retry", "3", "--output", f"cmake-{machine}.sh",
                f"https://github.com/Kitware/CMake/releases/download/v3.27.6/cmake-3.27.6-linux-{machine}.sh",
            ])
            .with_exec(["sha256sum", "-c", f"cmake-{machine}.sha256"])
            .with_exec(["chmod", "u+x", f"./cmake-{machine}.sh"])
            .with_exec([f"./cmake-{machine}.sh", "--skip-license", "--prefix=/usr/local"])
    )

    crt = (
        crt.with_exec(["yum", "clean", "all"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_rocky_linux_9(client, platform):
    crt = client.container(platform=platform).from_("rockylinux:9")

    crt = (
        crt.with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "install", "-y", "--nodocs", "dnf-command(config-manager)", "epel-release"])
                .with_exec(["dnf", "config-manager", "--set-enabled", "crb"])
                .with_exec(["dnf", "clean", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--allowerasing", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "automake",
                    "bash",
                    "bison",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "libcurl-devel",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git",
                    "golang",
                    "json-c-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libyaml-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "nc",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "python3",
                    "python3-pyyaml",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_rocky_linux_8(client, platform):
    crt = client.container(platform=platform).from_("rockylinux:8")

    crt = (
        crt.with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "install", "-y", "--nodocs", "dnf-command(config-manager)", "epel-release"])
                .with_exec(["dnf", "config-manager", "--set-enabled", "powertools"])
                .with_exec(["dnf", "clean", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bash",
                    "bison",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "libcurl-devel",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git",
                    "golang",
                    "json-c-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libyaml-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "nc",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "python3",
                    "python3-pyyaml",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_centos_stream_9(client, platform):
    crt = client.container(platform=platform).from_("quay.io/centos/centos:stream9")

    crt = (
        crt.with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "install", "-y", "--nodocs", "dnf-command(config-manager)", "epel-release"])
                .with_exec(["dnf", "config-manager", "--set-enabled", "crb"])
                .with_exec(["dnf", "clean", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--allowerasing", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "automake",
                    "bash",
                    "bison",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "libcurl-devel",
                    "libyaml-devel",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git",
                    "golang",
                    "json-c-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "nc",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "python3",
                    "python3-pyyaml",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_centos_stream_8(client, platform):
    crt = client.container(platform=platform).from_("quay.io/centos/centos:stream8")

    crt = (
        crt.with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "install", "-y", "--nodocs", "dnf-command(config-manager)", "epel-release"])
                .with_exec(["dnf", "config-manager", "--set-enabled", "powertools"])
                .with_exec(["dnf", "clean", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bash",
                    "bison",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "libcurl-devel",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git",
                    "golang",
                    "json-c-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libyaml-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "nc",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "python3",
                    "python3-pyyaml",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_oracle_linux_9(client, platform):
    crt = client.container(platform=platform).from_("oraclelinux:9")

    crt = (
        crt.with_file("/etc/yum.repos.d/ol9-epel.repo", client.host().file("./ci/ol9-epel.repo"))
                .with_exec(["dnf", "config-manager", "--set-enabled", "ol9_codeready_builder"])
                .with_exec(["dnf", "config-manager", "--set-enabled", "ol9_developer_EPEL"])
                .with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "clean", "-y", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "automake",
                    "bash",
                    "bison",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "libcurl-devel",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git",
                    "golang",
                    "json-c-devel",
                    "libyaml-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "nc",
                    "ninja-build",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "python3",
                    "python3-pyyaml",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_oracle_linux_8(client, platform):
    crt = client.container(platform=platform).from_("oraclelinux:8")

    crt = (
        crt.with_file("/etc/yum.repos.d/ol8-epel.repo", client.host().file("./ci/ol8-epel.repo"))
                .with_exec(["dnf", "config-manager", "--set-enabled", "ol8_codeready_builder"])
                .with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "clean", "-y", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bash",
                    "bison",
                    "cmake",
                    "cups-devel",
                    "curl",
                    "libcurl-devel",
                    "diffutils",
                    "elfutils-libelf-devel",
                    "findutils",
                    "flex",
                    "freeipmi-devel",
                    "gcc",
                    "gcc-c++",
                    "git",
                    "golang",
                    "json-c-devel",
                    "libyaml-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lm_sensors",
                    "lz4-devel",
                    "make",
                    "nc",
                    "ninja-build",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                    "pkgconfig(libmongoc-1.0)",
                    "procps",
                    "protobuf-c-devel",
                    "protobuf-compiler",
                    "protobuf-devel",
                    "python3",
                    "python3-pyyaml",
                    "rpm-build",
                    "rpm-devel",
                    "rpmdevtools",
                    "snappy-devel",
                    "systemd-devel",
                    "wget",
                    "zlib-devel",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_opensuse_tumbleweed(client, platform):
    crt = client.container(platform=platform).from_("opensuse/tumbleweed:latest")

    crt = (
        crt.with_exec(["zypper", "update", "-y"])
                .with_exec([
                    "zypper", "install", "-y", "--allow-downgrade",
                      "autoconf",
                      "autoconf-archive",
                      "autogen",
                      "automake",
                      "bison",
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
                      "go",
                      "json-glib-devel",
                      "judy-devel",
                      "libatomic1",
                      "libcurl-devel",
                      "libelf-devel",
                      "liblz4-devel",
                      "libjson-c-devel",
                      "libyaml-devel",
                      "libmnl0",
                      "libmnl-devel",
                      "libnetfilter_acct1",
                      "libnetfilter_acct-devel",
                      "libpcre2-8-0",
                      "libopenssl-devel",
                      "libtool",
                      "libuv-devel",
                      "libuuid-devel",
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
                      "tar",
                      "wget",
                      "xen-devel",
                ])
    )

    crt = (
        crt.with_exec(["zypper", "clean"])
                .with_exec(["rm", "-rf", "/var/cache/zypp/*/*"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/usr/src/packages/BUILD",
                    "/usr/src/packages/RPMS",
                    "/usr/src/packages/SOURCES",
                    "/usr/src/packages/SPECS",
                    "/usr/src/packages/SRPMS",
                ])
    )

    return crt


def build_opensuse_15_5(client, platform):
    crt = client.container(platform=platform).from_("opensuse/leap:15.5")

    crt = (
        crt.with_exec(["zypper", "update", "-y"])
                .with_exec([
                    "zypper", "install", "-y", "--allow-downgrade",
                      "autoconf",
                      "autoconf-archive",
                      "autogen",
                      "automake",
                      "bison",
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
                      "go",
                      "json-glib-devel",
                      "judy-devel",
                      "libatomic1",
                      "libcurl-devel",
                      "libelf-devel",
                      "liblz4-devel",
                      "libjson-c-devel",
                      "libyaml-devel",
                      "libmnl0",
                      "libmnl-devel",
                      "libnetfilter_acct1",
                      "libnetfilter_acct-devel",
                      "libpcre2-8-0",
                      "libopenssl-devel",
                      "libprotobuf-c-devel",
                      "libtool",
                      "libuv-devel",
                      "libuuid-devel",
                      "libzstd-devel",
                      "make",
                      "ninja",
                      "patch",
                      "pkg-config",
                      "protobuf-devel",
                      "rpm-build",
                      "rpm-devel",
                      "rpmdevtools",
                      "snappy-devel",
                      "systemd-devel",
                      "tar",
                      "wget",
                      "xen-devel",
                ])
    )

    crt = (
        crt.with_exec(["zypper", "clean"])
                .with_exec(["rm", "-rf", "/var/cache/zypp/*/*"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/usr/src/packages/BUILD",
                    "/usr/src/packages/RPMS",
                    "/usr/src/packages/SOURCES",
                    "/usr/src/packages/SPECS",
                    "/usr/src/packages/SRPMS",
                ])
    )

    return crt


def build_opensuse_15_4(client, platform):
    crt = client.container(platform=platform).from_("opensuse/leap:15.4")

    crt = (
        crt.with_exec(["zypper", "update", "-y"])
                .with_exec([
                    "zypper", "install", "-y", "--allow-downgrade",
                      "autoconf",
                      "autoconf-archive",
                      "autogen",
                      "automake",
                      "bison",
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
                      "go",
                      "json-glib-devel",
                      "judy-devel",
                      "libatomic1",
                      "libcurl-devel",
                      "libelf-devel",
                      "liblz4-devel",
                      "libjson-c-devel",
                      "libyaml-devel",
                      "libmnl0",
                      "libmnl-devel",
                      "libnetfilter_acct1",
                      "libnetfilter_acct-devel",
                      "libpcre2-8-0",
                      "libopenssl-devel",
                      "libprotobuf-c-devel",
                      "libtool",
                      "libuv-devel",
                      "libuuid-devel",
                      "libzstd-devel",
                      "make",
                      "ninja",
                      "patch",
                      "pkg-config",
                      "protobuf-devel",
                      "rpm-build",
                      "rpm-devel",
                      "rpmdevtools",
                      "snappy-devel",
                      "systemd-devel",
                      "tar",
                      "wget",
                      "xen-devel",
                ])
    )

    crt = (
        crt.with_exec(["zypper", "clean"])
                .with_exec(["rm", "-rf", "/var/cache/zypp/*/*"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/usr/src/packages/BUILD",
                    "/usr/src/packages/RPMS",
                    "/usr/src/packages/SOURCES",
                    "/usr/src/packages/SPECS",
                    "/usr/src/packages/SRPMS",
                ])
    )

    return crt


def build_fedora_37(client, platform):
    crt = client.container(platform=platform).from_("fedora:37")

    crt = (
        crt.with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "clean", "-y", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bash",
                    "bison",
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
                    "golang",
                    "json-c-devel",
                    "libcurl-devel",
                    "libyaml-devel",
                    "Judy-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libnetfilter_acct-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_fedora_38(client, platform):
    crt = client.container(platform=platform).from_("fedora:38")

    crt = (
        crt.with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "clean", "-y", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bash",
                    "bison",
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
                    "golang",
                    "json-c-devel",
                    "libcurl-devel",
                    "libyaml-devel",
                    "Judy-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libnetfilter_acct-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_fedora_39(client, platform):
    crt = client.container(platform=platform).from_("fedora:39")

    crt = (
        crt.with_exec(["dnf", "distro-sync", "-y", "--nodocs"])
                .with_exec(["dnf", "clean", "-y", "packages"])
                .with_exec([
                    "dnf", "install", "-y", "--nodocs", "--setopt=install_weak_deps=False", "--setopt=diskspacecheck=False",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bash",
                    "bison",
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
                    "golang",
                    "json-c-devel",
                    "libcurl-devel",
                    "libyaml-devel",
                    "Judy-devel",
                    "libatomic",
                    "libmnl-devel",
                    "libnetfilter_acct-devel",
                    "libtool",
                    "libuuid-devel",
                    "libuv-devel",
                    "libzstd-devel",
                    "lz4-devel",
                    "make",
                    "ninja-build",
                    "openssl-devel",
                    "openssl-perl",
                    "patch",
                    "pcre2-devel",
                    "pkgconfig",
                ])
    )

    crt = (
        crt.with_exec(["rm", "-rf", "/var/cache/dnf"])
                .with_exec(["c_rehash"])
                .with_exec([
                    "mkdir", "-p",
                    "/root/rpmbuild/BUILD",
                    "/root/rpmbuild/RPMS",
                    "/root/rpmbuild/SOURCES",
                    "/root/rpmbuild/SPECS",
                    "/root/rpmbuild/SRPMS",
                ])
    )

    return crt


def build_debian_10(client, platform):
    crt = client.container(platform=platform).from_("debian:buster")

    crt = (
        crt.with_env_variable("DEBIAN_FRONTEND", "noninteractive")
                .with_exec(["apt-get", "update"])
                .with_exec(["apt-get", "upgrade", "-y"])
                .with_exec([
                    "apt-get", "install", "-y", "--no-install-recommends",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bison",
                    "build-essential",
                    "ca-certificates",
                    "cmake",
                    "curl",
                    "dh-autoreconf",
                    "dh-make",
                    "dh-systemd",
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
                    "libxen-dev",
                    "libzstd-dev",
                    "make",
                    "ninja-build",
                    "pkg-config",
                    "protobuf-compiler",
                    "systemd",
                    "uuid-dev",
                    "wget",
                    "zlib1g-dev",
                ])
    )

    crt = (
        crt.with_exec(["apt-get", "clean"])
                .with_exec(["c_rehash"])
                .with_exec(["rm", "-rf", "/var/lib/apt/lists/*"])
    )

    return crt

def build_debian_11(client, platform):
    crt = client.container(platform=platform).from_("debian:bullseye")

    crt = (
        crt.with_env_variable("DEBIAN_FRONTEND", "noninteractive")
                .with_exec(["apt-get", "update"])
                .with_exec(["apt-get", "upgrade", "-y"])
                .with_exec([
                    "apt-get", "install", "-y", "--no-install-recommends",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bison",
                    "build-essential",
                    "ca-certificates",
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
                    "libxen-dev",
                    "libzstd-dev",
                    "make",
                    "ninja-build",
                    "pkg-config",
                    "protobuf-compiler",
                    "systemd",
                    "uuid-dev",
                    "wget",
                    "zlib1g-dev",
                ])
    )

    crt = (
        crt.with_exec(["apt-get", "clean"])
                .with_exec(["c_rehash"])
                .with_exec(["rm", "-rf", "/var/lib/apt/lists/*"])
    )

    return crt

def build_debian_12(client, platform):
    crt = client.container(platform=platform).from_("debian:bookworm")

    crt = (
        crt.with_env_variable("DEBIAN_FRONTEND", "noninteractive")
                .with_exec(["apt-get", "update"])
                .with_exec(["apt-get", "upgrade", "-y"])
                .with_exec([
                    "apt-get", "install", "-y", "--no-install-recommends",
                    "autoconf",
                    "autoconf-archive",
                    "autogen",
                    "automake",
                    "bison",
                    "build-essential",
                    "ca-certificates",
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
                ])
    )

    if platform != dagger.Platform("linux/i386"):
        crt = (
            crt.with_exec([
                "apt-get", "install", "-y", "--no-install-recommends", "libxen-dev"
            ])
        )

    crt = (
        crt.with_exec(["apt-get", "clean"])
                .with_exec(["c_rehash"])
                .with_exec(["rm", "-rf", "/var/lib/apt/lists/*"])
    )

    return crt
