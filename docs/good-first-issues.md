# Good first issues (ready to paste)

Reference for behaviour: Dragonfly (`vendor/dragonfly`, MIT). Packet bytes: gophertunnel.

## 1. Armor reduces damage
Equipped armor should reduce incoming damage (armor points, 4% per point up to 80%).
Look at `crates/server/src/survival.rs` (damage application) and `inventory.rs` (armor slots).
Reference: Dragonfly `server/player/player.go` `Hurt` and `server/item/armour*.go`. Add a test with a full iron set.

## 2. Tool durability
Tools and weapons lose durability on use and break at zero (with sound). Sync the damage value to the client.
See `crates/server/src/breaking.rs`, `survival.rs`, `items.rs`. Reference: Dragonfly `server/item/durability.go`.

## 3. Status effects (golden apple)
Add a minimal effect system (absorption, regeneration) and make golden apples apply it; send `MobEffect`.
See `crates/server/src/survival.rs` and `crates/server/data/foods.json`. Reference: Dragonfly `server/entity/effect/`.

## 4. Gender-neutral death messages
Death messages should use the vanilla translation keys (`death.attack.*`) instead of fixed text.
See `crates/server/src/survival.rs` (death handling) and `chat.rs`. Reference: Dragonfly `server/entity/damage.go` and the damage sources.

## 5. Dead player opening the inventory gets no reply
While on the death screen, container requests are silently ignored; answer with a proper rejection.
See `crates/server/src/handlers.rs` and `inventory.rs`. Reference: gophertunnel `ItemStackResponse` status codes.

## 6. Barrel
Add the barrel as a 27-slot container like the chest (single block, opens from any side).
See `crates/server/src/chest.rs` and `interact.rs`. Reference: Dragonfly `server/block/barrel.go`.

## 7. Blast furnace and smoker
Reuse the furnace logic with recipe filters (ores and metals for the blast furnace, food for the smoker) and double speed.
See `crates/server/src/furnace.rs` and `smelting.rs`. Reference: Dragonfly `server/block/blast_furnace.go`, `smoker.go`.

## 8. Lava and fire damage
Standing in lava or fire damages the player (lava 4 per hit, fire 1 plus burning). Apply it through the existing damage path.
See `crates/server/src/survival.rs` and `world.rs` (block lookup). Reference: Dragonfly `server/block/lava.go`, `fire.go`.

## 9. Drowning
Air supply that drops under water (300 ticks), then 2 damage per second; sync the air meter with the actor data flags.
See `crates/server/src/survival.rs` and `entity.rs`. Reference: Dragonfly `server/player/player.go` breathing logic.

## 10. Furnace XP payout
Furnaces accumulate experience per smelted recipe and give it when the output is taken.
See `crates/server/src/furnace.rs` and `smelting.rs`. Reference: Dragonfly `server/block/furnace.go` and the recipe experience values.
