#!/bin/bash
# Runs the bot against the local server for $1 seconds, extra args after; prints a summary.
secs=${1:-40}; shift
# Build from a copy without the knowledge agent's files (lib.rs, knowledge.rs), so its work
# in progress never blocks a live test.
mkdir -p /home/user/bot_build
rsync -a --delete --exclude target --exclude .git --exclude src/lib.rs --exclude src/knowledge.rs --exclude 'examples/' --exclude 'src/bin/' --exclude data/ --exclude tools/ /home/user/mc_speedrun/ /home/user/bot_build/
sed -i 's|path = "../azalea/azalea"|path = "/home/user/azalea/azalea"|' /home/user/bot_build/Cargo.toml
cd /home/user/bot_build && cargo build -q 2>&1 | grep -E "^error" -A6
RUST_LOG=${RUST_LOG:-speedrun_bot=info,azalea=info,azalea_client=info,azalea_buf=warn} timeout $secs target/debug/speedrun_bot --server 127.0.0.1:25565 "$@" > /tmp/claude-0/bot.log 2>&1
sed 's/\x1b\[[0-9;]*m//g' /tmp/claude-0/bot.log > /tmp/claude-0/bot.clean.log
echo "== errors/warnings by kind"; grep -E "ERROR|WARN" /tmp/claude-0/bot.clean.log | sed -E 's/^[^ ]+ +//' | cut -c1-140 | sed -E 's/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -15
echo "== bot info"; grep " INFO speedrun_bot" /tmp/claude-0/bot.clean.log | cut -c28-220 | tail -${TAILN:-15}
echo "== server"; grep -v "Saving\|Loading\|Preparing\|Time elapsed" /home/user/mcserver/run/server.log | tail -6
