use std::time::{Duration, Instant};

use plugin_api::{Plugin, plugin};

#[plugin]
struct ExamplePlugin {
    total_ticks: u64,

    second_start: Instant,
    ticks_this_second: u32,

    tick_start: Instant,
    total_tick_time: Duration,
}

impl Plugin for ExamplePlugin {
    fn new() -> Self {
        let now = Instant::now();

        Self {
            total_ticks: 0,

            second_start: now,
            ticks_this_second: 0,

            tick_start: now,
            total_tick_time: Duration::ZERO,
        }
    }

    fn on_client_started(&mut self) {
        tracing::info!("Started");
    }

    fn on_client_stopping(&mut self) {
        tracing::info!("Stopping");
        tracing::info!("Total ticks: {}", self.total_ticks);
    }

    fn on_client_tick_start(&mut self) {
        self.tick_start = Instant::now();
    }

    fn on_client_tick_end(&mut self) {
        self.total_ticks += 1;
        self.ticks_this_second += 1;

        self.total_tick_time += self.tick_start.elapsed();

        let elapsed = self.second_start.elapsed();

        if elapsed >= Duration::from_secs(1) {
            let tps = self.ticks_this_second as f64 / elapsed.as_secs_f64();
            let mspt = self.total_tick_time.as_secs_f64() * 1000.0 / self.ticks_this_second as f64;

            tracing::info!("TPS: {:.2} | MSPT: {:.2}", tps, mspt);

            self.second_start = Instant::now();
            self.ticks_this_second = 0;
            self.total_tick_time = Duration::ZERO;
        }
    }
}
