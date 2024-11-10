from dataclasses import dataclass
from typing import Optional
from enum import Enum

import dagger

class BuildType(Enum):
    DEBUG = "Debug"
    RELEASE = "Release"
    RELEASE_WITH_DEBUG_INFO = "RelWithDebInfo"


@dataclass
class Config:
    source_dir: str = "/src/netdata"
    build_dir: str = "/src/netdata/build"
    install_prefix: str = "/opt/netdata"

    build_type: str = "Release"
    build_shared_libs: Optional[bool] = None
    static_build: bool = False
    build_for_packaging: bool = False

    use_cxx11: bool = False
    use_mold: bool = True

    enable_ml: bool = True
    enable_dbengine: bool = True
    enable_dashboard: bool = True

    enable_plugin_go: bool = True
    enable_plugin_python: bool = True
    enable_plugin_apps: bool = True
    enable_plugin_charts: bool = True
    enable_plugin_cups: bool = True
    enable_plugin_freeipmi: bool = True
    enable_plugin_nfacct: bool = True
    enable_plugin_xenstat: bool = True

    enable_plugin_cgroup_network: bool = True
    enable_plugin_debugfs: bool = False
    enable_plugin_ebpf: bool = True
    enable_legacy_ebpf_programs: bool = True
    enable_plugin_local_listeners: bool = True
    enable_plugin_network_viewer: bool = True
    enable_plugin_perf: bool = True
    enable_plugin_slabinfo: bool = True
    enable_plugin_systemd_journal: bool = True

    enable_exporter_prometheus_remote_write: bool = True
    enable_exporter_mongodb: bool = True

    enable_bundled_jsonc: bool = False
    enable_bundled_yaml: bool = False
    enable_bundled_protobuf: bool = False

    enable_webrtc: bool = False
    enable_h2o: bool = False

    enable_sentry: bool = False
    force_legacy_libbpf: bool = False

    netdata_user: str = "netdata"

    def args(self) -> list[str]:
        l = [
            "-G", "Ninja",
            "-DCMAKE_EXPORT_COMPILE_COMMANDS=On",
            "-DCMAKE_C_COMPILER_LAUNCHER=ccache",
            "-DCMAKE_CXX_COMPILER_LAUNCHER=ccache",
        ]

        l.extend([
            f"-DCMAKE_BUILD_TYPE={self.build_type}",
            f"-DCMAKE_INSTALL_PREFIX={self.install_prefix}",
        ])

        if self.build_shared_libs is not None:
            l.append(f"-DBUILD_SHARED_LIBS={'On' if self.build_shared_libs else 'Off'}")

        if self.static_build:
            l.append("-DSTATIC_BUILD=On")

        if self.build_for_packaging:
            l.append("-DBUILD_FOR_PACKAGING=On")

        bool_options = {
            "USE_CXX_11": self.use_cxx11,
            "USE_MOLD": self.use_mold,
            "ENABLE_ML": self.enable_ml,
            "ENABLE_DBENGINE": self.enable_dbengine,
            "ENABLE_DASHBOARD": self.enable_dashboard,
            "ENABLE_PLUGIN_GO": self.enable_plugin_go,
            "ENABLE_PLUGIN_PYTHON": self.enable_plugin_python,
            "ENABLE_PLUGIN_APPS": self.enable_plugin_apps,
            "ENABLE_PLUGIN_CHARTS": self.enable_plugin_charts,
            "ENABLE_PLUGIN_CUPS": self.enable_plugin_cups,
            "ENABLE_PLUGIN_FREEIPMI": self.enable_plugin_freeipmi,
            "ENABLE_PLUGIN_NFACCT": self.enable_plugin_nfacct,
            "ENABLE_PLUGIN_XENSTAT": self.enable_plugin_xenstat,
            "ENABLE_PLUGIN_CGROUP_NETWORK": self.enable_plugin_cgroup_network,
            "ENABLE_PLUGIN_DEBUGFS": self.enable_plugin_debugfs,
            "ENABLE_PLUGIN_EBPF": self.enable_plugin_ebpf,
            "ENABLE_LEGACY_EBPF_PROGRAMS": self.enable_legacy_ebpf_programs,
            "ENABLE_PLUGIN_LOCAL_LISTENERS": self.enable_plugin_local_listeners,
            "ENABLE_PLUGIN_NETWORK_VIEWER": self.enable_plugin_network_viewer,
            "ENABLE_PLUGIN_PERF": self.enable_plugin_perf,
            "ENABLE_PLUGIN_SLABINFO": self.enable_plugin_slabinfo,
            "ENABLE_PLUGIN_SYSTEMD_JOURNAL": self.enable_plugin_systemd_journal,
            "ENABLE_EXPORTER_PROMETHEUS_REMOTE_WRITE": self.enable_exporter_prometheus_remote_write,
            "ENABLE_EXPORTER_MONGODB": self.enable_exporter_mongodb,
            "ENABLE_BUNDLED_JSONC": self.enable_bundled_jsonc,
            "ENABLE_BUNDLED_YAML": self.enable_bundled_yaml,
            "ENABLE_BUNDLED_PROTOBUF": self.enable_bundled_protobuf,
            "ENABLE_WEBRTC": self.enable_webrtc,
            "ENABLE_H2O": self.enable_h2o,
            "ENABLE_SENTRY": self.enable_sentry,
            "FORCE_LEGACY_LIBBPF": self.force_legacy_libbpf,
        }

        for option, value in bool_options.items():
            l.append(f"-D{option}={'On' if value else 'Off'}")

        if self.netdata_user:
            l.append(f"-DNETDATA_USER={self.netdata_user}")

        l.extend(["-S", self.source_dir, "-B", self.build_dir])

        return l

class CMake:
    def __init__(self, client: dagger.Client, cfg: Config):
        self.client = client
        self.cfg = cfg

    def configure(self, ctr: dagger.Container) -> dagger.Container:
        cmd = ["cmake"] + self.cfg.args()
        ctr = (
            ctr.with_exec(cmd)
        )
        return ctr
    
    def build(self, ctr: dagger.Container) -> dagger.Container:
        cmd = ["cmake", "--build", self.cfg.build_dir]
        ctr = (
            ctr.with_exec(cmd)
        )
        return ctr

    def install(self, ctr: dagger.Container) -> dagger.Container:
        cmd = ["cmake", "--install", self.cfg.build_dir]
        ctr = (
            ctr.with_exec(cmd)
               .with_exec(["/opt/netdata/usr/sbin/netdata", "-v"])
        )
        return ctr