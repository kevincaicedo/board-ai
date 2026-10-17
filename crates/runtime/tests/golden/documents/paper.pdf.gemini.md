<!-- ready: 6 pages · text, 2 tables and 3 of 3 figures read · figures by Gemini 3.8 Flash -->
# Digest: Unbundled Transactions: A Study of a Layered Key-Value Store
Read: 6 pages; figures by Gemini 3.8 Flash

## Sections
### 1 Introduction (p. 1)
The paper presents an unbundled transactional key-value store where ordering, durability, and storage are split into separate pipelined roles. A single sequencer provides a total commit order, the log servers act as the source of truth, and whole-system recovery replaces individual role repair. The resulting multi-hop latency is argued to be small, predictable, and justified by simpler scaling and testing.
### 2 Architecture (pp. 1–3)
The system separates responsibilities into stateless transaction roles (sequencer, proxies, resolvers), stateful log and storage servers, and configuration coordinators. Roles do not share memory or disks, communicating via versioned messages governed by coordinator configurations. Storage servers maintain key ranges partitioned across fault domains, with client-side cache redirects and bounded version histories in memory.
### 3 Committing a Transaction (pp. 3–4)
Clients obtain a read version from a proxy, execute reads against storage servers, and locally buffer writes until commit time. Proxies batch transactions, request commit versions from the sequencer, forward writes to resolvers for optimistic conflict checks, and push accepted writes to log servers. Resolvers validate read sets against commits from the trailing five-second window, rejecting stale transactions with retryable errors.
### 4 Recovery (pp. 4–5)
When a transaction component fails, the system executes an end-to-end recovery state machine instead of in-place repair. A new sequencer retrieves configuration state, locks prior log servers to establish a quorum barrier, recruits replacement roles, and replays unapplied log tails. If any fault occurs during this sequence, recovery restarts cleanly under an incremented generation counter.
### 5 Simulation Testing (p. 5)
Every role runs against abstract interfaces for clocks, networks, and disk I/O, allowing single-threaded deterministic simulation of entire clusters. The simulator injects faults and artificially accelerates time, enabling overnight exploration of millions of execution hours with seed-based replayability. Real-hardware clusters are reserved for operating system defects and performance regression testing rather than core logic verification.
### 6 Evaluation and Lessons (pp. 5–6)
Evaluation on 24 nodes shows sub-20ms 99th-percentile commit latencies across read-heavy, balanced, and write-heavy workloads, with storage reads scaling horizontally. Core takeaways emphasize that unbundling simplified role reasoning and simulator testing at modest latency costs. Operational resilience depends on strict bounds on all inter-role queues and buffers to prevent out-of-memory collapses under write bursts.

## Figures
### Figure 1 (p. 2): The architecture: clients, the transaction system (sequencer, proxies and resolvers), the log system and the storage system.
The diagram illustrates client applications interacting with the transaction tier, which coordinates with log and storage layers. Proxies interact with the sequencer and resolvers, push durable records to log servers, and storage servers pull updates from log servers. Coordinators oversee the system configuration alongside log and storage nodes.
<diagram title="Figure 1">
<node id="clients-grp" label="Clients" kind="group"/>
<node id="client" label="client" kind="actor" in="clients-grp"/>
<node id="tx-grp" label="Transaction system" kind="group"/>
<node id="seq" label="Sequencer" kind="service" in="tx-grp"/>
<node id="proxies" label="Proxies" kind="service" in="tx-grp"/>
<node id="resolvers" label="Resolvers" kind="service" in="tx-grp"/>
<node id="storage-grp" label="Log system and storage system" kind="group"/>
<node id="log-servers" label="Log servers" kind="queue" in="storage-grp"/>
<node id="storage-servers" label="Storage servers" kind="db" in="storage-grp"/>
<node id="coordinators" label="Coordinators" kind="service" in="storage-grp"/>
<edge from="client" to="proxies" label="1. read version, commit"/>
<edge from="proxies" to="seq" label=""/>
<edge from="proxies" to="resolvers" label=""/>
<edge from="proxies" to="log-servers" label="3. push"/>
<edge from="log-servers" to="storage-servers" label="pull"/>
</diagram>
### Figure 2 (p. 3): The commit pipeline, from the read version to the reply.
The figure displays the linear execution path of a transaction across six discrete stages. It flows from version retrieval and buffered local reads through proxy submission, resolver conflict checking, log server replication, and final client confirmation.
<diagram title="Figure 2">
<node id="step1" label="1. Get read version" kind="state"/>
<node id="step2" label="2. Read and buffer writes" kind="state"/>
<node id="step3" label="3. Send commit to a proxy" kind="state"/>
<node id="step4" label="4. Resolve conflicts" kind="state"/>
<node id="step5" label="5. Push to log servers" kind="state"/>
<node id="step6" label="6. Reply to the client" kind="state"/>
<edge from="step1" to="step2" label=""/>
<edge from="step2" to="step3" label=""/>
<edge from="step3" to="step4" label=""/>
<edge from="step4" to="step5" label=""/>
<edge from="step5" to="step6" label=""/>
</diagram>
### Figure 3 (p. 4): Recovery as a state machine; a failure in any state starts it again.
The diagram depicts the five-stage recovery cycle executed upon transaction system failure. The process transitions sequentially from reading configuration to locking logs, recruiting roles, recovering transactions, and accepting commits, with any intermediate fault resetting the sequence to the initial state.
<diagram title="Figure 3">
<node id="read-cfg" label="Reading the configuration" kind="state"/>
<node id="lock-logs" label="Locking the old logs" kind="state"/>
<node id="recruit" label="Recruiting new roles" kind="state"/>
<node id="recover-tx" label="Recovering transactions" kind="state"/>
<node id="accept-commits" label="Accepting commits" kind="state"/>
<edge from="read-cfg" to="lock-logs" label=""/>
<edge from="lock-logs" to="recruit" label=""/>
<edge from="recruit" to="recover-tx" label=""/>
<edge from="recover-tx" to="accept-commits" label=""/>
<edge from="accept-commits" to="read-cfg" label="failure"/>
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
- A transaction is committed once its writes are durable on the log servers, before any storage server has applied them. (p. 1)
- The store recovers from a failure by replacing the whole transaction system in a few seconds instead of repairing it. (p. 1)
- Only the log servers and the storage servers are stateful; every role of the transaction system can be replaced without losing data. (p. 2)
- Moving a split key range copies data first and switches assignment last, preventing readers from seeing ownerless ranges. (p. 2)
- Read-only transactions avoid resolvers and logs, scaling directly with storage server count. (p. 3)
- Resolvers retain only five seconds of write history, bounding their memory consumption and transaction execution times. (p. 4)
- A single sequencer process handled the workload of a 24-machine cluster using under one-tenth of a core due to proxy request batching. (p. 4)
- Locking old log servers guarantees recovery safety by preventing old proxies from making additional commits durable. (p. 4)
- Replacing hardware interfaces with deterministic simulation enables replaying any discovered cluster failure from a random seed. (p. 5)
- Ninety-ninth percentile commit latency remained under 20 milliseconds across tested workloads on 24 machines. (p. 5)
- Implementing fixed queue and buffer sizes with backpressure prevents out-of-memory exhaustion during sudden write bursts. (p. 6)
