# Runtime process ownership

The Windows stdio entry point explicitly initializes an owner Job Object before
serving requests. The noninheritable job handle stays open for the process lifetime;
Windows closes it on forced process termination without relying on Rust destructors
or a Tokio executor. All subsequently created children inherit this outer job.

Each DreamDaemon starts suspended, inherits the outer owner job, joins a separate
runtime job, and only then resumes. Per-runtime stop and cancellation terminate the
runtime job. This ordering covers both descendant creation before assignment and
owner termination between spawning and assignment. Assignment or resume failure
terminates the child and refuses launch. Runtime state also terminates its job on
Drop, including after executor shutdown. The same launch path covers Tracy runtimes.

Direct library hosts must call `process::initialize_runtime_owner()` explicitly in
a dedicated owning process before launching runtimes. This opts subsequent child
processes into owner-loss cleanup, including children created outside this library.
Do not opt a shared general-purpose host into this lifetime policy accidentally.
Uninitialized library launches fail closed with an actionable error. Existing
unrelated processes are never found or terminated by a stored PID.

Clean stdio EOF and transport errors call `MeridianServer::shutdown()`. It cancels
and drains strongly registered request owners, stops sessions before integrity
finalization, and drains non-abortable workers with a five-second overall deadline. A
timeout is an error; the kernel ownership fallback remains active until owner exit.
Forced owner loss cannot finalize an integrity journal. Existing unfinished-journal
recovery remains a separate operation and never kills a process from persisted PID
data.

Linux standard and Tracy runtimes use a sibling guardian process that leads a
private process group. DreamDaemon remains the direct child with its actual PID,
output, and exit status. Before exec it joins the guardian's group. The guardian
reads an owner lifetime pipe and kills its entire group on EOF, including when MCP
receives SIGKILL or its executor has stopped. Guardian dispatch runs synchronously
before configuration, tracing, or Tokio initialization and requires no environment.

The owner pipe is CLOEXEC and outside descriptors 0–2. Rust 1.95's fork path retains
it in the child until exec: this keeps the guardian alive if MCP dies before the
child joins the group. The pre-exec callback contains only `setpgid`. Exec closes
the inherited writer, so independently launched compilers, collectors, and sentinels
cannot retain the lease. Guardian readiness is bounded and invalid setup or target
exec failure cleans up without executing uncontained target code.

Unix library embeddings configure an absolute Meridian executable through
`process::initialize_runtime_owner_with_executable`. The normal binary configures
itself. Analysis-only embeddings need no guardian configuration. The shared process
runner and owned collector startup also use this containment path. Cancellation
does not release a compiler's execution scope while its worker or final evidence
publication remains active. A blocking worker retains its pool permit after its
caller stops waiting, and shutdown requires its actual completion acknowledgement.

The executable separately bounds Tokio runtime shutdown. This cannot abort an
already running blocking closure or establish cleanup proof. Cleanup overrun and
owner death leave durable active execution state quarantined even when its OS
lease becomes available; a free lock or a missing PID does not prove child exit.
Automatic recovery of that quarantine is outside the supported boundary.

Natural exit retains cleanup ownership until termination is confirmed. Standard
integrity finalization and Tracy post-stop finalization wait for this boundary.
Before the first Windows termination request, runtime containment captures
identity-stable handles for the current job members. It waits for those handles to
signal, releases them, and checks a successful job accounting query with zero
active processes. Accounting alone can precede the process handle's signal during
kernel teardown. This completion barrier covers the members observed before
termination; concurrent child creation during that collection remains a limitation
of the cooperative-runtime completion check. Linux retains the unreaped guardian identity
through signaling and checks readable `/proc` group records for nonrunning members
(gone, zombie, or dead), then reaps the guardian. Cleanup errors or a two-second
timeout preserve containment and leave the journal retryable and unfinalized.

This Unix guarantee covers owned processes that inherit their process group.
Intentional `setsid`/`setpgid` escape is outside it. Independently killing both the
guardian and MCP can defeat the user-space fallback; no kernel containment claim
is made for that double failure. Embeddings that fork without exec must close
inherited owner descriptors themselves. Other Unix platforms remain unqualified;
the Linux completion oracle requires readable procfs. Native fixture results do
not establish real DreamDaemon or Tracy capture behavior.

The Windows ownership regression uses an owned native fixture, process creation
identities and kernel wait status, plus an unrelated sentinel. Portable tests and
these synthetic processes do not establish real DreamDaemon or live Tracy behavior.
