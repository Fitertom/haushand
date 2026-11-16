# Handoff: mc_speedrun + azalea (Minecraft 26.3)

This branch only stores the speedrun bot next to the HausHand site files; the two are unrelated.

Start with `mc_speedrun/HANDOFF.md`, then `mc_speedrun/STATUS.md`, `mc_speedrun/TESTING.md`,
`mc_speedrun/KNOWLEDGE.md`, `azalea/PORT_26.3.md`.

- `mc_speedrun/` — the bot, full git history merged in.
- `azalea/` — Azalea port to 26.3 (was branch `mc26.3`). Upstream history (azalea-rs/azalea
  main `0d38e0d`) is squashed into one snapshot commit, because the handoff had a shallow clone;
  the three port commits are on top with their original dates.
- The bot finds Azalea at `../azalea/azalea`, so keep the two directories side by side.
