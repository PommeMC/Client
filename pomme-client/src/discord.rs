use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity::*};

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
        if self.state == PresenceState::InMenu {
            return;
        }
        self.state = PresenceState::InMenu;
        let _ = self.set_activity(base_activity(version).state("In the menu"));
    }

    pub fn playing_multiplayer(&mut self, version: &str) {
        if self.state == PresenceState::Multiplayer {
            return;
        }
        self.state = PresenceState::Multiplayer;
        let _ = self.set_activity(base_activity(version).state("In a server"));
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
