<!-- partial: Text read · 3 figures need a model that reads files -->
# Digest: Unbundled Transactions: A Study of a Layered Key-Value Store
Read: 6 pages; text only

## Sections
### Abstract (p. 1)
We describe a transactional key-value store that unbundles its parts: a transaction system that orders commits, a log system that makes them durable, and a storage system that serves reads. Each role runs as its own process and scales on its own.
### 1 Introduction (p. 1)
Distributed databases usually bundle ordering, durability and storage into one kind of server. When one concern needs more machines, all of them get more machines.
### 2 Architecture (pp. 1–3)
Figure 1 (page 2) shows the roles. Clients link a library that talks to the cluster.
### 3 Committing a Transaction (pp. 3–4)
Figure 2 (page 3) shows the six steps of a commit. A client first asks a proxy for a read version; the proxy asks the sequencer for the latest committed version and returns it.
### 4 Recovery (pp. 4–5)
When any role of the transaction system fails, the system does not try to repair it. Instead it runs recovery, the state machine in Figure 3 (page 4).
### 5 Simulation Testing (p. 5)
Every role is written against an interface for the network, the disk and the clock. In tests these interfaces are replaced by a simulator that runs the whole cluster in one thread.
### 6 Evaluation and Lessons (pp. 5–6)
Table 2 (page 5) reports throughput and latency for three workloads on a cluster of 24 machines. Commit latency stays below 20 milliseconds at the 99th percentile for small transactions.

## Figures
### Figure 1 (p. 2): The architecture: clients, the transaction system (sequencer, proxies and resolvers), the log system and the storage system.
### Figure 2 (p. 3): The commit pipeline, from the read version to the reply.
### Figure 3 (p. 4): Recovery as a state machine; a failure in any state starts it again.

## Tables
### Table 1 (p. 3): Roles in a typical cluster.
| Role Instances Keeps state |
| Sequencer 1 no |
| Proxies 3 to 8 no |
| Resolvers 2 to 4 no |
| Log servers 3 to 12 yes |
| Storage servers 10 to 500 yes |
### Table 2 (p. 5): Throughput and latency by workload on 24 machines.
| Workload Reads/s Writes/s p99 commit (ms) |
| Read-heavy (90/10) 2,400,000 260,000 11 |
| Balanced (50/50) 1,100,000 1,050,000 16 |
| Write-heavy (10/90) 240,000 1,900,000 19 |
