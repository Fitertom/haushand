# Azalea → Minecraft 26.3 (protocol 777)

Branch `mc26.3` of this repo, on upstream main `0d38e0d` (which supported 26.2). Commits:
"Update to Minecraft 26.3 (protocol 777)" and "26.3 fixes: ...". `cargo check --workspace`
passes; `--all-targets` (tests) was not fully checked — some simulation tests still use
`ServerboundSwing` / old `ServerboundAcceptTeleportation`.

## How it was done

1. `codegen/migrate.py 26.3` (needs Java 25, gradle, network to Mojang/Fabric). It stopped
   twice; fixed by hand:
   - the Pumpkin extractor (`codegen/__cache__/azalea-pumpkin-extractor`, Fabric mod) did
     not compile for 26.3: removed the Fuels / PotionBrewing / ComposterIncreaseChance
     extractors and the `DropExperienceBlock.CODEC` use in `Blocks.kt` (Azalea only needs
     `blocks`). `__cache__` is git-ignored: redo this if the cache is lost.
   - the interactive `input()` in entity metadata generation; the rest run via a small
     script calling `lib.code.data_components.generate`, `version.set_version_id`.
   - `codegen/lib/code/data_components.py`: skips defaults it cannot express
     (registry holders by id, ResolvableInt/Float references, sign text).
2. The protocol differences were found by comparing the 26.2 and 26.3 server jars'
   bytecode (`javap`), since 26.3 is unobfuscated: `dev/pkdiff.py`, `dev/pkcmp.py`
   (packet read/write sequences), `dev/enumdiff.py` (enum constant order).

## Changes (all verified live except where noted)

- Generated: blocks (poplar, wool/concrete slabs & stairs, …), shapes, registries, tags,
  items, entity metadata (`Cushion`, `DyeColor` serializer), language.
- Item components: new `AttackAnimation`/`InteractAnimation` (were `SwingAnimation`),
  `BlockTransformer`, `VillagerFood`, `Compostable`, `CookingFuel`, `BrewingFuel`
  (`Resolvable<T>`: bool + const or identifier), `MobVisibility`, `ProvidesPotteryPattern`,
  `SignTextFront/Back`, `Waxed`, `CushionColor`; `MapColor` removed; `PotDecorations` is
  four optional `ItemStackTemplate` (item, count, patch).
- `BitSet` on the wire is a byte array now (`BitSet.toByteArray`), not longs — light data.
- `ServerboundPlayerAction::Action`: `ChangeDestroyDirection` inserted at 1, all later
  ordinals +1 (+ `Stab`). Old numbers made every block mined get aborted.
- `ServerboundAcceptTeleportation`: id + position (3 doubles) + yRot, xRot.
  And no `MovePlayerPosRot` after it: 26.3 kicks for two position packets in one tick.
- Move entity packets: VarInt props (bit0 on ground, rest = step count) and a stepped delta
  (chain of VarInt ticks + 3 shorts); `delta` = sum. `MoveEntityRot`: on_ground before rot.
- `ClientboundEntityPositionSync`: `PositionPath` (VarInt type; linear Vec3 or list of
  {Vec3, VarInt tick}) + yRot, xRot floats + on ground; no velocity.
- `ClientboundLevelParticles`: particle first, x/y/z max speed, VarInt count,
  randomization enum.
- `ClientboundExplode`: + `play_sound` bool.
- Advancements: `added` is `PositionedAdvancement { advancement, x, y }`; `DisplayInfo` has
  no x/y and its icon is an `ItemStackTemplate`.
- Recipe `TagSlotDisplay` is a `HolderSet` of items.
- `ClientboundRegistryData`: entries may be non-compound NBT (kept under key `value`).
- Optional game mode (`previous_game_type`): VarInt 0 = none, else id+1. `GameMode` written
  as VarInt.
- New packets: `ServerboundPunch` (unit, replaces `ServerboundSwing` — used for arm swing),
  `ClientboundSwingAnimation`, `ClientboundAddTransientBlock`, `ClientboundPostEffects`
  (game and config). Handlers are empty.
- Tick rate: `ClientboundTickingState` now sets `azalea_core::tick::set_tick_rate`; the
  schedule loop uses it (`azalea-client/src/client.rs`). Verified 60.0 TPS.
- Changed in 26.3 but not believed to change bytes: many packets moved to
  `StreamCodec.composite` with the same field order (commands, light, chunk, chat …).
  `ClientboundOpenSignEditor`/`ServerboundSignUpdate` use a `SignTextSlot` enum — not
  updated (bot does not use signs).
