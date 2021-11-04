load('//bazel/build_settings:defs.bzl', 'expand_build_settings_template')

# TODO: check if we need more substitutions
SUBSTITUTIONS = {
    '//bazel/build_settings:configdir': '@configdir_POST@',
    '//bazel/build_settings:libconfigdir': '@libconfigdir_POST@',
    '//bazel/build_settings:localstatedir': '@localstatedir_POST@',
    '//bazel/build_settings:sbindir': '@sbindir_POST@',
}

expand_build_settings_template(
    name = 'edit_config',
    template = 'edit-config.in',
    out = 'edit-config',
    substitutions = SUBSTITUTIONS,
    is_executable = True,
)

expand_build_settings_template(
    name = 'netdata_freebsd',
    template = 'netdata-freebsd.in',
    out = 'netdata-freebsd',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_init_d',
    template = 'netdata-init-d.in',
    out = 'netdata-init-d',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_lsb',
    template = 'netdata-lsb.in',
    out = 'netdata-lsb',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_openrc',
    template = 'netdata-openrc.in',
    out = 'netdata-openrc',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_updater_service',
    template = 'netdata-updater.service.in',
    out = 'netdata-updater.service',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_crontab',
    template = 'netdata.crontab.in',
    out = 'netdata.crontab',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_logrotate',
    template = 'netdata.logrotate.in',
    out = 'netdata.logrotate',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_plist',
    template = 'netdata.plist.in',
    out = 'netdata.plist',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_service',
    template = 'netdata.service.in',
    out = 'netdata.service',
    substitutions = SUBSTITUTIONS,
)

expand_build_settings_template(
    name = 'netdata_service_v235',
    template = 'netdata.service.v235.in',
    out = 'netdata.service.v235',
    substitutions = SUBSTITUTIONS,
)
