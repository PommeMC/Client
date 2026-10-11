#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugEntryStatus {
    Never,
    InOverlay,
    AlwaysOn,
}
