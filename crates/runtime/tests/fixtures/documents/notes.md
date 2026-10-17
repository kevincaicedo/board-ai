# The inf-alloc arena

The arena hands out memory in slabs, one size class per slab, so a free never searches.

## How a slab is laid out

![The arena's slabs](images/arena.png)

Each slab holds a header, a free list and the slots themselves:

- the header names the size class and counts the free slots;
- the free list is a stack of slot indexes;
- a slot is returned to the stack when it is freed.

## Size classes

| Class | Slot size | Slots per slab |
|---|---|---|
| tiny | 16 B | 256 |
| small | 64 B | 64 |
| medium | 256 B | 16 |

## Elsewhere

The design follows a diagram drawn for an earlier talk: ![Remote](https://example.com/x.png)
