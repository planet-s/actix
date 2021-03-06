//! An actor implementation of Unix signal handling
//!
//! This module implements asynchronous signal handling for Actix. For each signal
//! `ProcessSignals` actor sends `Signal` message to all subscriber. To subscriber,
//! send `Subscribe` message to `ProcessSignals` actor.
//!
//! # Examples
//!
//! ```rust
//! extern crate actix;
//!
//! use actix::prelude::*;
//! use actix::actors::signal;
//!
//! struct Signals;
//!
//! impl Actor for Signals {
//!     type Context = Context<Self>;
//! }
//!
//! impl ResponseType<signal::Signal> for Signals {
//!     type Item = ();
//!     type Error = ();
//! }
//!
//! // Shutdown system on and of `SIGINT`, `SIGTERM`, `SIGQUIT` signals
//! impl Handler<signal::Signal> for Signals {
//!
//!     fn handle(&mut self, msg: signal::Signal, _: &mut Context<Self>)
//!               -> Response<Self, signal::Signal>
//!     {
//!         match msg.0 {
//!             signal::SignalType::Int => {
//!                 println!("SIGINT received, exiting");
//!                 Arbiter::system().send(msgs::SystemExit(0));
//!             },
//!             signal::SignalType::Hup => {
//!                 println!("SIGHUP received, reloading");
//!             },
//!             signal::SignalType::Term => {
//!                 println!("SIGTERM received, stopping");
//!                 Arbiter::system().send(msgs::SystemExit(0));
//!             },
//!             signal::SignalType::Quit => {
//!                 println!("SIGQUIT received, exiting");
//!                 Arbiter::system().send(msgs::SystemExit(0));
//!             }
//!             _ => (),
//!         };
//!         Self::empty()
//!     }
//! }
//!
//! fn main() {
//!    // initialize system
//!    let sys = System::new("test");
//!
//!    // Start signals handler
//!    let addr: SyncAddress<_> = Signals.start();
//!
//!    // send SIGTERM
//!    std::thread::spawn(move || {
//!       // emulate SIGNTERM
//!       addr.send(signal::Signal(signal::SignalType::Term));
//!    });
//!
//!    // Run system, this function blocks until system runs
//!    let code = sys.run();
//!    std::process::exit(code);
//! }
//! ```
use std;
use std::io;
use libc;
use futures::{Future, Stream};
use tokio_signal;
use tokio_signal::unix;

use prelude::*;

/// Different types of process signals
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum SignalType {
    /// SIGHUP
    Hup,
    /// SIGINT
    Int,
    /// SIGTERM
    Term,
    /// SIGQUIT
    Quit,
    /// SIGCHILD
    Child,
}

/// Process signal message
pub struct Signal(pub SignalType);

/// An actor implementation of Unix signal handling
pub struct ProcessSignals {
    subscribers: Vec<Box<Subscriber<Signal>>>,
}

impl Default for ProcessSignals {
    fn default() -> Self {
        ProcessSignals{subscribers: Vec::new()}
    }
}

impl Actor for ProcessSignals {
    type Context = Context<Self>;
}

impl Supervised for ProcessSignals {}

impl SystemService for ProcessSignals {

