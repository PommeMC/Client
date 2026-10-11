use crate::net::connection::ConnectionHandle;
use crate::singleplayer::World;
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::groups::DebugGroup;
use crate::ui::components::debug::info::DebugInfo;

pub struct TpsDebugEntry;

pub struct TpsDebugInfo<'a> {
    pub is_stepping_forward: bool,
    pub is_frozen: bool,
    pub target_mspt: f32,
    /// `Some` when in singleplayer
    pub integrated: Option<IntegratedServerInfo>,
    pub server_brand: &'a str,
    pub avg_sent_packets: f32,
    pub avg_received_packets: f32,
}

pub struct IntegratedServerInfo {
    pub is_sprinting: bool,
    pub smoothed_tick_ms: f32,
}

impl<'a> TpsDebugInfo<'a> {
    pub fn new(connection: &ConnectionHandle, world: Option<&mut World>) -> Self {
        Self {
            is_stepping_forward: false, // TODO(debug-overlay)
            is_frozen: false,           // TODO(debug-overlay)
            target_mspt: 50.0,          // TODO(debug-overlay)
            integrated: world.map(|_| IntegratedServerInfo {
                is_sprinting: false,   // TODO(debug-overlay)
                smoothed_tick_ms: 0.0, // TODO(debug-overlay)
            }),
            server_brand: "vanilla", // TODO(debug-overlay)
            avg_sent_packets: connection.packet_stats.avg_sent(),
            avg_received_packets: connection.packet_stats.avg_received(),
        }
    }
}

impl DebugEntry for TpsDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        let Some(tps) = &info.tps else { return };

        displayer.add_fact_to_group(DebugGroup::Misc, "Server", |fact| {
            let mut run_status = if tps.is_stepping_forward {
                "frozen - stepping"
            } else if tps.is_frozen {
                "frozen"
            } else {
                ""
            };

            if let Some(server) = &tps.integrated {
                if server.is_sprinting {
                    run_status = "sprinting";
                }
                let tps_target = if server.is_sprinting {
                    "-".to_string()
                } else {
                    format!("{:.1}", tps.target_mspt)
                };
                fact.value("Integrated")
                    .text(" @ ")
                    .value(format!("{:.1}", server.smoothed_tick_ms))
                    .text("/")
                    .value(tps_target)
                    .text(" ms");
            } else {
                fact.text("\"").value(tps.server_brand).text("\"");
            }

            if !run_status.is_empty() {
                fact.text(" (").value(run_status).text(")");
            }
        });

        displayer.add_fact_to_group(DebugGroup::Misc, "Packets", |fact| {
            fact.value(format!("{:.0}", tps.avg_sent_packets))
                .text(" tx, ")
                .value(format!("{:.0}", tps.avg_received_packets))
                .text(" rx");
        });
    }
}
