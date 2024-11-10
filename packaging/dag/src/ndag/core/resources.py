from importlib.resources import files

from pathlib import Path


def resource_path(resource: str) -> Path:
    return files('ndag.assets').joinpath(resource).as_posix()


def resource_read(resource: str) -> str:
    return resource_path(resource).read_text()
