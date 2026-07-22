use plugin_api::{Plugin, plugin};

#[plugin]
struct ExamplePlugin {}

impl Plugin for ExamplePlugin {
    fn new() -> Self {
        Self {}
    }

    fn on_init(&mut self) {
        tracing::info!("Hello from plugin!");
    }
}