    fn service_started(&mut self, ctx: &mut Context<Self>) {
        let handle = Arbiter::handle();

        // SIGINT
        tokio_signal::ctrl_c(handle).map_err(|_| ())
            .actfuture()
            .map(|sig, _: &mut ProcessSignals, ctx: &mut Context<Self>|
                 ctx.add_stream(sig.map(|_| SignalType::Int)))
            .spawn(ctx);

        // SIGHUP
        unix::Signal::new(libc::SIGHUP, handle).map_err(|_| ())
            .actfuture()
            .map(|sig, _: &mut ProcessSignals, ctx: &mut Context<Self>|
                 ctx.add_stream(sig.map(|_| SignalType::Hup)))
            .spawn(ctx);

        // SIGTERM
        unix::Signal::new(libc::SIGTERM, handle).map_err(|_| ())
            .actfuture()
            .map(|sig, _: &mut Self, ctx: &mut Context<Self>|
                 ctx.add_stream(sig.map(|_| SignalType::Term)))
            .spawn(ctx);

        // SIGQUIT
        unix::Signal::new(libc::SIGQUIT, handle).map_err(|_| ())
            .actfuture()
            .map(|sig, _: &mut ProcessSignals, ctx: &mut Context<Self>|
                 ctx.add_stream(sig.map(|_| SignalType::Quit)))
            .spawn(ctx);

        // SIGCHLD
        unix::Signal::new(libc::SIGCHLD, handle).map_err(|_| ())
            .actfuture()
            .map(|sig, _: &mut ProcessSignals, ctx: &mut Context<Self>|
                 ctx.add_stream(sig.map(|_| SignalType::Child)))
            .spawn(ctx);
    }
}

#[doc(hidden)]
impl StreamHandler<SignalType, io::Error> for ProcessSignals {}

impl ResponseType<SignalType> for ProcessSignals {
    type Item = ();
    type Error = ();
}

#[doc(hidden)]
impl Handler<SignalType, io::Error> for ProcessSignals {

    fn handle(&mut self, msg: SignalType, _: &mut Context<Self>) -> Response<Self, SignalType>
    {
        let subscribers = std::mem::replace(&mut self.subscribers, Vec::new());
        for subscr in subscribers {
            if subscr.send(Signal(msg)).is_ok() {
                self.subscribers.push(subscr);
            }
        }
        Self::empty()
    }

    fn error(&mut self, err: io::Error, _: &mut Context<ProcessSignals>) {
        error!("Error during signal handling: {}", err);
    }
}

/// Subscribe to process signals.
pub struct Subscribe(pub Box<Subscriber<Signal> + Send>);

impl ResponseType<Subscribe> for ProcessSignals {
    type Item = ();
    type Error = ();
}

/// Add subscriber for signals
impl Handler<Subscribe> for ProcessSignals {

    fn handle(&mut self, msg: Subscribe,
              _: &mut Context<ProcessSignals>) -> Response<Self, Subscribe>
    {
        self.subscribers.push(msg.0);
        Self::empty()
    }
}

/// Default signals handler. This actor sends `SystemExit` message to `System` actor
/// for each of `SIGINT`, `SIGTERM`, `SIGQUIT` signals.
pub struct DefaultSignalsHandler;

impl Default for DefaultSignalsHandler {
    fn default() -> Self {
        DefaultSignalsHandler
    }
}

impl Actor for DefaultSignalsHandler {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        let addr = Arbiter::system_registry().get::<ProcessSignals>();
        let slf: SyncAddress<_> = ctx.address();
        addr.send(Subscribe(slf.subscriber()))
    }
}

impl ResponseType<Signal> for DefaultSignalsHandler {
    type Item = ();
    type Error = ();
}

/// Handle `SIGINT`, `SIGTERM`, `SIGQUIT` signals and send `SystemExit(0)`
/// message to `System` actor.
impl Handler<Signal> for DefaultSignalsHandler {

    fn handle(&mut self, msg: Signal, _: &mut Context<Self>) -> Response<Self, Signal>
    {
        match msg.0 {
            SignalType::Int => {
                info!("SIGINT received, exiting");
                Arbiter::system().send(msgs::SystemExit(0));
            }
            SignalType::Hup => {
                info!("SIGHUP received, reloading");
            }
            SignalType::Term => {
                info!("SIGTERM received, stopping");
                Arbiter::system().send(msgs::SystemExit(0));
            }
            SignalType::Quit => {
                info!("SIGQUIT received, exiting");
                Arbiter::system().send(msgs::SystemExit(0));
            }
            _ => (),
        };
        Self::empty()
    }
}
