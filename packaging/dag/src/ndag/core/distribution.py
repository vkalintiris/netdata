import dagger

from .platform import SUPPORTED_PLATFORMS
from . import builders


SUPPORTED_DISTRIBUTIONS = set(
    [
        "alpine_3_18",
        "alpine_3_19",
        "amazonlinux2",
        "centos7",
        "centos-stream9",
        "debian10",
        "debian11",
        "debian12",
        "fedora37",
        "fedora38",
        "fedora39",
        "fedora40",
        "fedora41",
        "opensuse15.4",
        "opensuse15.5",
        "opensuse15.6",
        "opensusetumbleweed",
        "oraclelinux8",
        "oraclelinux9",
        "rockylinux8",
        "rockylinux9",
        "ubuntu20.04",
        "ubuntu22.04",
        "ubuntu23.04",
        "ubuntu23.10",
        "ubuntu24.04",
        "ubuntu24.10",
    ]
)


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
    )

    return ctr


class Distribution:
    def __init__(self, display_name):
        self.display_name = display_name

        if self.display_name == "alpine_3_18":
            self.docker_tag = "alpine:3.18"
            self.builder = builders.alpine_3_18
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "alpine_3_19":
            self.docker_tag = "alpine:3.19"
            self.builder = builders.alpine_3_19
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "amazonlinux2":
            self.docker_tag = "amazonlinux:2"
            self.builder = builders.amazon_linux_2
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "centos7":
            self.docker_tag = "centos:7"
            self.builder = builders.centos_7
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "centos-stream9":
            self.docker_tag = "quay.io/centos/centos:stream9"
            self.builder = builders.centos_stream_9
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "debian10":
            self.docker_tag = "debian:10"
            self.builder = builders.debian_10
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "debian11":
            self.docker_tag = "debian:11"
            self.builder = builders.debian_11
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "debian12":
            self.docker_tag = "debian:12"
            self.builder = builders.debian_12
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "fedora37":
            self.docker_tag = "fedora:37"
            self.builder = builders.fedora_37
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "fedora38":
            self.docker_tag = "fedora:38"
            self.builder = builders.fedora_38
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "fedora39":
            self.docker_tag = "fedora:39"
            self.platforms = SUPPORTED_PLATFORMS
            self.builder = builders.fedora_39
        elif self.display_name == "fedora40":
            self.docker_tag = "fedora:40"
            self.platforms = SUPPORTED_PLATFORMS
            self.builder = builders.fedora_40
        elif self.display_name == "fedora41":
            self.docker_tag = "fedora:41"
            self.platforms = SUPPORTED_PLATFORMS
            self.builder = builders.fedora_41
        elif self.display_name == "opensuse15.4":
            self.docker_tag = "opensuse/leap:15.4"
            self.builder = builders.opensuse_15_4
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "opensuse15.5":
            self.docker_tag = "opensuse/leap:15.5"
            self.builder = builders.opensuse_15_5
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "opensuse15.6":
            self.docker_tag = "opensuse/leap:15.6"
            self.builder = builders.opensuse_15_6
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "opensusetumbleweed":
            self.docker_tag = "opensuse/tumbleweed:latest"
            self.builder = builders.opensuse_tumbleweed
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "oraclelinux8":
            self.docker_tag = "oraclelinux:8"
            self.builder = builders.oracle_linux_8
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "oraclelinux9":
            self.docker_tag = "oraclelinux:9"
            self.builder = builders.oracle_linux_9
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "rockylinux8":
            self.docker_tag = "rockylinux:8"
            self.builder = builders.rocky_linux_8
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "rockylinux9":
            self.docker_tag = "rockylinux:9"
            self.builder = builders.rocky_linux_9
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "ubuntu20.04":
            self.docker_tag = "ubuntu:20.04"
            self.builder = builders.ubuntu_20_04
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "ubuntu22.04":
            self.docker_tag = "ubuntu:22.04"
            self.builder = builders.ubuntu_22_04
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "ubuntu23.04":
            self.docker_tag = "ubuntu:23.04"
            self.builder = builders.ubuntu_23_04
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "ubuntu23.10":
            self.docker_tag = "ubuntu:23.10"
            self.builder = builders.ubuntu_23_10
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "ubuntu24.04":
            self.docker_tag = "ubuntu:24.04"
            self.builder = builders.ubuntu_24_04
            self.platforms = SUPPORTED_PLATFORMS
        elif self.display_name == "ubuntu24.10":
            self.docker_tag = "ubuntu:24.10"
            self.builder = builders.ubuntu_24_10
            self.platforms = SUPPORTED_PLATFORMS
        else:
            raise ValueError(f"Unknown distribution: {self.display_name}")

    def container(
        self, client: dagger.Client, platform: dagger.Platform
    ) -> dagger.Container:
        if platform not in self.platforms:
            raise ValueError(
                f"Building {self.display_name} is not supported on {platform}."
            )

        ctr = self.builder(client, platform)
        ctr = _install_cargo(ctr)
        return ctr
