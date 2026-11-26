//! Registry of running streaming children.
//!
//! The registry is what lets a signal terminate the children one operation
//! started, including descendants a launcher left behind. Registration and the
//! cancellation check share one lock, so a sweep cannot slip between them and
//! leave a child running.

use std::{
    io,
    process::{Child, ExitStatus},
    sync::{
        Mutex,
        MutexGuard,
        PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often a waiting runner checks whether its child has exited.
///
/// The wait polls rather than blocking so the same [`Child`] can be terminated
/// by the signal listener, which needs the registry lock the waiter would
/// otherwise hold for the whole command.
const CHILD_WAIT_INTERVAL: Duration = Duration::from_millis(10);

/// How long a terminated child group may take to exit before it is killed.
///
/// A launcher that ignores SIGTERM would otherwise keep the runner waiting
/// forever, and a `git` or `ssh` process blocked on I/O is exactly the case
/// cancellation exists for.
const CHILD_KILL_GRACE: Duration = Duration::from_secs(5);

/// Type of a process-group identifier.
#[cfg(unix)]
type GroupId = libc::pid_t;

/// Type of a process-group identifier on platforms without process groups.
#[cfg(not(unix))]
type GroupId = ();

/// A streaming child that cancellation may terminate.
#[derive(Debug)]
struct RegisteredChild {
    /// Identifier of this registration.
    id: u64,
    /// Operation scope this child belongs to.
    scope: Option<u64>,
    /// The running child process.
    child: Child,
    /// Process group the child leads, when it was isolated into one.
    ///
    /// `None` when the child shares this process's group, which happens for
    /// commands that inherit a terminal and must keep terminal control.
    pgid: Option<GroupId>,
    /// Whether this child has been reaped.
    ///
    /// The registration outlives the reap so that a descendant still holding
    /// the output pipes can be reached by cancellation, but a reaped leader is
    /// never signalled through its bare process id.
    reaped: bool,
    /// When termination was requested, used to escalate to `SIGKILL`.
    terminated_at: Option<Instant>,
}

/// Identifier source for registrations, unique within the process.
static NEXT_CHILD_ID: AtomicU64 = AtomicU64::new(1);

/// Streaming children and the scopes cancellation has already been requested
/// for.
///
/// Both live under one lock so registering a child and observing whether its
/// scope was cancelled are a single atomic step. With two locks, a sweep could
/// run between the check and the insertion and leave the child running.
#[derive(Debug, Default)]
struct Registry {
    /// Streaming children that are currently registered.
    children: Vec<RegisteredChild>,
    /// Operation scopes for which cancellation has already been requested.
    cancelled_scopes: Vec<Option<u64>>,
}

/// The process-wide child registry.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    children: Vec::new(),
    cancelled_scopes: Vec::new(),
});

/// Locks the streaming-child registry, recovering a poisoned lock.
///
/// The registry holds no invariants that a panic can corrupt beyond the list
/// itself, so losing a registration because another thread panicked would
/// leave an unkillable child; recovery keeps it usable.
fn lock_registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Terminates the streaming children belonging to `scope`.
///
/// A child that is already gone is ignored: the registry keeps the entry until
/// its own waiter has drained the output, so a signal that races a natural exit
/// is harmless. Children registered for another operation are left alone.
///
/// # Parameters
///
/// - `scope`: the operation to terminate, as returned by
///   [`Cancellation::scope`](crate::cancel::Cancellation::scope). `None` names the unscoped runners
///   only, not every runner. The scope is also recorded, so a child registered after this call is
///   terminated at registration.
pub fn kill_active_children(scope: Option<u64>) {
    let mut registry = lock_registry();
    if !registry.cancelled_scopes.contains(&scope) {
        registry.cancelled_scopes.push(scope);
    }
    let now = Instant::now();
    for entry in registry
        .children
        .iter_mut()
        .filter(|entry| entry.scope == scope)
    {
        if terminate(entry) {
            entry.terminated_at.get_or_insert(now);
        }
    }
}

/// Requests termination of `entry`'s process group or process.
///
/// A launcher can start descendants that inherit stdout or stderr; killing only
/// the launcher would leave those descendants holding the pipes open, and the
/// output forwarders would wait for EOF forever. Returns whether a signal was
/// sent.
fn terminate(entry: &mut RegisteredChild) -> bool {
    #[cfg(unix)]
    {
        let Some(pgid) = entry.pgid else {
            // The child shares our process group, so a group signal would hit
            // this process too; only the child itself can be terminated.
            return entry.child.kill().is_ok();
        };
        // A reaped leader's id is only useful while a descendant keeps the
        // group alive. Probing with signal 0 distinguishes a live group from an
        // id the kernel may have recycled, so an unrelated process is never
        // signalled.
        if entry.reaped && !group_exists(pgid) {
            return false;
        }
        // SAFETY: `pgid` is the id of a process group this process created for
        // a child it spawned, and `killpg` only sends a signal.
        unsafe { libc::killpg(pgid, libc::SIGTERM) == 0 }
    }
    #[cfg(not(unix))]
    {
        entry.child.kill().is_ok()
    }
}

