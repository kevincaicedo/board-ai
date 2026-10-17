# Brief: inf-alloc overview

## Overview
inf-alloc is the allocator under infinitydb's storage engine.
It hands out fixed-size slots from arenas, and each arena carves its slots from aligned pages.

## Components
- arena · Arena · type · src/arena.rs — Owns a set of pages and hands out slots from them
- page · Page · type · src/page.rs — 64 KiB of memory aligned to its size
- slot-pool · Slot pool · module · src/pool/slot.rs — Keeps freed slots for reuse
- aligned · AlignedAlloc · function · src/pool/aligned.rs — Asks the OS for aligned memory

## Relations
- arena → page: carves slots from
- arena → slot-pool: returns freed slots to
- page → aligned: is allocated by

## Flows
### Allocate a slot
1. The caller asks the arena for a slot (src/arena.rs:42)
2. The arena takes one from the slot pool when it has one (src/pool/slot.rs:18-24)
3. Otherwise it bumps the pointer in its current page (src/page.rs:30)
### Free a slot
1. The slot goes back to the pool (src/pool/slot.rs:40)

## Diagram
<diagram title="How inf-alloc hands out memory">
<node id="arena" label="Arena" kind="service"/>
<node id="slot-pool" label="Slot pool" kind="queue"/>
<node id="page" label="Page" kind="db"/>
<edge from="arena" to="slot-pool" label="reuse"/>
<edge from="arena" to="page" label="carve"/>
</diagram>

## Details
### arena
The arena keeps one current page and a list of full ones.
<diagram title="Inside the arena">
<node id="page" label="Current page" kind="db"/>
</diagram>
### slot-pool
A LIFO free list, so a freed slot is reused while it is still in cache.

## Citations
- src/arena.rs:40-58
- src/page.rs:30
- src/pool/slot.rs:18-24
