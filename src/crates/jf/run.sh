#!/usr/bin/env bash

set -exu -o pipefail

cargo build -p nom --release
rm -f ~/opt/master/netdata/usr/libexec/netdata/plugins.d/nom.plugin
cp $PWD/target/release/nom ~/opt/master/netdata/usr/libexec/netdata/plugins.d/nom.plugin
find ~/opt/master/netdata/var/log -type f -delete
find ~/opt/master/netdata/var/cache -type f -delete

cd /home/vk/repos/nd/master
just run 19999
