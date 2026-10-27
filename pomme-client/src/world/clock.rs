//! Vanilla `ClientClockManager`: the world clocks, advanced locally by their
//! rate each tick and reset by the server's `SetTime`.

use std::collections::HashMap;

use pomme_protocol::lifecycle::ClockState;

#[derive(Default)]
pub struct ClientClocks {
    clocks: HashMap<String, ClockState>,
    last_tick_game_time: u64,
}

impl ClientClocks {
    /// Vanilla `tick`: each clock moves by the game ticks since the last
    /// call times its rate, carrying the fraction in its partial tick.
    pub fn tick(&mut self, game_time: u64) {
        let delta = game_time.wrapping_sub(self.last_tick_game_time) as i64;
        self.last_tick_game_time = game_time;
        for clock in self.clocks.values_mut() {
            let ticks = f64::from(clock.partial_tick) + delta as f64 * f64::from(clock.rate);
            let full = ticks.floor();
            clock.partial_tick = (ticks - full) as f32;
            clock.total_ticks += full as i64;
        }
    }

    /// Vanilla `handleUpdates`.
    pub fn handle_updates(&mut self, game_time: u64, updates: Vec<(String, ClockState)>) {
        self.tick(game_time);
        self.clocks.extend(updates);
    }

    /// Vanilla `Level.getDefaultClockTime`: a clock the server never sent
    /// reads 0, as does a dimension without one.
    pub fn total_ticks(&self, clock: Option<&str>) -> u64 {
        clock
            .and_then(|name| self.clocks.get(name))
            .map_or(0, |clock| clock.total_ticks as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clocks(rate: f32) -> ClientClocks {
        let mut clocks = ClientClocks::default();
        let state = ClockState {
            total_ticks: 6000,
            partial_tick: 0.0,
            rate,
        };
        clocks.handle_updates(100, vec![("minecraft:overworld".to_owned(), state)]);
        clocks
    }

    fn after(mut clocks: ClientClocks, ticks: u64) -> u64 {
        for game_time in 101..=100 + ticks {
            clocks.tick(game_time);
        }
        clocks.total_ticks(Some("minecraft:overworld"))
    }

    #[test]
    fn clocks_advance_by_their_rate() {
        assert_eq!(after(clocks(1.0), 10), 6010);
        assert_eq!(after(clocks(0.0), 10), 6000);
        assert_eq!(after(clocks(2.0), 10), 6020);
    }

    #[test]
    fn fractional_rates_carry_their_partial_tick() {
        assert_eq!(after(clocks(0.25), 3), 6000);
        assert_eq!(after(clocks(0.25), 4), 6001);
    }

    #[test]
    fn an_update_ticks_the_clocks_up_to_its_game_time_first() {
        let mut clocks = clocks(1.0);
        clocks.handle_updates(110, Vec::new());
        assert_eq!(clocks.total_ticks(Some("minecraft:overworld")), 6010);
    }

    #[test]
    fn unknown_clocks_read_zero() {
        let clocks = clocks(1.0);
        assert_eq!(clocks.total_ticks(Some("minecraft:the_end")), 0);
        assert_eq!(clocks.total_ticks(None), 0);
    }
}
