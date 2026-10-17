# Digest: Unbundled Transactions: A Study of a Layered Key-Value Store
## Sections
### Abstract (pp. 1–1)
The paper describes a transactional key-value store split into a transaction system that orders commits, a log system that makes them durable, and a storage system that serves reads. Each role runs as its own process and scales on its own. Transactions are serializable, using multi-version concurrency control with optimistic conflict checks. Recovery replaces the whole transaction system in seconds, and every process is tested in a deterministic simulator that replays failures from a seed.

### 1 Introduction (pp. 1–1)
Conventional distributed databases bundle ordering, durability and storage into one kind of server, so scaling one concern scales all of them. This design makes each concern a separate role. It rests on three properties: a single sequencer gives one total order of versions, the log is the source of truth, and failures are handled by whole-system recovery rather than in-place repair. The authors admit that each commit crosses several machines and so adds latency. They argue the cost is small because each role becomes easy to reason about, scale and test on its own.

### 2 Architecture (pp. 1–3)
Clients link a library. The transaction system has a sequencer that hands out read and commit versions, proxies that accept and batch commits, and resolvers that check conflicts. The log servers keep the durable log, the storage servers keep the data by key range, and a few coordinators store the cluster configuration so recovery can find it. Roles share no memory or disks, and every message carries a version so stale configurations are ignored. A data distribution process splits and moves hot or large ranges, copying data first and switching ownership last. Each range is held by three storage servers in different fault domains, and clients cache the key-range map only as a hint. The sequencer advances versions about a million times per second, and storage servers keep a few seconds of old versions in memory. Table 1 lists the roles.

### 3 Committing a Transaction (pp. 3–4)
A client gets a read version through a proxy, reads directly from storage servers at that version, and buffers its writes locally. At commit it sends the read set, write set and read version to a proxy. The proxy gets a commit version from the sequencer, and the resolvers reject the transaction if any key it read was written after its read version. Accepted transactions are pushed to the log servers, and the proxy replies once every log server has made the batch durable. Read-only transactions never touch the resolvers or logs. Proxies batch both commits and read-version requests. Resolvers keep five seconds of history, which also bounds how long a transaction may run. Clients retry rejected transactions with a new read version, so transactions must be safe to run more than once.

### 4 Recovery (pp. 4–5)
When any transaction-system role fails, the system runs recovery instead of repairing the role. A new sequencer reads the configuration from the coordinators, locks the old log servers, recruits new proxies, resolvers and log servers, and copies the old log tail that might hold committed but unapplied transactions. Recovery takes a few seconds, and during that time commits stop but reads of already committed versions continue. Locking a quorum of old log servers makes it safe, because no old-generation proxy can make another transaction durable. A failure during recovery just restarts it with a newer generation number.

### 5 Simulation Testing (pp. 5–5)
Every role is written against interfaces for network, disk and clock, and tests swap in a single-threaded simulator. The simulator picks message and timer order from a random seed, injects faults, and checks invariants after every step, so any failure replays exactly from its seed. It also compresses time, so a simulated hour of a busy cluster takes a few seconds. The cost is discipline: code may not use the real clock, threads or network, and a lint enforces this. Real clusters are still run nightly, but only for performance regressions and operating-system problems. The authors also plant bugs on purpose to find blind spots in the workload and fault injection.

### 6 Evaluation and Lessons (pp. 5–6)
Table 2 reports three workloads on 24 machines, with p99 commit latency below 20 ms. Read throughput grows almost linearly with storage servers, and write throughput is bounded by the log servers' disks. The lessons are that unbundling makes roles simple to reason about and test at a latency cost, that stateless roles can be replaced freely, and that every queue and buffer needs a fixed size and a defined behaviour when full. The authors say they would make the same choice again.

## Figures
### Figure 1 (p. 2): The architecture: clients, the transaction system (sequencer, proxies and resolvers), the log system and the storage system.
The figure has three dashed groups. Clients holds three client boxes. Transaction system holds the sequencer, proxies and resolvers. Log system and storage system holds the log servers, storage servers and coordinators. A single arrow labelled "1. read version, commit" runs from the middle client to the proxies, and the proxies have arrows to the sequencer and to the resolvers. An arrow labelled "3. push" runs from the proxies to the log servers, and an arrow labelled "pull" runs from the log servers to the storage servers. No step "2" is labelled, and the coordinators have no arrows.
<diagram title="Figure 1">
<node id="clients" label="Clients" kind="group"/>
<node id="txn-system" label="Transaction system" kind="group"/>
<node id="log-storage" label="Log system and storage system" kind="group"/>
<node id="client-1" label="client" kind="actor" in="clients"/>
<node id="client-2" label="client" kind="actor" in="clients"/>
<node id="client-3" label="client" kind="actor" in="clients"/>
<node id="sequencer" label="Sequencer" kind="service" in="txn-system"/>
<node id="proxies" label="Proxies" kind="service" in="txn-system"/>
<node id="resolvers" label="Resolvers" kind="service" in="txn-system"/>
<node id="log-servers" label="Log servers" kind="db" in="log-storage"/>
<node id="storage-servers" label="Storage servers" kind="db" in="log-storage"/>
<node id="coordinators" label="Coordinators" kind="service" in="log-storage"/>
<edge from="client-2" to="proxies" label="1. read version, commit"/>
<edge from="proxies" to="sequencer" label=""/>
<edge from="proxies" to="resolvers" label=""/>
<edge from="proxies" to="log-servers" label="3. push"/>
<edge from="log-servers" to="storage-servers" label="pull"/>
</diagram>

