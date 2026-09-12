use super::{ReliableError, Timing};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Listening,
    Turnaround,
    Transmitting,
    Settling,
}

/// Injectable monotonic turn scheduler. Two guard intervals before replying allow
/// the peer to finish its trailing edge and TX settle; one interval after local
/// playback suppresses residual self echo. The audio adapter owns mute/unmute.
pub struct Duplex {
    timing: Timing,
    state: TurnState,
    deadline_ms: Option<u64>,
}
impl Duplex {
    pub fn new(timing: Timing) -> Result<Self, ReliableError> {
        timing.validate()?;
        Ok(Self {
            timing,
            state: TurnState::Listening,
            deadline_ms: None,
        })
    }
    pub fn state(&self) -> TurnState {
        self.state
    }
    pub fn deadline_ms(&self) -> Option<u64> {
        self.deadline_ms
    }
    pub fn begin(&mut self, now_ms: u64, after_peer: bool) -> Result<(), ReliableError> {
        if self.state != TurnState::Listening {
            return Err(ReliableError::State(
                "transmit while duplex turn is occupied",
            ));
        }
        self.state = TurnState::Turnaround;
        self.deadline_ms = Some(now_ms.saturating_add(if after_peer {
            2 * self.timing.turnaround_ms
        } else {
            0
        }));
        self.tick(now_ms);
        Ok(())
    }
    pub fn playback_finished(&mut self, now_ms: u64) -> Result<(), ReliableError> {
        if self.state != TurnState::Transmitting {
            return Err(ReliableError::State("playback finished outside TX turn"));
        }
        self.state = TurnState::Settling;
        self.deadline_ms = Some(now_ms.saturating_add(self.timing.turnaround_ms));
        self.tick(now_ms);
        Ok(())
    }
    pub fn tick(&mut self, now_ms: u64) {
        if self.deadline_ms.is_some_and(|deadline| now_ms >= deadline) {
            self.state = match self.state {
                TurnState::Turnaround => TurnState::Transmitting,
                TurnState::Settling => TurnState::Listening,
                state => state,
            };
            self.deadline_ms = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn turns_require_playback_and_settle_before_listening() {
        let mut turn = Duplex::new(Timing::default()).unwrap();
        turn.begin(100, true).unwrap();
        assert_eq!(turn.state(), TurnState::Turnaround);
        assert!(turn.playback_finished(100).is_err());
        turn.tick(699);
        assert_eq!(turn.state(), TurnState::Turnaround);
        turn.tick(700);
        assert_eq!(turn.state(), TurnState::Transmitting);
        assert!(turn.begin(700, false).is_err());
        turn.playback_finished(1000).unwrap();
        turn.tick(1299);
        assert_eq!(turn.state(), TurnState::Settling);
        turn.tick(1300);
        assert_eq!(turn.state(), TurnState::Listening);
        turn.begin(1300, false).unwrap();
        assert_eq!(turn.state(), TurnState::Transmitting);
    }
}
