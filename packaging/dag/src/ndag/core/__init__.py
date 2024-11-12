import dagger

from .distribution import Distribution

def distro_container(client: dagger.Client, platform: str, distro: str):
    return Distribution(distro).container(client, dagger.Platform(platform))