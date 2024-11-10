import dagger


class Platform:
    def __init__(self, platform: str):
        self.platform = dagger.Platform(platform)

    def escaped(self) -> str:
        return str(self.platform).removeprefix("linux/").replace("/", "_")

    def __eq__(self, other):
        if isinstance(other, Platform):
            return self.platform == other.platform
        elif isinstance(other, dagger.Platform):
            return self.platform == other
        else:
            return NotImplemented

    def __ne__(self, other):
        return not (self == other)

    def __hash__(self):
        return hash(self.platform)

    def __str__(self) -> str:
        return str(self.platform)


SUPPORTED_PLATFORMS = set(
    [
        Platform("linux/x86_64"),
        Platform("linux/arm64"),
        Platform("linux/i386"),
        Platform("linux/arm/v7"),
        Platform("linux/arm/v6"),
        Platform("linux/ppc64le"),
        Platform("linux/s390x"),
        Platform("linux/riscv64"),
    ]
)
