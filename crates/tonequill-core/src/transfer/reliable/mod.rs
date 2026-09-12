//! Half-duplex selective-repeat file transfer. Control packets use the same PHY
//! and CRC framing as data. Scheduling, loss recovery and commit acknowledgments
//! are independent of audio hardware, clocks, threads and the filesystem.
mod control;
mod duplex;
pub mod evaluation;
mod receiver;
mod sender;
pub mod simulation;
mod timing;

pub use control::{Request, RequestKind, Status, StatusKind};
pub use duplex::{Duplex, TurnState};
pub use receiver::{ReceiveSession, ReceiverState};
pub use sender::{SendSession, SendStatistics, SenderState};
pub use timing::Timing;

use super::TransferError;
use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ReliableError {
    #[error("invalid reliable-transfer control: {0}")]
    InvalidControl(&'static str),
    #[error("receiver refused transfer: {0}")]
    Rejected(&'static str),
    #[error("retry limit reached without confirmed delivery")]
    RetryLimit,
    #[error("transfer cancelled")]
    Cancelled,
    #[error("invalid sender state: {0}")]
    State(&'static str),
    #[error(transparent)]
    Transfer(#[from] TransferError),
}

fn bitmap(count: u8) -> u32 {
    if count == 32 {
        u32::MAX
    } else {
        (1_u32 << count) - 1
    }
}