### Figure 2 (p. 3): The commit pipeline, from the read version to the reply.
Six boxes show the steps of a commit. The top row runs left to right: 1. Get read version, 2. Read and buffer writes, 3. Send commit to a proxy. A long arrow then drops from step 3 to step 4 on the bottom row. The bottom row continues left to right: 4. Resolve conflicts, 5. Push to log servers, 6. Reply to the client.
<diagram title="Figure 2">
<node id="step-1" label="1. Get read version" kind="state"/>
<node id="step-2" label="2. Read and buffer writes" kind="state"/>
<node id="step-3" label="3. Send commit to a proxy" kind="state"/>
<node id="step-4" label="4. Resolve conflicts" kind="state"/>
<node id="step-5" label="5. Push to log servers" kind="state"/>
<node id="step-6" label="6. Reply to the client" kind="state"/>
<edge from="step-1" to="step-2" label=""/>
<edge from="step-2" to="step-3" label=""/>
<edge from="step-3" to="step-4" label=""/>
<edge from="step-4" to="step-5" label=""/>
<edge from="step-5" to="step-6" label=""/>
</diagram>

### Figure 3 (p. 4): Recovery as a state machine; a failure in any state starts it again.
Five state boxes form a cycle. Reading the configuration leads to Locking the old logs, then to Recruiting new roles, then to Recovering transactions, then to Accepting commits. One arrow labelled "failure" runs from Accepting commits back to Reading the configuration. The caption and the text of Section 4 say a failure in any state restarts recovery, but the drawing shows only this one failure arrow.
<diagram title="Figure 3">
<node id="reading-config" label="Reading the configuration" kind="state"/>
<node id="locking-logs" label="Locking the old logs" kind="state"/>
<node id="recruiting" label="Recruiting new roles" kind="state"/>
<node id="recovering" label="Recovering transactions" kind="state"/>
<node id="accepting" label="Accepting commits" kind="state"/>
<edge from="reading-config" to="locking-logs" label=""/>
<edge from="locking-logs" to="recruiting" label=""/>
<edge from="recruiting" to="recovering" label=""/>
<edge from="recovering" to="accepting" label=""/>
<edge from="accepting" to="reading-config" label="failure"/>
</diagram>

## Tables
### Table 1 (p. 3): Roles in a typical cluster.
| Role | Instances | Keeps state |
|---|---|---|
| Sequencer | 1 | no |
| Proxies | 3 to 8 | no |
| Resolvers | 2 to 4 | no |
| Log servers | 3 to 12 | yes |
| Storage servers | 10 to 500 | yes |

### Table 2 (p. 5): Throughput and latency by workload on 24 machines.
| Workload | Reads/s | Writes/s | p99 commit (ms) |
|---|---|---|---|
| Read-heavy (90/10) | 2,400,000 | 260,000 | 11 |
| Balanced (50/50) | 1,100,000 | 1,050,000 | 16 |
| Write-heavy (10/90) | 240,000 | 1,900,000 | 19 |

## Claims
- The store splits ordering, durability and storage into separate roles that each run as their own process and scale independently. (p. 1)
- A single sequencer gives every committed transaction a place in one total order. (p. 1)
- A transaction counts as committed once its writes are durable on the log servers, before any storage server has applied them. (p. 1)
- Only the log servers and storage servers keep state, so every transaction-system role can be replaced without losing data. (p. 2)
- The client's key-range cache is only a hint, and a stale entry costs one extra round trip. (p. 2)
- A resolver rejects a transaction when any key it read was written by a transaction that committed after its read version. (p. 3)
- The proxy replies to the client only after every log server has made the batch durable. (p. 3)
- Read-only transactions never contact the resolvers or logs, which is why reads scale with the number of storage servers. (p. 3)
- Resolvers keep only five seconds of write history, which bounds their memory and the running time of a transaction. (p. 4)
- Batching read-version requests kept the sequencer under a tenth of one core on a 24-machine cluster. (p. 4)
- Recovery replaces the transaction system rather than repairing it, taking a few seconds during which reads of committed versions continue. (p. 4)
- Locking a quorum of the old log servers makes recovery safe, because no old-generation proxy can make another transaction durable. (p. 4)
- A failure during recovery restarts recovery from the first state with a newer generation number. (p. 5)
- The deterministic simulator replays any failure exactly from its seed, and a simulated hour of a busy cluster takes a few seconds of real time. (p. 5)
- Real-cluster tests are kept for performance regressions and operating-system problems, while logic errors are found and fixed in simulation. (p. 5)
- Commit latency stays below 20 ms at the 99th percentile, read throughput grows almost linearly with storage servers, and write throughput is bounded by the log servers' disks. (p. 5)
- Fixed-size queues and buffers that slow clients down when full were adopted after early unbounded versions ran out of memory under a write burst. (p. 6)
- The main cost of unbundling is that a commit crosses four roles, which the authors say adds a few milliseconds and was worth paying. (p. 6)
