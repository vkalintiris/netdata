import dagger

from .distribution import debian_12

def distro_container(client: dagger.Client, platform: str):
    return debian_12(client, dagger.Platform(platform))