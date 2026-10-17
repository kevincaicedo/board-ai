Here is the brief you asked for:

# Brief: storage module

Some text before the first section that is not part of any section.

## Overview:
The storage module writes pages to disk.

## Components
* writer | Page writer | type | src/storage/writer.rs -- Writes dirty pages
- `reader` · Page reader · type · `src/storage/reader.rs` – Reads pages back
- not an id with spaces · Broken · type · src/x.rs — Dropped
- lonely
- writer · Duplicate writer · type · src/storage/dup.rs — Dropped as a repeat

## Relations
- writer -> reader: shares the page cache
- writer → ghost: names a part that isn't listed
- nonsense without an arrow

## Notes
This section is not part of the shape.
- dropped

## Flows
1. A step before any flow is dropped
### Write a page
1) Mark the page dirty (src/storage/writer.rs:12)
2. Flush it (it goes to disk eventually)
not a numbered line

## Diagram
Here is the diagram in tags:
<diagram title="Storage">
<node id="writer" label="Page writer" kind="service"/>
<node id="reader" label="Page reader" kind="service"/>
<edge from="writer" to="reader"/>
<node id="cache" label="Page cache" kind="db"/>
</diagram>
<diagram title="A second diagram">
</diagram>

## Details
### ghost
Notes for a part that isn't listed.
### reader
Reads are cached.

## Citations
- src/storage/writer.rs:12
- /etc/passwd:1
- ../outside.rs:3
- src/storage/reader.rs:9-4
- src/storage/reader.rs:L5
- src/storage/writer.rs:12
