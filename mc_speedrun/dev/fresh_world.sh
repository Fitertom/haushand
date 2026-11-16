#!/bin/bash
# Restarts a local test server on a new world: fresh_world.sh [seed] [difficulty] [tick rate].
# Then sets the test conditions over RCON (the tick rate, difficulty, permanent day).
# MC_RUN (server dir, default /home/user/mcserver/run), MC_PORT, RCON_PORT pick the server;
# a new MC_RUN dir is set up from the main one's server.jar.
seed=${1:-}; diff=${2:-peaceful}; rate=${3:-60}
run=${MC_RUN:-/home/user/mcserver/run}; port=${MC_PORT:-25565}; export RCON_PORT=${RCON_PORT:-25575}
java=/usr/lib/jvm/java-25-openjdk-amd64/bin/java
rcon="$(cd "$(dirname "$0")" && pwd)/rcon.py"
if [ ! -f $run/server.jar ]; then
  mkdir -p $run && cp /home/user/mcserver/run/server.jar $run/ && echo eula=true > $run/eula.txt
  printf 'online-mode=false\nwhite-list=false\nenforce-whitelist=false\nenable-rcon=true\nrcon.password=testpass\nspawn-protection=0\n' > $run/server.properties
fi
# this server's own ports
sed -i "/^server-port=/d; /^rcon.port=/d" $run/server.properties
printf 'server-port=%s\nrcon.port=%s\n' $port $RCON_PORT >> $run/server.properties
pid=$(pgrep -f -- "-Dmcrun=$run " )
if [ -n "$pid" ]; then
  python3 "$rcon" stop >/dev/null 2>&1
  for _ in $(seq 60); do kill -0 $pid 2>/dev/null || break; sleep 1; done
fi
cd $run
[ -d world ] && mv world "world.old.$(date +%s)" && ls -d world.old.* | head -n -2 | xargs -r rm -rf
sed -i "/^level-seed=/d; /^difficulty=/d" server.properties
printf 'level-seed=%s\ndifficulty=%s\n' "$seed" $diff >> server.properties
setsid nohup $java -Dmcrun=$run -Xmx${MC_MEM:-3G} -jar server.jar nogui > server.log 2>&1 < /dev/null &
until grep -q "Done (" server.log; do sleep 1; grep -q "Exception\|FAILED" server.log && { tail server.log; exit 1; }; done
python3 "$rcon" "tick rate $rate" "difficulty $diff" "time set day" "gamerule advance_time false" "seed"
