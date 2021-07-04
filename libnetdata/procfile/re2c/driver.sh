#!/usr/bin/env bash

set -exu -o pipefail

re2c -W -s --verbose --output proc_pid_io.c proc_pid_io.in.c
re2c -W -s --verbose --output proc_pid_stat.c proc_pid_stat.in.c
re2c -W -s -T --verbose --output proc_pid_status.c proc_pid_status.in.c
re2c -W -s --verbose --output proc_stat.c proc_stat.in.c
