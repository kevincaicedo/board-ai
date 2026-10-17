<!-- ready: 2 pages · text read -->
# Digest: Notes on Write-Ahead Logging
Read: 2 pages; text only

## Sections
### 1 Introduction (pp. 1–2)
A write-ahead log records every change before the change reaches the data files. If the machine stops halfway through a write, the log still holds a complete description of what was meant to happen, and recovery replays it.
### 2 Design (p. 2)
Each log record carries a sequence number, the page it changes, and the bytes before and after the change. A checkpoint notes the oldest record that recovery still needs, so the log can be truncated behind it.
