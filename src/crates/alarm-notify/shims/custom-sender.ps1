# SPDX-License-Identifier: GPL-3.0-or-later
#
# Compatibility shim for the `custom` notification method on Windows.
#
# Windows has no bash, so a `custom_sender()` shell function in
# health_alarm_notify.conf cannot run there. Define a PowerShell function instead,
# in health_alarm_notify_custom.ps1 next to your netdata configuration:
#
#     function Custom-Sender {
#         param([string]$Recipients)
#         $body = "$env:host $env:status_message : $env:alarm"
#         Invoke-RestMethod -Method Post -Uri "https://example/hook" -Body $body
#     }
#
# Every notification variable is in the environment under the documented names, so
# $env:host, $env:status, $env:alarm, $env:goto_url and the rest are all available.
#
# Exit code 0 means delivered; anything else is reported as a failure.
#
# For a sender that works the same way on every platform, set
# CUSTOM_SENDER_COMMAND in health_alarm_notify.conf to any executable instead.

param([Parameter(Position = 0)][string]$Recipients = '')

$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($Recipients)) {
    [Console]::Error.WriteLine('no recipients were given')
    exit 1
}

$configDir = $env:NETDATA_USER_CONFIG_DIR
if ([string]::IsNullOrWhiteSpace($configDir)) {
    $configDir = Join-Path $env:PROGRAMFILES 'Netdata\etc\netdata'
}
$userScript = Join-Path $configDir 'health_alarm_notify_custom.ps1'

if (-not (Test-Path -LiteralPath $userScript)) {
    [Console]::Error.WriteLine("custom notifications are enabled but $userScript does not exist")
    exit 1
}

# Dot-source so the function lands in this scope.
. $userScript

if (-not (Get-Command -Name 'Custom-Sender' -CommandType Function -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine("$userScript does not define a Custom-Sender function")
    exit 1
}

try {
    Custom-Sender -Recipients $Recipients
    exit 0
}
catch {
    [Console]::Error.WriteLine("Custom-Sender failed: $($_.Exception.Message)")
    exit 1
}
