<!-- partial: Text read · 2 images need a model that reads files -->
# Digest: The inf-alloc arena
Read: 25 lines; text only

## Sections
### The inf-alloc arena (ll. 1–4)
The arena hands out memory in slabs, one size class per slab, so a free never searches.
### How a slab is laid out (ll. 5–14)
Each slab holds a header, a free list and the slots themselves:
### Size classes (ll. 15–22)
### Elsewhere (ll. 23–25)
The design follows a diagram drawn for an earlier talk: [image: Remote]

## Figures
### Figure 1 (l. 7): The arena's slabs
### Figure 2 (l. 25): Remote (linked from the web, not fetched)

## Tables
### Table 1 (ll. 17–21): Size classes
| Class | Slot size | Slots per slab |
|---|---|---|
| tiny | 16 B | 256 |
| small | 64 B | 64 |
| medium | 256 B | 16 |
