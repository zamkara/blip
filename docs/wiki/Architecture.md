# Runtime and queue

## Request path

~~~text
GitLab, GitHub, Gitea, or Codeberg
  │ POST /webhook/<project-key>
  ▼
Project map lookup
  ▼
provider-template authentication
  ▼
Durable FIFO admission (128 waiting entries)
  ▼
One worker
  ▼
One advisory queue lock
  ▼
Executable script file
  ▼
JSONL history record
~~~

Blip does not parse event or branch policy. The Git host decides which events to send; GitLab also provides a branch filter. Providers without an equivalent branch filter can trigger the fixed deployment executable for every selected event.

## Queue ownership

The service owns one global FIFO queue backed by **blip-deliveries.jsonl**. Its location is derived from the resolved history directory, so the service and management CLI inspect the same runtime state. A successful authentication attempts one atomic admission:

- New delivery with space available: append its ID, script path, sequence, and **queued** state, then return **202 queued**.
- Previously accepted project and delivery ID: return **202 duplicate** without queueing it again.
- Waiting capacity exhausted: return **503 Service Unavailable**.
- Journal unavailable or malformed: return **503 Service Unavailable** without accepting the delivery.

The queue permits 128 entries in **queued** state. A currently **running** entry does not consume waiting capacity. One worker selects the lowest persisted sequence and waits for its executable to finish before starting the next. Projects cannot run in parallel.

The queue is intentionally global rather than one queue per project. This protects a small host from several deployments competing for CPU, memory, disk I/O, package-manager databases, or build caches.

## Delivery deduplication

GitLab's **webhook-id** is stable across retries. Blip uses it as the delivery ID and accepts the legacy **Idempotency-Key** header when **webhook-id** is absent. If both headers are present, they must match.

Before acknowledging a new request, Blip appends its project key, delivery ID, script path, FIFO sequence, timestamps, and **queued** state to **blip-deliveries.jsonl** beside the history file. State transitions append another record for the same project and delivery ID; the latest record is authoritative. The journal file is locked only while it is inspected or appended. **blip.queue.lock** remains the only execution lock.

The journal serves both queue persistence and admission deduplication. Delivery IDs are scoped by project, so an identical ID used for another project remains independent. Existing registry records from releases before durable queue state are read as completed deliveries and remain deduplicated. A malformed or unavailable journal fails closed instead of risking a duplicate run.

## Restart recovery

Waiting entries remain **queued** and are selected when the service starts again. Before accepting traffic, Blip obtains **blip.queue.lock** and changes any leftover **running** entries back to **queued**. Taking the execution lock first distinguishes a crashed process from another live Blip process that is still executing a script.

Recovery is at least once for interrupted execution. A process or host can fail after the script changes external state but before Blip appends **completed**. The recovered entry then runs again. Blip cannot make an arbitrary deployment script transactional, so the script must be idempotent.

## Graceful shutdown

SIGTERM and SIGINT first close webhook admission. Requests that arrive after that point receive **503 shutting down**, while an HTTP request already being handled may finish admission. The HTTP listener then stops accepting connections.

The worker behaves according to its current state:

- an active script is allowed to finish, after which history and **completed** state are persisted;
- a worker waiting for the global execution lock stops without changing the candidate from **queued**;
- all other waiting entries remain **queued** and resume at the next startup;
- no new queued entry starts after shutdown has been observed.

The systemd unit uses **KillMode=mixed**, so the initial stop signal reaches
Blip without terminating its active deployment child. It has no stop timeout
because Blip has no execution timeout yet. An operator must therefore handle a
deployment script that never exits.

## Global lock

The worker opens one file named **blip.queue.lock** beside the history file and obtains an operating-system advisory exclusive lock before changing an entry from **queued** to **running**. It holds that lock until history and **completed** state are written. Every project uses this same lock.

The file is not a boolean state and is not deleted after each run. The kernel owns the actual lock:

- another Blip process using the same runtime directory waits;
- the next queued job cannot overtake the current job;
- process exit automatically releases ownership;
- a persistent empty lock file is normal and is not a stale deployment.

This is closer to package-manager locking than creating one marker file for every project.

## Script execution

The worker starts exactly the file in **script** and passes no webhook body or command-line arguments. Standard output and standard error inherit the Blip service streams and are available through **blip logs**.

The worker waits for the process. This preserves serial execution while the HTTP request itself remains asynchronous: the Git host receives **202** after queueing, not after completion.

## History

After the executable exits or fails to start, Blip appends one JSON object:

~~~json
{"timestamp":"2026-09-15T18:00:12Z","project":"example-app","delivery_id":"f5e5f430-f57b-4e6e-9fac-d9128cd7232f","status":"success","duration_ms":37238,"exit_code":0}
~~~

A failure may include an **error** string. Command output is not duplicated into history; inspect journald for output. This keeps the default execution model simple and avoids storing arbitrary build logs twice.

## Process boundaries

- Blip does not retry a failed script.
- An interrupted running script may execute again after restart.
- Blip does not terminate a long-running script with an internal timeout.
- Graceful service stop can wait indefinitely for a script that never exits.
- The append-only queue journal has no retention or compaction yet.
- Configuration changes require restart.

These are explicit roadmap items rather than hidden configuration switches.
