<!-- partial: Text read · 1 figure needs a model that reads files -->
# Digest: A Small Transaction Path
Read: 3 pages; text only

## Sections
### 1 Overview (pp. 1–2)
Clients send their commits to a proxy. The proxy asks the resolvers whether the transaction conflicts with one committed after it started, then pushes the accepted writes to the log servers.
### 2 Architecture (pp. 2–3)
The figure below shows the five roles and the path a commit takes between them. Each arrow is a network message; the label names what the message asks for.
### 3 Evaluation (p. 3)
We measured each operation on a three-machine cluster. Reads are served by the storage servers alone, so they are the fastest; commits pay for the resolver check and the log push, as Figure 1 shows.

## Figures
### Figure 1 (p. 2): The transaction path from clients to storage.

## Tables
### Table 1 (p. 3): Latency by operation.
| Operation p50 (ms) p99 (ms) |
| Get read version 0.4 1.2 |
| Read 0.3 0.9 |
| Commit 2.1 6.8 |
| Range read (100 keys) 1.1 3.5 |