/// Terminates the registered child `id`, if it is still registered.
pub(super) fn terminate_registered_child(id: u64) {
    let mut registry = lock_registry();
    let now = Instant::now();
    if let Some(entry) = registry.children.iter_mut().find(|entry| entry.id == id)
        && terminate(entry)
    {
        entry.terminated_at.get_or_insert(now);
    }
}

/// Reports whether the process group `pgid` still has a member.
#[cfg(unix)]
fn group_exists(pgid: GroupId) -> bool {
    // SAFETY: `killpg` with signal 0 only tests for the group's existence.
    let result = unsafe { libc::killpg(pgid, 0) };
    if result == 0 {
        return true;
    }
    // `EPERM` means members exist that this process may not signal, which still
    // proves the group was not recycled by an unrelated process.
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Escalates a group that ignored `SIGTERM` to `SIGKILL`.
fn escalate_if_needed(entry: &mut RegisteredChild) {
    let Some(terminated_at) = entry.terminated_at else {
        return;
    };
    if terminated_at.elapsed() < CHILD_KILL_GRACE {
        return;
    }
    #[cfg(unix)]
    {
        let Some(pgid) = entry.pgid else {
            drop(entry.child.kill());
            return;
        };
        if entry.reaped && !group_exists(pgid) {
            return;
        }
        // SAFETY: as in `terminate`; the group was ours and is still alive.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        drop(entry.child.kill());
    }
}

/// Returns the process group a freshly spawned `child` leads, if any.
#[cfg(unix)]
fn spawned_group(child: &Child) -> Option<GroupId> { libc::pid_t::try_from(child.id()).ok() }

/// Reports whether a child should be given its own process group.
///
/// A command that inherits a terminal must stay in the terminal's foreground
/// group: a child moved out of it stops with `SIGTTIN` on its first read from
/// standard input. Every other command gets its own group so cancellation can
/// terminate the descendants it started as well.
#[cfg(unix)]
pub(super) fn should_isolate_group() -> bool {
    use std::io::IsTerminal as _;

    !std::io::stdin().is_terminal()
}

/// Registers `child` so a signal can terminate it.
///
/// Returns the registration id and whether cancellation had already been
/// requested for `scope`. The caller kills and reaps the child in that case
/// instead of entering the normal wait.
pub(super) fn register_child(scope: Option<u64>, child: Child, isolated: bool) -> (u64, bool) {
    let id = NEXT_CHILD_ID.fetch_add(1, Ordering::Relaxed);
    let mut registry = lock_registry();
    let cancelled = registry.cancelled_scopes.contains(&scope);
    #[cfg(unix)]
    let pgid = if isolated {
        spawned_group(&child)
    } else {
        None
    };
    #[cfg(not(unix))]
    let pgid = {
        let _ = isolated;
        None
    };
    registry.children.push(RegisteredChild {
        id,
        scope,
        child,
        pgid,
        reaped: false,
        terminated_at: None,
    });
    (id, cancelled)
}

/// Escalates a stalled termination while the output is being drained.
///
/// Once the launcher is reaped, only the pipes keep the runner waiting; a
/// descendant that ignores `SIGTERM` would otherwise hold them forever because
/// nothing calls [`escalate_if_needed`] again.
pub(super) fn escalate_registered_child(id: u64) {
    let mut registry = lock_registry();
    if let Some(entry) = registry.children.iter_mut().find(|entry| entry.id == id) {
        escalate_if_needed(entry);
    }
}

/// Removes the registration `id`, if it is still present.
fn deregister_child(id: u64) { lock_registry().children.retain(|entry| entry.id != id); }

/// Waits until the registered child exits, allowing cancellation to kill it
/// meanwhile.
pub(super) fn wait_for_registered_child(id: u64) -> io::Result<ExitStatus> {
    loop {
        if let Some(status) = poll_registered_child(id)? {
            return Ok(status);
        }
        thread::sleep(CHILD_WAIT_INTERVAL);
    }
}

/// Polls the registered child once, escalating a termination that has stalled.
///
/// The entry is kept after the child is reaped, so cancellation can still reach
/// a descendant that holds the output pipes; [`deregister_child`] removes it
/// once those pipes are drained. A wait failure removes the entry instead: the
/// child's state is then unknown, and a later signal must not act on it.
pub(super) fn poll_registered_child(id: u64) -> io::Result<Option<ExitStatus>> {
    let mut registry = lock_registry();
    let Some(registered) = registry.children.iter_mut().find(|entry| entry.id == id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "streaming child disappeared from the registry",
        ));
    };
    let observed = registered.child.try_wait();
    match observed {
        Ok(Some(status)) => {
            registered.reaped = true;
            Ok(Some(status))
        }
        Ok(None) => {
            escalate_if_needed(registered);
            Ok(None)
        }
        Err(error) => {
            registry.children.retain(|entry| entry.id != id);
            Err(error)
        }
    }
}

/// Removes a registration when its runner leaves, however it leaves.
pub(super) struct Registration {
    /// Registration to remove.
    pub(super) id: u64,
}

impl Drop for Registration {
    fn drop(&mut self) { deregister_child(self.id); }
}
