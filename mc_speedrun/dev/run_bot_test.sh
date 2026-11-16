#!/bin/bash
# Runs the bot against a local server for $1 seconds, extra args after; prints a summary.
# Several test servers can run side by side: MC_PORT (25565), MC_RUN (server dir) and TAG
# (log name) pick one. Full log: /tmp/claude-0/bot$TAG.clean.log.
secs=${1:-40}; shift
port=${MC_PORT:-25565}; run=${MC_RUN:-/home/user/mcserver/run}; log=/tmp/claude-0/bot$TAG
cd "$(dirname "$0")/.." && cargo build -q 2>&1 | grep -E "^error" -A6
mkdir -p /tmp/claude-0
RUST_LOG=${RUST_LOG:-speedrun_bot=info,azalea=info,azalea_client=info,azalea_buf=warn} timeout $secs target/debug/speedrun_bot --server 127.0.0.1:$port "$@" > $log.log 2>&1
sed 's/\x1b\[[0-9;]*m//g' $log.log > $log.clean.log
echo "== errors/warnings by kind"; grep -E "ERROR|WARN" $log.clean.log | sed -E 's/^[^ ]+ +//' | cut -c1-140 | sed -E 's/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -15
echo "== bot info"; grep " INFO speedrun_bot" $log.clean.log | cut -c28-220 | tail -${TAILN:-15}
echo "== server"; grep -v "Saving\|Loading\|Preparing\|Time elapsed" $run/server.log | tail -6
