import os
import pathlib
import dagger
import dagger.log

from ndag.core import distro_container

from .config import Config, CMake


def install_agent(client: dagger.Client, platform: str, repo_root: pathlib.Path, cfg: Config):
    repo_root = repo_root.absolute().as_posix()
    cm = CMake(client, cfg)

    ctr = distro_container(client, platform)

    exclude_dirs = [
        os.path.join(repo_root, p) for p in [".git/", "build/", "packaging/dag/"]
    ]

    ctr = (
        ctr.with_directory(
            cfg.source_dir, client.host().directory(repo_root),
            exclude=exclude_dirs,
        )
        .with_workdir(cfg.source_dir)
    )

    ctr = ctr.with_mounted_cache("/root/.ccache", client.cache_volume("netdata-ccache"))

    ctr = cm.configure(ctr)
    ctr = cm.build(ctr)
    ctr = cm.install(ctr)
    return ctr
