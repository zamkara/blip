# Runtime and queue

## Request path

~~~text
GitLab
  │ POST /webhook/<project-key>
  ▼
Project map lookup
  ▼
GitLab template authentication
  ▼
Bounded queue (128 waiting entries)
  ▼
One worker
  ▼
One advisory queue lock
  ▼
Executable script file
  ▼
JSONL history record
~~~

Blip does not parse the event or branch. GitLab decides whether to send the request through the webhook's trigger controls.

## Queue ownership

The service owns one FIFO in-memory queue. A successful authentication attempts immediate admission:

- Space available: return **202 Accepted**.
- Queue full or worker unavailable: return **503 Service Unavailable**.

One worker removes entries and waits for each executable to finish before starting the next. Projects cannot run in parallel.

The queue is intentionally global rather than one queue per project. This protects a small host from several deployments competing for CPU, memory, disk I/O, package-manager databases, or build caches.

## Global lock

The worker opens one file named **blip.queue.lock** beside the history file and obtains an operating-system advisory exclusive lock before each execution. Every project uses this same lock.

The file is not a boolean state and is not deleted after each run. The kernel owns the actual lock:

- another Blip process using the same runtime directory waits;
- the next queued job cannot overtake the current job;
- process exit automatically releases ownership;
- a persistent empty lock file is normal and is not a stale deployment.

This is closer to package-manager locking than creating one marker file for every project.

## Script execution

The worker starts exactly the file in **script** and passes no webhook body or command-line arguments. Standard output and standard error inherit the Blip service streams and are available through **blip logs**.

The worker waits for the process. This preserves serial execution while the HTTP request itself remains asynchronous: GitLab receives **202** after queueing, not after completion.

## History

After the executable exits or fails to start, Blip appends one JSON object:

~~~json
{"timestamp":"2026-09-15T18:00:12Z","project":"example-app","status":"success","duration_ms":37238,"exit_code":0}
~~~

A failure may include an **error** string. Command output is not duplicated into history; inspect journald for output. This keeps the default execution model simple and avoids storing arbitrary build logs twice.

## Process boundaries

- The queue is not durable. A service restart loses waiting entries.
- Delivery IDs are not yet deduplicated.
- Blip does not retry a failed script.
- Blip does not terminate a long-running script with an internal timeout.
- Configuration changes require restart.

These are explicit roadmap items rather than hidden configuration switches.
