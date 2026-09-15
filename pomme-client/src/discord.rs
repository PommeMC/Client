use discord_rich_presence::activity::*;
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

const DISCORD_CLIENT_ID: &str = "1489624876909330452";

fn base_activity(version: &str) -> Activity<'static> {
    Activity::new()
        .details(format!("Pomme Client — {version}"))
        .assets(
            Assets::new()
                .large_image("green-apple")
                .large_text("Pomme Client"),
        )
}

#[derive(PartialEq)]
pub enum PresenceState {
    Loading,
    InMenu,
    Multiplayer,
    Singleplayer,
}

pub struct DiscordPresence {
    client: DiscordIpcClient,
    state: PresenceState,
}

impl DiscordPresence {
    pub fn start(version: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut client = DiscordIpcClient::new(DISCORD_CLIENT_ID);
        client.connect()?;
        client.set_activity(base_activity(version).state("Starting..."))?;

        Ok(Self {
            client,
            state: PresenceState::Loading,
        })
    }

    pub fn set_in_menu(&mut self, version: &str) {
        self.enter(PresenceState::InMenu, version, "In the menu");
    }

    pub fn playing_multiplayer(&mut self, version: &str) {
        self.enter(PresenceState::Multiplayer, version, "In a server");
    }

    pub fn playing_singleplayer(&mut self, version: &str) {
        self.enter(PresenceState::Singleplayer, version, "In a world");
    }

    /// Re-entering the current state sends nothing.
    fn enter(&mut self, state: PresenceState, version: &str, text: &str) {
        if self.state == state {
            return;
        }
        self.state = state;
        let _ = self.set_activity(base_activity(version).state(text));
    }

    fn set_activity(&mut self, payload: Activity) -> Result<(), Box<dyn std::error::Error>> {
        if self.client.set_activity(payload.clone()).is_err() {
            let _ = self.client.connect();
            self.client.set_activity(payload)?;
        }
        Ok(())
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        let _ = self.client.clear_activity();
        let _ = self.client.close();
    }
}
