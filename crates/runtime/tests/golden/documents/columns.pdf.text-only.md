<!-- ready: 2 pages · text and 2 tables read -->
# Digest: A Replicated Log, Set in Two Columns
Read: 2 pages; text only

## Sections
### 1 Introduction (p. 1)
A replicated log keeps the same sequence of records on several machines, so that losing one machine loses no record. Each record is appended by a leader and copied to the followers before it counts as written.
### 2 Roles (p. 1)
Table 1 lists the roles in a group of five replicas. Only the leader accepts appends; the followers copy and acknowledge them, and a learner copies without a vote.
### 3 Recovery (pp. 1–2)
When the leader stops answering, each follower waits out a random timeout and then asks the others for their votes. The first to win a majority becomes the new leader and goes on from the last record a majority holds.
### 4 Evaluation (p. 2)
Table 2 shows that each pair of replicas added to a group costs about half a millisecond at the median. The extra replicas buy tolerance of more failures, not speed.
### 5 Lessons (p. 2)
Keep the voting group small and add learners for reads. Measure the slowest follower that a majority must wait for, not the average one, since that follower sets the latency of every write.

## Tables
### Table 1 (p. 1): Replica roles in a group of five.
| Role Count Votes |
| Leader 1 yes |
| Follower 3 yes |
| Learner 1 no |
### Table 2 (p. 2): Write latency and throughput by group size.
| Replicas p50 write (ms) p99 write (ms) Writes/s |
| 3 1.8 4.9 61,000 |
| 5 2.3 6.1 54,000 |
| 7 2.9 7.7 47,000 |
