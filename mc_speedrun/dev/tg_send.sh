#!/bin/bash
# Sends a file to the owner's Telegram chat, and to no one else.
# The owner is @Oshk1, chat 1438692948 (confirmed by the owner, 2026-11-16). A file once
# went to the only other chat the bot knew; the chat id is fixed here and checked again
# against its username before every send, so that cannot happen twice.
set -e
OWNER_ID=1438692948
OWNER_USERNAME=Oshk1
file="$1"; caption="$2"
[ -f "$file" ] || { echo "no file $file"; exit 1; }
T=$(cat /home/user/.tg_token)
who=$(curl -sS -m 20 "https://api.telegram.org/bot$T/getChat?chat_id=$OWNER_ID" | python3 -c "import json,sys;print(json.load(sys.stdin).get('result',{}).get('username',''))")
[ "$who" = "$OWNER_USERNAME" ] || { echo "chat $OWNER_ID is '$who', not @$OWNER_USERNAME: not sending"; exit 1; }
curl -sS -m 300 -F chat_id=$OWNER_ID -F document=@"$file" -F caption="$caption" "https://api.telegram.org/bot$T/sendDocument" | python3 -c "import json,sys;d=json.load(sys.stdin);print('sent' if d.get('ok') else d.get('description'))"
