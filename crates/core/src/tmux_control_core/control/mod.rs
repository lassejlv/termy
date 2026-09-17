#![cfg_attr(not(unix), allow(unused_imports))]

pub mod channel;
pub mod coalescer;
pub mod parser;
pub mod worker;

pub use channel::{ControlCommandResult, ControlRequest, try_enqueue_control_request};
pub use channel::{
    FATAL_EXIT_QUEUE_BOUND, NOTIFICATION_QUEUE_BOUND, PENDING_QUEUE_BOUND, REQUEST_QUEUE_BOUND,
};
pub use coalescer::NotificationCoalescer;
pub use worker::spawn_control_threads;
