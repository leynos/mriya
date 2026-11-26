//! Signal-driven cancellation shared by the run and init orchestrators.
//!
//! A [`Cancellation`] is a cloneable view of one watch channel. Orchestrators
//! race their awaited steps against [`Cancellation::cancelled`] and check
//! [`Cancellation::is_cancelled`] between steps, so a SIGINT or SIGTERM still
//! reaches ownership-aware cleanup. A second signal during cleanup is exposed
//! separately, because it asks mriya to stop cleaning and leave the rest to
//! `mriya recover`.

use std::future::{Future, pending};

use tokio::sync::watch;

use crate::sync::kill_active_children;

/// Signal that requested cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    /// SIGINT, conventionally from Ctrl+C.
    Interrupt,
    /// SIGTERM, conventionally from a service manager.
    Terminate,
}

impl Signal {
    /// Returns the conventional shell exit status for this signal.
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }

    /// Returns the signal name used in messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }
}

/// Value published through the cancellation channel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CancelState {
    /// Most recent signal, or `None` before the first one.
    pub signal: Option<Signal>,
    /// Number of signals received so far.
    pub count: u32,
}

impl CancelState {
    /// Reports whether a second signal has arrived, which means cleanup should
    /// stop and defer to `mriya recover`.
    #[must_use]
    pub const fn is_repeated(self) -> bool { self.count >= 2 }
}

/// Cloneable read side of the cancellation channel.
#[derive(Clone, Debug, Default)]
pub struct Cancellation {
    /// Channel receiver; `None` produces a cancellation that never fires.
    receiver: Option<watch::Receiver<CancelState>>,
}

impl Cancellation {
    /// Returns a cancellation that never fires, used by callers with no signal
    /// handling, such as library tests.
    #[must_use]
    pub fn never() -> Self { Self::default() }

    /// Returns the most recent state without waiting.
    #[must_use]
    pub fn state(&self) -> CancelState {
        self.receiver
            .as_ref()
            .map_or_else(CancelState::default, |receiver| *receiver.borrow())
    }

    /// Reports whether any signal has arrived.
    #[must_use]
    pub fn is_cancelled(&self) -> bool { self.state().signal.is_some() }

    /// Returns the signal that requested cancellation, if any.
    #[must_use]
    pub fn signal(&self) -> Option<Signal> { self.state().signal }

    /// Resolves with the first signal, or never resolves when no channel is
    /// attached.
    pub async fn cancelled(&self) -> Signal {
        let Some(receiver) = self.receiver.as_ref() else {
            return pending().await;
        };
        wait_for_signal(receiver.clone()).await
    }

    /// Resolves when a second signal arrives, or never resolves when no
    /// channel is attached.
    pub async fn repeated(&self) {
        let Some(receiver) = self.receiver.as_ref() else {
            pending::<()>().await;
            return;
        };
        let mut waiting = receiver.clone();
        if waiting.wait_for(|state| state.is_repeated()).await.is_err() {
            pending::<()>().await;
        }
    }
}

/// Waits for the first signal published on `receiver`.
///
/// A dropped sender means no further signal can arrive, so the wait becomes a
/// future that never resolves rather than an error the caller must handle.
async fn wait_for_signal(mut receiver: watch::Receiver<CancelState>) -> Signal {
    loop {
        let Ok(state) = receiver.wait_for(|state| state.signal.is_some()).await else {
            return pending().await;
        };
        if let Some(signal) = state.signal {
            return signal;
        }
    }
}

/// Write side of the cancellation channel.
#[derive(Clone, Debug)]
pub struct CancelHandle {
    /// Channel sender.
    sender: watch::Sender<CancelState>,
}

impl CancelHandle {
    /// Creates a channel and its initial read side.
    #[must_use]
    pub fn channel() -> (Self, Cancellation) {
        let (sender, receiver) = watch::channel(CancelState::default());
        (
            Self { sender },
            Cancellation {
                receiver: Some(receiver),
            },
        )
    }

    /// Returns another read side of this channel.
    #[must_use]
    pub fn cancellation(&self) -> Cancellation {
        Cancellation {
            receiver: Some(self.sender.subscribe()),
        }
    }

    /// Records a signal, notifying every reader.
    ///
    /// The active streaming child is killed here rather than by each
    /// orchestrator, so a signal terminates the remote command even while the
    /// blocking SSH call owns the worker thread.
    pub fn cancel(&self, signal: Signal) {
        self.sender.send_modify(|state| {
            state.signal = Some(signal);
            state.count = state.count.saturating_add(1);
        });
        kill_active_children();
    }

    /// Installs SIGINT and SIGTERM handlers that cancel this channel.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when either handler cannot be
    /// installed, and [`std::io::ErrorKind::Unsupported`] on platforms without
    /// Unix signals.
    #[cfg(unix)]
    pub fn install_signal_handlers(&self) -> Result<(), std::io::Error> {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let handle = self.clone();
        tokio::spawn(async move {
            loop {
                let signal = tokio::select! {
                    received = interrupt.recv() => {
                        if received.is_none() {
                            return;
                        }
                        Signal::Interrupt
                    }
                    received = terminate.recv() => {
                        if received.is_none() {
                            return;
                        }
                        Signal::Terminate
                    }
                };
                handle.cancel(signal);
            }
        });
        Ok(())
    }

    /// Installs signal handlers where the platform provides none.
    ///
    /// # Errors
    ///
    /// Always returns [`std::io::ErrorKind::Unsupported`].
    #[cfg(not(unix))]
    pub fn install_signal_handlers(&self) -> Result<(), std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "signal handling requires a Unix platform",
        ))
    }
}

/// Resolves `future`, returning `None` when cancellation arrives first.
///
/// The future is dropped when cancellation wins, which is what lets a signal
/// interrupt an awaited provider call between its request and its response.
pub async fn until_cancelled<F, T>(future: F, cancellation: &Cancellation) -> Option<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        output = future => Some(output),
        _signal = cancellation.cancelled() => None,
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for cancellation state transitions.

    use super::*;

    #[tokio::test]
    async fn a_fresh_channel_is_not_cancelled() {
        let (handle, cancellation) = CancelHandle::channel();
        assert!(!cancellation.is_cancelled());
        drop(handle);
        assert!(!cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_publishes_the_signal_and_counts_repeats() {
        let (handle, cancellation) = CancelHandle::channel();
        handle.cancel(Signal::Interrupt);
        assert!(cancellation.is_cancelled());
        assert_eq!(cancellation.signal(), Some(Signal::Interrupt));
        assert_eq!(cancellation.cancelled().await, Signal::Interrupt);

        handle.cancel(Signal::Terminate);
        assert_eq!(cancellation.signal(), Some(Signal::Terminate));
        assert!(cancellation.state().is_repeated());
        cancellation.repeated().await;
    }

    #[tokio::test]
    async fn an_unattached_cancellation_never_resolves() {
        let cancellation = Cancellation::never();
        assert!(!cancellation.is_cancelled());
        assert_eq!(cancellation.signal(), None);
    }

    #[tokio::test]
    async fn until_cancelled_returns_the_output_when_nothing_interrupts() {
        let (_handle, cancellation) = CancelHandle::channel();
        let output = until_cancelled(async { 7 }, &cancellation).await;
        assert_eq!(output, Some(7));
    }

    #[tokio::test]
    async fn until_cancelled_drops_the_future_when_a_signal_arrives() {
        let (handle, cancellation) = CancelHandle::channel();
        let awaited = until_cancelled(std::future::pending::<()>(), &cancellation);
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            handle.cancel(Signal::Interrupt);
        });
        assert_eq!(awaited.await, None);
    }
}
