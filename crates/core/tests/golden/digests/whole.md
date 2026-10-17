# Digest: Unbundled Transactions: A Study of a Layered Key-Value Store
Read: 7 pages; figures by Gemini 3.8 Flash

## Sections
### Abstract (p. 1)
The store splits transactions, logging and storage into separately scaled roles.
### 1 Introduction (pp. 1–2)
Why an unbundled design scales each role on its own.
### 2 Architecture (pp. 2–3)
Clients talk to proxies; a sequencer hands out versions; resolvers check conflicts; log servers make commits durable; storage servers serve reads.
### 3 Commit pipeline (pp. 3–4)
A commit gets a version, is checked for conflicts, logged, then acknowledged.
### 4 Recovery (pp. 5–6)
On a failure the transaction system is replaced as a whole, then the log is recovered.

## Figures
### Figure 1 (p. 2): The architecture of the layered store.
Clients send reads to storage servers and commits to proxies. Proxies ask the sequencer for versions and resolvers for conflicts, then push to the log servers.
<diagram title="Figure 1">
<node id="clients" label="Clients" kind="actor"/>
<node id="txn" label="Transaction system" kind="group"/>
<node id="proxies" label="Proxies" kind="service" in="txn"/>
<node id="sequencer" label="Sequencer" kind="service" in="txn"/>
<node id="resolvers" label="Resolvers" kind="service" in="txn"/>
<node id="logs" label="Log servers" kind="db"/>
<node id="storage" label="Storage servers" kind="db"/>
<edge from="clients" to="proxies" label="commit"/>
<edge from="proxies" to="sequencer" label="get version"/>
<edge from="proxies" to="resolvers" label="check conflicts"/>
<edge from="proxies" to="logs" label="push"/>
<edge from="logs" to="storage" label="pull"/>
</diagram>
### Figure 2 (p. 4): The commit pipeline, step by step.
Five steps from a client's commit to its acknowledgement.
<diagram title="Figure 2">
<node id="get-version" label="1. Get commit version" kind="state"/>
<node id="resolve" label="2. Resolve conflicts" kind="state"/>
<node id="log" label="3. Log" kind="state"/>
<node id="ack" label="4. Acknowledge" kind="state"/>
<edge from="get-version" to="resolve"/>
<edge from="resolve" to="log"/>
<edge from="log" to="ack"/>
</diagram>
### Figure 3 (p. 6): Recovery as a state machine.
The cluster controller moves through reading the old configuration, locking the logs, recruiting a new transaction system and accepting commits again.

## Tables
### Table 1 (p. 3): Roles and what each one scales with.
| Role | Scales with |
|---|---|
| Proxies | commit rate |
| Resolvers | conflict ranges |
| Storage servers | data and reads |
### Table 2 (p. 7): Latency by operation.
| Operation | p50 (ms) | p99 (ms) |
|---|---|---|
| read | 1 | 4 |
| commit | 3 | 12 |

## Claims
- Read versions come from the sequencer through the proxies. (p. 2)
- A commit is durable once every log server it was pushed to has it. (p. 4)
- Recovery replaces the whole transaction system rather than repairing it. (p. 5)
- Deterministic simulation found most bugs before release. (p. 7)
